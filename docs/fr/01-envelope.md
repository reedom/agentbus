---
refs:
  id: fr:01-envelope
  kind: fr
  title: "Message envelope and wire format"
  related:
    - ref:protocol
    - fr:04-router
  modules:
    - crates/agentbus-core/src/envelope.rs
    - crates/agentbus-core/src/ids.rs
---

# FR 01: Message envelope and wire format

> The single canonical JSON envelope carried uniformly across REST, SSE, MCP stdio, and the persistent log.

## Purpose

agentbus uses one message format on every surface. The envelope is the contract
between any pair of participants — AI instances, humans on the CLI, and external
programs — and between the daemon and its own log. A uniform format means a
caller learns the shape once and reuses it for REST, SSE, MCP tools, and replay.

## User-visible Behavior

Every message is an envelope with these fields:

| Field | Required for | Notes |
|---|---|---|
| `id` | all | ULID, server-assigned at ingress |
| `kind` | all | `message`, `ask`, `reply`, `event` |
| `from` | all | sender `instance_id`, or `ext:<label>` for unregistered talkers |
| `to` | `message`, `ask`, `reply` | absent / null for broadcast `event` |
| `request_id` | `reply` (required), optional on `ask` | correlates a reply to its ask |
| `timeout_ms` | `ask` | clamped to `[1000, 86_400_000]` |
| `ts` | all | RFC3339 UTC, server-assigned at ingress |
| `payload` | all | opaque JSON; the bus never interprets it |

`id` and `ts` are always overwritten by the daemon at ingress, regardless of
what the caller supplied. This prevents id forgery and gives a single
authoritative ordering. Callers may omit both fields.

Kind semantics:

- **message** — one-way notification to `to`; no response expected.
- **ask** — RPC request to `to`; the caller blocks until a matching `reply` or
  until `timeout_ms` elapses.
- **reply** — answer to an earlier `ask`; `request_id` matches the ask's `id`,
  `from`/`to` are the reverse of the ask.
- **event** — broadcast with no recipient; reaches SSE subscribers and the log.

## Capabilities

- One envelope type serialized identically on every surface.
- ULID `id` is sortable and collision-safe; it doubles as a dedup key.
- Server-assigned `id` and `ts` give a forge-proof, monotonic ordering.
- `timeout_ms` is clamped into a safe range rather than rejected.
- `payload` is fully opaque — any JSON value is accepted and passed through.
- `ext:<label>` addressing lets unregistered programs participate as senders.

## Boundaries

- The bus never inspects, validates, or transforms `payload` contents.
- The envelope carries no schema, versioning, or content-type for `payload` —
  that is a concern for the participants.
- No authentication or signing of envelopes; identity in `from` is asserted,
  not verified (auth is post-v1 future work, spec §13).
- The envelope does not encode delivery guarantees; durability is best-effort
  plus replay (spec §1.1).
- `payload` size is bounded at ingress (default 64 KB); larger payloads are
  rejected — see Error Handling.

## Error Handling

- Payload cap (spec §8.10): the daemon rejects any envelope whose `payload`
  exceeds the configured byte limit (default 65536, `AGENTBUS_LOG_MAX_PAYLOAD`).
  The cap bounds per-line log size and per-event memory.
- `timeout_ms` outside `[1000, 86_400_000]` is clamped to the nearest bound
  rather than treated as an error.

## Traceability

- Reference docs: ref:protocol
- Related FR: fr:04-router

## When to update

- A field is added, removed, or renamed on the envelope.
- A new `kind` value is introduced or kind semantics change.
- The id scheme (ULID) or timestamp format changes.
- The default payload cap or its clamping behavior changes.
