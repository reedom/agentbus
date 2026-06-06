---
refs:
  id: fr:08-mcp-shim
  kind: fr
  title: "MCP stdio shim over the store"
  related:
    - fr:01-envelope
    - fr:04-router
    - fr:09-hook-inbox
    - fr:10-cli
  modules:
    - crates/agentbus-stdio/src
---

# FR 08: MCP stdio shim over the store

> A single-threaded MCP stdio server that opens the spool store directly — no daemon, no socket.

## Purpose

Each AI session launches its own `agentbus-stdio` process. In v0.2 the shim
is not a proxy: it opens `~/.agentbus/bus.db` and the inbox spool directory
directly, performing all store operations in-process. There is no daemon or
Unix socket. The shim is thin enough that its entire state is one open store
handle and the list of non-persistent instance ids registered in this session.

## User-visible Behavior

The shim exposes nine MCP tools over a JSON-RPC line loop on stdin/stdout:

| Tool | Purpose |
|---|---|
| `register(instance_id, persistent?, on_delivery?)` | Claim an id for this session |
| `unregister(instance_id)` | Release an id early |
| `list_instances()` | Enumerate registered instances with liveness |
| `await_message(instance_id, timeout_ms?)` | Block until messages arrive or timeout (returns `{"envelopes": [...]}`) |
| `check_inbox(instance_id)` | Non-blocking drain (returns `{"envelopes": [...]}`) |
| `send(from, to, payload)` | One-way message |
| `ask(from, to, payload, timeout_ms?)` | RPC; blocks until reply or timeout |
| `reply(from, request_id, payload)` | Answer an inbound ask |
| `publish_event(from, payload)` | Append a broadcast event to the log |

v0.2 surface changes from v0.1:

- `register` gains `persistent` and `on_delivery`; loses `mailbox_size` (spool
  files are unbounded).
- `await_message` and `check_inbox` return envelope batches
  (`{"envelopes": [...]}`) rather than a single envelope. `await_message`
  returns an empty list on timeout.
- There is no socket reconnect logic — the shim owns the store connection.

The shim runs a single-threaded synchronous line loop: one JSON-RPC request at
a time; no async I/O. Each tool call executes synchronously and the response is
written before the next line is read.

## Capabilities

- Full bus access via the nine tools above, without a running daemon.
- Non-persistent registrations made through the shim are released on stdin EOF
  (`session.cleanup` unregisters them in order).
- Abrupt kills and I/O-error exits leave rows behind; pid-liveness sweep
  (fr:02-instance-registry) reclaims them.
- `payload` fields that arrive as JSON-encoded strings are transparently parsed
  into native JSON values before dispatch.

## Error Handling

- Tool errors are returned as JSON-RPC error objects:
  `{"code": -32000, "message": "<stable code>", "data": "<human text>"}`.
  The `message` field carries the stable machine-readable code (e.g.
  `unknown_instance`, `instance_id_taken`); `data` carries the human-readable
  detail.
- On `ask` timeout the error `data` field contains prose including the
  `request_id`; clients that need the id should call `ask_result` rather than
  parse the prose.
- Unknown method: `{"code": -32601, "message": "method not found"}`.
- Missing required argument: `{"code": -32602, "message": "missing `<key>`"}`.
- Malformed JSON input lines are logged to stderr and skipped; the loop
  continues.

## Boundaries

- Single-threaded; concurrent tool calls from one MCP client are serialized by
  the line loop.
- The shim does not interpret `payload` content beyond JSON parsing.
- It does not expose REST endpoints or SSE; external programs use the CLI
  (fr:10-cli).
- One shim serves one AI session; it does not multiplex multiple clients.

## Traceability

- Related FR: fr:01-envelope, fr:04-router, fr:09-hook-inbox, fr:10-cli

## When to update

- A tool is added, removed, or its input schema changes.
- The batch envelope return shape changes.
- Session cleanup behavior (EOF handling) changes.
- The shim gains async I/O or multi-threading.
- The error code format changes.
