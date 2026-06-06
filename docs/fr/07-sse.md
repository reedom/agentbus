---
refs:
  id: fr:07-sse
  kind: fr
  title: "Server-Sent Events streaming (superseded)"
  related:
    - fr:05-eventlog
---

# Server-Sent Events streaming (superseded)

Deleted in v0.2 (spool model, 2026-06-05 design). There is no daemon and no
HTTP surface, so SSE streams no longer exist. Event streaming is now
`agentbus events --follow`, which tails the ordered `event_log` table in
`bus.db` using a cursor-and-poll loop (fr:05-eventlog). The recipient-side
notifier is `agentbus watch`, which tails the same table filtered to a single
instance.
