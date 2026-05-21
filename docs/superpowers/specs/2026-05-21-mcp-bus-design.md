# mcp-bus — design

- Date: 2026-05-21
- Status: approved (brainstorm complete; implementation plan TBD)
- Working repository: `github.com/reedom/claude-comm` (will be renamed to `mcp-bus`)
- Origin: extracted and generalized from a prior MCP design for managing AI session state and events

## 1. Goal

Provide a small, MCP-native message bus that lets any MCP-capable AI instance (Claude Code, future Codex/Cursor, custom agents) talk to and listen to:

- other AI instances
- humans via CLI / TUI
- arbitrary external programs (scripts, CI, webhooks, Slack/Discord bridges, on-call systems)

Bidirectional in both directions: AI → external (events, asks) and external → AI (messages, replies, prompts). The wire format is a single envelope used uniformly across REST, SSE, MCP stdio, and the persistent log.

### 1.1 Non-goals (v1)

- Authentication, authorization, TLS, multi-tenant isolation.
- Multi-host federation. Single daemon per host.
- Persistent durable queues with delivery guarantees beyond best-effort + replay.
- Shipping concrete bridges (Slack, Discord, etc.). Those are documented as user-built integrations.
- A built-in web dashboard UI. REST + SSE only; dashboards are downstream consumers.

### 1.2 Originating use case

External workflow daemons that drive Claude Code subprocesses through phase transitions (orchestrator session ↔ phase subprocesses) using stdin/stdout JSON need a generic way to deliver events to the orchestrator Claude and receive responses. Rather than build a project-specific IPC, we extract a general-purpose MCP-based bus so any project with similar needs can reuse it.

## 2. Glossary

- **Instance** — any participant on the bus, identified by a client-provided `instance_id`. May be an MCP client (e.g. Claude Code) or an external program connected via REST.
- **Talker / Listener** — informal: the side sending vs the side receiving an envelope. Most instances are both.
- **Envelope** — the single canonical JSON message format on the wire.
- **Mailbox** — bounded in-memory queue per registered instance, holding envelopes directed at it.
- **Daemon** — `mcp-busd`, the single long-running process per host.
- **Shim** — `mcp-bus-stdio`, the MCP stdio server launched by each AI client; proxies to the daemon over a Unix socket.

## 3. Topology

```
                   ┌────────────────────────────────────────────────┐
                   │  mcp-busd  (long-running daemon, per host)     │
                   │  - InstanceRegistry (by ID)                    │
   extbot/scripts ─┤  - MessageRouter                               ├─→ SSE clients
   curl / bridges  │  - JSONL EventLog (persistence + replay)       │
   webhooks        │  - REST :PORT (127.0.0.1)  +  SSE /events      │
                   │  - Per-instance mailboxes                      │
                   └──────────────┬─────────────────────────────────┘
                                  │ Unix socket: $XDG_RUNTIME_DIR/mcp-bus.sock
                                  │ JSON-RPC frames
                   ┌──────────────▼─────────────┐
                   │  mcp-bus-stdio (stdio MCP) │
                   │  - rmcp server             │
                   │  - Thin proxy to daemon    │
                   └──────────────┬─────────────┘
                                  │ stdio MCP
                          ┌───────▼────────┐
                          │ MCP client     │
                          │ (Claude Code,  │
                          │  …)            │
                          └────────────────┘
```

### 3.1 Why split daemon and shim

- MCP stdio servers must be subprocesses of the AI client. Each AI session gets its own shim.
- The bus needs state that outlives any individual AI session: registry, mailboxes, event log, in-flight RPCs. That state lives in the daemon.
- The shim is intentionally stateless: it forwards MCP tool calls to the daemon and streams responses back. If the daemon is unavailable it surfaces structured errors.

### 3.2 Binaries

- `mcp-busd` — the daemon (HTTP + SSE + Unix socket).
- `mcp-bus-stdio` — the stdio MCP shim launched by AI clients via `.mcp.json`.
- `mcp-bus` — a small CLI wrapping the REST API (`mcp-bus send`, `mcp-bus tail`, `mcp-bus ask`, `mcp-bus ls`, `mcp-bus rm`).

## 4. Language and runtime

- **Rust**, single workspace (`cargo workspace`).
- Async runtime: `tokio`.
- HTTP: `axum`.
- MCP server: `rmcp`.
- Serialization: `serde` + `serde_json`.
- IDs: `ulid` (sortable; readable; collision-safe).
- Logging: `tracing` + `tracing-subscriber`.

Choice rationale: the surrounding orchestrator ecosystem is Rust; unified toolchain. `axum` + `rmcp` are mature. Single static binary distribution.

## 5. Wire format — the envelope

Every message on every surface is an envelope:

```json
{
  "id": "msg_01HXYZ...",
  "kind": "message" | "ask" | "reply" | "event",
  "from": "extbot-ENG-123",
  "to":   "impl-ENG-123",
  "request_id": "msg_01H...",
  "timeout_ms": 300000,
  "ts": "2026-05-21T08:12:34Z",
  "payload": { /* free-form JSON */ }
}
```

Fields:

| Field | Required for | Notes |
|---|---|---|
| `id` | all | ULID, server-assigned at ingress |
| `kind` | all | `message`, `ask`, `reply`, `event` |
| `from` | all | `instance_id` of sender, or `ext:<label>` for unregistered external talkers |
| `to` | `message`, `ask`, `reply` | absent / null for broadcast `event` |
| `request_id` | `reply`, optional on `ask` | correlates a reply with its `ask` |
| `timeout_ms` | `ask` | clamped to [1000, 86_400_000] |
| `ts` | all | RFC3339 UTC, server-assigned at ingress |
| `payload` | all | opaque JSON; bus does not interpret |

Server-assigned fields (`id`, `ts`) are always overwritten by the daemon at ingress to prevent forgery and ensure ordering.

### 5.1 Kind semantics

- **message** — one-way notification to `to`. No response expected.
- **ask** — RPC request to `to`. Caller blocks (HTTP holds connection open; MCP `ask` tool blocks) until a `reply` with matching `request_id` arrives or `timeout_ms` elapses.
- **reply** — response to an earlier `ask`. `request_id` matches the ask's `id`. `from` matches the original `to`; `to` matches the original `from`.
- **event** — broadcast notification with no specific recipient. Goes only to SSE subscribers (and the JSONL log).

### 5.2 Identity and addressing

- Instances are addressed by client-provided `instance_id`. Format: `[A-Za-z0-9_.:-]{1,128}`.
- Registration is exclusive: collisions are rejected.
- External programs that talk without registering use `from: "ext:<free-form label>"`. They cannot be addressed by others (no inbox) but can `send`, `ask`, and subscribe to events.

## 6. Components

### 6.1 `instance::Registry`

- `HashMap<InstanceId, Instance>` guarded by `RwLock`.
- `Instance` holds: `id`, `alias` (optional), `registered_at`, mailbox sender, owner-connection handle.
- Owner-connection handle ties registration to a single underlying connection (Unix socket for MCP shim; HTTP keep-alive for REST registration). When the connection drops, the daemon auto-unregisters. No TTL or heartbeat.
- `register(id)` rejects on collision unless the request arrives on the same owner connection (idempotent re-register).

### 6.2 `mailbox::Mailbox`

- Bounded `tokio::sync::mpsc` channel per instance, default capacity 256, configurable at registration.
- On overflow: drop the oldest queued envelope; emit a synthetic broadcast `event` `{type: "dropped", instance_id, dropped_id}` so observers can see the loss.
- Close on auto-unregister; any blocked `await_message` returns `instance_closed`.

### 6.3 `router`

- `send(env)` → look up `to` in registry, push to mailbox; if `kind=ask`, register a `oneshot::Sender` keyed by `env.id` into `pending: HashMap<RequestId, (oneshot::Sender<Value>, deadline)>` and arm a timeout task.
- `reply(env)` → look up `env.request_id` in `pending`; if found, consume the oneshot, send the payload; if not, return `unknown_request_id` and log.
- On instance unregister: cancel all `pending` whose `from` or `to` equals that instance with error `instance_disconnected`.

### 6.4 `eventlog::JsonlLog`

- Append-only JSONL at `$XDG_STATE_HOME/mcp-bus/events.jsonl` (default).
- Each line is one envelope as serialized JSON.
- Writes use a single `write` syscall per line (under `PIPE_BUF` ≈ 4 KB on POSIX → atomic; oversized payloads are still atomic at the application level because the daemon is the sole writer, but a soft size cap on `payload` (default 64 KB) is enforced at ingress).
- No fsync per event (best-effort durability; documented).
- Replay: `since=<ts>` linear scan from the start (good enough at v1 sizes). Rotation is a v1.x concern, not v1.
- Parse errors on a line: skip the line, log a warning.
- File truncation (`fi.size() < offset`): reset offset to 0.

### 6.5 HTTP surface (REST)

All endpoints are versioned under `/v1` and bound to `127.0.0.1`.

```
POST   /v1/instances                   register {instance_id, mailbox_size?}
DELETE /v1/instances/{id}              unregister
GET    /v1/instances                   list active instances

GET    /v1/instances/{id}/inbox        SSE — envelopes addressed to {id}
POST   /v1/instances/{id}/messages     send a message to {id}
POST   /v1/instances/{id}/ask          ask {id}; HTTP blocks until reply or timeout
POST   /v1/instances/{id}/replies      reply to an ask {request_id, payload}

GET    /v1/events                      SSE — global broadcast + history replay
                                       query: since=<ts>, instance=<id>, kind=<kind>
POST   /v1/events                      publish a broadcast event from external
```

- The registration on `POST /v1/instances` is bound to the HTTP connection's lifetime via a keep-alive long-poll: the response is `200 OK` with `Connection: keep-alive` and the body is an SSE-style heartbeat stream. Closing the connection unregisters. Alternative: explicit `DELETE` if the client uses pooled connections (a registration token in `WWW-Authenticate`-style header binds future requests to the registration).
- `POST /v1/instances/{id}/ask` accepts `?timeout_ms=` (default 30_000, max 24h). On timeout returns `504` `{error: "timeout", request_id}`.
- `GET /v1/instances/{id}/inbox` requires the caller to be the registered owner (matched by connection). A different connection requesting another instance's inbox returns `403`.

### 6.6 SSE surface

- `GET /v1/events`: replay-then-live. Daemon snapshots the log offset at subscribe time, replays matching events up to that offset, then attaches the subscriber to live broadcasts. No gap, no duplicate.
- Per-subscriber bounded broadcast channel (capacity 64). On full: drop, and emit once `{type: "slow_subscriber"}` to that subscriber. Other subscribers and publishers are unaffected.
- Server detects disconnect via `tokio::sync::oneshot` / response future cancellation and cleans up the subscriber.

### 6.7 MCP shim (`mcp-bus-stdio`)

Tools exposed:

| Tool | Purpose |
|---|---|
| `register(instance_id, mailbox_size?)` | claim ID for this session |
| `unregister()` | release ID early (also happens on shim exit) |
| `await_message(timeout_secs?)` | block until a message arrives, or return empty on timeout |
| `check_inbox()` | non-blocking drain (returns 0..N envelopes) |
| `reply(request_id, payload)` | answer an inbound `ask` |
| `send(to, payload)` | one-way message to another instance |
| `ask(to, payload, timeout_secs?)` | RPC; blocks until reply or timeout |
| `publish_event(kind, payload)` | broadcast to all SSE subscribers |
| `list_instances()` | enumerate active instances |

The shim connects to the daemon's Unix socket at startup. Reconnect with exponential backoff (200 ms, 500 ms, 1 s, 3 s, 3 s …) for the first 5 s; thereafter, each tool call returns `{code: "daemon_unavailable", retryable: true}` immediately. The shim never crashes the MCP client.

### 6.8 Hook-driven inbox (third inbound mode)

For workflows that cannot or do not want to use blocking `await_message`, the daemon optionally writes envelopes addressed to a given instance to `$INBOX_DIR/<instance_id>.jsonl` and a shipped reference hook script reads them at `SessionStart` / `UserPromptSubmit` time, injecting the content as context.

- Daemon opens the file `O_CREATE|O_APPEND` and writes a single line per envelope.
- Hook script: atomically rename `inbox.jsonl` → `inbox.processing.<pid>`, read, format, emit hook output, delete. Daemon will recreate the file on next write.
- Daemon never truncates; only the hook script does.
- The reference hook script ships in `scripts/inject-inbox.sh` (or `.ts`) as a starting point, not as a binary.

### 6.9 CLI client (`mcp-bus`)

Thin axum-client wrapper:

```
mcp-bus ls                                    # list instances
mcp-bus send <to> [-f file | -]               # send a message
mcp-bus ask  <to> [-f file | -] [--timeout 30s]
mcp-bus tail [--instance <id>] [--since <ts>] # SSE viewer
mcp-bus reply <request_id> [-f file | -]
mcp-bus rm <id>                               # unregister someone (admin)
```

## 7. End-to-end flows

### 7.1 AI ↔ AI

1. Both Claudes register: `register("orch")`, `register("impl")`.
2. `orch` calls `ask("impl", {task: "..."}, timeout_secs: 600)`.
3. Daemon enqueues `kind=ask` envelope into `impl`'s mailbox; allocates oneshot keyed by envelope id.
4. `impl`'s `await_message` returns the envelope; `impl` processes and calls `reply(request_id, {result})`.
5. Daemon resolves oneshot; `orch`'s blocking `ask` returns.

### 7.2 External script ↔ AI

```
curl -X POST http://127.0.0.1:PORT/v1/instances/impl-ENG-123/messages \
     -H 'content-type: application/json' \
     -d '{"payload": {"hint": "use TDD"}}'
```

AI receives via `await_message`. No reply expected.

### 7.3 AI ↔ human via Slack

1. Slack bridge program: `POST /v1/instances {instance_id: "slack-ask"}` (registration; keep-alive holds it).
2. Bridge opens `GET /v1/instances/slack-ask/inbox` SSE.
3. AI: `ask("slack-ask", {question, options}, timeout_secs: 1800)`.
4. Bridge receives envelope `kind=ask, request_id=...`.
5. Bridge posts to Slack with interactive buttons; `block_id` carries `request_id`.
6. Human clicks → Slack interaction webhook → bridge.
7. Bridge: `POST /v1/instances/slack-ask/replies {request_id, payload: {choice: "deploy"}}`.
8. Daemon resolves oneshot; AI's `ask` returns the human's choice.

Failure: if the bridge disconnects mid-flight, the daemon cancels pending asks targeting `slack-ask` with `instance_disconnected`. AI sees the failure immediately rather than waiting for timeout.

### 7.4 Replay for a late-joining dashboard

`GET /v1/events?since=2026-05-21T08:00:00Z&instance=impl-ENG-123` — daemon replays all matching envelopes from the JSONL log, then keeps the stream open for live events. Dashboard dedups by envelope `id`.

## 8. Error handling

### 8.1 Daemon unavailable (shim side)

- Reconnect with backoff for 5 s.
- After that, every MCP tool call returns `{code: "daemon_unavailable", retryable: true}`.
- Shim never panics or exits; the MCP client stays up.

### 8.2 Instance ID collision

- `register` returns `{code: "instance_id_taken"}`.
- Idempotent re-register from the same owner connection succeeds.

### 8.3 Stale registrations

- Tied to owner connection. Connection drop (shim crash, AI client exit, REST disconnect) auto-unregisters and cancels in-flight asks.
- No heartbeat protocol.

### 8.4 Mailbox overflow

- Drop oldest. Emit `{kind: "event", payload: {type: "dropped", instance_id, dropped_id}}` once per drop.
- Per-instance capacity configurable at `register`. Default 256.

### 8.5 `ask` timeout

- Daemon awaits up to `timeout_ms` (default 30 s, max 24 h).
- On timeout: HTTP returns `504` `{error: "timeout", request_id}`; oneshot is canceled; the entry is removed from `pending`.
- Late `reply` for an unknown `request_id` returns `unknown_request_id` and is dropped.

### 8.6 SSE replay correctness

- Snapshot log offset at subscribe time. Replay strictly before offset, then attach to live broadcast.
- Each envelope has a unique `id`; clients dedup defensively across reconnects.

### 8.7 Slow SSE subscriber

- Per-subscriber bounded channel (64). Full → drop the event for that subscriber, emit a `slow_subscriber` event to that subscriber once, and continue.
- Other subscribers and publishers unaffected.

### 8.8 JSONL log integrity

- Skip and warn on unparseable lines.
- On truncation (`size < offset`): reset offset to 0.
- Single-writer (the daemon) so no cross-process append contention in v1.

### 8.9 Hook-injection mode races

- Daemon writes with `O_APPEND`. Hook script reads only after atomic rename; daemon never truncates.

### 8.10 Security (v1)

- Binds `127.0.0.1` only. No auth. Unix socket permissions `0600`.
- Daemon refuses to start when `--bind` is a non-loopback address; the error message points at the future auth/TLS work.
- Payload size cap (default 64 KB) at ingress to bound per-line log size and per-event memory.

### 8.11 Graceful shutdown

- SIGTERM: stop accepting new connections; drain in-flight RPCs up to 5 s; fsync the JSONL log; send `event: shutdown\ndata: {}` to SSE subscribers; close listeners; exit.

## 9. Configuration

Environment variables, all optional:

| Var | Default | Meaning |
|---|---|---|
| `MCP_BUS_PORT` | `8765` | HTTP port (loopback) |
| `MCP_BUS_SOCKET` | `$XDG_RUNTIME_DIR/mcp-bus.sock` | Unix socket path for shim |
| `MCP_BUS_LOG_PATH` | `$XDG_STATE_HOME/mcp-bus/events.jsonl` | JSONL log location |
| `MCP_BUS_LOG_MAX_PAYLOAD` | `65536` | reject envelopes whose `payload` exceeds this byte size |
| `MCP_BUS_INBOX_DIR` | `$XDG_RUNTIME_DIR/mcp-bus/inbox` | hook-injection inbox directory |
| `MCP_BUS_DEFAULT_TIMEOUT_MS` | `30000` | default `ask` timeout |
| `MCP_BUS_MAX_TIMEOUT_MS` | `86400000` | maximum allowed `ask` timeout (24 h) |
| `RUST_LOG` | `info,mcp_bus=debug` | tracing filter |

CLI flags on `mcp-busd` override the corresponding env vars.

## 10. Repository layout

```
mcp-bus/
├─ Cargo.toml                    # workspace
├─ crates/
│  ├─ mcp-bus-core/              # envelope, registry, mailbox, router, eventlog
│  ├─ mcp-busd/                  # daemon binary (axum HTTP+SSE, IPC server)
│  ├─ mcp-bus-stdio/             # MCP stdio shim binary
│  └─ mcp-bus-cli/               # CLI binary
├─ scripts/
│  ├─ smoke-curl.sh
│  ├─ smoke-extbot.sh            # exercises with a stub MCP client
│  └─ inject-inbox.sh            # reference hook for option 3
├─ docs/
│  ├─ README.md
│  ├─ protocol.md                # envelope + REST + MCP tool surface
│  ├─ examples/
│  │  ├─ slack-bridge.md
│  │  └─ extbot-integration.md
│  └─ superpowers/specs/2026-05-21-mcp-bus-design.md
└─ tests/                        # integration tests (cargo nextest)
```

## 11. Testing strategy

### 11.1 Unit

- `envelope` — serde roundtrip; ULID and timestamp invariants; field combinations per kind.
- `mailbox` — overflow drops oldest + emits `dropped`; close behavior; backpressure with bounded receivers.
- `registry` — collision rejection; auto-unregister on owner-connection drop; idempotent re-register from same owner.
- `router` — `pending` oneshot lifecycle (resolve / timeout / disconnect); unknown `request_id` rejected; late replies discarded.
- `eventlog` — append; parse with skip-on-corrupt; truncation reset; `since=ts` linear scan.
- `sse` — replay-then-live no gap / no dup; slow-subscriber single-shot warning.

### 11.2 Integration

- End-to-end REST: register → POST message → SSE inbox receives → reply → POST ask response received.
- MCP shim driven over stdio: spawn the shim binary in tests, drive it as a real MCP server.
- AI↔AI: two registered instances; A.ask(B) → B.reply, A receives.
- External-instance scenario (Slack-bridge stand-in): register over REST, AI asks, fake bridge replies after delay.
- Daemon restart with persistent log: subscribe with `since=`, get replay then live.
- Owner-disconnect unregister: kill shim mid-session, registry entry gone, ask to that ID returns `unknown_instance`.

### 11.3 Property

- Envelope serde: any valid envelope roundtrips.
- Mailbox: random publish/drain/overflow sequences preserve FIFO for delivered items and accurate drop count.

### 11.4 Concurrency

- `loom` (or `shuttle`) over the registry's register/unregister/lookup paths and the router's pending map. Targeted, narrow models — not the whole system.

### 11.5 Manual smoke

- `scripts/smoke-curl.sh` — bash + curl reproduces the REST flow end-to-end.
- `scripts/smoke-extbot.sh` — exercises the external-bot orchestrator → mcp-bus path with a stub MCP client.

### 11.6 CI

- `cargo test --workspace --all-features`
- `cargo nextest run` for integration tests with ephemeral port allocation
- `cargo clippy --workspace -- -D warnings`
- `cargo fmt --all -- --check`
- `cargo deny check`

## 12. Open questions deferred to implementation planning

These are intentionally not resolved here; they belong in the implementation plan or v1.x.

- Exact registration-binding mechanism on REST (`Connection: keep-alive` long-poll vs registration token) — pick during implementation after a quick spike on `axum` connection lifecycles.
- Log rotation policy (size threshold + max retained files) — sized for v1.x once usage shapes are known.
- Whether `mcp-bus-cli` should also be able to register as an instance (acting as an interactive terminal-attached human peer), or only as a transient talker. The protocol supports both; the CLI surface is a UX decision.
- Whether to ship a `mcp-bus-bridge` helper binary that turns an SSE inbox into a Unix-pipe stream for shell integration — likely yes, but out of scope for v1.

## 13. Future work (explicitly post-v1)

- Authentication (bearer tokens, mTLS).
- Non-loopback binding with TLS termination.
- Multi-host federation (daemons relaying between hosts).
- Durable message store with delivery guarantees beyond replay.
- Reference bridges shipped in the repo (Slack, Discord, email, PagerDuty).
- Web dashboard (separate repo, depends only on REST + SSE).
