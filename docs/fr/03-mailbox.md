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

- The mailbox is a bounded in-memory queue (`VecDeque` guarded by a mutex),
  default capacity 256, configurable per instance at registration time via
  `mailbox_size`.
- Delivered envelopes preserve FIFO order.
- On overflow, the mailbox evicts the *oldest* queued envelope to make room for
  the new one. The push call returns the evicted envelope's id to the caller
  (`Pushed { dropped: Some(id) }`); the eviction is not otherwise observable.
- On auto-unregister the mailbox is closed; any blocked `await_message` on it
  resolves with a closed error (`RecvError::Closed`, message `"mailbox closed"`)
  rather than hanging.
- A consumer drains the mailbox either by blocking (`await_message`) or by a
  non-blocking drain (`check_inbox`) — see fr:08-mcp-shim and fr:07-sse.

## Capabilities

- Bounded memory per instance — overflow can never grow the queue.
- FIFO ordering for every envelope that is actually delivered.
- The evicted envelope's id is returned to the caller of `push`, so the
  enqueueing path can observe which id was dropped.
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
- Spec §8.4 intended overflow to emit an observable synthetic broadcast
  `event {type: "dropped", instance_id, dropped_id}`; the current
  implementation does not do this — the eviction is silent to observers and the
  evicted id is only returned to the caller of `push`.

## Error Handling

- Mailbox overflow (spec §8.4): evict the oldest queued envelope and return its
  id to the caller of `push` (`Pushed { dropped: Some(id) }`). Capacity is
  configurable at `register`; default 256. See Boundaries for the unimplemented
  synthetic `dropped` event.
- Mailbox close: a closed mailbox resolves a blocked `await_message` with a
  closed error (`RecvError::Closed`, message `"mailbox closed"`) rather than
  hanging.

## Traceability

- Related FR: fr:02-instance-registry, fr:04-router

## When to update

- The default mailbox capacity or the overflow policy changes.
- The eviction-id reporting (or a synthetic `dropped` event) changes.
- The close behavior or its surfaced error changes.
- The mailbox gains persistence or delivery guarantees.
