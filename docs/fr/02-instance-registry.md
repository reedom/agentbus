---
refs:
  id: fr:02-instance-registry
  kind: fr
  title: "Instance registration and identity"
  related:
    - fr:01-envelope
    - fr:09-hook-inbox
  modules:
    - crates/agentbus-core/src/store/instances.rs
    - crates/agentbus-core/src/store/liveness.rs
---

# FR 02: Instance registration and identity

> Rows in `bus.db` that map client-provided instance IDs to addressable participants, with pid-based liveness and optional persistence.

## Purpose

To be addressable, a participant registers an `instance_id` with the store.
The `instances` table in `~/.agentbus/bus.db` is the authority on which
instances exist. In v0.2 there is no daemon holding a connection: identity
is scoped either to a pid (non-persistent) or to an explicit unregister call
(persistent). Liveness is checked on demand via `kill(pid, 0)` rather than
through heartbeats or TTLs.

## User-visible Behavior

- An instance registers an `instance_id` matching `[A-Za-z0-9_.:-]{1,128}`.
  An id that is empty, longer than 128 bytes, or contains any other character
  is rejected with `invalid_instance_id`.
- Registrations are rows in the `instances` table with columns `id`, `pid`,
  `persistent`, `on_delivery`, and `registered_at` (RFC3339 UTC).
- A **non-persistent** row records the caller's pid (`std::process::id()`)
  unless the caller supplies an explicit pid via `RegisterOpts` — exposed to
  users as `agentbus register --pid <pid>` (fr:10-cli), the anchor for
  session-scoped registration without the shim. That pid is
  the liveness anchor: the row is considered live while `kill(pid, 0)`
  succeeds (including `EPERM`, which means "exists but not ours").
- A **persistent** row sets `pid = NULL`. It is exempt from liveness checks
  and survives process exit and machine reboot. Persistent rows suit
  long-lived agents registered at session-start time.
- Liveness is the same-machine `kill(pid, 0)` check. `EPERM` counts as alive
  (the process exists but is owned by a different uid). There are no
  heartbeats, no TTLs, and no out-of-band tracking. This is honest because
  agentbus is single-machine only.
- Registrations survive reboots only when persistent. After a reboot all
  non-persistent pids are dead; their rows are replaced lazily on the next
  `register` call for the same id, or cleaned up by `agentbus sweep`.
- `unregister(id)` deletes the row. The inbox spool file
  (`~/.agentbus/inbox/<instance_id>.jsonl`) is left in place — undelivered
  mail is not destroyed. `agentbus sweep --purge-orphans` can remove orphaned
  inbox files later.
- Listing active instances (`list_instances`) queries all rows and evaluates
  liveness per row after the query. The `alive` field in the result reflects
  the state at query time and may lag by milliseconds.
- Each row optionally carries an `on_delivery` command string, which senders
  execute after a successful inbox append. The execution contract is
  documented in the on-delivery FR (Task 16 of the spool-model plan).

## Capabilities

- Collision matrix:
  - **Live non-persistent row, different pid**: `instance_id_taken`. Only
    this case blocks registration.
  - **Dead non-persistent row** (`kill(pid, 0)` returns `ESRCH`): replaced
    unconditionally.
  - **Same-pid re-register**: idempotent upsert; succeeds silently.
  - **Persistent row** (`pid = NULL`): always upserts (single-user trust
    model); the caller becomes the new owner of the row's fields.
- Concurrent-safe mutations via SQLite WAL with `BEGIN IMMEDIATE`; no
  separate in-process lock is needed.
- `on_delivery` is stored per row and retrieved by senders at send time
  (`on_delivery_of`), keeping the execution path decoupled from registration.

## Boundaries

- No authentication of who may claim an id — the first live caller wins.
  Trust rests on `0700` filesystem permissions on `~/.agentbus` (single-user
  posture, same as v0.1's loopback binding in practice).
- No multi-host or federated registries; one store per machine.
- The registry does not deliver messages; it only asserts whether an instance
  exists and what its `on_delivery` command is. Delivery is the sender
  appending to the inbox file (fr:09-hook-inbox).
- `ext:<label>` senders are never entered into the registry and cannot be
  enumerated or addressed.
- **Pid reuse** (spec open question 3): `kill(pid, 0)` can false-positive if
  the OS reuses the dead owner's pid before the row is replaced. Recording
  the process start time alongside the pid would close this window; the
  mitigation is not yet implemented.

## Error Handling

- `invalid_instance_id`: the id is empty, longer than 128 bytes, or contains
  a character outside `[A-Za-z0-9_.:-]`.
- `instance_id_taken`: `register` finds an existing non-persistent row whose
  pid is live and different from the caller's pid.
- `unknown_instance`: `on_delivery_of` (called by senders) finds no row for
  the given id; the send is rejected with this code.

## Traceability

- Related FR: fr:01-envelope, fr:09-hook-inbox

## When to update

- The `instance_id` format constraint or byte limit changes.
- The persistent / pid-scoped duality changes (e.g. a TTL or heartbeat is
  added).
- The collision matrix changes (new cases or different outcomes).
- The `on_delivery` field is added, removed, or its semantics change.
- The pid-reuse mitigation (process start time) is implemented.
- `unregister` behavior toward the inbox file changes.
