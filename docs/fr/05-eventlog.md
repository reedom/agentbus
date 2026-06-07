---
refs:
  id: fr:05-eventlog
  kind: fr
  title: "Ordered event log and cursor replay"
  related:
    - fr:01-envelope
    - fr:04-router
  modules:
    - crates/agentbus-core/src/store/events.rs
---

# FR 05: Ordered event log and cursor replay

> One SQLite table records every envelope in strict sequence; cursor reads page through it without gaps or duplicates.

## Purpose

The `event_log` table in `bus.db` is the authoritative audit trail and replay
substrate for all envelopes crossing the bus. Because `seq` is assigned inside
the same `BEGIN IMMEDIATE` transaction that writes the envelope, sequencing is
immune to wall-clock skew between writer processes. The v0.1 guarantee
(snapshot-replay-then-live with no gap and no duplicate) is trivially satisfied:
the table itself enforces it by construction.

## User-visible Behavior

- Every `send`, `ask`, `reply`, and `publish_event` call appends the envelope
  to `event_log` inside its transaction, receiving a monotonically increasing
  `seq` assigned by SQLite's `rowid`.
- `events_since(after_seq, limit, filter)` returns a page of rows with
  `seq > after_seq`, oldest first, up to `limit` rows. The returned `cursor`
  value advances past EVERY scanned row, including those filtered out, so a
  follow loop that passes `cursor` as the next `after_seq` never rescans a row.
- Filters (applied post-deserialization, non-exclusive):
  - `instance`: envelopes whose `from` OR `to` equals the given id.
  - `kind`: envelopes of exactly this kind (`message`, `ask`, `reply`, `event`).
  - `to`: envelopes addressed TO this id only (watch mode — fr:10-cli `watch`).
- Corrupt rows (unparseable JSON) are skipped with a `tracing::warn`; the
  cursor still advances past them so follow loops make progress.
- `max_seq()` returns the current high-water mark (0 on an empty log).

## Capabilities

- Strict, gap-free ordering by construction (no external coordination needed).
- Cursor-based pagination that is safe to resume across restarts.
- Three orthogonal filter dimensions: sender/recipient identity, message kind,
  and recipient-only (watch mode).
- Corrupt-row tolerance without stalling the consumer.

## Boundaries

- No payload size limit in v0.2 (v0.1 had a `max_payload` cap of 64 KB).
  Reintroduce if abuse or excessive row sizes become a problem.
- No log rotation or compaction. The table grows unboundedly until an operator
  manually prunes or archives it.
- The log is not a durable delivery queue; it has no acknowledgement or
  redelivery semantics. Inbox delivery is the spool file (fr:09-hook-inbox).
- No `fsync` per row — SQLite WAL provides OS-level durability on clean shutdown
  but not on hard power loss.

## Error Handling

- Unparseable JSON in a row: skipped with a warning; cursor advances normally.
- SQLite errors surface as `StoreError::Sqlite`.

## Traceability

- Related FR: fr:01-envelope, fr:04-router

## When to update

- The `event_log` schema changes (new columns, index, retention policy).
- Log rotation or compaction is introduced.
- The cursor semantics change (e.g. filtered-out rows no longer advance cursor).
- A payload size limit is added or removed.
- The filter set is extended or its semantics change.
