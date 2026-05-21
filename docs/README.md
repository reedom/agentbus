# agentbus

An MCP-native message bus that lets MCP-capable AI instances talk to and listen
to other AI instances, humans, and arbitrary external programs.

- Spec: [superpowers/specs/2026-05-21-mcp-bus-design.md](superpowers/specs/2026-05-21-mcp-bus-design.md)
- Wire protocol + REST + MCP tools: [protocol.md](protocol.md)
- Examples: [examples/](examples/)

## Install (Claude Code users)

Add the marketplace and install the plugin:

```bash
claude plugin marketplace add github.com/reedom/agentbus
claude plugin install agentbus
```

Then in Claude Code, run the install slash command to fetch the daemon
and shim binaries from crates.io and start the daemon:

```
/agentbus:install
```

That command runs `cargo install --locked agentbusd agentbus-stdio agentbus-cli`,
verifies the binaries are on PATH, and launches `agentbusd` on
`127.0.0.1:8765`. Restart Claude Code so the `agentbus` MCP server picks
up the new `agentbus-stdio` binary.

## Build from source (contributors)

```bash
cargo build --release
./target/release/agentbusd &
./scripts/smoke-curl.sh
```

To wire a locally built shim without using the marketplace plugin, add
to `.mcp.json`:

```json
{
  "mcpServers": {
    "agentbus": {
      "type": "stdio",
      "command": "/absolute/path/to/agentbus-stdio"
    }
  }
}
```

## What's here

- `protocol.md` — the envelope schema, REST endpoint table, and MCP tool table,
  with example payloads for each surface.
- `examples/slack-bridge.md` — how to bridge `ask` calls to a human in Slack and
  return their interactive button choice as the reply.
- `examples/extbot-integration.md` — how an external orchestrator (`extbot`) can
  inject events and ask questions of a Claude registered as `extbot-<ticket>`.
- `../skills/agentbus/SKILL.md` — a Claude Code skill that teaches AI agents
  the workflow, tool-choice matrix, and gotchas for using the MCP surface.
  Loaded automatically by the plugin.
- `../commands/install.md` — the `/agentbus:install` slash command that
  installs the daemon and shim from crates.io. Explicit-invocation only;
  does not auto-trigger.

## Surfaces at a glance

| Surface | Audience | Transport |
|---|---|---|
| MCP shim (`agentbus-stdio`) | MCP-capable AI clients | stdio + Unix socket |
| REST (`/v1`) on `127.0.0.1` | External programs, bridges, CLI | HTTP |
| SSE (`/v1/events`, `/v1/instances/{id}/inbox`) | Subscribers, dashboards | HTTP long-lived |
| Hook-driven inbox | Workflows without blocking `await_message` | JSONL files |

All four surfaces speak the same envelope format.
