---
refs:
  id: fr:04-router
  kind: fr
  title: "Message delivery: send, ask, reply"
  related:
    - fr:01-envelope
    - fr:02-instance-registry
    - fr:09-hook-inbox
  modules:
    - crates/agentbus-core/src/store/messages.rs
---

# FR 04: Message delivery: send, ask, reply

> Sender-performed delivery over the spool store: recipient check, event log, inbox append, and ask/reply correlation.

## Purpose

In v0.2 there is no daemon router. The sender process performs delivery
directly: one `BEGIN IMMEDIATE` transaction checks the recipient and logs the
envelope; the inbox append and on-delivery hook run outside the transaction
after commit. Ask/reply correlation is managed via an `asks` table in `bus.db`.

## User-visible Behavior

### send

1. A `BEGIN IMMEDIATE` transaction:
   - Calls `on_delivery_of(tx, to)` — fails with `unknown_instance` if the
     recipient row does not exist.
   - Appends the envelope to `event_log`.
   - Commits.
2. After commit, the sender appends the envelope to the recipient's inbox spool
   (`~/.agentbus/inbox/<to>.jsonl`).
3. If `on_delivery` is set on the recipient's row, the sender spawns and awaits
   the hook process.

Partial failure: if the spool append fails after the event_log commit, the
envelope is logged but not delivered. `agentbus sweep` re-fires on_delivery
hooks for stuck inboxes. A retry produces a NEW envelope id — there is no
idempotency key.

### ask

Same as send, plus:

- The transaction also inserts an `asks` row:
  `(request_id = envelope.id, from_id, to_id, expires_at = ts + timeout)`.
- After commit and inbox append, the sender polls `asks.reply_payload` in a
  loop with 50 ms starting delay, doubling to a 250 ms cap, until a reply
  appears or the monotonic deadline elapses.
- On expiry, `StoreError::Timeout(request_id)` is returned; the `asks` row is
  NOT deleted — `agentbus ask-result <id>` can retrieve a late reply.

Two-clock drift: the poll deadline is monotonic (starts after delivery work),
while `expires_at` is wall-clock (anchored at envelope timestamp). A just-
timed-out ask may briefly read `Pending` from `ask_result`.

### reply

1. A `BEGIN IMMEDIATE` transaction:
   - Looks up `from_id` from `asks WHERE request_id = ?`.
     Unknown request_id → `unknown_request_id`.
   - `UPDATE asks SET reply_payload = ?, replied_at = ? WHERE request_id = ?
     AND reply_payload IS NULL` — first write wins; concurrent repliers all
     succeed at the UPDATE level (zero rows affected is not an error).
   - Appends a Reply envelope to `event_log`.
   - Commits.
2. No inbox write — the asker reads `asks.reply_payload` directly.

Every reply (including losing concurrent repliers) is appended to the event_log.
The `asks` row stays after the reply so `ask_result` can read it later.

## Capabilities

- Transactional recipient verification and event logging.
- Sender-side inbox append decoupled from the transaction (enables async
  append failure recovery via sweep).
- First-write-wins reply semantics with full event_log audit trail.
- Late reply retrieval: timed-out asks are retrievable via `ask_result`.
- 50 ms → 250 ms backoff poll keeps ask load low.

## Boundaries

- No acknowledgement or redelivery beyond sweep re-firing hooks.
- A retry after spool failure stamps a new envelope id — there is no
  idempotency guarantee.
- `publish_event` (broadcast) is covered by fr:05-eventlog; `await_message`
  and `check_inbox` are covered by fr:09-hook-inbox.
- The two-clock drift on ask expiry is a known limitation (see above).

## Error Handling

- `unknown_instance`: `send` or `ask` finds no row for the recipient in
  `instances`; the transaction is rolled back and nothing is logged.
- `unknown_request_id`: `reply` or `ask_result` finds no row for the given
  request_id in `asks`.
- `StoreError::Timeout(request_id)`: `ask` deadline elapsed before a reply
  arrived; the `asks` row remains for late retrieval.

## Traceability

- Related FR: fr:01-envelope, fr:02-instance-registry, fr:09-hook-inbox

## When to update

- The send transaction steps change (e.g. spool append moves inside the tx).
- The ask poll backoff parameters change.
- The `asks` row retention policy changes (currently: never deleted).
- Sweep re-delivery semantics change.
- The two-clock drift mitigation is implemented.
