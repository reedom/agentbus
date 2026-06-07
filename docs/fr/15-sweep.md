---
refs:
  id: fr:15-sweep
  kind: fr
  title: "sweep: periodic crash recovery"
  related:
    - fr:02-instance-registry
    - fr:09-hook-inbox
    - fr:13-on-delivery
  modules:
    - crates/agentbus-core/src/store/sweep.rs
---

# FR 15: sweep: periodic crash recovery

> `agentbus sweep` is a periodic CLI that prunes dead registrations, reports
> expired asks, recovers snapshots stranded by crashed consumers, and re-fires
> stuck on_delivery hooks — no resident process.

## Purpose

The daemonless spool model has no watchdog to clean up after crashes. Sweep
fills that role as a periodic, optional CLI (e.g. a launchd interval job).
Without sweep the same recovery happens lazily at the next send; sweep makes
it proactive and visible.

## User-visible Behavior

### Invocation

```
agentbus sweep [--grace-secs <s>] [--purge-orphans]
```

`--grace-secs` defaults to 60. `--purge-orphans` is off by default.
Output is a pretty-printed JSON `SweepReport`.

### Five actions, in order

**1. Remove dead non-persistent instance rows.**

For every non-persistent instance row, sweep checks `kill(pid, 0)`. Rows
whose pid is dead (or NULL) are deleted. The DELETE uses a pid predicate:

```sql
DELETE FROM instances WHERE id = ?1 AND persistent = 0 AND pid IS ?2
```

The predicate closes the liveness-check-to-delete window: a row
re-registered under a new live pid between the check and the DELETE is left
alone. Persistent rows are never touched by this step.

**2. Report each expired unanswered ask exactly once.**

Sweep selects asks where `reply_payload IS NULL AND expired_notified = 0`,
then checks whether `expires_at <= now` for each candidate. For each
expired ask:

- A `bus.ask_expired` event is published to `event_log` **before** the
  `expired_notified` flag is set. If the process crashes between the two,
  the event may be published again on the next sweep run. This is
  intentional: at-least-once delivery beats silent suppression. A duplicate
  `bus.ask_expired` event is operationally preferable to a lost one.
- Then `expired_notified = 1` is set in a transaction.

**3. Recover inbox snapshots stranded by crashed consumers.**

A consumer that dies between `check_inbox`'s rename and remove leaves its
batch in `<id>.processing.<pid>` (fr:09-hook-inbox rename-snapshot
contract). For every such file whose pid is dead, sweep merges the snapshot
back into the live `<id>.jsonl` spool (creating it if absent, appending
under the sender flock protocol if a new spool appeared since the crash),
removes the snapshot, and — when the owner has an `on_delivery` hook —
re-fires it immediately via `run_sweep_hook`, without waiting for the
grace period of action 4.

Snapshots whose pid is still alive are in-flight consumes and are left
untouched. Concurrent sweepers recover each snapshot exactly once (the
snapshot is removed under its flock, and a competing sweeper rechecks the
inode after acquiring it). A sweeper crashing between merge and remove
duplicates the batch on the next sweep: at-least-once, never silent loss.

**4. Re-fire on_delivery for stale non-empty inboxes.**

For every instance with a non-NULL `on_delivery`, sweep checks the inbox
file. If the file is non-empty and its mtime is older than the grace period
(default 60 s), sweep calls `run_sweep_hook` (fr:13-on-delivery). This
covers "sender crashed between inbox append and hook execution".

The sweep hook receives only `AGENTBUS_INSTANCE` (no envelope-specific
vars). Hooks must be idempotent with respect to receiving only that
variable.

**5. Purge orphan inbox files (opt-in).**

With `--purge-orphans`, sweep deletes `<id>.jsonl` files in the inbox
directory that have no corresponding instance row. Files matching
`.processing.*` (rename-snapshot in-flight artifacts) are left untouched.

### SweepReport

```json
{
  "dead_instances":    ["<id>", ...],
  "recovered_inboxes": ["<id>", ...],
  "rehooked":          ["<id>", ...],
  "expired_asks":      ["<request_id>", ...],
  "purged_inboxes":    ["<id>", ...]
}
```

## Capabilities

- Stateless: sweep reads only the store; it holds no persistent state of its
  own.
- Safe to run concurrently: pid-predicate DELETE and `expired_notified` flag
  make each action idempotent or at-least-once safe.
- Optional: skipping sweep degrades to lazy recovery at the next send.
- `--purge-orphans` is gated behind an explicit flag to prevent accidental
  data loss.
- SweepReport is machine-readable JSON for scripting and monitoring.

## Boundaries

- Sweep is a periodic CLI, not a resident. It must be scheduled externally
  (launchd, cron, systemd timer). Scheduling is outside the scope of this FR.
- Sweep does not re-deliver envelopes — it only re-fires hooks and moves
  stranded snapshots back into the spool. The envelope in the inbox spool is
  the delivery.
- The `bus.ask_expired` event may be emitted more than once for the same
  `request_id` in crash scenarios (at-least-once). Consumers must tolerate
  duplicate `bus.ask_expired` events.
- Snapshot recovery is likewise at-least-once: a sweeper crash between merge
  and remove re-merges the same batch on the next run, so consumers may see
  duplicate envelopes after a sweeper crash.
- `.processing.*` snapshot files are never deleted by `--purge-orphans`;
  they are only reclaimed by action 3 once their owning pid is dead.
- Pid reuse caveat: a recycled pid that happens to be alive delays recovery
  of an old snapshot until that unrelated process exits (same caveat as
  fr:02 liveness).
- Sweep does not purge `asks` rows; those stay indefinitely so that
  `ask-result` can retrieve late replies.
- on_delivery re-fire behavior is covered jointly by this FR and
  fr:13-on-delivery.

## Error Handling

- Store open errors propagate as `StoreError` and exit non-zero.
- Errors mid-sweep (e.g. a SQLite error during expired-ask processing)
  propagate as `StoreError` and abort the sweep run.
- `run_sweep_hook` failure (non-zero exit, timeout) is non-fatal per
  fr:13-on-delivery: the failure is logged and a `bus.delivery_hook_failed`
  event is published; sweep continues to the next instance.
- `fs::remove_file` failure during `--purge-orphans` propagates as
  `StoreError::Io` and aborts the purge pass.

## Traceability

- Related FR: fr:02-instance-registry, fr:13-on-delivery
- Spec sections: 6.8 (sweep), 6.5 (on_delivery re-fire)

## When to update

- The default grace period changes (currently 60 s).
- A new sweep action is added (e.g. compacting the event_log).
- The at-least-once guarantee for `bus.ask_expired` is strengthened to
  exactly-once (e.g. by moving publish and flag-set into one transaction).
- The pid-predicate DELETE logic changes.
- The `SweepReport` shape changes.
- `.processing.*` exclusion logic changes.
