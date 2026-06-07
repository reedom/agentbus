---
name: agentbus
description: Use when you need to coordinate with another AI session, agent, human, or external process via the agentbus message bus — sending messages, asking questions and waiting for answers, broadcasting events, draining an inbox, or registering this session under a stable instance_id. Trigger on phrases like "send to another claude", "ask the orchestrator", "coordinate with bob", "broadcast event", "agentbus", "message bus", "register me as", "check my inbox", "talk to another session".
---

# agentbus

agentbus is a daemonless message bus driven by the `agentbus` CLI. This
skill teaches you how to USE it well; `agentbus <verb> --help` tells you
WHAT each flag does.

## Mental model

There is **no daemon**. The bus is a shared local store (`~/.agentbus/`:
a SQLite database plus per-instance JSONL inbox spool files). Every CLI
invocation operates on the store directly, in-process. Think Maildir or
git.

- **Instance**: a participant with a stable id (a session, an agent, a
  script). A registration is a database row. Non-persistent rows are
  anchored to a pid and vanish when that process dies; `--persistent`
  rows survive reboots.
- **Envelope**: every wire message — has `id`, `kind`, `from`, optional
  `to`, optional `request_id`, `ts`, and a structured `payload` (JSON).
- **Kinds**:
  - `message` — fire-and-forget, one recipient, spooled to their inbox.
  - `ask` — request that blocks the sender until a `reply` or timeout.
  - `reply` — resolves a specific `ask` by its `request_id`.
  - `event` — broadcast appended to the ordered event log (no `to`).
- **Inbox**: per-instance append-only spool file, unbounded and durable.
  Messages to an absent instance wait in its spool until consumed.

## Quickstart

```bash
# 1. register yourself first (see "Registering this session" below)
agentbus register "$MY_ID" --pid "$PPID"

# 2. talk
echo '{"hello":"world"}' | agentbus send bob --from "$MY_ID"
echo '{"q":"ready?"}'    | agentbus ask bob --from "$MY_ID" --timeout-ms 60000
echo '{"deploy":"done"}' | agentbus publish --from "$MY_ID"

# 3. drain incoming (both print {"envelopes": [...]} batches)
agentbus check-inbox "$MY_ID"                  # non-blocking
agentbus await "$MY_ID" --timeout-ms 60000     # blocking

# 4. on an inbound ask (kind="ask"), answer by its envelope id
echo '{"a":"yes"}' | agentbus reply <ask-envelope-id> "$MY_ID"
```

## Registering this session

Non-persistent rows need a pid anchor that lives as long as your session.
A bare `agentbus register` records the CLI's own pid, which exits
immediately — the row would be instantly dead. Pick one:

- `--pid <session-pid>`: anchor to the harness process. From a shell you
  spawn, `$PPID` usually is the harness pid; verify once with
  `ps -o comm= -p $PPID` if unsure. Cleanup is automatic: when the
  harness dies, the row is reclaimed lazily (next `register`) or by
  `agentbus sweep`.
- `--persistent`: a durable address that survives reboots; release it
  with `agentbus unregister <id>` when the role ends.

Only **recipients** need registration. Any `--from` string is accepted
for sending; register only ids that must receive.

Naming: stable + descriptive (`code-reviewer-pr123`,
`orchestrator-deploy`), charset `[A-Za-z0-9_.:-]{1,128}`.

## Picking the right verb

| Need | Verb | Notes |
|---|---|---|
| Tell another instance something, don't wait | `send` | recipient must be registered |
| Ask a question and need the answer | `ask` | blocks; exit 2 on timeout |
| Answer someone else's ask | `reply` | first arg = the ask envelope's `id` |
| Broadcast to observers | `publish` | no recipient; readers tail the log |
| Pull pending messages once | `check-inbox` | non-blocking, drains all |
| Wait for messages | `await` | blocks up to `--timeout-ms`; empty list on timeout |
| Who is registered? | `ls` | rows carry an `alive` flag |
| Follow the event log | `events --follow` | `--since <seq>` to resume |
| Crash cleanup | `sweep` | prunes dead rows, reports expired asks |

## Blocking calls under a harness

`ask` and `await` block the shell. Keep `--timeout-ms` comfortably below
your harness's shell-command timeout (e.g. with a 2-minute limit, use
`--timeout-ms 100000`) and loop if you need to wait longer. An `ask`
timeout exits 2 and does NOT discard the request — stderr names the
request_id; fetch a late answer with `agentbus ask-result <request_id>`.

## Patterns

### Ask/reply roundtrip (you are the answerer)

```bash
agentbus await "$MY_ID" --timeout-ms 60000
# in the printed envelopes, find kind=="ask"; its "id" is the request id
echo '{"answer":42}' | agentbus reply msg_01HXY... "$MY_ID"
```

### Wake a recipient on delivery (no polling)

```bash
agentbus register worker-1 --pid "$PPID" \
  --on-delivery "bellhop dispatch worker-1"
```

Every sender executes the command (15 s cap) after spooling to you. Hook
failures are non-fatal — the envelope is already durably spooled.
Security: the command runs as your OS user in the sender's process;
register only commands you trust.

### Fanout broadcast

```bash
echo '{"kind":"deploy.started","sha":"abc"}' | agentbus publish --from "$MY_ID"
agentbus events --follow --since 42     # consumers replay/follow
```

### Harness-dependent extras (skip if yours lacks the facility)

- **Hook-injected inbox** (e.g. Claude Code SessionStart hook): inject
  `~/.agentbus/inbox/<your-id>.jsonl` into prompt context at boot; then
  you do not call `await` at all. See `scripts/inject-inbox.sh`.
- **Live monitor** (e.g. Claude Code Monitor): run
  `agentbus watch <id>` under the monitor; it prints one line per
  envelope addressed to you and never consumes the inbox — react with
  `check-inbox`. See `docs/reference/watch-integration.md`.

## Gotchas

- **Self-ask deadlocks.** Never `ask` your own instance id; the answer
  would have to come from you, but you are blocked waiting for it.
- **`reply` takes the ask envelope's `id`** as its request_id argument —
  not a message id; plain `message` envelopes take no reply.
- **Batches.** `check-inbox`/`await` print `{"envelopes": [...]}` —
  possibly several, possibly empty (empty = timeout, a normal outcome).
- **Payload is structured JSON** read from stdin or `--file`; pass an
  object/array, not double-encoded text.
- **Delivery is durable.** Spools are unbounded append-only files;
  nothing is dropped and mail survives reboots.
- **No daemon exists.** If `agentbus` is missing, install it
  (`cargo install agentbus-cli@^0.3`); do not look for a server process.

## Errors (CLI stderr: `error[<code>]: <detail with recovery hint>`)

| Code | Meaning | Recover by |
|---|---|---|
| `unknown_instance` | recipient not registered | `agentbus ls`; register the target first |
| `instance_id_taken` | a live process owns that id | pick a different id (dead owners auto-replaced) |
| `invalid_instance_id` | bad id syntax | use `[A-Za-z0-9_.:-]{1,128}` |
| `timeout` (exit 2) | ask expired unanswered | `agentbus ask-result <request_id>` later |
| `unknown_request_id` | no such ask | you replied with a message id, or to a plain `message` |
| `store_locked` | write contention | retry after a short wait |

## When NOT to use agentbus

- Within a single process — use normal function calls.
- Cross-machine — the store is a local directory (`0700`), one machine,
  one user.
- Job queues needing acks/retries/leases — delivery is durable but
  consume-once.
- Auth-required surfaces — trust boundary is filesystem ownership only.

## MCP fallback

Clients that cannot run shell commands can load the `agentbus-stdio` MCP
server instead; it exposes the same operations as nine tools and teaches
itself via initialize `instructions`. If this skill is present, prefer
the CLI and do not install the shim.
