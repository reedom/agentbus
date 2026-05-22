# agentbus

An MCP-native message bus that lets MCP-capable AI instances talk to and listen
to other AI instances, humans, and arbitrary external programs.

agentbus exposes one envelope format across four surfaces — an MCP stdio shim,
a loopback REST API, Server-Sent Events, and a hook-driven inbox — so any
MCP-capable agent, CLI, bridge, or script can exchange `message` / `ask` /
`reply` / `event` envelopes.

## Crates

| Crate | Purpose |
|---|---|
| `agentbus-core` | Envelope, registry, mailbox, router, and event log types. |
| `agentbusd` | The daemon: HTTP/REST + SSE message bus server. |
| `agentbus-stdio` | stdio MCP bridge exposing the bus as MCP tools. |
| `agentbus-cli` | Command-line client (`agentbus`) over the REST API. |

## Install (Claude Code users)

```bash
claude plugin marketplace add github.com/reedom/agentbus
claude plugin install agentbus
```

Then run `/agentbus:install` in Claude Code to fetch the daemon and shim from
crates.io and start `agentbusd` on `127.0.0.1:8765`.

## Build from source

```bash
cargo build --release
./target/release/agentbusd &
./scripts/smoke-curl.sh
```

## Documentation

- [`docs/README.md`](docs/README.md) — documentation index.
- [`docs/fr/`](docs/fr/index.md) — functional requirements, the design of
  record per feature, tracked by the `kusara` cross-reference graph.
- [`docs/reference/protocol.md`](docs/reference/protocol.md) — envelope schema,
  REST endpoints, and MCP tool surface.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or
[MIT License](LICENSE-MIT) at your option.
