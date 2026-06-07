---
refs:
  id: fr:10-cli
  kind: fr
  title: "agentbus CLI"
  related:
    - fr:04-router
    - fr:05-eventlog
    - fr:09-hook-inbox
  modules:
    - crates/agentbus-cli/src/main.rs
    - crates/agentbus-cli/src/commands.rs
---

# FR 10: agentbus CLI

> A thin synchronous wrapper over the spool store for humans and shell scripts.

## Purpose

The `agentbus` binary gives humans and shell scripts direct access to the bus
without writing code. In v0.2 it opens the store directly (`--dir` /
`AGENTBUS_DIR`, default `~/.agentbus`) — there is no daemon or REST layer.
Every subcommand maps to one store call. Payload is read from `--file` or
stdin; `--from` defaults to `ext:cli`.

## User-visible Behavior

### Verb table

| Verb | Key flags | Semantics |
|---|---|---|
| `register` | `--persistent`, `--on-delivery`, `--pid` | Register an instance id; `--pid <pid>` anchors the non-persistent row to a long-lived process (e.g. an AI harness) for session-scoped identity; `--persistent` for durable addresses; `--pid` and `--persistent` conflict |
| `unregister` | | Remove a registration; inbox file is kept |
| `ls` | | List instances with liveness |
| `send` | `--from`, `--file` | One-way message; prints `{"id": "..."}` |
| `ask` | `--from`, `--file`, `--timeout-ms` | RPC; prints pretty-JSON reply on success; exits 2 on timeout |
| `ask-result` | | Fetch status of an earlier ask (Pending / Replied / Expired); retrieves late replies after a timeout |
| `reply` | `--file` | Answer an ask as `<from>` |
| `check-inbox` | | Drain inbox without blocking; prints `{"envelopes": [...]}` |
| `await` | `--timeout-ms` | Block until messages arrive; prints `{"envelopes": [...]}` (empty list on timeout) |
| `publish` | `--from`, `--file` | Append a broadcast event; prints `{"id": "..."}` |
| `events` | `--follow`, `--since`, `--instance`, `--kind`, `--interval-ms` | Read event log as `{"seq":..,"envelope":..}` lines; `--follow` polls indefinitely |
| `watch` | `--interval-ms` | Stream envelopes addressed to one instance, one compact JSON envelope per line; starts at current max seq (no replay); never consumes the inbox; for harness monitor tools |
| `sweep` | `--purge-orphans`, `--grace-secs` | Crash recovery: prune dead registrations, re-fire stale hooks, report expired asks |

### Output format

- Single results: pretty JSON on stdout.
- Streams (`events`, `watch`) and small acks: compact JSON lines on stdout.
- `send` hook warnings: stderr only (does not affect exit code).

### Exit codes

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Store or other error; `error[<code>]: <message>` printed on stderr |
| 2 | `ask` timeout; stderr includes `agentbus ask-result <id>` hint. Note: clap usage errors also exit 2 |

## Capabilities

- Full bus access (send, ask, reply, events, inbox, sweep) without a daemon.
- `watch` for harness monitor tools: live event-log tail filtered to one
  recipient, bare envelope per line, never touching the inbox file.
- `ask-result` for late-reply retrieval after an ask has timed out.
- `sweep` for crash recovery and orphan cleanup.

## Boundaries

- A bare non-persistent `register` records the CLI process's pid, which
  exits immediately — the row is instantly dead. Session-scoped identity
  uses `--pid <session-pid>` (the primary AI path, taught by the agentbus
  skill); durable addresses use `--persistent`. The shim (fr:08-mcp-shim)
  remains an alternative anchor for MCP-only clients.
- The CLI adds no logic of its own beyond argument parsing and output
  formatting; all semantics live in the store.
- It does not expose MCP tools; that surface is the shim (fr:08-mcp-shim).
- It does not interpret `payload` content.

## Error Handling

- Store errors are caught, printed as `error[<code>]: <message>` on stderr,
  and exit 1.
- `ask` timeout is a special case: exit 2 with a hint to use `ask-result`.
- `on_delivery` hook warnings from `send` are printed to stderr but do not
  change the exit code.

## Traceability

- Related FR: fr:04-router, fr:05-eventlog, fr:09-hook-inbox

## When to update

- A subcommand is added, removed, or its flags change.
- Output shapes change (pretty vs compact, field names).
- Exit code assignments change.
- The `--dir` / `AGENTBUS_DIR` default changes.
- The `--pid` anchoring contract changes (e.g. pid validation or a
  `--pid auto` mode is added).
