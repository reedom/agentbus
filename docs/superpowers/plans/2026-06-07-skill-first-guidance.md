# Skill-First Client Guidance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the skill+CLI the primary AI surface of agentbus and demote the MCP shim to a self-teaching fallback, per `docs/superpowers/specs/2026-06-07-skill-first-guidance.md`.

**Architecture:** Three small code changes (CLI `register --pid`, store error recovery hints, shim `instructions` at initialize with a `--instructions` flag), then the documentation flip (SKILL.md rewritten CLI-first, plugin packaging drops the shim, new fr:16, deltas to fr:02/08/10/12, protocol reference, README).

**Tech Stack:** Rust (workspace: agentbus-core, agentbus-cli, agentbus-stdio), clap 4 (CLI only — the shim parses its one flag by hand), kusara for the doc graph.

**Branch:** create `feat/skill-first-guidance` from `main` before Task 1 (`git checkout -b feat/skill-first-guidance`). Worktree isolation per superpowers:using-git-worktrees if executing alongside other work.

**Conventions that apply to every code step** (from the user's global CLAUDE.md):
- Never use `>` or `>=` in comparisons; write `0 < n`, `x <= max` instead. The codebase already follows this (see `instances.rs:96` `Ok(0 < n)`).
- No emojis anywhere. Conventional commits, lowercase titles, max 50 chars.
- After any `.md` edit a PostToolUse hook runs `kusara validate` automatically; after any `crates/` edit it runs `kusara touched`. Do not run these manually unless a hook reports a failure.

## File structure

| File | Action | Responsibility |
|---|---|---|
| `crates/agentbus-cli/src/commands.rs` | modify | `register` gains `--pid` |
| `crates/agentbus-cli/tests/cli.rs` | modify | golden test for `--pid` |
| `crates/agentbus-core/src/store/error.rs` | modify | recovery hints in Display strings |
| `crates/agentbus-stdio/src/instructions.rs` | create | guidance levels + texts (single place the shim's usage prose lives) |
| `crates/agentbus-stdio/src/main.rs` | modify | `--instructions` flag, instructions in initialize result |
| `crates/agentbus-stdio/tests/rpc.rs` | modify | initialize-shape tests |
| `skills/agentbus/SKILL.md` | rewrite | CLI-first usage guidance (source of truth) |
| `.mcp.json` | delete | plugin no longer registers the shim |
| `commands/install.md` | rewrite | v0.2 install (CLI only; shim optional) |
| `.claude-plugin/plugin.json` | modify | description no longer mentions daemon |
| `docs/fr/16-usage-guidance.md` | create | design of record for the channel hierarchy |
| `docs/fr/02-instance-registry.md`, `08-mcp-shim.md`, `10-cli.md`, `12-store.md` | modify | deltas per spec section 7 |
| `docs/reference/protocol.md`, `README.md` | modify | surface repositioning |

---

### Task 1: CLI `register --pid`

**Files:**
- Modify: `crates/agentbus-cli/src/commands.rs:31-40` (Cmd::Register variant), `:122-137` (run match arm)
- Test: `crates/agentbus-cli/tests/cli.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/agentbus-cli/tests/cli.rs`:

```rust
#[test]
fn register_with_explicit_pid_anchors_to_that_process() {
    let tmp = tempfile::tempdir().unwrap();
    // Anchor the row to this test process, which outlives the CLI invocation
    // (the session-pid pattern from the skill-first spec).
    let my_pid = std::process::id();
    let reg = agentbus(
        tmp.path(),
        &["register", "sess", "--pid", &my_pid.to_string()],
        None,
    );
    assert_eq!(stdout_json(&reg)["ok"], true);
    let ls = stdout_json(&agentbus(tmp.path(), &["ls"], None));
    assert_eq!(ls["instances"][0]["id"], "sess");
    assert_eq!(ls["instances"][0]["pid"], my_pid);
    // The CLI process is long gone; liveness must come from the anchored pid.
    assert_eq!(ls["instances"][0]["alive"], true);
}

#[test]
fn register_rejects_pid_combined_with_persistent() {
    let tmp = tempfile::tempdir().unwrap();
    let out = agentbus(
        tmp.path(),
        &["register", "sess", "--pid", "1", "--persistent"],
        None,
    );
    assert!(!out.status.success());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p agentbus-cli --test cli register_ -- --nocapture`
Expected: both new tests FAIL — the spawned binary exits with a clap error `unexpected argument '--pid'` (the first test panics inside `stdout_json` printing that stderr; the second passes trivially only if clap rejects, so confirm the first one is the failure you see).

- [ ] **Step 3: Implement the flag**

In `crates/agentbus-cli/src/commands.rs`, replace the `Register` variant:

```rust
    /// Register an instance id (non-persistent rows die with the anchor pid;
    /// default anchor is this process — pass --pid to anchor to a long-lived
    /// session process, or --persistent for durable addresses).
    Register {
        id: String,
        #[arg(long, conflicts_with = "pid")]
        persistent: bool,
        #[arg(long)]
        on_delivery: Option<String>,
        /// Owner pid for the non-persistent row (e.g. the AI harness process).
        #[arg(long)]
        pid: Option<i32>,
    },
```

and the match arm in `run`:

```rust
        Cmd::Register {
            id,
            persistent,
            on_delivery,
            pid,
        } => {
            store.register(
                &id,
                &RegisterOpts {
                    persistent,
                    on_delivery,
                    pid,
                },
            )?;
            println!("{}", serde_json::json!({"ok": true}));
        }
```

No core change: `RegisterOpts.pid` already exists (`crates/agentbus-core/src/store/instances.rs:15`) and `Store::register` already honors it (`instances.rs:50-54`).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p agentbus-cli --test cli`
Expected: all tests PASS (including the pre-existing golden tests).

- [ ] **Step 5: Commit**

```bash
git add crates/agentbus-cli/src/commands.rs crates/agentbus-cli/tests/cli.rs
git commit -m "feat(cli): add --pid flag to register"
```

---

### Task 2: Store error recovery hints

**Files:**
- Modify: `crates/agentbus-core/src/store/error.rs:4-24` (variant messages), `:55-86` (tests)

The wire contract: `message`/`error[<code>]` carries the stable `code()`
string (unchanged); the Display string is what reaches `data` (shim) and
stderr detail (CLI). Hints go into Display only.

- [ ] **Step 1: Write the failing test**

Append inside `mod tests` in `crates/agentbus-core/src/store/error.rs`:

```rust
    #[test]
    fn messages_carry_recovery_hints() {
        let unknown = StoreError::UnknownInstance("x".into()).to_string();
        assert!(unknown.contains("register"), "got: {unknown}");
        let taken = StoreError::InstanceIdTaken("x".into()).to_string();
        assert!(taken.contains("different id"), "got: {taken}");
        let no_ask = StoreError::UnknownRequestId("x".into()).to_string();
        assert!(no_ask.contains("ask envelope"), "got: {no_ask}");
        let locked = StoreError::StoreLocked.to_string();
        assert!(locked.contains("retry"), "got: {locked}");
        // Timeout already carries its hint; pin it so it never regresses.
        let timeout = StoreError::Timeout("x".into()).to_string();
        assert!(timeout.contains("ask-result"), "got: {timeout}");
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p agentbus-core messages_carry_recovery_hints`
Expected: FAIL on the first assert (`unknown instance \`x\`` contains no "register").

- [ ] **Step 3: Update the Display strings**

In `crates/agentbus-core/src/store/error.rs`, replace the four `#[error]` attributes (leave `InvalidInstanceId`, `Timeout`, `InvalidEnvelope`, `Io`, `Sqlite` as they are):

```rust
    #[error("unknown instance `{0}` (recipients must register first; check list_instances / `agentbus ls`)")]
    UnknownInstance(String),
    #[error("instance_id `{0}` is registered to another live process (pick a different id; dead owners are replaced automatically)")]
    InstanceIdTaken(String),
```

and:

```rust
    #[error("unknown request_id `{0}` (a reply's request_id must be the ask envelope's id; plain `message` envelopes take no reply)")]
    UnknownRequestId(String),
    #[error("store locked: busy_timeout exhausted (transient write contention; retry after a short wait)")]
    StoreLocked,
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p agentbus-core && cargo test -p agentbus-cli && cargo test -p agentbus-stdio`
Expected: all PASS. (`cli.rs` only asserts the `error[unknown_instance]` prefix and `rpc.rs` only asserts the `message` code, so no existing assertion breaks — but run the full suite to prove it.)

- [ ] **Step 5: Commit**

```bash
git add crates/agentbus-core/src/store/error.rs
git commit -m "feat(core): add recovery hints to store errors"
```

---

### Task 3: Shim instructions module

**Files:**
- Create: `crates/agentbus-stdio/src/instructions.rs`
- Modify: `crates/agentbus-stdio/src/main.rs:4` (module decl)

- [ ] **Step 1: Create the module with unit tests (TDD note: this module is data + a 3-line parser; write the file in one pass, tests included, then watch them pass)**

Create `crates/agentbus-stdio/src/instructions.rs`:

```rust
//! Initialize-time usage guidance for MCP clients (fr:16): the fallback
//! channel when no skill is teaching this client. The full text is a
//! condensed, hand-maintained subset of skills/agentbus/SKILL.md — when
//! that skill's mental model or gotchas change, re-derive this text.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    None,
    Minimal,
    Full,
}

impl Level {
    pub fn parse(s: &str) -> Option<Level> {
        match s {
            "none" => Some(Level::None),
            "minimal" => Some(Level::Minimal),
            "full" => Some(Level::Full),
            _ => None,
        }
    }
}

pub fn text(level: Level) -> Option<&'static str> {
    match level {
        Level::None => None,
        Level::Minimal => Some(MINIMAL),
        Level::Full => Some(FULL),
    }
}

const MINIMAL: &str = "\
agentbus message bus. register(instance_id) first; send/ask to talk; \
check_inbox or await_message to receive (batch; empty = timeout). Answer \
an inbound ask with reply(request_id = the ask envelope's id; omit `to`). \
Never ask your own id (deadlock).";

const FULL: &str = "\
agentbus is a daemonless message bus over a shared local store \
(~/.agentbus). Envelope kinds: message (one-way), ask (blocks for a \
reply), reply (resolves an ask), event (broadcast log).

Quickstart:
1. register(instance_id) first. Only recipients need registration; any \
`from` string may send.
2. send / ask / publish_event to talk. check_inbox (non-blocking) or \
await_message (blocking) to receive; both return {\"envelopes\": [...]}. \
An empty await_message batch means timeout — a normal outcome, not an \
error.
3. To answer an inbound ask: reply(from=<you>, request_id=<the ask \
envelope's id>, payload=...). Do not set `to`; the store routes it.

Rules:
- Never ask your own instance_id: you would block waiting for a reply \
only you could write (deadlock).
- payload is structured JSON (object/array/number), not a stringified \
blob.
- An ask timeout does not discard the request; the error data carries \
the request_id and a late reply stays retrievable (CLI: agentbus \
ask-result <request_id>).
- Registrations default to dying with this session; persistent=true \
survives until unregister.
- Errors arrive as {\"message\": <stable code>, \"data\": <detail with \
a recovery hint>}.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_the_three_levels_and_rejects_junk() {
        assert_eq!(Level::parse("none"), Some(Level::None));
        assert_eq!(Level::parse("minimal"), Some(Level::Minimal));
        assert_eq!(Level::parse("full"), Some(Level::Full));
        assert_eq!(Level::parse("FULL"), None);
        assert_eq!(Level::parse(""), None);
    }

    #[test]
    fn none_yields_no_text_and_others_teach_the_reply_rule() {
        assert!(text(Level::None).is_none());
        for level in [Level::Minimal, Level::Full] {
            let t = text(level).unwrap();
            assert!(t.contains("ask envelope's id"), "{level:?}: {t}");
        }
    }
}
```

- [ ] **Step 2: Declare the module**

In `crates/agentbus-stdio/src/main.rs`, after `mod tools;` add:

```rust
mod instructions;
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p agentbus-stdio instructions`
Expected: 2 tests PASS. (A `dead_code` warning for the not-yet-wired module is expected and disappears in Task 4; if it is denied by clippy later, Task 4 resolves it before the final gate.)

- [ ] **Step 4: Commit**

```bash
git add crates/agentbus-stdio/src/instructions.rs crates/agentbus-stdio/src/main.rs
git commit -m "feat(stdio): add instructions guidance module"
```

---

### Task 4: Shim `--instructions` flag and initialize wiring

**Files:**
- Modify: `crates/agentbus-stdio/src/main.rs` (flag parsing, `handle` signature, initialize arm)
- Test: `crates/agentbus-stdio/tests/rpc.rs`

- [ ] **Step 1: Write the failing tests**

In `crates/agentbus-stdio/tests/rpc.rs`, replace `Shim::spawn` with an args-aware pair (keeping every existing call site working):

```rust
    fn spawn(dir: &std::path::Path) -> Shim {
        Shim::spawn_args(dir, &[])
    }

    fn spawn_args(dir: &std::path::Path, args: &[&str]) -> Shim {
        let mut child = Command::new(env!("CARGO_BIN_EXE_agentbus-stdio"))
            .env("AGENTBUS_DIR", dir)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Shim {
            child,
            stdin,
            stdout,
        }
    }
```

Append two tests:

```rust
#[test]
fn initialize_includes_instructions_by_default() {
    let tmp = tempfile::tempdir().unwrap();
    let mut shim = Shim::spawn(tmp.path());
    let init = shim.call(serde_json::json!({
        "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {}
    }));
    let text = init["result"]["instructions"].as_str().unwrap();
    assert!(text.contains("ask envelope's id"), "got: {text}");
}

#[test]
fn instructions_none_omits_the_field() {
    let tmp = tempfile::tempdir().unwrap();
    let mut shim = Shim::spawn_args(tmp.path(), &["--instructions=none"]);
    let init = shim.call(serde_json::json!({
        "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {}
    }));
    assert!(init["result"]["instructions"].is_null());
    assert_eq!(init["result"]["serverInfo"]["name"], "agentbus-stdio");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p agentbus-stdio --test rpc`
Expected: `initialize_includes_instructions_by_default` FAILS (unwrap on a missing `instructions` field); `instructions_none_omits_the_field` PASSES only by accident today — confirm the first failure, that is the driving test.

- [ ] **Step 3: Wire the flag and the initialize field**

In `crates/agentbus-stdio/src/main.rs`:

(a) at the top of `main`, after the tracing init, resolve the level and text:

```rust
    let level = instructions_level();
    let instructions = instructions::text(level);
```

(b) change the dispatch call inside the loop:

```rust
        let resp = handle(&mut store, &mut session, instructions, method, params);
```

(c) change `handle`'s signature and its initialize arm:

```rust
fn handle(
    store: &mut Store,
    session: &mut tools::Session,
    instructions: Option<&'static str>,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, serde_json::Value> {
    match method {
        "initialize" => {
            let mut result = json!({
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": "agentbus-stdio", "version": env!("CARGO_PKG_VERSION")},
                "capabilities": {"tools": {}}
            });
            if let Some(text) = instructions {
                result["instructions"] = json!(text);
            }
            Ok(result)
        }
```

(d) add the parser at the bottom of `main.rs` (hand-rolled: the shim deliberately has no clap dependency and exactly one flag):

```rust
/// Parse `--instructions=<v>` / `--instructions <v>` (none|minimal|full).
/// Default: full. Unknown values are a startup error; unknown flags are
/// ignored so future MCP-host-injected args do not kill the shim.
fn instructions_level() -> instructions::Level {
    let mut level = instructions::Level::Full;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = match arg.strip_prefix("--instructions=") {
            Some(v) => Some(v.to_string()),
            None if arg == "--instructions" => args.next(),
            None => None,
        };
        let Some(value) = value else { continue };
        match instructions::Level::parse(&value) {
            Some(l) => level = l,
            None => {
                eprintln!("error: invalid --instructions value `{value}` (expected none|minimal|full)");
                std::process::exit(2);
            }
        }
    }
    level
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p agentbus-stdio`
Expected: all PASS (rpc integration tests + instructions unit tests).

- [ ] **Step 5: Commit**

```bash
git add crates/agentbus-stdio/src/main.rs crates/agentbus-stdio/tests/rpc.rs
git commit -m "feat(stdio): emit usage instructions at initialize"
```

---

### Task 5: Rewrite SKILL.md CLI-first

**Files:**
- Rewrite: `skills/agentbus/SKILL.md`

No automated test; the verification is reading it against the spec's section 6 checklist in the step below.

- [ ] **Step 1: Replace the body**

Overwrite `skills/agentbus/SKILL.md` with:

````markdown
---
name: agentbus
description: Use when you need to coordinate with another AI session, agent, human, or external process via the agentbus message bus — sending messages, asking questions and waiting for answers, broadcasting events, draining an inbox, or registering this session under a stable instance_id. Trigger on phrases like "send to another claude", "ask the orchestrator", "coordinate with bob", "broadcast event", "agentbus", "message bus", "register me as", "check my inbox", "talk to another session".
---

# agentbus

agentbus is a daemonless message bus driven by the `agentbus` CLI. This
skill teaches you how to USE it well; `agentbus <verb> --help` tells you
WHAT each flag does.

## Mental model

There is **no daemon**. The bus is a shared local store (`~/.agentbus/`:
a SQLite database plus per-instance JSONL inbox spool files). Every CLI
invocation operates on the store directly, in-process. Think Maildir or
git.

- **Instance**: a participant with a stable id (a session, an agent, a
  script). A registration is a database row. Non-persistent rows are
  anchored to a pid and vanish when that process dies; `--persistent`
  rows survive reboots.
- **Envelope**: every wire message — has `id`, `kind`, `from`, optional
  `to`, optional `request_id`, `ts`, and a structured `payload` (JSON).
- **Kinds**:
  - `message` — fire-and-forget, one recipient, spooled to their inbox.
  - `ask` — request that blocks the sender until a `reply` or timeout.
  - `reply` — resolves a specific `ask` by its `request_id`.
  - `event` — broadcast appended to the ordered event log (no `to`).
- **Inbox**: per-instance append-only spool file, unbounded and durable.
  Messages to an absent instance wait in its spool until consumed.

## Quickstart

```bash
# 1. register yourself first (see "Registering this session" below)
agentbus register "$MY_ID" --pid "$PPID"

# 2. talk
echo '{"hello":"world"}' | agentbus send bob --from "$MY_ID"
echo '{"q":"ready?"}'    | agentbus ask bob --from "$MY_ID" --timeout-ms 60000
echo '{"deploy":"done"}' | agentbus publish --from "$MY_ID"

# 3. drain incoming (both print {"envelopes": [...]} batches)
agentbus check-inbox "$MY_ID"                  # non-blocking
agentbus await "$MY_ID" --timeout-ms 60000     # blocking

# 4. on an inbound ask (kind="ask"), answer by its envelope id
echo '{"a":"yes"}' | agentbus reply <ask-envelope-id> "$MY_ID"
```

## Registering this session

Non-persistent rows need a pid anchor that lives as long as your session.
A bare `agentbus register` records the CLI's own pid, which exits
immediately — the row would be instantly dead. Pick one:

- `--pid <session-pid>`: anchor to the harness process. From a shell you
  spawn, `$PPID` usually is the harness pid; verify once with
  `ps -o comm= -p $PPID` if unsure. Cleanup is automatic: when the
  harness dies, the row is reclaimed lazily (next `register`) or by
  `agentbus sweep`.
- `--persistent`: a durable address that survives reboots; release it
  with `agentbus unregister <id>` when the role ends.

Only **recipients** need registration. Any `--from` string is accepted
for sending; register only ids that must receive.

Naming: stable + descriptive (`code-reviewer-pr123`,
`orchestrator-deploy`), charset `[A-Za-z0-9_.:-]{1,128}`.

## Picking the right verb

| Need | Verb | Notes |
|---|---|---|
| Tell another instance something, don't wait | `send` | recipient must be registered |
| Ask a question and need the answer | `ask` | blocks; exit 2 on timeout |
| Answer someone else's ask | `reply` | first arg = the ask envelope's `id` |
| Broadcast to observers | `publish` | no recipient; readers tail the log |
| Pull pending messages once | `check-inbox` | non-blocking, drains all |
| Wait for messages | `await` | blocks up to `--timeout-ms`; empty list on timeout |
| Who is registered? | `ls` | rows carry an `alive` flag |
| Follow the event log | `events --follow` | `--since <seq>` to resume |
| Crash cleanup | `sweep` | prunes dead rows, reports expired asks |

## Blocking calls under a harness

`ask` and `await` block the shell. Keep `--timeout-ms` comfortably below
your harness's shell-command timeout (e.g. with a 2-minute limit, use
`--timeout-ms 100000`) and loop if you need to wait longer. An `ask`
timeout exits 2 and does NOT discard the request — stderr names the
request_id; fetch a late answer with `agentbus ask-result <request_id>`.

## Patterns

### Ask/reply roundtrip (you are the answerer)

```bash
agentbus await "$MY_ID" --timeout-ms 60000
# in the printed envelopes, find kind=="ask"; its "id" is the request id
echo '{"answer":42}' | agentbus reply msg_01HXY... "$MY_ID"
```

### Wake a recipient on delivery (no polling)

```bash
agentbus register worker-1 --pid "$PPID" \
  --on-delivery "bellhop dispatch worker-1"
```

Every sender executes the command (15 s cap) after spooling to you. Hook
failures are non-fatal — the envelope is already durably spooled.
Security: the command runs as your OS user in the sender's process;
register only commands you trust.

### Fanout broadcast

```bash
echo '{"kind":"deploy.started","sha":"abc"}' | agentbus publish --from "$MY_ID"
agentbus events --follow --since 42     # consumers replay/follow
```

### Harness-dependent extras (skip if yours lacks the facility)

- **Hook-injected inbox** (e.g. Claude Code SessionStart hook): inject
  `~/.agentbus/inbox/<your-id>.jsonl` into prompt context at boot; then
  you do not call `await` at all. See `scripts/inject-inbox.sh`.
- **Live monitor** (e.g. Claude Code Monitor): run
  `agentbus watch <id>` under the monitor; it prints one line per
  envelope addressed to you and never consumes the inbox — react with
  `check-inbox`. See `docs/reference/watch-integration.md`.

## Gotchas

- **Self-ask deadlocks.** Never `ask` your own instance id; the answer
  would have to come from you, but you are blocked waiting for it.
- **`reply` takes the ask envelope's `id`** as its request_id argument —
  not a message id; plain `message` envelopes take no reply.
- **Batches.** `check-inbox`/`await` print `{"envelopes": [...]}` —
  possibly several, possibly empty (empty = timeout, a normal outcome).
- **Payload is structured JSON** read from stdin or `--file`; pass an
  object/array, not double-encoded text.
- **Delivery is durable.** Spools are unbounded append-only files;
  nothing is dropped and mail survives reboots.
- **No daemon exists.** If `agentbus` is missing, install it
  (`cargo install agentbus-cli@^0.2`); do not look for a server process.

## Errors (CLI stderr: `error[<code>]: <detail with recovery hint>`)

| Code | Meaning | Recover by |
|---|---|---|
| `unknown_instance` | recipient not registered | `agentbus ls`; register the target first |
| `instance_id_taken` | a live process owns that id | pick a different id (dead owners auto-replaced) |
| `invalid_instance_id` | bad id syntax | use `[A-Za-z0-9_.:-]{1,128}` |
| `timeout` (exit 2) | ask expired unanswered | `agentbus ask-result <request_id>` later |
| `unknown_request_id` | no such ask | you replied with a message id, or to a plain `message` |
| `store_locked` | write contention | retry after a short wait |

## When NOT to use agentbus

- Within a single process — use normal function calls.
- Cross-machine — the store is a local directory (`0700`), one machine,
  one user.
- Job queues needing acks/retries/leases — delivery is durable but
  consume-once.
- Auth-required surfaces — trust boundary is filesystem ownership only.

## MCP fallback

Clients that cannot run shell commands can load the `agentbus-stdio` MCP
server instead; it exposes the same operations as nine tools and teaches
itself via initialize `instructions`. If this skill is present, prefer
the CLI and do not install the shim.
````

- [ ] **Step 2: Verify against the spec checklist**

Check each item from spec section 6 is present in the new file: CLI quickstart (yes), `--pid` registration with pid-discovery guidance and `--persistent` alternative (yes), timeout-below-harness-limit advice with loop (yes), harness-dependent sections marked (yes), mental model/gotchas/error table preserved (yes).

- [ ] **Step 3: Commit**

```bash
git add skills/agentbus/SKILL.md
git commit -m "docs(skills): rewrite agentbus skill cli-first"
```

---

### Task 6: Plugin packaging — drop the shim

**Files:**
- Delete: `.mcp.json`
- Rewrite: `commands/install.md`
- Modify: `.claude-plugin/plugin.json` (description)

Note: removing `.mcp.json` also removes the `mcp__agentbus__*` tools from
contributors' own sessions in this repo — that is intended dogfooding of
the skill+CLI path.

- [ ] **Step 1: Remove the shim registration**

```bash
git rm .mcp.json
```

- [ ] **Step 2: Rewrite the stale daemon-era install command**

Overwrite `commands/install.md` with:

```markdown
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
   - `agentbus --version` — if it prints a `0.2.x` version, the CLI is
     installed.
   - With `--with-shim`: also check `agentbus-stdio` resolves on PATH
     (`command -v agentbus-stdio`).
2. Install what is missing:
   - `cargo install agentbus-cli@^0.2` (add `--root <prefix>` when the
     user passed `--prefix`).
   - With `--with-shim`: `cargo install agentbus-stdio@^0.2`.
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
```

- [ ] **Step 3: Fix the plugin description**

In `.claude-plugin/plugin.json`, replace the `description` value with:

```json
  "description": "Message bus for AI-agent coordination. Ships a skill that drives the agentbus CLI (daemonless spool store) and an install command; an MCP stdio shim remains available for clients without skills or shell.",
```

- [ ] **Step 4: Commit**

```bash
git add -A .mcp.json commands/install.md .claude-plugin/plugin.json
git commit -m "chore(plugin): ship skill+cli, drop shim default"
```

---

### Task 7: New FR 16 — usage guidance channels

**Files:**
- Create: `docs/fr/16-usage-guidance.md`
- Regenerate: `docs/fr/index.md` (via `kusara index`, never by hand)

- [ ] **Step 1: Create the FR**

Create `docs/fr/16-usage-guidance.md`:

```markdown
---
refs:
  id: fr:16-usage-guidance
  kind: fr
  title: "Usage guidance channels"
  related:
    - fr:08-mcp-shim
    - fr:10-cli
    - fr:12-store
  modules:
    - skills/agentbus/SKILL.md
    - crates/agentbus-stdio/src/instructions.rs
---

# FR 16: Usage guidance channels

> How clients learn to use the bus correctly, at the lowest possible
> context cost per session.

## Purpose

The knowledge that makes an agent use the bus correctly (ask/reply
request_id semantics, self-ask deadlock, batch returns, registration
lifetimes) must reach every client kind without taxing sessions that
never touch the bus. This FR is the design of record for the channel
hierarchy and the suppression contract. Design rationale:
`docs/superpowers/specs/2026-06-07-skill-first-guidance.md`.

## User-visible Behavior

Guidance reaches a client through the cheapest channel it supports,
in this order:

| Channel | Cost profile | Carrier |
|---|---|---|
| Skill (`skills/agentbus/SKILL.md`) | one description line per session; body loads on use | skill-capable harnesses; teaches the CLI |
| Error detail strings | zero until a mistake; arrives exactly when needed | `StoreError` Display (fr:12-store), all surfaces |
| MCP `instructions` | paid at initialize by shim sessions | `agentbus-stdio` (fr:08-mcp-shim) |
| MCP tool descriptions | paid at initialize by shim sessions | `agentbus-stdio` tool specs |

- Skill-capable clients (Claude Code, Codex) use the skill + CLI and do
  not load the shim at all.
- MCP-only clients load the shim and receive condensed guidance via the
  initialize `instructions` field, suppressible with
  `--instructions=none|minimal|full` (default `full`).
- The fork between the two is decided at install/packaging time by
  whoever configures the client — never sniffed at runtime. A
  configuration that installs the skill must not also register the shim.

## Capabilities

- `skills/agentbus/SKILL.md` is the single source of truth for usage
  prose. The shim's `instructions` text
  (`crates/agentbus-stdio/src/instructions.rs`) is a hand-maintained
  condensed subset of it.
- Error Display strings carry recovery hints (fr:12-store), so even an
  instructions-suppressed, skill-less client self-corrects in one
  roundtrip.
- Tool descriptions stay terse one-liners; correctness teaching lives in
  the channels above.

## Boundaries

- No runtime client detection: `clientInfo` is unauthenticated free-form
  prose and must not gate behavior.
- The shim's nine-tool surface is not collapsed into a meta-tool (spec
  open question 1; deferred).
- No second skill packaging for non-Claude hosts exists yet (spec open
  question 2); the SKILL.md body is written host-neutrally so one can be
  derived.

## Error Handling

None of its own; the recovery-hint wording is owned by fr:12-store.

## Traceability

- Reference docs: ref:protocol
- Related FR: fr:08-mcp-shim, fr:10-cli, fr:12-store
- Spec: docs/superpowers/specs/2026-06-07-skill-first-guidance.md

## When to update

- A guidance channel is added or removed (e.g. a meta-tool, a second
  skill packaging).
- The `--instructions` levels or default change.
- SKILL.md's mental model or gotchas change (the condensed shim text must
  be re-derived).
- The packaging fork (skill vs shim) gains any runtime detection.
```

- [ ] **Step 2: Regenerate the index**

Run: `kusara index`
Expected: `docs/fr/index.md` gains the fr:16 row. The post-edit hook has already run `kusara validate`; if it reported errors, fix the frontmatter before continuing.

- [ ] **Step 3: Commit**

```bash
git add docs/fr/16-usage-guidance.md docs/fr/index.md
git commit -m "docs(fr): add fr:16 usage guidance channels"
```

---

### Task 8: FR deltas — fr:02, fr:08, fr:10, fr:12

**Files:**
- Modify: `docs/fr/02-instance-registry.md`, `docs/fr/08-mcp-shim.md`, `docs/fr/10-cli.md`, `docs/fr/12-store.md`

- [ ] **Step 1: fr:02 — explicit pid is now user-reachable**

In `docs/fr/02-instance-registry.md`, replace the bullet

```markdown
- A **non-persistent** row records the caller's pid (`std::process::id()`)
  unless the caller supplies an explicit pid via `RegisterOpts`. That pid is
```

with

```markdown
- A **non-persistent** row records the caller's pid (`std::process::id()`)
  unless the caller supplies an explicit pid via `RegisterOpts` — exposed to
  users as `agentbus register --pid <pid>` (fr:10-cli), the anchor for
  session-scoped registration without the shim. That pid is
```

- [ ] **Step 2: fr:08 — fallback role + instructions**

In `docs/fr/08-mcp-shim.md`:

(a) Replace the `## Purpose` first paragraph's opening sentence

```markdown
Each AI session launches its own `agentbus-stdio` process. In v0.2 the shim
```

with

```markdown
The shim is the fallback AI surface: for MCP-capable clients that cannot
load skills or run shell commands (fr:16-usage-guidance). Skill-capable
clients drive the CLI instead and do not load the shim. A client that does
load it launches its own `agentbus-stdio` process. In v0.2 the shim
```

(b) After the v0.2 surface-changes list in `## User-visible Behavior`, add:

```markdown
The `initialize` result carries an `instructions` string with condensed
usage guidance (`src/instructions.rs`), controlled by the
`--instructions=none|minimal|full` startup flag (default `full`).
Packagings that ship the skill pass `none` to avoid paying the duplicate
context cost. Unknown flag values are a startup error (exit 2); unknown
flags are ignored.
```

(c) In the frontmatter `related:` list and the `## Traceability` line, add `fr:16-usage-guidance`. Add `crates/agentbus-stdio/src/instructions.rs` to `modules:`.

(d) In `## When to update`, add bullets:

```markdown
- The `instructions` text, its levels, or the flag default changes.
- The shim's role as fallback (vs primary) surface changes.
```

- [ ] **Step 3: fr:10 — register --pid**

In `docs/fr/10-cli.md`:

(a) Replace the `register` row of the verb table with:

```markdown
| `register` | `--persistent`, `--on-delivery`, `--pid` | Register an instance id; `--pid <pid>` anchors the non-persistent row to a long-lived process (e.g. an AI harness) for session-scoped identity; `--persistent` for durable addresses; `--pid` and `--persistent` conflict |
```

(b) Replace the first `## Boundaries` bullet (`- Non-persistent `register` from the CLI records ...` through `... embed the store directly.`) with:

```markdown
- A bare non-persistent `register` records the CLI process's pid, which
  exits immediately — the row is instantly dead. Session-scoped identity
  uses `--pid <session-pid>` (the primary AI path, taught by the agentbus
  skill); durable addresses use `--persistent`. The shim (fr:08-mcp-shim)
  remains an alternative anchor for MCP-only clients.
```

(c) In `## When to update`, replace

```markdown
- Non-persistent register pid-exit behavior is addressed (e.g. a warning is
  added).
```

with

```markdown
- The `--pid` anchoring contract changes (e.g. pid validation or a
  `--pid auto` mode is added).
```

- [ ] **Step 4: fr:12 — recovery hints**

In `docs/fr/12-store.md`, in `## Error Handling` after the
`StoreError::code()` paragraph, add:

```markdown
Display strings — the `data` field on the MCP wire and the detail after
`error[<code>]:` on CLI stderr — carry recovery hints (e.g.
`unknown_request_id` explains that a reply takes the ask envelope's id).
The stable `code()` strings never change; only the hint prose may evolve.
Hints exist so that clients receiving no other guidance channel
(fr:16-usage-guidance) self-correct at failure time.
```

And in `## When to update`, add:

```markdown
- A recovery hint is added to or removed from an error Display string.
```

- [ ] **Step 5: Commit**

```bash
git add docs/fr/02-instance-registry.md docs/fr/08-mcp-shim.md docs/fr/10-cli.md docs/fr/12-store.md
git commit -m "docs(fr): apply skill-first guidance deltas"
```

---

### Task 9: Protocol reference and README

**Files:**
- Modify: `docs/reference/protocol.md`, `README.md`

- [ ] **Step 1: protocol.md**

(a) Section 2 table, `register` row — replace with:

```markdown
| `register(id, persistent?, on_delivery?, pid?)` | Claim an instance id. Non-persistent rows are anchored to the caller's pid, or to an explicit `pid` (CLI `--pid`) for session-scoped identity (fr:02). | `instance_id_taken`, `invalid_instance_id` |
```

(b) Section 2.1, after the error-code table, add:

```markdown
Error detail strings (CLI stderr / MCP `data`) include recovery hints;
the codes above are the stable contract, the prose is not.
```

(c) Section 3 intro — after the sentence ending `See [fr:08-mcp-shim](../fr/08-mcp-shim.md) for the full specification.`, add:

```markdown
The shim is the fallback surface for clients without skills or shell
access; skill-capable clients drive the CLI instead (fr:16-usage-guidance).
At `initialize` the shim returns an `instructions` string of condensed
usage guidance; suppress or shrink it with `--instructions=none|minimal|full`
(default `full`).
```

(d) Section 4 register example — extend the bash block's first stanza with:

```bash
# Session-scoped registration without the shim: anchor to a long-lived pid
agentbus register sess-ENG-123 --pid 48211
# {"ok": true}
```

- [ ] **Step 2: README.md**

(a) Replace the surface table rows so the CLI leads and the shim is marked fallback:

```markdown
| Surface | Audience | Transport |
|---|---|---|
| CLI (`agentbus`) | AI sessions (skill-guided), humans, shell scripts, external programs | subcommand per verb |
| MCP stdio shim (`agentbus-stdio`) | MCP-capable AI clients without skills or shell (fallback) | stdin/stdout JSON-RPC |
| Hook-driven inbox | Workflows at prompt / session boundaries | JSONL spool files |
| Watch streaming | Harnesses with a persistent monitor facility | event-log tail |
```

(b) Replace the paragraph + JSON snippet introduced by `To wire the MCP shim into Claude Code, add to `.mcp.json`:` with:

```markdown
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
```

(c) In the Install section, change `cargo install agentbus-cli@^0.2 agentbus-stdio@^0.2` to:

```bash
cargo install agentbus-cli@^0.2          # the CLI: all most setups need
cargo install agentbus-stdio@^0.2        # optional MCP shim (fallback clients)
```

- [ ] **Step 3: Commit**

```bash
git add docs/reference/protocol.md README.md
git commit -m "docs: reposition cli as primary surface"
```

---

### Task 10: Final verification gate

**Files:** none (checks only)

- [ ] **Step 1: Workspace gates**

Run, in order, expecting every one to pass:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
kusara validate
kusara index   # must be a no-op now; commit if it changed anything
git status     # clean except the plan/spec docs themselves
```

- [ ] **Step 2: Manual smoke of the two new behaviors**

```bash
# instructions flag end-to-end
printf '%s\n' '{"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}' \
  | ./target/debug/agentbus-stdio --instructions=minimal | head -1
# expect: ..."instructions":"agentbus message bus. ..."

# --pid end-to-end against a scratch store
AGENTBUS_DIR=$(mktemp -d) sh -c \
  './target/debug/agentbus register sess --pid $$ && ./target/debug/agentbus ls'
# expect: row "sess" with the sh pid, "alive": true
```

- [ ] **Step 3: Push and hand off**

Per superpowers:finishing-a-development-branch — run the verification skill, then offer merge/PR options. Suggested PR title: `feat: skill-first client guidance`.

---

## Self-review notes (already applied)

- Spec coverage: spec section 4 decisions map to Tasks 1 (`--pid`), 2 (error hints), 3+4 (instructions + flag), 5 (skill source of truth), 6 (packaging fork); section 6 → Task 5; section 7 FR deltas → Tasks 7-9. Open questions 1-4 are deliberately NOT implemented (deferred in spec).
- The `instructions` text in Task 3 and the SKILL.md in Task 5 both phrase the reply rule as "the ask envelope's id" — Task 2's hint and Task 3's test assert that exact phrase; keep them in sync if rewording.
- `Shim::spawn_args` (Task 4) matches the existing `Shim::spawn` call sites; no other test file constructs the shim.
- No `>`/`>=` appears in any code block above.
