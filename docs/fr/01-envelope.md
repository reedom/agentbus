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

> The single canonical JSON envelope carried uniformly across the MCP shim, the CLI, the inbox spool files, and the event log.

## Purpose

agentbus uses one message format on every surface. The envelope is the contract
between any pair of participants — AI instances, humans on the CLI, and external
programs — and between the store and its own log. A uniform format means a
caller learns the shape once and reuses it for MCP tools, CLI verbs, hook-inbox
consumption, and event replay.

## User-visible Behavior

Every message is an envelope with these fields:

| Field | Required for | Notes |
|---|---|---|
| `id` | all | ULID, stamped by the sending process |
| `kind` | all | `message`, `ask`, `reply`, `event` |
| `from` | all | sender `instance_id`, or `ext:<label>` for unregistered talkers |
| `to` | `message`, `ask`, `reply` | absent / null for broadcast `event` |
| `request_id` | `reply` (required), optional on `ask` | correlates a reply to its ask |
| `timeout_ms` | `ask` | honored verbatim; no clamping (CLI and shim default to 30 000) |
| `ts` | all | RFC3339 UTC, stamped by the sending process |
| `payload` | all | opaque JSON; the bus never interprets it |

`id` and `ts` are always stamped by the sender's store call, regardless of
what the caller supplied at the tool/CLI layer. Global ordering does not
depend on `ts`: the event log's transactionally assigned `seq` is the
authoritative order (fr:05-eventlog).

Kind semantics:

- **message** — one-way notification to `to`; no response expected.
- **ask** — RPC request to `to`; the caller blocks until a matching `reply` or
  until `timeout_ms` elapses.
- **reply** — answer to an earlier `ask`; `request_id` matches the ask's `id`,
  `from`/`to` are the reverse of the ask.
- **event** — broadcast with no recipient; reaches the event log, `events
  --follow`, and `watch` streams.

## Capabilities

- One envelope type serialized identically on every surface.
- ULID `id` is sortable and collision-safe; it doubles as a dedup key.
- Compact JSON serialization keeps one envelope on one line (newlines inside
  strings are escaped), which the spool and stream surfaces rely on.
- `payload` is fully opaque — any JSON value is accepted and passed through.
- `ext:<label>` addressing lets unregistered programs participate as senders.

## Boundaries

- The bus never inspects, validates, or transforms `payload` contents.
- The envelope carries no schema, versioning, or content-type for `payload` —
  that is a concern for the participants.
- No authentication or signing of envelopes; identity in `from` is asserted,
  not verified. The trust boundary is the 0700 store directory (fr:12-store):
  every participant is the same OS user.
- The envelope does not encode delivery guarantees; durability comes from the
  event log and inbox spool (fr:05-eventlog, fr:09-hook-inbox).
- There is no payload size limit in v0.2. v0.1's daemon enforced a 64 KB cap
  at ingress; the spool model dropped it (no resident process to protect).
  Reintroduce a cap if oversized payloads become a problem (fr:05-eventlog
  Boundaries).

## Error Handling

- Structural validation (`Envelope::validate`): `ask`/`message` require `to`;
  `reply` requires `to` and `request_id`; `event` must not have `to`; `from`
  and `id` must be non-empty. Violations surface as `invalid_envelope`.
- `timeout_ms` is not validated or clamped; absurd values are honored.

## Traceability

- Reference docs: ref:protocol
- Related FR: fr:04-router

## When to update

- A field is added, removed, or renamed on the envelope.
- A new `kind` value is introduced or kind semantics change.
- The id scheme (ULID) or timestamp format changes.
- A payload size limit is introduced.
