---
refs:
  id: fr:11-daemon-lifecycle
  kind: fr
  title: "Daemon lifecycle, configuration, and security"
  related:
    - fr:05-eventlog
    - fr:06-rest-api
    - fr:08-mcp-shim
  modules:
    - crates/agentbusd/src/main.rs
    - crates/agentbusd/src/config.rs
    - crates/agentbusd/src/shutdown.rs
    - crates/agentbusd/src/state.rs
---

# FR 11: Daemon lifecycle, configuration, and security

> The `agentbusd` process: its runtime, configuration, security posture, and graceful shutdown.

## Purpose

`agentbusd` is the single long-running process per host that owns all bus
state — registry, mailboxes, router, and event log. This FR covers the daemon
as a process: the runtime it is built on, how it is configured, its v1 security
posture, and how it shuts down cleanly.

## User-visible Behavior

- The daemon is a single Rust binary built on `tokio`, with `axum` for HTTP/SSE
  and `rmcp`-shaped IPC over a Unix socket (spec §4).
- Configuration is via environment variables, all optional, overridable by CLI
  flags on `agentbusd`:

  | Var | Default | Meaning |
  |---|---|---|
  | `AGENTBUS_PORT` | `8765` | HTTP port (loopback) |
  | `AGENTBUS_SOCKET` | `$XDG_RUNTIME_DIR/agentbus.sock` | Unix socket for the shim |
  | `AGENTBUS_LOG_PATH` | `$XDG_STATE_HOME/agentbus/events.jsonl` | JSONL log location |
  | `AGENTBUS_LOG_MAX_PAYLOAD` | `65536` | reject envelopes whose `payload` exceeds this size |
  | `AGENTBUS_INBOX_DIR` | `$XDG_RUNTIME_DIR/agentbus/inbox` | hook-injection inbox directory |
  | `AGENTBUS_DEFAULT_TIMEOUT_MS` | `30000` | default `ask` timeout |
  | `AGENTBUS_MAX_TIMEOUT_MS` | `86400000` | maximum allowed `ask` timeout (24 h) |
  | `RUST_LOG` | `info,agentbus=debug` | tracing filter |

- The daemon binds HTTP only to `127.0.0.1` and the Unix socket with `0600`
  permissions. It refuses to start when `--bind` names a non-loopback address.
- On `SIGTERM` the daemon shuts down gracefully.

## Capabilities

- Single static binary, one process per host, owning all bus state.
- Optional environment configuration with CLI-flag override.
- Loopback-only network exposure and `0600` socket permissions by default.
- A startup guard that refuses non-loopback binds.
- An ingress payload cap that bounds per-line log size and per-event memory.
- Graceful, bounded-drain shutdown that notifies SSE subscribers.

## Boundaries

- No authentication, authorization, or TLS in v1 — security rests on loopback
  binding and socket permissions (spec §1.1, §8.10); these are post-v1 (§13).
- No multi-host federation; exactly one daemon per host (spec §1.1).
- The daemon does not provide durable queues — durability is the best-effort
  event log (fr:05-eventlog) plus replay.
- A built-in web dashboard is out of scope; the daemon offers REST + SSE only
  (spec §1.1).
- Component behavior (routing, mailboxes, SSE) is owned by the respective FRs;
  this FR owns only process lifecycle, config, and security posture.

## Error Handling

- Payload cap (spec §8.10): envelopes whose `payload` exceeds
  `AGENTBUS_LOG_MAX_PAYLOAD` (default 65536) are rejected at ingress.
- Non-loopback bind refusal (spec §8.10): the daemon refuses to start when
  `--bind` is a non-loopback address, and the error message points at the
  future auth/TLS work.
- Graceful shutdown (spec §8.11): on `SIGTERM` the daemon stops accepting new
  connections, drains in-flight RPCs for up to 5 s, `fsync`s the JSONL log,
  sends `event: shutdown\ndata: {}` to SSE subscribers, closes listeners, and
  exits.

## Traceability

- Related FR: fr:05-eventlog, fr:06-rest-api, fr:08-mcp-shim

## When to update

- A configuration variable is added, removed, or its default changes.
- The security posture changes (auth, TLS, non-loopback binding).
- The graceful-shutdown sequence or its drain budget changes.
- The runtime stack (`tokio`, `axum`, `rmcp`) changes materially.
