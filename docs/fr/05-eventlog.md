---
refs:
  id: fr:05-eventlog
  kind: fr
  title: "Append-only JSONL event log and replay"
  related:
    - fr:01-envelope
    - fr:07-sse
  modules:
    - crates/agentbus-core/src/eventlog.rs
---

# FR 05: Append-only JSONL event log and replay

> A single append-only JSONL file recording every envelope, scannable for replay.

## Purpose

The event log is the daemon's persistence and replay substrate. Every envelope
is appended as one JSON line so a late-joining subscriber can be brought up to
date by scanning history before attaching to the live stream. It provides
best-effort durability, not a durable queue.

## User-visible Behavior

- The log lives at `$XDG_STATE_HOME/agentbus/events.jsonl` by default,
  overridable via `AGENTBUS_LOG_PATH`.
- Each line is one envelope serialized as JSON.
- Each line is written with a single `write` syscall; lines under `PIPE_BUF`
  (~4 KB) are atomic, and the daemon being the sole writer keeps even larger
  lines append-consistent. The ingress payload cap (default 64 KB) bounds line
  size.
- The daemon does not `fsync` per event — durability is best-effort and
  documented as such.
- Replay: a `since=<ts>` query is served by a linear scan from the start of the
  file, sufficient at v1 sizes.

## Capabilities

- Durable-ish record of every envelope crossing the bus.
- `since=<ts>` replay for late joiners (consumed by fr:07-sse).
- Atomic per-line appends with a single-writer guarantee.
- Self-healing reads: corrupt lines are skipped, truncation resets the offset.
- Plain JSONL — readable and processable with ordinary tools.

## Boundaries

- No `fsync` per event; envelopes written shortly before a crash may be lost
  (best-effort durability, by design).
- No log rotation in v1 — rotation policy (size threshold, retained file count)
  is deferred to v1.x (spec §12).
- The log is not a durable delivery queue and offers no acknowledgement or
  redelivery semantics (spec §1.1).
- Single-writer only — the daemon is the sole writer; no cross-process append
  is supported.
- The log does not index; replay is a linear scan, acceptable only at v1
  volumes.

## Error Handling

- Log integrity (spec §8.8):
  - Unparseable line on read: skip the line and log a warning.
  - File truncation (`file size < tracked offset`): reset the offset to 0.
  - Single-writer design means there is no cross-process append contention in
    v1.

## Traceability

- Related FR: fr:01-envelope, fr:07-sse

## When to update

- The log file path default or format changes.
- Log rotation is introduced.
- The atomicity or `fsync` policy changes.
- The replay scan strategy or `since` semantics change.

