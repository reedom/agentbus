---
refs:
  id: fr:04-router
  kind: fr
  title: "Message routing — send, ask, reply"
  related:
    - fr:01-envelope
    - fr:03-mailbox
    - ref:protocol
  modules:
    - crates/agentbus-core/src/router.rs
---

# FR 04: Message routing — send, ask, reply

> Resolves a recipient, enqueues the envelope, and correlates `ask`/`reply` RPC pairs.

## Purpose

The router is the daemon's delivery core. It takes an ingested envelope, finds
the target instance, places the envelope in that instance's mailbox, and — for
`ask` envelopes — manages the in-flight RPC: it holds the caller's pending
correlation slot and resolves it when a matching `reply` arrives or the request
times out.

## User-visible Behavior

- `send(env)` resolves `env.to` in the registry and pushes the envelope into
  that instance's mailbox. This covers `message`, `ask`, and `reply` delivery.
- For a `kind = ask` envelope, the router additionally registers a
  `oneshot::Sender` keyed by `env.id` in a `pending` map
  `HashMap<RequestId, (oneshot::Sender<Value>, deadline)>` and arms a timeout
  task for `timeout_ms`.
- `reply(env)` looks up `env.request_id` in `pending`. On a hit it consumes the
  oneshot and delivers the reply payload to the waiting caller. On a miss it
  returns `unknown_request_id` and logs.
- When an instance unregisters, the router cancels every `pending` entry whose
  `from` or `to` is that instance, resolving those asks with
  `instance_disconnected` so callers fail fast instead of waiting for timeout.
- AI↔AI flow (spec §7.1): caller `ask`s, the daemon mailboxes the ask for the
  callee and allocates the oneshot; the callee `reply`s; the daemon resolves
  the oneshot and the caller's blocking `ask` returns.

## Capabilities

- Single delivery path for `message`, `ask`, and `reply` envelopes.
- RPC correlation by envelope `id` ↔ `request_id` via a `pending` map.
- Per-ask deadline enforcement through an armed timeout task.
- Fast-fail on peer disconnect: in-flight asks are cancelled the moment an
  involved instance unregisters.
- Late or unknown replies are rejected cleanly rather than misdelivered.

## Boundaries

- The router does not buffer for absent recipients beyond the mailbox bound;
  it has no separate retry queue (durability is out of scope, §1.1).
- It does not interpret `payload`.
- It does not deliver `event` envelopes to mailboxes — broadcasts go to SSE
  (fr:07-sse).
- It does not persist `pending` state; a daemon restart drops all in-flight
  asks.
- Exact REST connection-binding behind a disconnect is an open question
  (spec §12); the router only reacts to the unregister signal it receives.

## Error Handling

- `ask` timeout (spec §8.5): the router awaits up to `timeout_ms` (default
  30 s, max 24 h). On timeout the oneshot is cancelled, the `pending` entry is
  removed, and the HTTP caller receives `504 {error: "timeout", request_id}`.
- Unknown `request_id`: a `reply` for an ask that timed out or never existed
  returns `unknown_request_id` and is dropped.
- Peer disconnect: pending asks whose `from` or `to` matches an unregistering
  instance are cancelled with `instance_disconnected`.
- Unknown recipient: `send` to an unregistered `to` fails to resolve in the
  registry (surfaced as `unknown_instance`).

## Traceability

- Reference docs: ref:protocol
- Related FR: fr:01-envelope, fr:03-mailbox

## When to update

- The `send`/`reply` resolution logic changes.
- The `pending`-map keying or correlation rule changes.
- Disconnect cancellation scope changes.
- Default or maximum `ask` timeout values change.
