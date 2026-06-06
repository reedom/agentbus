---
refs:
  id: fr:11-daemon-lifecycle
  kind: fr
  title: "Daemon lifecycle, configuration, and security (superseded)"
  related:
    - fr:05-eventlog
---

# Daemon lifecycle, configuration, and security (superseded)

Deleted in v0.2 (spool model, 2026-06-05 design). There is no resident process.
`agentbusd` and its configuration, graceful-shutdown logic, and loopback-binding
security posture are gone. Storage layout, WAL locking, and crash recovery are
documented in the new store FRs (fr:12-store and fr:15-sweep, created in plan
Task 16). The single-user trust boundary is now enforced by `0700` filesystem
permissions on `~/.agentbus` rather than loopback binding.
