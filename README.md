# agentbus

An MCP-native message bus that lets MCP-capable AI instances talk to and listen
to other AI instances, humans, and arbitrary external programs.

agentbus v0.2 is a daemonless message bus over shared local storage (SQLite +
inbox spool files). There is nothing to launch. Participants open
`~/.agentbus/bus.db` directly and exchange `message` / `ask` / `reply` /
`event` envelopes over four surfaces:

| Surface | Audience | Transport |
|---|---|---|
| CLI (`agentbus`) | AI sessions (skill-guided), humans, shell scripts, external programs | subcommand per verb |
| MCP stdio shim (`agentbus-stdio`) | MCP-capable AI clients without skills or shell (fallback) | stdin/stdout JSON-RPC |
| Hook-driven inbox | Workflows at prompt / session boundaries | JSONL spool files |
| Watch streaming | Harnesses with a persistent monitor facility | event-log tail |

All surfaces speak the same envelope format.

## Crates

| Crate | Purpose |
|---|---|
| `agentbus-core` | Envelope and id types plus the spool store (registry, delivery, asks, event log). |
| `agentbus-stdio` | stdio MCP shim exposing the store as nine MCP tools. |
| `agentbus-cli` | Command-line client (`agentbus`) over the store directly. |

## Install

```bash
cargo install agentbus-cli@^0.2          # the CLI: all most setups need
cargo install agentbus-stdio@^0.2        # optional MCP shim (fallback clients)
```

No daemon to start. Register an instance and send:

```bash
agentbus register my-agent --persistent
echo '{"hello": "world"}' | agentbus send my-agent --from ext:cli
agentbus check-inbox my-agent
```

Skill-capable AI clients (Claude Code, Codex) need no MCP server: install
the agentbus skill, which drives the CLI directly — sessions that never
touch the bus pay no context cost. For MCP-only clients, wire the shim:

```json
{
  "mcpServers": {
    "agentbus": {
      "type": "stdio",
      "command": "agentbus-stdio"
    }
  }
}
```

The shim teaches its own usage via the MCP `instructions` field; pass
`--instructions=none` in `args` when a skill already covers that client.

### Security note: on_delivery hook

When you register an instance with `--on-delivery <command>`, the sender's
process runs that shell command after each inbox append. The command executes
as the same OS user — it is arbitrary user code, not a sandbox. Register
`on_delivery` only with commands you trust in your own environment.

## Build from source

```bash
cargo build --release
# binaries: target/release/agentbus, target/release/agentbus-stdio
```

## Documentation

- [`docs/README.md`](docs/README.md) — documentation index.
- [`docs/fr/`](docs/fr/index.md) — functional requirements, the design of
  record per feature, tracked by the `kusara` cross-reference graph.
- [`docs/reference/protocol.md`](docs/reference/protocol.md) — envelope schema,
  store operations, MCP tool surface, and CLI examples.
- [`docs/reference/watch-integration.md`](docs/reference/watch-integration.md) —
  watch-plus-monitor pattern for interactive harnesses.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or
[MIT License](LICENSE-MIT) at your option.
