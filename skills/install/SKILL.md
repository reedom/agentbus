---
description: Install the agentbus daemon and shim binaries from crates.io, then start the daemon. Use when the user invokes `/agentbus:install`, asks to "install agentbus", or when the agentbus MCP server fails to connect because no `agentbus-stdio` binary is on PATH.
argument-hint: "[--no-start] [--prefix <cargo-install-root>]"
allowed-tools: Bash
---

# /agentbus:install

This is a user-invoked slash command. Follow the steps below.

## Goal

Ensure the user has `agentbusd`, `agentbus-stdio`, and `agentbus` (CLI)
installed from crates.io and that the daemon is running on the loopback
interface.

## Steps

### 1. Verify prerequisites

Run:

```bash
command -v cargo >/dev/null 2>&1 || echo "MISSING_CARGO"
```

If output contains `MISSING_CARGO`, stop and tell the user:

> Cargo is required to install agentbus binaries. Install Rust toolchain
> from https://rustup.rs/ first, then re-run `/agentbus:install`.

Do not proceed.

### 2. Parse arguments

- `--no-start`: skip launching the daemon after install.
- `--prefix <path>`: pass `--root <path>` to `cargo install` so binaries
  land outside `~/.cargo/bin`.

Default behavior: install to `~/.cargo/bin`, start the daemon.

### 3. Install crates

Run (build the argument list based on `--prefix`):

```bash
cargo install --locked agentbusd agentbus-stdio agentbus-cli
```

If installation succeeds, proceed. If it fails, surface the error to the
user and stop — do not attempt to start a daemon that does not exist.

### 4. Verify PATH

Run:

```bash
command -v agentbus-stdio && command -v agentbusd && command -v agentbus
```

If any binary is missing from PATH, tell the user the install directory
(usually `~/.cargo/bin`) must be on PATH, and suggest adding it to their
shell rc file.

### 5. Start the daemon (unless `--no-start`)

Check whether a daemon is already running:

```bash
pgrep -f '^agentbusd' >/dev/null && echo RUNNING || echo NOT_RUNNING
```

If `NOT_RUNNING`, launch in background and log to a temp file:

```bash
nohup agentbusd > /tmp/agentbusd.log 2>&1 &
sleep 0.8
curl -sS http://127.0.0.1:8765/v1/health
```

Expect `{"status":"ok"}`. If the health check fails, show the user
`tail -n 20 /tmp/agentbusd.log` and stop.

### 6. Confirm MCP wiring

Tell the user:

> agentbus is installed. The `agentbus` MCP server is wired via this
> plugin's `.mcp.json` (`command: agentbus-stdio`). Restart Claude Code
> if the `mcp__agentbus__*` tools are not yet visible.

### 7. Suggest next steps

> Try `/agentbus` or ask "register me as <id> via agentbus" to begin.
> See `docs/protocol.md` for the full envelope spec.

## Notes

- Do not embed version pins; let `cargo install` pull the latest.
- Do not run `cargo install` with `--force` unless the user explicitly
  asks for a reinstall.
- The daemon binds 127.0.0.1 only by default. Do not expose it to a
  non-loopback address.
- If the user already has the binaries (verified at step 4 before
  install), skip cargo install and only proceed to start the daemon.
