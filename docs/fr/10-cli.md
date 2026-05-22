---
refs:
  id: fr:10-cli
  kind: fr
  title: "agentbus CLI client"
  related:
    - ref:protocol
    - fr:06-rest-api
  modules:
    - crates/agentbus-cli/src
---

# FR 10: agentbus CLI client

> A thin command-line wrapper over the REST API for humans and shell scripts.

## Purpose

The `agentbus` CLI gives humans and shell scripts a direct way onto the bus
without writing HTTP calls by hand. It is a thin client over the REST API
(fr:06-rest-api): every subcommand maps to one or more REST endpoints.

## User-visible Behavior

The CLI exposes these subcommands:

```
agentbus ls                                    # list instances
agentbus send <to> [-f file | -]               # send a message
agentbus ask  <to> [-f file | -] [--timeout 30s]
agentbus tail [--instance <id>] [--since <ts>] # SSE viewer
agentbus reply <request_id> [-f file | -]
agentbus rm <id>                               # unregister someone (admin)
```

- `send`, `ask`, and `reply` take their payload from a file via `-f` or from
  stdin via `-`.
- `ask` blocks until a reply or its `--timeout` elapses, mirroring the REST
  `ask` behavior.
- `tail` is an SSE viewer over `/v1/events`, accepting `--instance` and
  `--since` filters.
- `rm` unregisters another instance — an administrative action.
- The binary is `agentbus`, distinct from the `agentbusd` daemon and the
  `agentbus-stdio` shim (spec §3.2).

## Capabilities

- Human- and script-friendly access to the bus's core operations.
- File or stdin payload input for `send` / `ask` / `reply`.
- A live event viewer (`tail`) with instance and since filtering.
- Administrative unregister of a stray instance.

## Boundaries

- The CLI is a transient talker; whether it should also be able to *register*
  as an interactive terminal-attached peer is an open UX question (spec §12).
  This FR documents only the transient-talker surface.
- The CLI adds no logic of its own — it carries no bus state and delegates all
  behavior to the REST API.
- It does not expose MCP tools; that surface is the shim (fr:08-mcp-shim).
- It does not interpret `payload`.

## Error Handling

- The CLI surfaces REST-layer errors directly to the user; it owns no error
  cases of its own. `ask` timeouts, `instance_id_taken`, and similar are
  produced by fr:06-rest-api and fr:04-router and reported by the CLI.

## Traceability

- Reference docs: ref:protocol
- Related FR: fr:06-rest-api

## When to update

- A subcommand is added, removed, or its flags change.
- The payload-input convention (`-f` / `-`) changes.
- The CLI gains the ability to register as an instance.
- The binary name changes.
