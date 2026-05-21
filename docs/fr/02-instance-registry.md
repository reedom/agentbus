---
refs:
  id: fr:02-instance-registry
  kind: fr
  title: "Instance registration and identity"
  related:
    - ref:protocol
    - fr:01-envelope
    - fr:03-mailbox
  modules:
    - crates/agentbus-core/src/registry.rs
---

# FR 02: Instance registration and identity

> The registry that maps client-provided instance IDs to live, addressable bus participants.

## Purpose

To be addressable, a participant must register an `instance_id` with the daemon.
The registry is the authority on which instances exist, owns each instance's
mailbox sender, and ties a registration to the connection that created it so
identity disappears when the owner does — no stale entries, no heartbeat
protocol.

## User-visible Behavior

- An instance registers an `instance_id` matching `[A-Za-z0-9_.:-]{1,128}`.
- Registration is exclusive. A second registration of the same ID from a
  different owner connection is rejected.
- A re-registration of the same ID on the *same* owner connection is idempotent
  and succeeds.
- Each registration is bound to its owner connection: the Unix socket for an
  MCP shim, the HTTP keep-alive connection for a REST registration. When that
  connection drops, the daemon auto-unregisters the instance.
- Listing active instances is supported (`GET /v1/instances`, MCP
  `list_instances()`).
- Each `Instance` record holds `id`, an optional `alias`, `registered_at`, the
  mailbox sender, and the owner-connection handle.
- External programs that do not register cannot be addressed; they participate
  only as senders using `from: "ext:<label>"`.

## Capabilities

- Exclusive ID ownership with idempotent same-owner re-registration.
- Connection-bound lifetime: registration ends exactly when its owner
  connection ends, with no TTL and no heartbeat.
- Auto-unregister on connection drop, which also triggers cancellation of
  in-flight asks involving that instance (see fr:04-router).
- Concurrent-safe lookups: the registry is a `HashMap` under an `RwLock`.
- Holds the mailbox sender so the router can enqueue without re-resolving.

## Boundaries

- The registry does not persist; it is in-memory and empty on daemon start.
- No authentication of who may claim an ID — first connection to register wins
  (auth is post-v1, spec §13).
- No multi-host or federated registries; one registry per daemon (spec §1.1).
- The registry does not deliver messages; it only resolves IDs to mailboxes
  (delivery is fr:03-mailbox and fr:04-router).
- `ext:<label>` senders are never entered into the registry and cannot be
  enumerated or addressed.

## Error Handling

- Instance ID collision (spec §8.2): `register` returns
  `{code: "instance_id_taken"}` when the ID is held by a different owner
  connection. Same-owner re-register succeeds idempotently.
- Stale registrations (spec §8.3): there is no stale state to clean up — a
  dropped owner connection (shim crash, client exit, REST disconnect)
  auto-unregisters the instance and cancels its in-flight asks. No heartbeat
  exists, so liveness is never inferred.

## Traceability

- Reference docs: ref:protocol
- Related FR: fr:01-envelope, fr:03-mailbox

## When to update

- The `instance_id` format constraint changes.
- The connection-binding model changes (e.g. a TTL or heartbeat is added).
- The `Instance` record gains or loses fields.
- Idempotent re-registration semantics change.
