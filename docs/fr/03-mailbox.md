---
refs:
  id: fr:03-mailbox
  kind: fr
  title: "Per-instance mailbox"
  related:
    - fr:02-instance-registry
    - fr:04-router
  modules:
    - crates/agentbus-core/src/mailbox.rs
---

# FR 03: Per-instance mailbox

> A bounded in-memory queue per registered instance, holding envelopes directed at it.

## Purpose

Every registered instance has a mailbox: a bounded queue that buffers envelopes
addressed to it between the moment the router enqueues them and the moment the
instance drains them. The bound is what keeps a slow or absent consumer from
growing daemon memory without limit.

## User-visible Behavior

- The mailbox is a bounded `tokio::sync::mpsc` channel, default capacity 256,
  configurable per instance at registration time via `mailbox_size`.
- Delivered envelopes preserve FIFO order.
- On overflow, the mailbox drops the *oldest* queued envelope to make room for
  the new one, then emits a synthetic broadcast `event`
  `{type: "dropped", instance_id, dropped_id}` so observers can see the loss.
- On auto-unregister the mailbox is closed; any blocked `await_message` on it
  resolves with `instance_closed`.
- A consumer drains the mailbox either by blocking (`await_message`) or by a
  non-blocking drain (`check_inbox`) — see fr:08-mcp-shim and fr:07-sse.

## Capabilities

- Bounded memory per instance — overflow can never grow the queue.
- FIFO ordering for every envelope that is actually delivered.
- Loss is observable: every drop emits exactly one `dropped` event naming the
  instance and the dropped envelope's id.
- Per-instance capacity tuning at registration for instances with bursty or
  slow consumers.
- Clean close semantics so consumers learn promptly when their mailbox ends.

## Boundaries

- The mailbox does not persist; queued envelopes are lost on daemon exit.
- It does not retry, redeliver, or guarantee delivery — overflow drops are
  permanent (durability beyond best-effort + replay is out of scope, §1.1).
- It does not route or address; it only buffers what the router hands it
  (routing is fr:04-router).
- It does not apply per-message priority — strictly FIFO with oldest-drop.
- `event` envelopes are not mailboxed; they go to SSE subscribers only.

## Error Handling

- Mailbox overflow (spec §8.4): drop the oldest queued envelope and emit one
  `{kind: "event", payload: {type: "dropped", instance_id, dropped_id}}` per
  drop. Capacity is configurable at `register`; default 256.
- Mailbox close: a closed mailbox resolves a blocked `await_message` with
  `instance_closed` rather than hanging.

## Traceability

- Related FR: fr:02-instance-registry, fr:04-router

## When to update

- The default mailbox capacity or the overflow policy changes.
- The `dropped` event shape changes.
- The close behavior or its surfaced error changes.
- The mailbox gains persistence or delivery guarantees.
