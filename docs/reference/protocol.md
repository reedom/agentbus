---
refs:
  id: ref:protocol
  kind: reference
  title: "Wire protocol and operation surface"
  related:
    - fr:01-envelope
    - fr:04-router
    - fr:08-mcp-shim
    - fr:10-cli
    - fr:12-store
---

# Protocol reference

agentbus v0.2 is a daemonless message bus over shared local storage (SQLite +
inbox spool files). There is nothing to launch: participants open
`~/.agentbus/bus.db` directly.

This document covers the envelope wire format, the full store operation
surface, the nine MCP tools, CLI invocation examples, and the delivery mode
summary. The design of record for each area is in [`../fr/`](../fr/index.md).

## 1. The envelope

Every message on every surface is an envelope. See [fr:01-envelope](../fr/01-envelope.md) for the complete field table and kind semantics.

```json
{
  "id": "msg_01HXYZ...",
  "kind": "message",
  "from": "extbot-ENG-123",
  "to":   "impl-ENG-123",
  "request_id": null,
  "timeout_ms": null,
  "ts": "2026-05-21T08:12:34Z",
  "payload": { "hint": "use TDD" }
}
```

| Field | Required for | Notes |
|---|---|---|
| `id` | all | ULID, stamped by the sending process |
| `kind` | all | `message`, `ask`, `reply`, `event` |
| `from` | all | sender `instance_id`, or `ext:<label>` for unregistered callers |
| `to` | `message`, `ask`, `reply` | absent / null for broadcast `event` |
| `request_id` | `reply` (required), optional on `ask` | correlates a reply to its ask |
| `timeout_ms` | `ask` | honored verbatim, no clamping (CLI and shim default to 30 000) |
| `ts` | all | RFC3339 UTC, stamped by the sending process |
| `payload` | all | opaque JSON; the bus never interprets it |

`id` and `ts` are always stamped by the sender; caller-supplied values are
ignored.

### 1.1 Kind semantics

- **message** — one-way notification to `to`; no response expected.
- **ask** — RPC request; the caller blocks until a matching `reply` arrives or `timeout_ms` elapses.
- **reply** — answer to an ask; `request_id` matches the ask's `id`, `from`/`to` reversed.
- **event** — broadcast with no recipient; reaches the event log and `watch` streams.

### 1.2 Identity and addressing

- `instance_id` format: `[A-Za-z0-9_.:-]{1,128}`.
- Registration is exclusive for non-persistent rows (collision rejected if the existing owner pid is alive).
- Persistent rows: re-registering the same id is an idempotent upsert (updates `on_delivery`).
- `ext:<label>` senders can send but have no inbox.

## 2. Store operations

All operations are performed in-process by the caller (CLI, shim, or embedded
crate). See [fr:12-store](../fr/12-store.md) for the schema and concurrency rules.

| Operation | Semantics | Key error codes |
|---|---|---|
| `register(id, persistent?, on_delivery?)` | Claim an instance id. Non-persistent rows are anchored to the caller's pid (fr:02). | `instance_id_taken`, `invalid_instance_id` |
| `unregister(id)` | Remove a registration; inbox file is kept. An absent row is not an error: it reports `ok: false`. | — |
| `list()` | Return all rows with liveness status. | — |
| `send(from, to, payload)` | One-way message: check recipient, log to event_log, append to inbox spool, fire on_delivery hook. Returns the envelope id. | `unknown_instance` |
| `ask(from, to, payload, timeout_ms?)` | RPC send + poll asks table for reply. Blocks until reply or timeout. | `unknown_instance`, `timeout` |
| `ask_result(request_id)` | Fetch status of an earlier ask: `Pending`, `Replied`, or `Expired`. Used to retrieve late replies after a timeout. | `unknown_request_id` |
| `reply(from, request_id, payload)` | Write reply_payload to asks row; first write wins. Appends reply envelope to event_log. | `unknown_request_id` |
| `check_inbox(id)` | Atomic rename-snapshot drain. Returns `{"envelopes": [...]}`. Non-blocking. | `io` |
| `await_message(id, timeout_ms?)` | Like check_inbox but blocks until at least one envelope is spooled or timeout elapses. Returns `{"envelopes": []}` on timeout (not an error). | `io` |
| `publish_event(from, payload)` | Append a broadcast event to the event log. Returns event id. | — |
| `events_since(cursor, filter?)` | Read event log from cursor; filter by `instance`, `kind`, or recipient `to`. | — |
| `watch(id, interval_ms?)` | Live stream of envelopes addressed to `id`, one compact JSON line per event. Starts at current max_seq (no replay). Never consumes the inbox. Runs until killed. | — |
| `sweep(grace_secs?, purge_orphans?)` | Crash recovery: prune dead non-persistent registrations, recover inbox snapshots stranded by crashed consumers, re-fire stale on_delivery hooks, report expired asks. | — |

### 2.1 Error codes

| Code | When |
|---|---|
| `unknown_instance` | send/ask finds no registration for the recipient |
| `instance_id_taken` | register finds an existing live owner (non-persistent collision) |
| `invalid_instance_id` | id fails `[A-Za-z0-9_.:-]{1,128}` |
| `timeout` | ask polling deadline elapsed before a reply arrived |
| `unknown_request_id` | reply or ask_result finds no asks row for the given request_id |
| `store_locked` | SQLite busy_timeout (5 s) exhausted |
| `invalid_envelope` | fr:01 validation failed |
| `io` | filesystem I/O error (inbox append, layout creation) |

## 3. MCP tool surface

The `agentbus-stdio` shim exposes nine MCP tools over a synchronous JSON-RPC
line loop on stdin/stdout. It opens the store directly — no daemon or socket.
See [fr:08-mcp-shim](../fr/08-mcp-shim.md) for the full specification.

| Tool | Input (required) | Purpose |
|---|---|---|
| `register` | `instance_id`; opt: `persistent`, `on_delivery` | Claim an id for this session |
| `unregister` | `instance_id` | Release an id early |
| `list_instances` | — | Enumerate registered instances with liveness |
| `await_message` | `instance_id`; opt: `timeout_ms` | Block until messages arrive (returns `{"envelopes": [...]}`) |
| `check_inbox` | `instance_id` | Non-blocking drain (returns `{"envelopes": [...]}`) |
| `send` | `from`, `to`, `payload` | One-way message |
| `ask` | `from`, `to`, `payload`; opt: `timeout_ms` | RPC; blocks until reply |
| `reply` | `from`, `request_id`, `payload` | Answer an inbound ask |
| `publish_event` | `from`, `payload` | Append a broadcast event |

### 3.1 v0.2 surface changes from v0.1

- `register` gains `persistent` and `on_delivery`; loses `mailbox_size` (spool files are unbounded).
- `await_message` and `check_inbox` return envelope batches (`{"envelopes": [...]}`) instead of a single envelope. `await_message` returns an empty list on timeout.
- There is no socket reconnect logic — the shim owns the store connection.

### 3.2 Error shape

Tool errors are JSON-RPC error objects:

```json
{ "code": -32000, "message": "<stable code>", "data": "<human detail>" }
```

The `message` field carries the stable machine-readable code (e.g. `unknown_instance`). On `ask` timeout the error `message` is `timeout` and `data` contains prose with the `request_id`. Callers that need the id should use `ask_result` rather than parse the prose.

### 3.3 Example MCP tool exchange

`orch` calls `ask`:

```json
{
  "from": "orch",
  "to": "impl",
  "payload": { "task": "Refactor module X" },
  "timeout_ms": 600000
}
```

Returns:

```json
{ "request_id": "msg_01HXY_ASK", "payload": { "result": "ok", "files_changed": 3 } }
```

`impl` calls `check_inbox` and gets:

```json
{
  "envelopes": [
    {
      "id": "msg_01HXY_ASK",
      "kind": "ask",
      "from": "orch",
      "to": "impl",
      "timeout_ms": 600000,
      "ts": "2026-05-21T08:12:34Z",
      "payload": { "task": "Refactor module X" }
    }
  ]
}
```

`impl` calls `reply`:

```json
{ "from": "impl", "request_id": "msg_01HXY_ASK", "payload": { "result": "ok", "files_changed": 3 } }
```

## 4. CLI invocation examples

`agentbus` opens `AGENTBUS_DIR` (default `~/.agentbus`) directly. See
[fr:10-cli](../fr/10-cli.md) for the full verb table.

### register / ls / unregister

```bash
# Register a persistent address with an on_delivery hook
agentbus register impl --persistent --on-delivery "bellhop dispatch impl"
# {"ok": true}

# List
agentbus ls
# {
#   "instances": [
#     { "id": "impl", "pid": null, "persistent": true, "on_delivery": "bellhop dispatch impl" }
#   ]
# }

# Unregister
agentbus unregister impl
# {"ok": true}
```

### send

Payload is read from `--file` or stdin:

```bash
echo '{"hint": "use TDD"}' | agentbus send impl --from orch
# {"id": "msg_01HXYZ..."}
```

### ask / ask-result

```bash
# Ask (blocks until reply or timeout):
echo '{"task": "review PR"}' | agentbus ask impl --from orch --timeout-ms 120000
# On success, prints pretty JSON of the reply payload.
# On timeout (exit 2), stderr contains:
#   error[timeout]: no reply within 120000 ms; retrieve a late reply with: agentbus ask-result msg_01HXYZ...

# Retrieve a late reply:
agentbus ask-result msg_01HXYZ...
# {
#   "status": "replied",
#   "payload": { "verdict": "lgtm" },
#   ...
# }
```

### reply

```bash
echo '{"verdict": "lgtm"}' | agentbus reply msg_01HXYZ... impl
# {"ok": true}
```

### check-inbox / await

```bash
agentbus check-inbox impl
# {"envelopes": [...]}

agentbus await impl --timeout-ms 5000
# {"envelopes": [...]}   (empty list if nothing arrived)
```

### publish / events

```bash
echo '{"type": "build_ok"}' | agentbus publish --from ext:ci
# {"id": "msg_01HEVT..."}

agentbus events
# {"seq":1,"envelope":{"id":"msg_01HEVT...","kind":"event","from":"ext:ci","ts":"...","payload":{"type":"build_ok"}}}

agentbus events --follow --interval-ms 1000
# streams indefinitely
```

### watch

```bash
agentbus watch impl --interval-ms 500
# streams one compact JSON envelope per line as messages arrive; never exits unless killed
```

### sweep

```bash
agentbus sweep
# {
#   "dead_instances": [],
#   "recovered_inboxes": [],
#   "rehooked": [],
#   "expired_asks": [],
#   "purged_inboxes": []
# }
```

## 5. Delivery modes

Five complementary delivery modes suit different interaction patterns.

| Mode | Trigger | Consumes inbox? | Blocks? | Reference |
|---|---|---|---|---|
| `on_delivery` hook | sender-side, fires after each inbox append | no | yes (15 s cap) | fr:13-on-delivery |
| `await_message` | recipient polls blocking call | yes | yes (until message or timeout) | fr:08-mcp-shim |
| `check_inbox` | recipient pull | yes | no | fr:09-hook-inbox |
| `watch` streaming | recipient-side process tails event log | no | runs until killed | fr:14-watch |
| hook-inbox file | SessionStart/UserPromptSubmit hook script | yes (rename-snapshot) | no | fr:09-hook-inbox |

Key properties:

- `on_delivery` is sender-executed and bounded to 15 seconds. It wakes a
  recipient that cannot keep a long-running process.
- `await_message` burns a tool-call slot; useful for sessions that exist
  solely to receive.
- `check_inbox` is a non-blocking pull; suits session-boundary polling via a
  Stop/turn hook.
- `watch` is a long-running per-recipient notifier for harnesses that can host
  a persistent monitor process (e.g. Claude Code's Monitor tool). It never
  consumes the inbox; agents react by calling `check_inbox`. See
  [ref:watch-integration](watch-integration.md) for the pattern.
- hook-inbox file consumption uses an atomic rename-snapshot; the reference
  script (`scripts/inject-inbox.sh`) implements this for shell hooks.
