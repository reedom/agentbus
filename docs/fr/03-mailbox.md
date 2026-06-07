---
refs:
  id: fr:03-mailbox
  kind: fr
  title: "Per-instance mailbox (superseded)"
  related:
    - fr:09-hook-inbox
---

# Per-instance mailbox (superseded)

Deleted in v0.2 (spool model, 2026-06-05 design). The in-memory bounded queue
that buffered envelopes between the daemon router and each instance is gone.
In v0.2 the sender writes directly to the recipient's inbox spool file
(`~/.agentbus/inbox/<instance_id>.jsonl`); those files are the mailbox.
The rename-snapshot consume contract (fr:09-hook-inbox) is unchanged.
