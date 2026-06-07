---
description: Install the agentbus CLI (and optional MCP shim) from crates.io.
argument-hint: "[--with-shim] [--prefix <cargo-install-root>]"
allowed-tools: Bash
---

# /agentbus:install

Slash command. Runs only when the user types `/agentbus:install`.

## Goal

Ensure the user has the `agentbus` CLI installed from crates.io. There is
no daemon: the bus is a shared local store under `~/.agentbus` that the
CLI opens directly.

The `agentbus-stdio` MCP shim is NOT installed by default — this plugin
ships the agentbus skill, which drives the CLI. Install the shim only
when the user passes `--with-shim` (for wiring an MCP-only client that
cannot use skills or shell).

## Steps

1. Check what is already present:
   - `agentbus --version` — if it prints a `0.3.x` version, the CLI is
     installed.
   - With `--with-shim`: also check `agentbus-stdio` resolves on PATH
     (`command -v agentbus-stdio`).
2. Install what is missing:
   - `cargo install agentbus-cli@^0.3` (add `--root <prefix>` when the
     user passed `--prefix`).
   - With `--with-shim`: `cargo install agentbus-stdio@^0.3`.
3. Verify: `agentbus ls` against a temp store must succeed:
   `AGENTBUS_DIR=$(mktemp -d) agentbus ls`.
4. Report what was installed and where. If `--with-shim` was used, print
   the MCP client config snippet:

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

   and note the `--instructions=none|minimal|full` flag (default `full`;
   pass `none` when a skill already teaches that client).

## Failure handling

- `cargo` missing: tell the user to install Rust via rustup
  (https://rustup.rs) and stop.
- `cargo install` failure: show the error verbatim; do not retry blindly.
