---
refs:
  id: fr:08-mcp-shim
  kind: fr
  title: "MCP stdio shim and tool surface"
  related:
    - ref:protocol
    - fr:01-envelope
    - fr:02-instance-registry
    - fr:04-router
  modules:
    - crates/agentbus-stdio/src
    - crates/agentbusd/src/ipc
---

# FR 08: MCP stdio shim and tool surface

> The stateless stdio MCP server that exposes the bus as MCP tools and proxies to the daemon.

## Purpose

MCP stdio servers must run as subprocesses of the AI client, but the bus needs
state that outlives any one AI session. The shim resolves this split: each AI
session launches its own `agentbus-stdio` shim, which is a thin, stateless proxy
forwarding MCP tool calls to the long-lived daemon over a Unix socket and
streaming responses back.

## User-visible Behavior

The shim exposes these MCP tools:

| Tool | Purpose |
|---|---|
| `register(instance_id, mailbox_size?)` | Claim an ID for this session |
| `unregister()` | Release the ID early (also on shim exit) |
| `await_message(timeout_secs?)` | Block until a message arrives, or return empty on timeout |
| `check_inbox()` | Non-blocking drain (0..N envelopes) |
| `reply(request_id, payload)` | Answer an inbound `ask` |
| `send(to, payload)` | One-way message to another instance |
| `ask(to, payload, timeout_secs?)` | RPC; blocks until reply or timeout |
| `publish_event(kind, payload)` | Broadcast to all SSE subscribers |
| `list_instances()` | Enumerate active instances |

- The shim connects to the daemon's Unix socket
  (`$XDG_RUNTIME_DIR/agentbus.sock`) at startup, exchanging JSON-RPC frames.
- The shim's registration is bound to its Unix-socket connection; when the shim
  exits, the daemon auto-unregisters the instance.
- On a lost daemon connection the shim reconnects with exponential backoff
  (200 ms, 500 ms, 1 s, 3 s, 3 s …) for the first 5 s.
- The shim is stateless: it holds no bus state, only the connection.

## Capabilities

- Per-session MCP tool surface for the full bus (register through
  `list_instances`).
- A subprocess-friendly shim split from the long-lived daemon (spec §3.1).
- Connection-bound registration with auto-unregister on shim exit.
- Resilient reconnect with bounded exponential backoff.
- The shim never crashes the host MCP client, even when the daemon is down.

## Boundaries

- The shim holds no registry, mailbox, log, or pending-RPC state — all of that
  lives in the daemon.
- It does not interpret `payload`.
- It does not bridge to non-MCP transports; external programs use the REST API
  (fr:06-rest-api).
- Routing, correlation, and mailbox semantics are owned by fr:04-router and
  fr:03-mailbox; the shim only proxies the calls.
- One shim serves one AI session; it does not multiplex sessions.

## Error Handling

- Daemon unavailable (spec §8.1): the shim reconnects with backoff for 5 s.
  After that, every tool call returns `{code: "daemon_unavailable",
  retryable: true}` immediately. The shim never panics or exits, so the MCP
  client stays up.

## Traceability

- Reference docs: ref:protocol
- Related FR: fr:01-envelope, fr:02-instance-registry, fr:04-router

## When to update

- An MCP tool is added, removed, or its signature changes.
- The Unix-socket path or IPC frame format changes.
- The reconnect backoff schedule or the 5 s cutover window changes.
- The shim gains state or stops being a pure proxy.
