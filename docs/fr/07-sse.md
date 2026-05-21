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
- `GET /v1/instances/{id}/inbox` streams envelopes addressed to `{id}`. It does
  a registry lookup on `{id}`: if the instance is unknown it returns
  `404 unknown_instance`; otherwise it drains and streams that instance's
  mailbox to any local caller (no owner check — see Boundaries).
- All `/v1/events` subscribers share a single `tokio::sync::broadcast` channel
  (capacity 256). Each subscriber holds its own receiver into that shared
  channel; when a receiver falls behind far enough that the channel laps it,
  the daemon emits a single `{type: "slow_subscriber"}` notice to that
  receiver and continues. Other subscribers and publishers are unaffected.
- The server detects disconnect via response-future cancellation and cleans up
  the subscriber.
- Because every envelope has a unique `id`, clients should still dedup
  defensively across reconnects.

## Capabilities

- Gapless replay-then-live cutover anchored on a snapshotted log offset.
- Server-side `since` / `instance` / `kind` filtering.
- Per-receiver isolation over a shared broadcast channel — one slow consumer
  cannot stall others or publishers.
- Explicit, one-shot back-pressure signalling via `slow_subscriber`.
- Prompt subscriber cleanup on disconnect.

## Boundaries

- SSE does not persist or buffer beyond the shared broadcast channel; missed
  events after a `slow_subscriber` drop are recovered only via log replay.
- The inbox stream does no caller authorization: it only does a registry
  lookup on `{id}` and streams the mailbox to any local caller. Spec §6.5
  intended an owner-only check returning `403` for cross-instance reads; the
  current implementation does not do this (consistent with the v1
  loopback-only, no-auth posture).
- SSE does not deliver to mailboxes — mailbox delivery is fr:03-mailbox.
- The log scan that backs replay belongs to fr:05-eventlog; this FR consumes
  it.
- No reconnect/resume token is provided; clients reconnect with `since` and
  dedup by `id`.

## Error Handling

- Replay correctness (spec §8.6): the log offset is snapshotted at subscribe
  time; replay is strictly before that offset, then the subscriber attaches to
  live broadcasts. Unique envelope `id`s let clients dedup defensively.
- Slow subscriber (spec §8.7): subscribers share a single bounded broadcast
  channel (capacity 256). When a receiver lags far enough that the channel
  laps it, that receiver's stream emits a `{type: "slow_subscriber"}` event
  once and continues. Other subscribers and publishers are unaffected.
- Unknown inbox instance: `GET /v1/instances/{id}/inbox` returns
  `404 unknown_instance` when `{id}` is not in the registry. There is no
  caller-authorization check (see Boundaries).

## Traceability

- Reference docs: ref:protocol
- Related FR: fr:05-eventlog, fr:06-rest-api

## When to update

- The replay-then-live cutover algorithm changes.
- The shared broadcast channel capacity or drop policy changes.
- The `slow_subscriber` signal shape changes.
- A reconnect/resume token mechanism is introduced.
