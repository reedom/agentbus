---
name: agentbus
description: Use when you need to coordinate with another AI session, agent, human, or external process via the agentbus message bus — sending messages, asking questions and waiting for answers, broadcasting events, draining an inbox, or registering this session under a stable instance_id. Trigger on phrases like "send to another claude", "ask the orchestrator", "coordinate with bob", "broadcast event", "agentbus", "MCP message bus", "register me as", "check my inbox", "talk to another session".
---

# agentbus

agentbus is an MCP-native message bus. Tools are exposed as `mcp__agentbus__*`.
This skill teaches you how to USE them well; the tool descriptions tell you
WHAT each one does.

## Mental model

There is **no daemon**. The bus is a shared local store (`~/.agentbus/`:
a SQLite database plus per-instance JSONL inbox spool files). Every tool
call operates on the store directly, in-process. Think Maildir or git.

- **Instance**: a participant with a stable id (a session, an agent, a script).
  A registration is a database row. Non-persistent rows are tied to the shim
  process's pid and vanish when the session ends; `persistent` rows survive
  reboots.
- **Envelope**: every wire message — has `id`, `kind`, `from`, optional `to`,
  optional `request_id`, `ts`, and a structured `payload` (JSON, not string).
- **Kinds**:
  - `message` — fire-and-forget, one recipient, spooled to their inbox.
  - `ask` — request that blocks the sender until a `reply` or timeout.
  - `reply` — resolves a specific `ask` by its `request_id`.
  - `event` — broadcast appended to the ordered event log (no `to`).
- **Inbox**: per-instance append-only spool file, unbounded and durable.
  Messages to an absent instance wait in its spool until consumed.

## Quickstart

```text
1. mcp__agentbus__register(instance_id="<your-id>")        # do this first
2. either:
     mcp__agentbus__send(from=..., to=..., payload=...)
   or:
     mcp__agentbus__ask(from=..., to=..., payload=..., timeout_ms=...)
   or:
     mcp__agentbus__publish_event(from=..., payload=...)
3. drain incoming (both return {"envelopes": [...]} batches):
     mcp__agentbus__check_inbox(instance_id=...)           # non-blocking
     mcp__agentbus__await_message(instance_id=..., timeout_ms=...) # blocking
4. on inbound ask:
     mcp__agentbus__reply(from=<you>, request_id=<asker_msg_id>, payload=...)
```

## Picking the right tool

| Need | Use | Notes |
|---|---|---|
| Tell another instance something, don't wait | `send` | one recipient; recipient must be registered |
| Ask a question and need an answer | `ask` | blocks; set realistic `timeout_ms` (default 30s) |
| Answer someone else's `ask` | `reply` | `request_id` = the ask envelope's `id` |
| Broadcast to many observers | `publish_event` | no `to`; readers tail the event log |
| Pull pending messages once | `check_inbox` | non-blocking, drains all, returns batch |
| Wait for messages | `await_message` | blocks up to `timeout_ms`; empty list on timeout |
| List who is registered | `list_instances` | each row carries an `alive` flag (pid liveness) |
| Bind this session to an id | `register` | optional `persistent`, `on_delivery` |
| Release the id early | `unregister` | non-persistent ids auto-release at session end |

## Naming instance ids

- Stable + descriptive: `code-reviewer-pr123`, `orchestrator-deploy-2026-05-22`.
- Allowed chars: `[A-Za-z0-9_.:-]{1,128}`.
- Only **recipients** need registration. Any `from` string is accepted for
  sending; register only the ids that must receive messages.

## Patterns

### Ask/reply roundtrip (you are the asker)

```text
ask(from="you", to="bob", payload={"q":"..."}, timeout_ms=30000)
# blocks until bob replies or 30s elapses
# returns {"request_id": ..., "payload": <bob's answer>}
```

### Ask/reply roundtrip (you are the answerer)

```text
await_message(instance_id="you", timeout_ms=60000)
# returns {"envelopes": [...]}; find the one with kind="ask"
# extract envelope.id as the request_id
reply(from="you", request_id=<that id>, payload=<your answer>)
# `to` is auto-filled from the asks row — do not set it
```

### Retrieving a late reply after an ask timeout

An `ask` timeout does NOT discard the request: the asks row stays, and a
late `reply` still lands in it. The timeout error's `data` field contains
the `request_id`; retrieve the answer out-of-band with the CLI:

```text
agentbus ask-result <request_id>
```

### Wake a recipient on delivery (no daemon, no polling)

Register with an `on_delivery` command; every sender executes it (15s
timeout) after spooling a message to you:

```text
register(instance_id="worker-1", on_delivery="bellhop dispatch worker-1")
```

Hook failures are non-fatal — the envelope is already durably spooled.

### Stateless session catch-up (hook-injected inbox)

If your session may restart between messages, configure the SessionStart
hook to inject `~/.agentbus/inbox/<your-id>.jsonl` into your prompt
context. You do NOT need to call `await_message` in that mode — the
harness delivers the queue at boot.

### Live notification stream (interactive harness)

`agentbus watch <instance_id>` (CLI, not an MCP tool) tails the event log
and prints one line per envelope addressed to you. It never consumes the
inbox — react by calling `check_inbox`. See
`docs/reference/watch-integration.md` for running it under a session
monitor.

### Fanout broadcast

```text
publish_event(from="you", payload={"kind":"deploy.started","sha":"abc"})
```

Events append to the ordered log; consumers replay/follow with
`agentbus events --follow [--since <seq>]`.

## Gotchas

- **Self-ask deadlocks**. Do not `ask` your own instance_id; the answer would
  have to come from you, but you are blocked waiting for it.
- **payload is structured JSON**. Pass an object/array/number, not a JSON
  string. The shim auto-parses string payloads as a safety net but native
  values are clearer.
- **reply does not need `to`**. The store looks up the original asker from
  the asks row.
- **await_message returns a batch**. `{"envelopes": [...]}` — possibly
  several, possibly empty (empty = timeout, which is a normal outcome, not
  an error).
- **Delivery is durable**. Inbox spools are unbounded, append-only files;
  nothing is dropped on overflow and messages survive reboots. Undelivered
  mail waits until the recipient consumes it.
- **Registrations have two lifetimes**. Non-persistent (default): tied to
  this session's shim process, auto-released at session end, reclaimed by
  pid-liveness if the session dies abruptly. `persistent: true`: survives
  until explicit `unregister` (or `agentbus sweep --purge-orphans`).
- **There is no daemon to start**. If the `mcp__agentbus__*` tools are
  missing, the `agentbus-stdio` binary is not on PATH or the MCP server
  entry is stale — fix the config, do not look for a daemon process.

## Error codes (RPC)

Tool errors arrive as `{"code": -32000, "message": "<stable code>",
"data": "<human detail>"}`. The stable codes:

| Code | Meaning | Recover by |
|---|---|---|
| `unknown_instance` | recipient not registered | check `list_instances`; register the target first |
| `instance_id_taken` | a live process owns that id | pick a different id (dead owners are auto-replaced) |
| `invalid_instance_id` | bad id syntax | use `[A-Za-z0-9_.:-]{1,128}` |
| `timeout` | ask expired unanswered | `data` has the request_id; `agentbus ask-result` later |
| `unknown_request_id` | no such ask | request_id typo, or replying to a plain `message` |
| `store_locked` | pathological write contention | retry after a short wait |
| `invalid_envelope` | envelope validation failed | fix arg shape (see protocol doc) |

`-32602` = missing required argument; `-32601` = unknown tool.

## When NOT to use agentbus

- Within a single process — use normal function calls.
- Cross-machine — the store is a local directory (`0700`), single machine,
  single user.
- Job queues needing acks/retries/leases — delivery is durable but
  consume-once; there is no redelivery protocol.
- Auth-required surfaces — trust boundary is filesystem ownership only.

## See also

- `docs/reference/protocol.md` — envelope schema + store operations + tool tables.
- `docs/reference/watch-integration.md` — live notification under a harness monitor.
- `docs/reference/extbot-integration.md` — external orchestrator pattern.
- `docs/reference/slack-bridge.md` — human-in-the-loop via Slack.
