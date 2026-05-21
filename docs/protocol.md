# Protocol reference

This document describes the wire format used by every mcp-bus surface, the REST
endpoints exposed by the daemon, and the MCP tools exposed by the stdio shim.

It mirrors sections 5, 6.5, and 6.7 of the
[design spec](superpowers/specs/2026-05-21-mcp-bus-design.md) with example
payloads added for each surface.

## 1. The envelope

Every message on every surface is an envelope:

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

### 1.1 Fields

| Field | Required for | Notes |
|---|---|---|
| `id` | all | ULID, server-assigned at ingress |
| `kind` | all | `message`, `ask`, `reply`, `event` |
| `from` | all | `instance_id` of sender, or `ext:<label>` for unregistered external talkers |
| `to` | `message`, `ask`, `reply` | absent / null for broadcast `event` |
| `request_id` | `reply` (required), optional on `ask` | correlates a reply with its `ask` |
| `timeout_ms` | `ask` | clamped to `[1000, 86_400_000]` |
| `ts` | all | RFC3339 UTC, server-assigned at ingress |
| `payload` | all | opaque JSON; bus does not interpret |

`id` and `ts` are always overwritten by the daemon at ingress to prevent forgery
and ensure ordering. Callers may omit them.

### 1.2 Kind semantics

- **message** — one-way notification to `to`. No response expected.
- **ask** — RPC request to `to`. Caller blocks until a `reply` with matching
  `request_id` arrives or `timeout_ms` elapses.
- **reply** — response to an earlier `ask`. `request_id` matches the ask's `id`.
  `from` matches the original `to`; `to` matches the original `from`.
- **event** — broadcast notification with no specific recipient. Goes only to
  SSE subscribers (and the JSONL log).

### 1.3 Identity and addressing

- Instances are addressed by client-provided `instance_id`. Format:
  `[A-Za-z0-9_.:-]{1,128}`.
- Registration is exclusive: collisions are rejected.
- External programs that talk without registering use `from: "ext:<label>"`.
  They cannot be addressed by others (no inbox) but can `send`, `ask`, and
  subscribe to events.

### 1.4 Example envelopes

`ask` envelope on the wire:

```json
{
  "id": "msg_01HXY_ASK",
  "kind": "ask",
  "from": "orch",
  "to": "impl",
  "timeout_ms": 600000,
  "ts": "2026-05-21T08:12:34Z",
  "payload": { "task": "Refactor module X" }
}
```

Corresponding `reply`:

```json
{
  "id": "msg_01HXY_REPLY",
  "kind": "reply",
  "from": "impl",
  "to": "orch",
  "request_id": "msg_01HXY_ASK",
  "ts": "2026-05-21T08:13:10Z",
  "payload": { "result": "ok", "files_changed": 3 }
}
```

Broadcast `event`:

```json
{
  "id": "msg_01HXY_EVT",
  "kind": "event",
  "from": "ext:ci",
  "ts": "2026-05-21T08:14:00Z",
  "payload": { "type": "build_finished", "status": "green" }
}
```

## 2. REST surface

All endpoints are versioned under `/v1` and bound to `127.0.0.1`.

| Method | Path | Purpose |
|---|---|---|
| `POST` | `/v1/instances` | Register `{instance_id, mailbox_size?}` |
| `DELETE` | `/v1/instances/{id}` | Unregister |
| `GET` | `/v1/instances` | List active instances |
| `GET` | `/v1/instances/{id}/inbox` | SSE — envelopes addressed to `{id}` |
| `POST` | `/v1/instances/{id}/messages` | Send a `message` to `{id}` |
| `POST` | `/v1/instances/{id}/ask` | Ask `{id}`; HTTP blocks until reply or timeout |
| `POST` | `/v1/instances/{id}/replies` | Reply to an ask `{request_id, payload}` |
| `GET` | `/v1/events` | SSE — global broadcast + history replay (`since`, `instance`, `kind`) |
| `POST` | `/v1/events` | Publish a broadcast event from external |

### 2.1 Registration lifetime

`POST /v1/instances` binds the registration to the HTTP connection's lifetime
via a keep-alive long-poll: the response is `200 OK` with `Connection:
keep-alive` and the body is an SSE-style heartbeat stream. Closing the
connection unregisters.

For clients that use pooled connections, an explicit `DELETE
/v1/instances/{id}` is also supported.

### 2.2 Ask timeout and inbox ownership

- `POST /v1/instances/{id}/ask` accepts `?timeout_ms=` (default `30_000`,
  max 24h). On timeout the server returns `504` with body
  `{"error": "timeout", "request_id": "..."}`.
- `GET /v1/instances/{id}/inbox` requires the caller to be the registered
  owner (matched by connection). A different connection requesting another
  instance's inbox returns `403`.

### 2.3 Example REST exchanges

Send a message:

```bash
curl -X POST http://127.0.0.1:PORT/v1/instances/impl-ENG-123/messages \
     -H 'content-type: application/json' \
     -d '{"payload": {"hint": "use TDD"}}'
```

Ask with timeout (blocks until reply or 504):

```bash
curl -X POST 'http://127.0.0.1:PORT/v1/instances/slack-ask/ask?timeout_ms=1800000' \
     -H 'content-type: application/json' \
     -d '{"payload": {"question": "Deploy now?", "options": ["yes","no"]}}'
```

Reply to an ask (used by bridges):

```bash
curl -X POST http://127.0.0.1:PORT/v1/instances/slack-ask/replies \
     -H 'content-type: application/json' \
     -d '{"request_id": "msg_01HXY_ASK", "payload": {"choice": "yes"}}'
```

Replay events since a timestamp:

```
GET /v1/events?since=2026-05-21T08:00:00Z&instance=impl-ENG-123
```

The daemon snapshots the log offset at subscribe time, replays matching
envelopes up to that offset, then attaches the subscriber to live broadcasts —
no gap, no duplicate. Clients should still dedup by envelope `id` defensively
across reconnects.

## 3. MCP shim (`mcp-bus-stdio`)

The shim connects to the daemon's Unix socket and exposes the bus as MCP tools.

| Tool | Purpose |
|---|---|
| `register(instance_id, mailbox_size?)` | Claim ID for this session |
| `unregister()` | Release ID early (also happens on shim exit) |
| `await_message(timeout_secs?)` | Block until a message arrives, or return empty on timeout |
| `check_inbox()` | Non-blocking drain (returns 0..N envelopes) |
| `reply(request_id, payload)` | Answer an inbound `ask` |
| `send(to, payload)` | One-way message to another instance |
| `ask(to, payload, timeout_secs?)` | RPC; blocks until reply or timeout |
| `publish_event(kind, payload)` | Broadcast to all SSE subscribers |
| `list_instances()` | Enumerate active instances |

### 3.1 Reconnect behavior

The shim reconnects to the daemon with exponential backoff (200 ms, 500 ms,
1 s, 3 s, 3 s ...) for the first 5 seconds. Thereafter, each tool call returns
`{code: "daemon_unavailable", retryable: true}` immediately. The shim never
crashes the MCP client.

### 3.2 Example MCP tool exchange

`orch` instance (caller):

```jsonc
// tool: ask
{
  "to": "impl",
  "payload": { "task": "Refactor module X" },
  "timeout_secs": 600
}
// returns:
{ "result": "ok", "files_changed": 3 }
```

`impl` instance (callee):

```jsonc
// tool: await_message
{ "timeout_secs": 60 }
// returns an envelope:
{
  "id": "msg_01HXY_ASK",
  "kind": "ask",
  "from": "orch",
  "to": "impl",
  "ts": "2026-05-21T08:12:34Z",
  "payload": { "task": "Refactor module X" }
}

// tool: reply
{
  "request_id": "msg_01HXY_ASK",
  "payload": { "result": "ok", "files_changed": 3 }
}
```

## 4. Error codes

Common error shapes the daemon returns:

| Code | Meaning |
|---|---|
| `instance_id_taken` | Registration collision with a different owner connection |
| `instance_disconnected` | Pending `ask` cancelled because peer's owner connection dropped |
| `unknown_request_id` | `reply` arrived for an ask that timed out or never existed |
| `timeout` | `ask` exceeded its `timeout_ms` |
| `daemon_unavailable` | Shim cannot reach daemon socket (retryable) |
| `instance_closed` | `await_message` resolved because the mailbox was closed |
