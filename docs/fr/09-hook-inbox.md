---
refs:
  id: fr:09-hook-inbox
  kind: fr
  title: "Hook-driven inbox delivery"
  related:
    - fr:01-envelope
    - fr:02-instance-registry
  modules:
    - crates/agentbus-core/src/store/spool.rs
    - scripts/inject-inbox.sh
---

# FR 09: Hook-driven inbox delivery

> A file-backed inbox mode for instances that cannot use blocking `await_message`.

## Purpose

Some workflows cannot or should not block on `await_message` — for example a
Claude Code session that should pick up pending messages at prompt boundaries
rather than mid-turn. The hook-driven inbox is a third inbound delivery mode:
the sender process writes envelopes to a per-instance JSONL file, and a hook
script reads them at `SessionStart` / `UserPromptSubmit` time, injecting their
content as context.

In v0.2 the writer is the sender process (fr:04-router), not a daemon. The
file format and the consumer contract are unchanged from v0.1.

## User-visible Behavior

- The inbox file lives at `$AGENTBUS_DIR/inbox/<instance_id>.jsonl`
  (default `~/.agentbus/inbox/<instance_id>.jsonl`).
- The sender opens the file `O_CREATE | O_APPEND`, acquires an exclusive
  `flock`, verifies that the file descriptor still names the live spool file
  (same `dev` + `ino`), and writes one complete JSON line per envelope. If a
  consumer renamed the file between open and lock, the sender reopens and
  retries — this keeps lock-free shell consumers safe.
- Consumers use the rename-snapshot contract:
  1. Rename `<id>.jsonl` → `<id>.processing.<pid>` (atomic).
  2. Read all lines, process, delete the processing file.
- Consumers never need a lock (the sender's dev+ino reopen loop protects
  them). The Rust consumer (`check_inbox`) additionally acquires the
  exclusive `flock` once after the rename, as a barrier ensuring an
  in-flight sender append completes before reading.
- The shipped reference hook script (`scripts/inject-inbox.sh`) implements
  the lock-free contract for shell use: rename, read, delete — no `flock`
  (the utility is not portable to stock macOS). It accepts a tiny residual
  window where a final line still being appended is read torn; agents that
  cannot tolerate that should consume via `check_inbox`. The script requires
  `AGENTBUS_INSTANCE` and honours `AGENTBUS_INBOX_DIR` as an override; the
  default matches the store path above.
- Senders never delete inbox files. Consumers (the hook script or the Rust
  `check_inbox` / `await_message` calls) are the sole removers.
- `agentbus sweep --purge-orphans` removes inbox files whose instance id has
  no live registration.

## Capabilities

- Non-blocking, file-backed inbound mode complementing `await_message` and
  `check_inbox` (fr:08-mcp-shim, fr:10-cli).
- Race-free hand-off via atomic rename; the Rust consumer adds an flock
  barrier so in-flight appends are never read torn.
- Dev+ino reopen loop on the sender side keeps lock-free shell consumers safe.
- One file per instance, named by `instance_id`.
- A reference hook script operators can adapt to their client's hook system.

## Boundaries

- The inbox files are not the durable event log (fr:05-eventlog) and have no
  replay semantics; once consumed they are gone.
- Senders never truncate or delete inbox files — consumption is the consumer's
  responsibility.
- agentbus ships only a reference hook script; operators wire it into their
  client's hook configuration.
- Ordering within one file is append order; there is no cross-instance
  ordering guarantee.

## Error Handling

- Sender open/flock/write errors are surfaced as `StoreError::Io` after the
  event_log commit (partial-failure path; see fr:04-router).
- Consumer: corrupt JSONL lines (unparseable JSON) are skipped with a warning;
  well-formed lines before and after are still delivered.
- The `flock` call is retried on `EINTR` (signal interrupt); it does not use
  `LOCK_NB`.

## Traceability

- Related FR: fr:01-envelope, fr:02-instance-registry

## When to update

- The inbox directory path default changes.
- The flock protocol (exclusive lock, dev+ino reopen) changes.
- The atomic-rename consumer contract changes.
- The reference hook script's interface or environment variables change.
- Senders gain the ability to delete inbox files.
