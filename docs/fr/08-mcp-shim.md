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
    - fr:16-usage-guidance
  modules:
    - crates/agentbus-stdio/src/main.rs
    - crates/agentbus-stdio/src/tools.rs
---

# FR 08: MCP stdio shim over the store

> A single-threaded MCP stdio server that opens the spool store directly — no daemon, no socket.

## Purpose

The shim is the fallback AI surface: for MCP-capable clients that cannot
load skills or run shell commands (fr:16-usage-guidance). Skill-capable
clients drive the CLI instead and do not load the shim. A client that does
load it launches its own `agentbus-stdio` process. In v0.2 the shim
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

The `initialize` result carries an `instructions` string with condensed
usage guidance (`src/instructions.rs`), controlled by the
`--instructions=none|minimal|full` startup flag (default `full`).
Packagings that ship the skill pass `none` to avoid paying the duplicate
context cost. Unknown flag values are a startup error (exit 2); unknown
flags are ignored.

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
- On `ask` timeout the error `message` is `timeout` and the `data` field
  contains prose including the `request_id`. The shim exposes no `ask_result`
  tool: an MCP client that needs the late reply must parse the `request_id`
  out of `data` and retrieve it out-of-band via the CLI
  (`agentbus ask-result`, fr:10-cli).
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

- Related FR: fr:01-envelope, fr:04-router, fr:09-hook-inbox, fr:10-cli,
  fr:16-usage-guidance

## When to update

- A tool is added, removed, or its input schema changes.
- The batch envelope return shape changes.
- Session cleanup behavior (EOF handling) changes.
- The shim gains async I/O or multi-threading.
- The error code format changes.
- The `instructions` text, its levels, or the flag default changes.
- The shim's role as fallback (vs primary) surface changes.
