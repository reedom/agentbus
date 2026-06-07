---
refs:
  id: fr:06-rest-api
  kind: fr
  title: "REST API surface (superseded)"
  related:
    - fr:09-hook-inbox
---

# REST API surface (superseded)

Deleted in v0.2 (spool model, 2026-06-05 design). The bus is no longer a
daemon; there is no HTTP surface. All REST endpoints (`/v1/instances`,
`/v1/events`, etc.) and the `agentbusd` binary are gone. If remote access
returns in a future version, it will be a thin HTTP view over the same SQLite
store; the spool remains the source of truth (spec non-goal, section 3).
