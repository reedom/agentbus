# agentbus

An MCP-native message bus that lets MCP-capable AI instances talk to and listen
to other AI instances, humans, and arbitrary external programs.

agentbus v0.2 is daemonless: all state lives in `~/.agentbus/` (SQLite +
inbox spool files). Participants open the store directly — there is nothing to
launch.

- Functional requirements: [fr/index.md](fr/index.md)
- Wire protocol, store operations, and MCP tool surface: [reference/protocol.md](reference/protocol.md)
- Integration examples: [reference/](reference/)

## Install

```bash
cargo install agentbus-cli@^0.3          # the CLI: all most setups need
cargo install agentbus-stdio@^0.3        # optional MCP shim (fallback clients)
```

Register an instance and send a message:

```bash
agentbus register my-agent --persistent
echo '{"hello": "world"}' | agentbus send my-agent --from ext:cli
agentbus check-inbox my-agent
```

To use the MCP shim from Claude Code, add to `.mcp.json`:

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

### Security note: on_delivery hook

When an instance is registered with `on_delivery = "<command>"`, the sender's
process runs that shell command after each inbox append. The command executes
as the same OS user — it is arbitrary user code, not a sandbox. Register
`on_delivery` only with commands you trust in your own environment. This is
documented in spec section 9 and [fr:13-on-delivery](fr/13-on-delivery.md).

## Surfaces at a glance

| Surface | Audience | Transport |
|---|---|---|
| MCP shim (`agentbus-stdio`) | MCP-capable AI clients | stdin/stdout JSON-RPC |
| CLI (`agentbus`) | Humans, shell scripts, external programs | subcommand per verb |
| Hook-driven inbox | Workflows at prompt/session boundaries | JSONL spool files |
| Watch streaming | Harnesses with a persistent monitor facility | event-log tail |

All surfaces speak the same envelope format.

## What's here

- `fr/` — functional requirement docs, one per product feature, tracked by the
  `kusara` cross-reference graph. See `fr/README.md`.
- `reference/protocol.md` — the envelope schema, store operation table, and
  MCP tool table, with CLI examples for each verb.
- `reference/watch-integration.md` — the watch-plus-monitor pattern for
  interactive harnesses (session-start hook launches `watch` under the
  harness's monitor facility; lifecycle owned by the harness).
- `reference/slack-bridge.md` — how to bridge `ask` calls to a human in Slack
  and return their interactive button choice as the reply.
- `reference/extbot-integration.md` — how an external orchestrator (`extbot`)
  can inject events and ask questions of a Claude registered as `extbot-<ticket>`.
