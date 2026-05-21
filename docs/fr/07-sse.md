---
refs:
  id: fr:07-sse
  kind: fr
  title: "Server-Sent Events streaming"
  related:
    - ref:protocol
    - fr:05-eventlog
    - fr:06-rest-api
  modules:
    - crates/agentbusd/src/http/events.rs
    - crates/agentbusd/src/http/inbox.rs
---

# FR 07: Server-Sent Events streaming

> Replay-then-live SSE streams for global events and per-instance inboxes.

## Purpose

SSE is how subscribers receive a continuous stream of envelopes — broadcast
events on `/v1/events` and addressed envelopes on a per-instance inbox. The key
guarantee is gapless, duplicate-free delivery across the boundary between
historical replay and the live broadcast.

## User-visible Behavior

- `GET /v1/events` is replay-then-live: at subscribe time the daemon snapshots
  the event-log offset, replays matching envelopes strictly up to that offset,
  then attaches the subscriber to the live broadcast. No gap, no duplicate.
- `/v1/events` honors the `since`, `instance`, and `kind` query filters.
- `GET /v1/instances/{id}/inbox` streams envelopes addressed to `{id}` to that
  instance's owner.
- Each subscriber has its own bounded broadcast channel (capacity 64). When it
  fills, the daemon drops events for that subscriber and emits a single
  `{type: "slow_subscriber"}` notice to it; other subscribers and publishers
  are unaffected.
- The server detects disconnect via response-future cancellation and cleans up
  the subscriber.
- Because every envelope has a unique `id`, clients should still dedup
  defensively across reconnects.

## Capabilities

- Gapless replay-then-live cutover anchored on a snapshotted log offset.
- Server-side `since` / `instance` / `kind` filtering.
- Per-subscriber isolation — one slow consumer cannot stall others or
  publishers.
- Explicit, one-shot back-pressure signalling via `slow_subscriber`.
- Prompt subscriber cleanup on disconnect.

## Boundaries

- SSE does not persist or buffer beyond the per-subscriber channel; missed
  events after a `slow_subscriber` drop are recovered only via log replay.
- The inbox stream enforces owner-only access; cross-instance inbox reads are
  rejected (see Error Handling).
- SSE does not deliver to mailboxes — mailbox delivery is fr:03-mailbox.
- The log scan that backs replay belongs to fr:05-eventlog; this FR consumes
  it.
- No reconnect/resume token is provided; clients reconnect with `since` and
  dedup by `id`.

## Error Handling

- Replay correctness (spec §8.6): the log offset is snapshotted at subscribe
  time; replay is strictly before that offset, then the subscriber attaches to
  live broadcasts. Unique envelope `id`s let clients dedup defensively.
- Slow subscriber (spec §8.7): the per-subscriber bounded channel (64) drops
  events when full, emits a `slow_subscriber` event to that subscriber once,
  and continues. Other subscribers and publishers are unaffected.

## Traceability

- Reference docs: ref:protocol
- Related FR: fr:05-eventlog, fr:06-rest-api

## When to update

- The replay-then-live cutover algorithm changes.
- The per-subscriber channel capacity or drop policy changes.
- The `slow_subscriber` signal shape changes.
- A reconnect/resume token mechanism is introduced.
