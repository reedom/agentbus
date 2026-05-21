---
name: agentbus
description: Use when you need to coordinate with another AI session, agent, human, or external process via the agentbus message bus — sending messages, asking questions and waiting for answers, broadcasting events, draining an inbox, or registering this session under a stable instance_id. Trigger on phrases like "send to another claude", "ask the orchestrator", "coordinate with bob", "broadcast event", "agentbus", "MCP message bus", "register me as", "check my inbox", "talk to another session".
---

# agentbus

agentbus is an MCP-native message bus. Tools are exposed as `mcp__agentbus__*`.
This skill teaches you how to USE them well; the tool descriptions tell you
WHAT each one does.

## Mental model

- **Instance**: a participant with a stable id (a session, an agent, a script).
- **Envelope**: every wire message — has `id`, `kind`, `from`, optional `to`,
  optional `request_id`, `ts`, and a structured `payload` (JSON, not string).
- **Kinds**:
  - `message` — fire-and-forget, one recipient.
  - `ask` — request that blocks the sender until a `reply` or timeout.
  - `reply` — resolves a specific `ask` by its `request_id`.
  - `event` — broadcast to all SSE subscribers (no `to`).
- **Mailbox**: per-instance, bounded, drops oldest on overflow.
- **Owner token**: returned on register; tied to your connection. If the
  connection drops, the daemon unregisters your instance automatically.

## Quickstart

```text
1. mcp__agentbus__register(instance_id="<your-id>")        # do this first
2. either:
     mcp__agentbus__send(from=..., to=..., payload=...)
   or:
     mcp__agentbus__ask(from=..., to=..., payload=..., timeout_ms=...)
   or:
     mcp__agentbus__publish_event(from=..., payload=..., kind="<topic>")
3. drain incoming:
     mcp__agentbus__check_inbox(instance_id=...)           # non-blocking
     mcp__agentbus__await_message(instance_id=..., timeout_ms=...) # blocking
4. on inbound ask:
     mcp__agentbus__reply(from=<you>, request_id=<asker_msg_id>, payload=...)
```

## Picking the right tool

| Need | Use | Notes |
|---|---|---|
| Tell another instance something, don't wait | `send` | one recipient |
| Ask a question and need an answer | `ask` | blocks; set realistic `timeout_ms` |
| Answer someone else's `ask` | `reply` | `request_id` = the ask envelope's `id` |
| Broadcast to many observers | `publish_event` | no `to`; subscribers via SSE |
| Pull pending messages once | `check_inbox` | non-blocking, drains all |
| Wait for the next message | `await_message` | blocks up to `timeout_ms` |
| List who is online | `list_instances` | |
| Bind this session to an id | `register` | mailbox_size default 64 |
| Release the id | `unregister` | also happens on disconnect |

## Naming instance ids

- Stable + descriptive: `code-reviewer-pr123`, `orchestrator-deploy-2026-05-22`.
- Allowed chars: `[A-Za-z0-9_.:-]{1,128}`.
- Prefix external scripts with `ext:` (e.g. `ext:slackbot`); the daemon does
  not require registration for `ext:*` senders.

## Patterns

### Ask/reply roundtrip (you are the asker)

```text
ask(from="you", to="bob", payload={"q":"..."}, timeout_ms=30000)
# blocks until bob replies or 30s elapses
# returns the reply envelope; read .payload
```

### Ask/reply roundtrip (you are the answerer)

```text
await_message(instance_id="you", timeout_ms=60000)
# returns envelope with kind="ask"
# extract envelope.id as the request_id
reply(from="you", request_id=<that id>, payload=<your answer>)
# `to` is auto-filled from the original asker — do not set it
```

### Stateless session catch-up (hook-injected inbox)

If your session may restart between messages, configure the SessionStart
hook to inject `$INBOX_DIR/<your-id>.jsonl` into your prompt context. You
do NOT need to call `await_message` in that mode — the harness delivers
the queue at boot.

### Fanout broadcast

```text
publish_event(from="you", kind="deploy.started", payload={"sha":"abc"})
```
All SSE subscribers on `/v1/events` receive the envelope. No mailbox.

## Gotchas

- **Self-ask deadlocks**. Do not `ask` your own instance_id; the answer would
  have to come from you, but you are blocked waiting for it.
- **payload is structured JSON**. Pass an object/array/number, not a JSON
  string. The shim auto-parses string payloads as a safety net but native
  values are clearer.
- **reply does not need `to`**. The daemon looks up the original asker.
  Supply `to` only if you intentionally want to redirect.
- **Daemon restart = identity loss**. If you see `daemon_unavailable`, wait
  1–3 seconds (shim auto-reconnects), then `register` again before sending.
- **Mailbox is lossy under pressure**. Bounded with drop-oldest; do not
  rely on it for durable delivery. Use `publish_event` + replay for that.
- **Owner-disconnect cancels your pending asks**. If you crashed mid-ask,
  the daemon returns `instance_disconnected` to whoever was waiting on
  your reply.
- **Timeouts are clamped**. `ask` timeout is clamped to
  `[1s, AGENTBUS_MAX_TIMEOUT_MS]` (default max 24h). Default if unset:
  `AGENTBUS_DEFAULT_TIMEOUT_MS` (30s).

## Error codes (RPC)

| Code | Meaning | Recover by |
|---|---|---|
| -32000 | `daemon_unavailable` | wait, retry — shim reconnects |
| -32602 | invalid params / validation | fix arg shape (see envelope spec) |
| 1001 | unknown_instance | target not registered; check `list_instances` |
| 1002 | id_collision | another owner holds it; pick different id |
| 1003 | invalid_id | use `[A-Za-z0-9_.:-]{1,128}` |
| 1004 | timeout | retry or extend `timeout_ms` |
| 1005 | unknown_request_id | reply target already resolved/expired |
| 1006 | instance_disconnected | answerer crashed; do not retry blindly |
| 1007 | payload_too_large | shrink payload (default cap 64KB) |

## When NOT to use agentbus

- Within a single process — use normal function calls.
- Cross-machine — agentbus binds loopback only in v1.
- Durable queues — events are JSONL-logged but not transactional.
- Auth-required surfaces — v1 has no auth; trust the local socket.

## See also

- `docs/protocol.md` — wire envelope schema + REST + MCP tool tables.
- `docs/examples/extbot-integration.md` — external orchestrator pattern.
- `docs/examples/slack-bridge.md` — human-in-the-loop via Slack.
