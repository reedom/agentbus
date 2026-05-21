---
refs:
  id: fr:09-hook-inbox
  kind: fr
  title: "Hook-driven inbox delivery"
  related:
    - fr:01-envelope
    - fr:02-instance-registry
  modules:
    - crates/agentbusd/src/hookinbox.rs
    - scripts/inject-inbox.sh
---

# FR 09: Hook-driven inbox delivery

> A file-backed inbox mode for instances that cannot use blocking `await_message`.

## Purpose

Some workflows cannot or should not block on `await_message` — for example a
Claude Code session that should pick up pending messages at prompt boundaries
rather than mid-turn. The hook-driven inbox is a third inbound delivery mode:
the daemon writes envelopes to a per-instance file, and a hook script reads them
at `SessionStart` / `UserPromptSubmit` time, injecting their content as context.

## User-visible Behavior

- When enabled for an instance, the daemon writes each envelope addressed to it
  as one JSON line to `$INBOX_DIR/<instance_id>.jsonl`.
- The daemon opens the file `O_CREATE | O_APPEND` and never truncates it.
- The shipped reference hook script (`scripts/inject-inbox.sh`) does the read
  side: it atomically renames `inbox.jsonl` → `inbox.processing.<pid>`, reads
  and formats the envelopes, emits hook output, then deletes the processing
  file. The daemon recreates the file on its next write.
- Only the hook script ever removes the file; the daemon only appends.
- The reference script ships as a starting point, not a binary — operators are
  expected to adapt it.

## Capabilities

- A non-blocking, file-backed inbound mode complementing `await_message`
  (fr:08-mcp-shim) and the inbox SSE stream (fr:07-sse).
- Race-free hand-off via atomic rename: the daemon appends, the hook script
  consumes a renamed snapshot.
- One file per instance, named by `instance_id`.
- A reference hook script operators can adapt to their client's hook system.

## Boundaries

- The inbox files are not the durable event log (fr:05-eventlog) and have no
  replay semantics; once consumed by the hook script they are gone.
- The daemon never truncates or deletes inbox files — consumption is entirely
  the hook script's responsibility.
- agentbus ships only a reference hook script, not a packaged integration; the
  operator wires it into their client's hook configuration.
- This mode does not provide ordering or delivery guarantees beyond append
  order within a single file.

## Error Handling

- Hook-injection races (spec §8.9): the daemon writes with `O_APPEND`; the hook
  script reads only after an atomic rename, and the daemon never truncates.
  This keeps the append side and the consume side from racing on the same file.

## Traceability

- Related FR: fr:01-envelope, fr:02-instance-registry

## When to update

- The inbox file path layout or naming changes.
- The daemon's write flags or append-only contract change.
- The atomic-rename hand-off protocol changes.
- The reference hook script's interface changes.
