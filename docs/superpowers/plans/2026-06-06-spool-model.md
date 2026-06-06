# agentbus v0.2 Spool Model Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the agentbusd daemon with a daemonless spool store (SQLite + per-instance JSONL inbox files) per `docs/superpowers/specs/2026-06-05-spool-model-design.md`.

**Architecture:** A new synchronous `store` module in `agentbus-core` (rusqlite, WAL) absorbs registry/router/eventlog semantics; delivery is the sender appending to the recipient's inbox spool and running its `on_delivery` hook. The MCP shim and CLI call the store in-process. `agentbusd` and the in-memory mailbox/registry/router/eventlog modules are deleted.

**Tech Stack:** Rust 2021 (MSRV 1.85), rusqlite 0.40 (bundled), libc 0.2 (flock + kill), clap 4, serde/serde_json, thiserror, ulid, time. tokio/axum/reqwest are removed.

---

## Design decisions locked by this plan

These are implementation choices the spec leaves open. Do not revisit them mid-execution.

1. **The store is synchronous.** rusqlite is sync; CLI invocations are short-lived; the MCP shim is a single-threaded line loop. tokio is removed from every remaining crate. Blocking waits use `std::thread::sleep` with the spec's 50 ms → 250 ms backoff.
2. **`libc` provides both syscalls we need**: `flock(2)` for inbox appends and `kill(pid, 0)` for liveness.
3. **One `rusqlite::Connection` per `Store`.** Mutating ops take `&mut self` and run inside `BEGIN IMMEDIATE` via a `with_tx` helper. Pure reads take `&self`.
4. **Compact JSON per line is the stream format.** `watch` and `events` print one compact-JSON envelope per line; serde_json escapes newlines inside strings, so the "one message = one line" contract of spec section 6.7 holds by construction.
5. **Comparison style:** never use `>` or `>=` in Rust or SQL. Write `0 < n`, `deadline <= now`, `WHERE ?1 < seq`. This is a repo-wide rule.

## Spec deltas (fold back into the spec after review)

- `asks` gains an `expired_notified INTEGER NOT NULL DEFAULT 0` column so `sweep` reports each expired ask exactly once (spec 6.8 needs idempotency; the row must survive for `ask-result`).
- Error model gains `unknown_request_id` (reply/ask-result against a nonexistent ask; spec section 8 table omits it but section 6.3 implies it).
- Re-registering an existing **persistent** row is an idempotent upsert (updates `on_delivery`). Single-user trust model makes takeover a non-issue.
- `await_message` returns an empty list on timeout (not an error): "nothing arrived" is a normal outcome for the MCP tool.
- `scripts/inject-inbox.sh` default inbox dir changes from `$XDG_RUNTIME_DIR/agentbus/inbox` to `$HOME/.agentbus/inbox` (the v0.2 store layout). `AGENTBUS_INBOX_DIR` still overrides.

## Execution notes

- The `.claude/hooks/refs-postedit.sh` hook will warn that FR docs own the files you touch under `crates/`. Doc rewrites are Tasks 14–17; ignore the warnings until then.
- Every task ends with the workspace compiling and `cargo test --workspace` green. CI's exact commands: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo build --workspace --all-targets`, `cargo test --workspace`.
- Commit after every task (conventional commits, lowercase, title ≤ 50 chars).
- cmux-bellhop changes (spec section 13) are a separate repo and explicitly out of scope.

## File structure (end state)

```
crates/agentbus-core/src/
  lib.rs            exports envelope, ids, store
  envelope.rs       unchanged + Kind::as_str / FromStr
  ids.rs            unchanged
  store/
    mod.rs          Store, open/open_at, SCHEMA, with_tx, new_envelope, append_event
    paths.rs        base dir resolution, 0700 layout
    error.rs        StoreError + stable codes (spec section 8)
    liveness.rs     pid_alive (kill(pid, 0))
    instances.rs    register / unregister / list (spec 6.1)
    spool.rs        inbox append (flock+O_APPEND), check_inbox, await_message (fr:09, spec 6.6)
    events.rs       publish_event, events_since, max_seq (spec 6.4)
    messages.rs     send, ask, reply, ask_result (spec 6.2, 6.3)
    hook.rs         on_delivery execution (spec 6.5)
    sweep.rs        sweep (spec 6.8)
crates/agentbus-stdio/src/
  main.rs           sync JSON-RPC loop over stdin/stdout
  tools.rs          MCP tool specs + dispatch onto Store
crates/agentbus-cli/src/
  main.rs           parse + error printing with codes
  commands.rs       clap tree + dispatch onto Store (incl. watch, spec 6.7)
crates/agentbusd/    DELETED
```

---

### Task 1: Store foundation (deps, paths, schema, errors, liveness)

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Modify: `crates/agentbus-core/Cargo.toml`
- Modify: `crates/agentbus-core/src/lib.rs`
- Modify: `crates/agentbus-core/src/envelope.rs`
- Create: `crates/agentbus-core/src/store/mod.rs`
- Create: `crates/agentbus-core/src/store/paths.rs`
- Create: `crates/agentbus-core/src/store/error.rs`
- Create: `crates/agentbus-core/src/store/liveness.rs`

- [ ] **Step 1: Add workspace dependencies**

In the root `Cargo.toml` `[workspace.dependencies]` section, add:

```toml
rusqlite = { version = "0.40", features = ["bundled"] }
libc = "0.2"
```

In `crates/agentbus-core/Cargo.toml` `[dependencies]`, add:

```toml
rusqlite = { workspace = true }
libc = { workspace = true }
```

- [ ] **Step 2: Add Kind helpers to envelope.rs (failing tests first)**

Append to the `#[cfg(test)] mod tests` in `crates/agentbus-core/src/envelope.rs`:

```rust
#[test]
fn kind_as_str_roundtrips_with_fromstr() {
    for kind in [Kind::Message, Kind::Ask, Kind::Reply, Kind::Event] {
        assert_eq!(kind.as_str().parse::<Kind>().unwrap(), kind);
    }
}

#[test]
fn kind_fromstr_rejects_unknown() {
    assert!("bogus".parse::<Kind>().is_err());
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p agentbus-core kind_ -- --nocapture`
Expected: compile error (`as_str` not found).

- [ ] **Step 4: Implement Kind helpers**

Add below the `Kind` enum in `crates/agentbus-core/src/envelope.rs`:

```rust
impl Kind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Kind::Message => "message",
            Kind::Ask => "ask",
            Kind::Reply => "reply",
            Kind::Event => "event",
        }
    }
}

impl std::str::FromStr for Kind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "message" => Ok(Kind::Message),
            "ask" => Ok(Kind::Ask),
            "reply" => Ok(Kind::Reply),
            "event" => Ok(Kind::Event),
            other => Err(format!("unknown kind `{other}`")),
        }
    }
}
```

Run: `cargo test -p agentbus-core kind_` — expected: PASS.

- [ ] **Step 5: Create the store submodules**

Create `crates/agentbus-core/src/store/paths.rs`:

```rust
//! Store layout (spec section 5): a single 0700 directory holding bus.db
//! and per-instance inbox spool files.

use std::path::{Path, PathBuf};

/// Store root: $AGENTBUS_DIR when set, else ~/.agentbus.
pub fn base_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("AGENTBUS_DIR") {
        return PathBuf::from(p);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    home.join(".agentbus")
}

pub fn inbox_dir(base: &Path) -> PathBuf {
    base.join("inbox")
}

pub fn db_path(base: &Path) -> PathBuf {
    base.join("bus.db")
}

/// Create base and inbox dirs, tightening permissions to 0700 even when
/// they already exist (spec section 9).
pub fn ensure_layout(base: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    for dir in [base.to_path_buf(), inbox_dir(base)] {
        std::fs::create_dir_all(&dir)?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn ensure_layout_creates_0700_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("bus");
        ensure_layout(&base).unwrap();
        for dir in [&base, &inbox_dir(&base)] {
            let mode = std::fs::metadata(dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "{dir:?}");
        }
        // Idempotent on a second call.
        ensure_layout(&base).unwrap();
    }
}
```

Create `crates/agentbus-core/src/store/error.rs`:

```rust
use crate::envelope::ValidationError;

/// Spec section 8 error model. `code()` is the stable wire identifier.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("unknown instance `{0}`")]
    UnknownInstance(String),
    #[error("instance_id `{0}` is registered to another live process")]
    InstanceIdTaken(String),
    #[error("invalid instance_id (must match [A-Za-z0-9_.:-]{{1,128}})")]
    InvalidInstanceId,
    #[error("ask `{0}` timed out (a late reply stays retrievable via ask-result)")]
    Timeout(String),
    #[error("unknown request_id `{0}`")]
    UnknownRequestId(String),
    #[error("store locked: busy_timeout exhausted")]
    StoreLocked,
    #[error("invalid envelope: {0}")]
    InvalidEnvelope(#[from] ValidationError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("sqlite: {0}")]
    Sqlite(rusqlite::Error),
}

impl StoreError {
    pub fn code(&self) -> &'static str {
        match self {
            StoreError::UnknownInstance(_) => "unknown_instance",
            StoreError::InstanceIdTaken(_) => "instance_id_taken",
            StoreError::InvalidInstanceId => "invalid_instance_id",
            StoreError::Timeout(_) => "timeout",
            StoreError::UnknownRequestId(_) => "unknown_request_id",
            StoreError::StoreLocked => "store_locked",
            StoreError::InvalidEnvelope(_) => "invalid_envelope",
            StoreError::Io(_) => "io",
            StoreError::Sqlite(_) => "sqlite",
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        if let rusqlite::Error::SqliteFailure(f, _) = &e {
            if f.code == rusqlite::ErrorCode::DatabaseBusy {
                return StoreError::StoreLocked;
            }
        }
        StoreError::Sqlite(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable() {
        assert_eq!(StoreError::UnknownInstance("x".into()).code(), "unknown_instance");
        assert_eq!(StoreError::InstanceIdTaken("x".into()).code(), "instance_id_taken");
        assert_eq!(StoreError::Timeout("x".into()).code(), "timeout");
        assert_eq!(StoreError::StoreLocked.code(), "store_locked");
    }
}
```

Create `crates/agentbus-core/src/store/liveness.rs`:

```rust
//! Pid liveness (spec section 4): same-machine assumption makes
//! kill(pid, 0) an honest check. EPERM means "exists, not ours" -> alive.

pub fn pid_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    let rc = unsafe { libc::kill(pid, 0) };
    if rc == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_pid_is_alive() {
        assert!(pid_alive(std::process::id() as i32));
    }

    #[test]
    fn reaped_child_is_dead() {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id() as i32;
        child.wait().unwrap();
        assert!(!pid_alive(pid));
    }

    #[test]
    fn nonpositive_pids_are_dead() {
        assert!(!pid_alive(0));
        assert!(!pid_alive(-1));
    }
}
```

Create `crates/agentbus-core/src/store/mod.rs`:

```rust
//! The spool store (spec sections 5-6): SQLite for registry/asks/events,
//! per-instance JSONL inbox files for delivery. No daemon; every operation
//! is performed by the calling process.

mod error;
mod liveness;
mod paths;

pub use error::StoreError;
pub use paths::base_dir;

use std::path::{Path, PathBuf};

use rusqlite::{Connection, Transaction, TransactionBehavior};

use crate::envelope::{Envelope, Kind};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS instances (
  id            TEXT PRIMARY KEY,
  pid           INTEGER,
  persistent    INTEGER NOT NULL DEFAULT 0,
  on_delivery   TEXT,
  registered_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS asks (
  request_id       TEXT PRIMARY KEY,
  from_id          TEXT NOT NULL,
  to_id            TEXT NOT NULL,
  expires_at       TEXT NOT NULL,
  reply_payload    TEXT,
  replied_at       TEXT,
  expired_notified INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS event_log (
  seq      INTEGER PRIMARY KEY AUTOINCREMENT,
  ts       TEXT NOT NULL,
  envelope TEXT NOT NULL
);
"#;

pub struct Store {
    conn: Connection,
    base: PathBuf,
}

impl Store {
    /// Open the default store (~/.agentbus or $AGENTBUS_DIR).
    pub fn open() -> Result<Self, StoreError> {
        Self::open_at(&paths::base_dir())
    }

    pub fn open_at(base: &Path) -> Result<Self, StoreError> {
        paths::ensure_layout(base)?;
        let conn = Connection::open(paths::db_path(base))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA busy_timeout=5000;",
        )?;
        conn.execute_batch(SCHEMA)?;
        Ok(Store { base: base.to_path_buf(), conn })
    }

    pub fn base(&self) -> &Path {
        &self.base
    }

    /// All bus.db mutations run inside one BEGIN IMMEDIATE transaction per
    /// operation (spec 5.2). WAL serializes writers; busy_timeout absorbs
    /// contention.
    fn with_tx<T>(
        &mut self,
        f: impl FnOnce(&Transaction) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }
}

/// Stamp id and ts (spec 6.2): senders assign both.
pub(crate) fn new_envelope(
    kind: Kind,
    from: &str,
    to: Option<&str>,
    payload: serde_json::Value,
) -> Envelope {
    Envelope {
        id: crate::ids::new_envelope_id(),
        kind,
        from: from.to_string(),
        to: to.map(str::to_string),
        request_id: None,
        timeout_ms: None,
        ts: crate::ids::now_utc(),
        payload,
    }
}

pub(crate) fn rfc3339(ts: &time::OffsetDateTime) -> String {
    ts.format(&time::format_description::well_known::Rfc3339)
        .expect("UTC timestamps format")
}

/// Append one envelope to event_log; seq is assigned transactionally and is
/// therefore immune to wall-clock skew between writer processes (spec 5.1).
pub(crate) fn append_event(tx: &Transaction, env: &Envelope) -> Result<i64, StoreError> {
    tx.execute(
        "INSERT INTO event_log (ts, envelope) VALUES (?1, ?2)",
        rusqlite::params![
            rfc3339(&env.ts),
            serde_json::to_string(env).expect("envelope serializes")
        ],
    )?;
    Ok(tx.last_insert_rowid())
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::Store;

    pub fn test_store() -> (tempfile::TempDir, Store) {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open_at(tmp.path()).unwrap();
        (tmp, store)
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::test_store;
    use super::*;

    #[test]
    fn open_creates_layout_and_tables() {
        let (tmp, store) = test_store();
        assert!(paths::db_path(tmp.path()).exists());
        assert!(paths::inbox_dir(tmp.path()).is_dir());
        let n: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='table' AND name IN ('instances','asks','event_log')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 3);
    }

    #[test]
    fn open_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        Store::open_at(tmp.path()).unwrap();
        Store::open_at(tmp.path()).unwrap();
    }
}
```

In `crates/agentbus-core/src/lib.rs`, add the module export:

```rust
pub mod store;
```

- [ ] **Step 6: Verify**

Run: `cargo test -p agentbus-core store:: && cargo clippy -p agentbus-core --all-targets -- -D warnings && cargo fmt --all`
Expected: all store tests PASS, no clippy warnings.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock crates/agentbus-core
git commit -m "feat: add spool store foundation to core"
```

---

### Task 2: Instance registration (spec 6.1)

**Files:**
- Create: `crates/agentbus-core/src/store/instances.rs`
- Modify: `crates/agentbus-core/src/store/mod.rs` (add `mod instances;` and `pub use instances::{InstanceRow, RegisterOpts};`)

- [ ] **Step 1: Write the failing tests**

The tests live in-module (codebase convention). Create `crates/agentbus-core/src/store/instances.rs` with ONLY the test module first:

```rust
#[cfg(test)]
mod tests {
    use crate::store::testutil::test_store;
    use crate::store::{RegisterOpts, StoreError};

    fn opts() -> RegisterOpts {
        RegisterOpts::default()
    }

    #[test]
    fn register_and_list_roundtrip() {
        let (_tmp, mut store) = test_store();
        store.register("alice", &opts()).unwrap();
        let rows = store.list_instances().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "alice");
        assert!(rows[0].alive); // our own pid
        assert!(!rows[0].persistent);
    }

    #[test]
    fn reregister_same_pid_is_idempotent() {
        let (_tmp, mut store) = test_store();
        store.register("alice", &opts()).unwrap();
        store.register("alice", &opts()).unwrap();
        assert_eq!(store.list_instances().unwrap().len(), 1);
    }

    #[test]
    fn live_foreign_pid_collides() {
        let (_tmp, mut store) = test_store();
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let foreign = RegisterOpts { pid: Some(child.id() as i32), ..opts() };
        store.register("alice", &foreign).unwrap();
        let err = store.register("alice", &opts()).unwrap_err();
        assert!(matches!(err, StoreError::InstanceIdTaken(_)));
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn dead_pid_row_is_replaced() {
        let (_tmp, mut store) = test_store();
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id() as i32;
        child.wait().unwrap();
        store.register("alice", &RegisterOpts { pid: Some(pid), ..opts() }).unwrap();
        store.register("alice", &opts()).unwrap(); // dead owner -> replace
        let rows = store.list_instances().unwrap();
        assert_eq!(rows[0].pid, Some(std::process::id() as i32));
    }

    #[test]
    fn persistent_row_has_no_pid_and_upserts() {
        let (_tmp, mut store) = test_store();
        let p = RegisterOpts { persistent: true, on_delivery: Some("true".into()), ..opts() };
        store.register("bot", &p).unwrap();
        let p2 = RegisterOpts { persistent: true, on_delivery: Some("false".into()), ..opts() };
        store.register("bot", &p2).unwrap();
        let rows = store.list_instances().unwrap();
        assert_eq!(rows[0].pid, None);
        assert!(rows[0].alive); // persistent rows are exempt from liveness
        assert_eq!(rows[0].on_delivery.as_deref(), Some("false"));
    }

    #[test]
    fn invalid_ids_are_rejected() {
        let (_tmp, mut store) = test_store();
        for bad in ["", "a/b", "a b", &"x".repeat(129)] {
            assert!(matches!(
                store.register(bad, &opts()),
                Err(StoreError::InvalidInstanceId)
            ));
        }
    }

    #[test]
    fn unregister_removes_row_and_reports_absence() {
        let (_tmp, mut store) = test_store();
        store.register("alice", &opts()).unwrap();
        assert!(store.unregister("alice").unwrap());
        assert!(!store.unregister("alice").unwrap());
        assert!(store.list_instances().unwrap().is_empty());
    }
}
```

Add to `store/mod.rs`: `mod instances;` and `pub use instances::{InstanceRow, RegisterOpts};`

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p agentbus-core instances::`
Expected: compile errors (RegisterOpts, register, list_instances missing).

- [ ] **Step 3: Implement**

Prepend to `crates/agentbus-core/src/store/instances.rs` (above the test module):

```rust
//! Instance registration (spec 6.1): a database row, persistent or
//! pid-scoped. Liveness is kill(pid, 0); persistent rows are exempt.

use rusqlite::{params, OptionalExtension, Transaction};
use serde::Serialize;

use super::liveness::pid_alive;
use super::{rfc3339, Store, StoreError};

#[derive(Debug, Clone, Default)]
pub struct RegisterOpts {
    pub persistent: bool,
    pub on_delivery: Option<String>,
    /// Owner pid for non-persistent rows. Defaults to the calling process.
    pub pid: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InstanceRow {
    pub id: String,
    pub pid: Option<i32>,
    pub persistent: bool,
    pub on_delivery: Option<String>,
    pub registered_at: String,
    pub alive: bool,
}

pub(crate) fn valid_instance_id(id: &str) -> bool {
    let ok_byte = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'-');
    !id.is_empty() && id.len() <= 128 && id.bytes().all(ok_byte)
}

/// Recipient lookup used by send/ask: row must exist (spec 6.2).
pub(crate) fn on_delivery_of(tx: &Transaction, id: &str) -> Result<Option<String>, StoreError> {
    let row: Option<Option<String>> = tx
        .query_row(
            "SELECT on_delivery FROM instances WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .optional()?;
    row.ok_or_else(|| StoreError::UnknownInstance(id.to_string()))
}

impl Store {
    pub fn register(&mut self, id: &str, opts: &RegisterOpts) -> Result<(), StoreError> {
        if !valid_instance_id(id) {
            return Err(StoreError::InvalidInstanceId);
        }
        let pid: Option<i32> = if opts.persistent {
            None
        } else {
            Some(opts.pid.unwrap_or(std::process::id() as i32))
        };
        self.with_tx(|tx| {
            let existing: Option<(Option<i32>, bool)> = tx
                .query_row(
                    "SELECT pid, persistent FROM instances WHERE id = ?1",
                    params![id],
                    |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0)),
                )
                .optional()?;
            // Only a LIVE non-persistent row owned by a different pid blocks
            // us. Dead rows are replaced; same-pid and persistent rows upsert.
            if let Some((Some(old_pid), false)) = existing {
                if pid_alive(old_pid) && pid != Some(old_pid) {
                    return Err(StoreError::InstanceIdTaken(id.to_string()));
                }
            }
            tx.execute(
                "INSERT INTO instances (id, pid, persistent, on_delivery, registered_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(id) DO UPDATE SET
                   pid = excluded.pid,
                   persistent = excluded.persistent,
                   on_delivery = excluded.on_delivery,
                   registered_at = excluded.registered_at",
                params![
                    id,
                    pid,
                    opts.persistent as i64,
                    &opts.on_delivery,
                    rfc3339(&crate::ids::now_utc())
                ],
            )?;
            Ok(())
        })
    }

    /// Delete the row; the inbox file stays in place (spec 6.1).
    pub fn unregister(&mut self, id: &str) -> Result<bool, StoreError> {
        self.with_tx(|tx| {
            let n = tx.execute("DELETE FROM instances WHERE id = ?1", params![id])?;
            Ok(0 < n)
        })
    }

    pub fn list_instances(&self) -> Result<Vec<InstanceRow>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, pid, persistent, on_delivery, registered_at
             FROM instances ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(InstanceRow {
                id: r.get(0)?,
                pid: r.get(1)?,
                persistent: r.get::<_, i64>(2)? != 0,
                on_delivery: r.get(3)?,
                registered_at: r.get(4)?,
                alive: false,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            let mut row = row?;
            row.alive = row.persistent || row.pid.map(pid_alive).unwrap_or(false);
            out.push(row);
        }
        Ok(out)
    }
}
```

- [ ] **Step 4: Verify**

Run: `cargo test -p agentbus-core instances:: && cargo clippy -p agentbus-core --all-targets -- -D warnings`
Expected: 7 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/agentbus-core
git commit -m "feat: add instance registration to store"
```

---

### Task 3: Inbox spool — append, check_inbox, await_message (fr:09, spec 6.6)

**Files:**
- Create: `crates/agentbus-core/src/store/spool.rs`
- Modify: `crates/agentbus-core/src/store/mod.rs` (add `mod spool;`)
- Modify: `scripts/inject-inbox.sh` (default dir)

- [ ] **Step 1: Write the failing tests**

Create `crates/agentbus-core/src/store/spool.rs` with the test module:

```rust
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use crate::envelope::Kind;
    use crate::store::testutil::test_store;
    use crate::store::{new_envelope, paths};

    fn env(n: u64) -> crate::envelope::Envelope {
        new_envelope(Kind::Message, "alice", Some("bob"), json!({ "n": n }))
    }

    #[test]
    fn append_then_consume_roundtrip() {
        let (tmp, store) = test_store();
        super::append(tmp.path(), "bob", &env(1)).unwrap();
        super::append(tmp.path(), "bob", &env(2)).unwrap();
        let got = store.check_inbox("bob").unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].payload, json!({"n": 1}));
        // Spool file is consumed: second check is empty.
        assert!(store.check_inbox("bob").unwrap().is_empty());
        assert!(!paths::inbox_dir(tmp.path()).join("bob.jsonl").exists());
    }

    #[test]
    fn consume_skips_corrupt_lines() {
        let (tmp, store) = test_store();
        super::append(tmp.path(), "bob", &env(1)).unwrap();
        let path = paths::inbox_dir(tmp.path()).join("bob.jsonl");
        let mut content = std::fs::read_to_string(&path).unwrap();
        content.push_str("{not json\n");
        std::fs::write(&path, content).unwrap();
        super::append(tmp.path(), "bob", &env(2)).unwrap();
        assert_eq!(store.check_inbox("bob").unwrap().len(), 2);
    }

    #[test]
    fn parallel_appends_keep_lines_intact() {
        // Spec section 11 + open question 1: large lines under contention.
        let (tmp, store) = test_store();
        let big = "x".repeat(8 * 1024);
        let mut handles = Vec::new();
        for w in 0..8u64 {
            let base = tmp.path().to_path_buf();
            let big = big.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..25u64 {
                    let e = crate::store::new_envelope(
                        Kind::Message,
                        "alice",
                        Some("bob"),
                        json!({"w": w, "i": i, "pad": big}),
                    );
                    super::append(&base, "bob", &e).unwrap();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let got = store.check_inbox("bob").unwrap();
        assert_eq!(got.len(), 200); // every line parsed back -> no interleaving
    }

    #[test]
    fn await_message_returns_messages_when_they_arrive() {
        let (tmp, store) = test_store();
        let base = tmp.path().to_path_buf();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(120));
            super::append(&base, "bob", &env(7)).unwrap();
        });
        let got = store.await_message("bob", Duration::from_secs(2)).unwrap();
        writer.join().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].payload, json!({"n": 7}));
    }

    #[test]
    fn await_message_times_out_empty() {
        let (_tmp, store) = test_store();
        let got = store.await_message("bob", Duration::from_millis(120)).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn inject_inbox_script_consumes_store_written_spool() {
        // fr:09 contract: the existing rename-consume hook script works
        // verbatim against sender-written inbox files (spec section 11).
        let (tmp, _store) = test_store();
        super::append(tmp.path(), "bob", &env(42)).unwrap();
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/inject-inbox.sh");
        let out = std::process::Command::new("bash")
            .arg(script)
            .env("AGENTBUS_INSTANCE", "bob")
            .env("AGENTBUS_INBOX_DIR", crate::store::paths::inbox_dir(tmp.path()))
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("agentbus inbox:"), "stdout: {stdout}");
        assert!(stdout.contains("42"), "stdout: {stdout}");
        assert!(!crate::store::paths::inbox_dir(tmp.path()).join("bob.jsonl").exists());
    }
}
```

Add to `store/mod.rs`: `mod spool;` — and make `paths` visible to tests: change `mod paths;` to `pub(crate) mod paths;` and keep the `pub use paths::base_dir;`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p agentbus-core spool::`
Expected: compile errors (append, check_inbox, await_message missing).

- [ ] **Step 3: Implement**

Prepend to `crates/agentbus-core/src/store/spool.rs`:

```rust
//! Per-instance inbox spool (fr:09): append-only JSONL written by senders,
//! consumed by rename-snapshot. Writers append one complete line under an
//! exclusive flock; consumers rename first, then take the same flock once
//! as a barrier against an in-flight append (spec 5.2 plus open question 1).

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::envelope::Envelope;

use super::paths::inbox_dir;
use super::{Store, StoreError};

fn inbox_path(base: &Path, id: &str) -> PathBuf {
    inbox_dir(base).join(format!("{id}.jsonl"))
}

fn flock_exclusive(f: &File) -> std::io::Result<()> {
    let rc = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) };
    if rc == 0 {
        return Ok(());
    }
    Err(std::io::Error::last_os_error())
}

/// Sender-side append (spec 6.2). The lock releases when `f` drops.
pub(crate) fn append(base: &Path, id: &str, env: &Envelope) -> Result<(), StoreError> {
    // Ids are validated at registration; never let one escape the inbox dir.
    if id.contains('/') || id.contains("..") {
        return Err(StoreError::InvalidInstanceId);
    }
    let mut line = serde_json::to_vec(env).expect("envelope serializes");
    line.push(b'\n');
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(inbox_path(base, id))?;
    flock_exclusive(&f)?;
    f.write_all(&line)?;
    Ok(())
}

impl Store {
    /// Rename-snapshot consume (fr:09): return and remove everything queued.
    pub fn check_inbox(&self, id: &str) -> Result<Vec<Envelope>, StoreError> {
        let src = inbox_path(&self.base, id);
        let snap = inbox_dir(&self.base).join(format!("{id}.processing.{}", std::process::id()));
        match std::fs::rename(&src, &snap) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        }
        let f = File::open(&snap)?;
        flock_exclusive(&f)?; // barrier: any in-flight append completes first
        let mut out = Vec::new();
        for line in BufReader::new(&f).lines() {
            let line = line?;
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str(&line) {
                Ok(env) => out.push(env),
                Err(e) => tracing::warn!(error = %e, "skipping corrupt inbox line"),
            }
        }
        drop(f);
        std::fs::remove_file(&snap)?;
        Ok(out)
    }

    /// Poll own inbox for nonzero size, then consume (spec 6.6).
    /// Returns an empty vec when nothing arrives within `timeout`.
    pub fn await_message(&self, id: &str, timeout: Duration) -> Result<Vec<Envelope>, StoreError> {
        let src = inbox_path(&self.base, id);
        let deadline = Instant::now() + timeout;
        let mut delay = Duration::from_millis(50);
        loop {
            let len = std::fs::metadata(&src).map(|m| m.len()).unwrap_or(0);
            if 0 < len {
                return self.check_inbox(id);
            }
            let now = Instant::now();
            if deadline <= now {
                return Ok(Vec::new());
            }
            std::thread::sleep(delay.min(deadline - now));
            delay = (delay * 2).min(Duration::from_millis(250));
        }
    }
}
```

- [ ] **Step 4: Update the hook script default for the v0.2 layout**

In `scripts/inject-inbox.sh` change line 8 from:

```bash
INBOX_DIR=${AGENTBUS_INBOX_DIR:-${XDG_RUNTIME_DIR:-/tmp}/agentbus/inbox}
```

to:

```bash
INBOX_DIR=${AGENTBUS_INBOX_DIR:-$HOME/.agentbus/inbox}
```

- [ ] **Step 5: Verify**

Run: `cargo test -p agentbus-core spool:: && cargo clippy -p agentbus-core --all-targets -- -D warnings`
Expected: 6 tests PASS (the script test requires `bash` and `python3`, both present on dev machines and CI's ubuntu image).

- [ ] **Step 6: Commit**

```bash
git add crates/agentbus-core scripts/inject-inbox.sh
git commit -m "feat: add inbox spool append and consume"
```

---

### Task 4: Event log operations (spec 6.4)

**Files:**
- Create: `crates/agentbus-core/src/store/events.rs`
- Modify: `crates/agentbus-core/src/store/mod.rs` (add `mod events;` and `pub use events::{EventFilter, EventsPage, SeqEnvelope};`)

- [ ] **Step 1: Write the failing tests**

Create `crates/agentbus-core/src/store/events.rs` with the test module:

```rust
#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::EventFilter;
    use crate::envelope::Kind;
    use crate::store::testutil::test_store;

    #[test]
    fn publish_assigns_monotonic_seq() {
        let (_tmp, mut store) = test_store();
        store.publish_event("alice", json!({"a": 1})).unwrap();
        store.publish_event("alice", json!({"a": 2})).unwrap();
        let page = store.events_since(0, 100, &EventFilter::default()).unwrap();
        assert_eq!(page.events.len(), 2);
        assert!(page.events[0].seq < page.events[1].seq);
        assert_eq!(page.cursor, page.events[1].seq);
    }

    #[test]
    fn cursor_resumes_without_gap_or_duplicate() {
        let (_tmp, mut store) = test_store();
        for i in 0..5 {
            store.publish_event("alice", json!({ "i": i })).unwrap();
        }
        let first = store.events_since(0, 2, &EventFilter::default()).unwrap();
        let rest = store
            .events_since(first.cursor, 100, &EventFilter::default())
            .unwrap();
        assert_eq!(first.events.len() + rest.events.len(), 5);
        let seqs: Vec<i64> = first
            .events
            .iter()
            .chain(rest.events.iter())
            .map(|e| e.seq)
            .collect();
        let mut deduped = seqs.clone();
        deduped.dedup();
        assert_eq!(seqs, deduped);
    }

    #[test]
    fn filters_match_kind_and_instance() {
        let (_tmp, mut store) = test_store();
        store.publish_event("alice", json!({"x": 1})).unwrap();
        store.publish_event("bob", json!({"x": 2})).unwrap();
        let alice_only = EventFilter { instance: Some("alice".into()), ..Default::default() };
        let page = store.events_since(0, 100, &alice_only).unwrap();
        assert_eq!(page.events.len(), 1);
        assert_eq!(page.events[0].envelope.from, "alice");
        // The cursor still advances past filtered-out rows.
        assert_eq!(page.cursor, store.max_seq().unwrap());
        let events_only = EventFilter { kind: Some(Kind::Event), ..Default::default() };
        assert_eq!(store.events_since(0, 100, &events_only).unwrap().events.len(), 2);
    }

    #[test]
    fn to_filter_selects_addressed_envelopes() {
        let (_tmp, mut store) = test_store();
        store.publish_event("alice", json!({"x": 1})).unwrap();
        let to_bob = EventFilter { to: Some("bob".into()), ..Default::default() };
        assert!(store.events_since(0, 100, &to_bob).unwrap().events.is_empty());
    }

    #[test]
    fn max_seq_is_zero_on_empty_log() {
        let (_tmp, store) = test_store();
        assert_eq!(store.max_seq().unwrap(), 0);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p agentbus-core events::`
Expected: compile errors.

- [ ] **Step 3: Implement**

Prepend to `crates/agentbus-core/src/store/events.rs`:

```rust
//! Ordered event log (spec 6.4): one table, transactional seq, cursor reads.
//! The v0.1 snapshot-replay-then-live guarantee is trivial here: no gap and
//! no duplicate by construction.

use rusqlite::params;
use serde::Serialize;
use serde_json::Value;

use crate::envelope::{Envelope, Kind};

use super::{append_event, new_envelope, Store, StoreError};

#[derive(Debug, Serialize)]
pub struct SeqEnvelope {
    pub seq: i64,
    pub envelope: Envelope,
}

/// One page of event_log rows. `cursor` advances past every SCANNED row,
/// including filtered-out ones, so follow loops never rescan.
#[derive(Debug)]
pub struct EventsPage {
    pub events: Vec<SeqEnvelope>,
    pub cursor: i64,
}

#[derive(Debug, Default, Clone)]
pub struct EventFilter {
    /// Envelopes whose `from` or `to` equals this id.
    pub instance: Option<String>,
    pub kind: Option<Kind>,
    /// Only envelopes addressed TO this id (watch mode, spec 6.7).
    pub to: Option<String>,
}

impl Store {
    pub fn publish_event(&mut self, from: &str, payload: Value) -> Result<String, StoreError> {
        let env = new_envelope(Kind::Event, from, None, payload);
        env.validate()?;
        self.with_tx(|tx| append_event(tx, &env))?;
        Ok(env.id)
    }

    /// Rows after `after_seq`, oldest first; filters apply post-deserialize.
    pub fn events_since(
        &self,
        after_seq: i64,
        limit: usize,
        filter: &EventFilter,
    ) -> Result<EventsPage, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT seq, envelope FROM event_log WHERE ?1 < seq ORDER BY seq LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![after_seq, limit as i64], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut page = EventsPage { events: Vec::new(), cursor: after_seq };
        for row in rows {
            let (seq, text) = row?;
            page.cursor = seq;
            let Ok(envelope) = serde_json::from_str::<Envelope>(&text) else {
                tracing::warn!(seq, "skipping corrupt event_log row");
                continue;
            };
            if matches(filter, &envelope) {
                page.events.push(SeqEnvelope { seq, envelope });
            }
        }
        Ok(page)
    }

    pub fn max_seq(&self) -> Result<i64, StoreError> {
        Ok(self
            .conn
            .query_row("SELECT COALESCE(MAX(seq), 0) FROM event_log", [], |r| r.get(0))?)
    }
}

fn matches(filter: &EventFilter, env: &Envelope) -> bool {
    if let Some(kind) = &filter.kind {
        if env.kind != *kind {
            return false;
        }
    }
    if let Some(to) = &filter.to {
        if env.to.as_deref() != Some(to.as_str()) {
            return false;
        }
    }
    if let Some(id) = &filter.instance {
        let hit = env.from == *id || env.to.as_deref() == Some(id.as_str());
        if !hit {
            return false;
        }
    }
    true
}
```

- [ ] **Step 4: Verify**

Run: `cargo test -p agentbus-core events:: && cargo clippy -p agentbus-core --all-targets -- -D warnings`
Expected: 5 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/agentbus-core
git commit -m "feat: add event log table operations"
```

---

### Task 5: send (spec 6.2)

**Files:**
- Create: `crates/agentbus-core/src/store/messages.rs`
- Modify: `crates/agentbus-core/src/store/mod.rs` (add `mod messages;` and `pub use messages::Delivered;`)

- [ ] **Step 1: Write the failing tests**

Create `crates/agentbus-core/src/store/messages.rs` with the test module:

```rust
#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::envelope::Kind;
    use crate::store::testutil::test_store;
    use crate::store::{EventFilter, RegisterOpts, StoreError};

    #[test]
    fn send_to_unknown_instance_fails_and_logs_nothing() {
        let (_tmp, mut store) = test_store();
        let err = store.send("alice", "ghost", json!({})).unwrap_err();
        assert!(matches!(err, StoreError::UnknownInstance(_)));
        assert_eq!(store.max_seq().unwrap(), 0);
        assert!(store.check_inbox("ghost").unwrap().is_empty());
    }

    #[test]
    fn send_logs_event_and_spools_inbox() {
        let (_tmp, mut store) = test_store();
        store.register("bob", &RegisterOpts::default()).unwrap();
        let delivered = store.send("alice", "bob", json!({"hi": 1})).unwrap();
        assert!(delivered.envelope.id.starts_with("msg_"));
        assert_eq!(delivered.envelope.kind, Kind::Message);
        assert!(delivered.hook.is_none()); // no on_delivery registered
        // event_log holds the same envelope...
        let page = store.events_since(0, 10, &EventFilter::default()).unwrap();
        assert_eq!(page.events.len(), 1);
        assert_eq!(page.events[0].envelope.id, delivered.envelope.id);
        // ...and the inbox spool delivers it.
        let inbox = store.check_inbox("bob").unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].payload, json!({"hi": 1}));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p agentbus-core messages::`
Expected: compile errors (send, Delivered missing).

- [ ] **Step 3: Implement**

Prepend to `crates/agentbus-core/src/store/messages.rs`:

```rust
//! Sender-performed delivery (spec 6.2-6.3): in one transaction the
//! recipient is checked and the envelope logged; the inbox append and the
//! on_delivery hook happen after commit, outside the transaction.

use serde::Serialize;
use serde_json::Value;

use crate::envelope::Kind;

use super::instances::on_delivery_of;
use super::{append_event, new_envelope, spool, Store, StoreError};

#[derive(Debug, Serialize)]
pub struct Delivered {
    pub envelope: crate::envelope::Envelope,
    /// Set when the recipient registered an on_delivery hook (Task 6 wires
    /// the actual execution; until then this stays None).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hook: Option<super::hook_outcome_placeholder::HookOutcome>,
}

impl Store {
    pub fn send(&mut self, from: &str, to: &str, payload: Value) -> Result<Delivered, StoreError> {
        let env = new_envelope(Kind::Message, from, Some(to), payload);
        env.validate()?;
        let _on_delivery = self.with_tx(|tx| {
            let cmd = on_delivery_of(tx, to)?;
            append_event(tx, &env)?;
            Ok(cmd)
        })?;
        spool::append(&self.base, to, &env)?;
        Ok(Delivered { envelope: env, hook: None })
    }
}
```

And add the temporary placeholder module to `store/mod.rs` (Task 6 deletes it):

```rust
/// Removed in Task 6 when store/hook.rs lands.
pub(crate) mod hook_outcome_placeholder {
    #[derive(Debug, Clone, serde::Serialize)]
    pub struct HookOutcome {
        pub ok: bool,
        pub detail: String,
    }
}
```

- [ ] **Step 4: Verify**

Run: `cargo test -p agentbus-core messages:: && cargo clippy -p agentbus-core --all-targets -- -D warnings`
Expected: 2 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/agentbus-core
git commit -m "feat: add sender-side message delivery"
```

---

### Task 6: on_delivery hook execution (spec 6.5)

**Files:**
- Create: `crates/agentbus-core/src/store/hook.rs`
- Modify: `crates/agentbus-core/src/store/mod.rs` (add `mod hook;`, `pub use hook::HookOutcome;`, delete `hook_outcome_placeholder`)
- Modify: `crates/agentbus-core/src/store/messages.rs` (wire hook into send)

- [ ] **Step 1: Write the failing tests**

Create `crates/agentbus-core/src/store/hook.rs` with the test module:

```rust
#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::store::testutil::test_store;
    use crate::store::{EventFilter, RegisterOpts};

    fn register_with_hook(store: &mut crate::store::Store, cmd: &str) {
        let opts = RegisterOpts {
            persistent: true,
            on_delivery: Some(cmd.to_string()),
            ..Default::default()
        };
        store.register("bob", &opts).unwrap();
    }

    #[test]
    fn hook_runs_with_envelope_env_vars() {
        let (tmp, mut store) = test_store();
        let marker = tmp.path().join("hook.out");
        let cmd = format!(
            "echo \"$AGENTBUS_INSTANCE $AGENTBUS_KIND $AGENTBUS_FROM $AGENTBUS_ENVELOPE_ID\" > {}",
            marker.display()
        );
        register_with_hook(&mut store, &cmd);
        let delivered = store.send("alice", "bob", json!({})).unwrap();
        let hook = delivered.hook.expect("hook ran");
        assert!(hook.ok, "{}", hook.detail);
        let written = std::fs::read_to_string(&marker).unwrap();
        assert_eq!(
            written.trim(),
            format!("bob message alice {}", delivered.envelope.id)
        );
    }

    #[test]
    fn hook_failure_is_nonfatal_and_emits_event() {
        let (_tmp, mut store) = test_store();
        register_with_hook(&mut store, "exit 3");
        let delivered = store.send("alice", "bob", json!({})).unwrap();
        let hook = delivered.hook.expect("hook ran");
        assert!(!hook.ok);
        // The send still succeeded: envelope is spooled.
        assert_eq!(store.check_inbox("bob").unwrap().len(), 1);
        // And a bus.delivery_hook_failed event was logged.
        let page = store.events_since(0, 100, &EventFilter::default()).unwrap();
        let failed = page.events.iter().any(|e| {
            e.envelope.from == "bus"
                && e.envelope.payload["event"] == "bus.delivery_hook_failed"
        });
        assert!(failed);
    }
}
```

A 15 s timeout test would slow the suite; the timeout path is covered by code review plus the `wait_with_timeout` unit test below (uses a 15 s constant injected as a parameter).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p agentbus-core hook::`
Expected: compile errors.

- [ ] **Step 3: Implement**

Prepend to `crates/agentbus-core/src/store/hook.rs`:

```rust
//! Sender-executed on_delivery hooks (spec 6.5): sh -c, 15 s timeout,
//! non-fatal failure. This is how integrators wake agents without a daemon.

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::json;

use crate::envelope::Envelope;

use super::Store;

const HOOK_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Serialize)]
pub struct HookOutcome {
    pub ok: bool,
    pub detail: String,
}

impl Store {
    /// Best-effort: the send/ask already succeeded (the envelope is durably
    /// spooled); a failure only produces a bus.delivery_hook_failed event.
    pub(crate) fn run_delivery_hook(
        &mut self,
        cmd: &str,
        instance: &str,
        env: &Envelope,
    ) -> HookOutcome {
        let envs = [
            ("AGENTBUS_INSTANCE", instance.to_string()),
            ("AGENTBUS_ENVELOPE_ID", env.id.clone()),
            ("AGENTBUS_KIND", env.kind.as_str().to_string()),
            ("AGENTBUS_FROM", env.from.clone()),
        ];
        let outcome = run(cmd, &envs, HOOK_TIMEOUT);
        self.record_failure(instance, Some(&env.id), &outcome);
        outcome
    }

    /// Sweep re-fire (spec 6.8): no specific envelope, instance only.
    pub(crate) fn run_sweep_hook(&mut self, cmd: &str, instance: &str) -> HookOutcome {
        let envs = [("AGENTBUS_INSTANCE", instance.to_string())];
        let outcome = run(cmd, &envs, HOOK_TIMEOUT);
        self.record_failure(instance, None, &outcome);
        outcome
    }

    fn record_failure(&mut self, instance: &str, envelope_id: Option<&str>, outcome: &HookOutcome) {
        if outcome.ok {
            return;
        }
        let payload = json!({
            "event": "bus.delivery_hook_failed",
            "instance": instance,
            "envelope_id": envelope_id,
            "detail": outcome.detail,
        });
        let _ = self.publish_event("bus", payload);
        tracing::warn!(instance, detail = %outcome.detail, "on_delivery hook failed");
    }
}

fn run(cmd: &str, envs: &[(&str, String)], timeout: Duration) -> HookOutcome {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (k, v) in envs {
        command.env(k, v);
    }
    match command.spawn() {
        Ok(mut child) => wait_with_timeout(&mut child, timeout),
        Err(e) => HookOutcome { ok: false, detail: format!("spawn: {e}") },
    }
}

fn wait_with_timeout(child: &mut Child, timeout: Duration) -> HookOutcome {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                return HookOutcome { ok: true, detail: "ok".into() };
            }
            Ok(Some(status)) => {
                return HookOutcome { ok: false, detail: format!("exit: {status}") };
            }
            Ok(None) => {}
            Err(e) => return HookOutcome { ok: false, detail: format!("wait: {e}") },
        }
        if deadline <= Instant::now() {
            let _ = child.kill();
            let _ = child.wait();
            return HookOutcome { ok: false, detail: "timeout".into() };
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
```

Add to the bottom of the `tests` module in `hook.rs`:

```rust
#[test]
fn wait_with_timeout_kills_overrunning_hook() {
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let outcome = super::wait_with_timeout(&mut child, std::time::Duration::from_millis(200));
    assert!(!outcome.ok);
    assert_eq!(outcome.detail, "timeout");
}
```

- [ ] **Step 4: Wire the hook into send**

In `crates/agentbus-core/src/store/messages.rs`, change `Delivered.hook`'s type to the real `super::hook::HookOutcome` (drop the placeholder import) and replace the send tail:

```rust
        let on_delivery = self.with_tx(|tx| {
            let cmd = on_delivery_of(tx, to)?;
            append_event(tx, &env)?;
            Ok(cmd)
        })?;
        spool::append(&self.base, to, &env)?;
        let hook = on_delivery.map(|cmd| self.run_delivery_hook(&cmd, to, &env));
        Ok(Delivered { envelope: env, hook })
```

Delete `hook_outcome_placeholder` from `store/mod.rs`; add `mod hook;` and `pub use hook::HookOutcome;`.

- [ ] **Step 5: Verify**

Run: `cargo test -p agentbus-core store:: && cargo clippy -p agentbus-core --all-targets -- -D warnings`
Expected: all store tests PASS, including the two new hook tests.

- [ ] **Step 6: Commit**

```bash
git add crates/agentbus-core
git commit -m "feat: add on_delivery hook execution"
```

---

### Task 7: ask / reply / ask-result (spec 6.3)

**Files:**
- Modify: `crates/agentbus-core/src/store/messages.rs`
- Modify: `crates/agentbus-core/src/store/mod.rs` (extend the messages re-export to `pub use messages::{AskReply, AskStatus, Delivered};`)

- [ ] **Step 1: Write the failing tests**

Append to the `tests` module in `messages.rs`:

```rust
use std::time::Duration;

#[test]
fn ask_reply_roundtrip_across_threads() {
    let (tmp, mut store) = test_store();
    store.register("bob", &RegisterOpts::default()).unwrap();
    let base = tmp.path().to_path_buf();
    let responder = std::thread::spawn(move || {
        let mut bob = crate::store::Store::open_at(&base).unwrap();
        // Wait for the ask to land in bob's inbox, then reply.
        let asks = bob.await_message("bob", Duration::from_secs(2)).unwrap();
        assert_eq!(asks.len(), 1);
        assert_eq!(asks[0].kind, Kind::Ask);
        bob.reply("bob", &asks[0].id, json!({"pong": true})).unwrap();
    });
    let reply = store
        .ask("alice", "bob", json!({"ping": true}), Duration::from_secs(2))
        .unwrap();
    responder.join().unwrap();
    assert_eq!(reply.payload, json!({"pong": true}));
}

#[test]
fn ask_times_out_but_late_reply_is_retrievable() {
    let (_tmp, mut store) = test_store();
    store.register("bob", &RegisterOpts::default()).unwrap();
    let err = store
        .ask("alice", "bob", json!({"q": 1}), Duration::from_millis(120))
        .unwrap_err();
    let StoreError::Timeout(request_id) = err else { panic!("want timeout") };
    // Late reply still lands in the row (improvement over v0.1).
    store.reply("bob", &request_id, json!({"late": true})).unwrap();
    let status = store.ask_result(&request_id).unwrap();
    let crate::store::AskStatus::Replied { payload, .. } = status else {
        panic!("want replied")
    };
    assert_eq!(payload, json!({"late": true}));
}

#[test]
fn first_reply_wins_but_both_are_logged() {
    let (_tmp, mut store) = test_store();
    store.register("bob", &RegisterOpts::default()).unwrap();
    let err = store
        .ask("alice", "bob", json!({}), Duration::from_millis(60))
        .unwrap_err();
    let StoreError::Timeout(rid) = err else { panic!() };
    store.reply("bob", &rid, json!({"n": 1})).unwrap();
    store.reply("bob", &rid, json!({"n": 2})).unwrap();
    let crate::store::AskStatus::Replied { payload, .. } = store.ask_result(&rid).unwrap()
    else {
        panic!()
    };
    assert_eq!(payload, json!({"n": 1})); // first write won
    let page = store.events_since(0, 100, &EventFilter::default()).unwrap();
    let replies = page
        .events
        .iter()
        .filter(|e| e.envelope.kind == Kind::Reply)
        .count();
    assert_eq!(replies, 2); // both recorded in event_log
}

#[test]
fn reply_to_unknown_request_id_fails() {
    let (_tmp, mut store) = test_store();
    assert!(matches!(
        store.reply("bob", "msg_nope", json!({})),
        Err(StoreError::UnknownRequestId(_))
    ));
    assert!(matches!(
        store.ask_result("msg_nope"),
        Err(StoreError::UnknownRequestId(_))
    ));
}

#[test]
fn ask_result_reports_pending_then_expired() {
    let (_tmp, mut store) = test_store();
    store.register("bob", &RegisterOpts::default()).unwrap();
    let err = store
        .ask("alice", "bob", json!({}), Duration::from_millis(60))
        .unwrap_err();
    let StoreError::Timeout(rid) = err else { panic!() };
    // After expiry with no reply, status is Expired.
    assert!(matches!(
        store.ask_result(&rid).unwrap(),
        crate::store::AskStatus::Expired { .. }
    ));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p agentbus-core messages::`
Expected: compile errors (ask, reply, ask_result, AskReply, AskStatus missing).

- [ ] **Step 3: Implement**

Add to `messages.rs` (below `Delivered`):

```rust
#[derive(Debug, Serialize)]
pub struct AskReply {
    pub request_id: String,
    pub payload: Value,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AskStatus {
    Pending { expires_at: String },
    Replied { payload: Value, replied_at: String },
    Expired { expires_at: String },
}
```

Add to the `impl Store` block in `messages.rs`:

```rust
    /// Spec 6.3: as send, plus an asks row; then poll for the reply with
    /// 50 ms -> 250 ms backoff. On expiry the row stays for ask_result.
    pub fn ask(
        &mut self,
        from: &str,
        to: &str,
        payload: Value,
        timeout: Duration,
    ) -> Result<AskReply, StoreError> {
        let mut env = new_envelope(Kind::Ask, from, Some(to), payload);
        env.timeout_ms = Some(timeout.as_millis() as u64);
        env.validate()?;
        let expires_at = super::rfc3339(&(env.ts + timeout));
        let on_delivery = self.with_tx(|tx| {
            let cmd = on_delivery_of(tx, to)?;
            append_event(tx, &env)?;
            tx.execute(
                "INSERT INTO asks (request_id, from_id, to_id, expires_at)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![env.id, from, to, expires_at],
            )?;
            Ok(cmd)
        })?;
        spool::append(&self.base, to, &env)?;
        if let Some(cmd) = on_delivery {
            self.run_delivery_hook(&cmd, to, &env);
        }
        self.poll_reply(&env.id, timeout)
    }

    fn poll_reply(&self, request_id: &str, timeout: Duration) -> Result<AskReply, StoreError> {
        let deadline = Instant::now() + timeout;
        let mut delay = Duration::from_millis(50);
        loop {
            if let Some(payload) = self.reply_payload(request_id)? {
                return Ok(AskReply { request_id: request_id.to_string(), payload });
            }
            let now = Instant::now();
            if deadline <= now {
                return Err(StoreError::Timeout(request_id.to_string()));
            }
            std::thread::sleep(delay.min(deadline - now));
            delay = (delay * 2).min(Duration::from_millis(250));
        }
    }

    fn reply_payload(&self, request_id: &str) -> Result<Option<Value>, StoreError> {
        let text: Option<Option<String>> = self
            .conn
            .query_row(
                "SELECT reply_payload FROM asks WHERE request_id = ?1",
                rusqlite::params![request_id],
                |r| r.get(0),
            )
            .optional()?;
        match text.flatten() {
            Some(t) => Ok(Some(serde_json::from_str(&t).unwrap_or(Value::String(t)))),
            None => Ok(None),
        }
    }

    /// Spec 6.3: first reply wins; every reply is logged. No inbox write --
    /// the asker reads the row.
    pub fn reply(&mut self, from: &str, request_id: &str, payload: Value) -> Result<(), StoreError> {
        self.with_tx(|tx| {
            let asker: Option<String> = tx
                .query_row(
                    "SELECT from_id FROM asks WHERE request_id = ?1",
                    rusqlite::params![request_id],
                    |r| r.get(0),
                )
                .optional()?;
            let asker =
                asker.ok_or_else(|| StoreError::UnknownRequestId(request_id.to_string()))?;
            let mut env = new_envelope(Kind::Reply, from, Some(&asker), payload.clone());
            env.request_id = Some(request_id.to_string());
            env.validate()?;
            tx.execute(
                "UPDATE asks SET reply_payload = ?1, replied_at = ?2
                 WHERE request_id = ?3 AND reply_payload IS NULL",
                rusqlite::params![
                    serde_json::to_string(&payload).expect("payload serializes"),
                    super::rfc3339(&env.ts),
                    request_id
                ],
            )?;
            append_event(tx, &env)?;
            Ok(())
        })
    }

    pub fn ask_result(&self, request_id: &str) -> Result<AskStatus, StoreError> {
        let row: Option<(String, Option<String>, Option<String>)> = self
            .conn
            .query_row(
                "SELECT expires_at, reply_payload, replied_at FROM asks WHERE request_id = ?1",
                rusqlite::params![request_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let (expires_at, reply, replied_at) =
            row.ok_or_else(|| StoreError::UnknownRequestId(request_id.to_string()))?;
        if let (Some(text), Some(replied_at)) = (reply, replied_at) {
            let payload = serde_json::from_str(&text).unwrap_or(Value::String(text));
            return Ok(AskStatus::Replied { payload, replied_at });
        }
        let expired = crate::ids::parse_rfc3339(&expires_at)
            .map(|t| t <= crate::ids::now_utc())
            .unwrap_or(false);
        if expired {
            return Ok(AskStatus::Expired { expires_at });
        }
        Ok(AskStatus::Pending { expires_at })
    }
```

Update the `use` lines at the top of `messages.rs` to:

```rust
use std::time::{Duration, Instant};

use rusqlite::OptionalExtension;
use serde::Serialize;
use serde_json::Value;

use crate::envelope::Kind;

use super::instances::on_delivery_of;
use super::{append_event, new_envelope, spool, Store, StoreError};
```

- [ ] **Step 4: Verify**

Run: `cargo test -p agentbus-core messages:: && cargo clippy -p agentbus-core --all-targets -- -D warnings`
Expected: 7 tests PASS (2 from Task 5, 5 new).

- [ ] **Step 5: Commit**

```bash
git add crates/agentbus-core
git commit -m "feat: add ask/reply with polling and ask-result"
```

---

### Task 8: sweep (spec 6.8)

**Files:**
- Create: `crates/agentbus-core/src/store/sweep.rs`
- Modify: `crates/agentbus-core/src/store/mod.rs` (add `mod sweep;` and `pub use sweep::{SweepOpts, SweepReport};`)

- [ ] **Step 1: Write the failing tests**

Create `crates/agentbus-core/src/store/sweep.rs` with the test module:

```rust
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::SweepOpts;
    use crate::store::testutil::test_store;
    use crate::store::{EventFilter, RegisterOpts};

    fn zero_grace(purge: bool) -> SweepOpts {
        SweepOpts { grace: Duration::ZERO, purge_orphans: purge }
    }

    #[test]
    fn removes_dead_nonpersistent_rows_only() {
        let (_tmp, mut store) = test_store();
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let dead_pid = child.id() as i32;
        child.wait().unwrap();
        store
            .register("dead", &RegisterOpts { pid: Some(dead_pid), ..Default::default() })
            .unwrap();
        store.register("live", &RegisterOpts::default()).unwrap();
        store
            .register("bot", &RegisterOpts { persistent: true, ..Default::default() })
            .unwrap();
        let report = store.sweep(&zero_grace(false)).unwrap();
        assert_eq!(report.dead_instances, vec!["dead".to_string()]);
        let ids: Vec<String> = store.list_instances().unwrap().into_iter().map(|r| r.id).collect();
        assert_eq!(ids, vec!["bot".to_string(), "live".to_string()]);
    }

    #[test]
    fn reports_expired_asks_exactly_once() {
        let (_tmp, mut store) = test_store();
        store.register("bob", &RegisterOpts::default()).unwrap();
        let err = store
            .ask("alice", "bob", json!({}), Duration::from_millis(60))
            .unwrap_err();
        let crate::store::StoreError::Timeout(rid) = err else { panic!() };
        let first = store.sweep(&zero_grace(false)).unwrap();
        assert_eq!(first.expired_asks, vec![rid.clone()]);
        let second = store.sweep(&zero_grace(false)).unwrap();
        assert!(second.expired_asks.is_empty());
        let page = store.events_since(0, 100, &EventFilter::default()).unwrap();
        let expired_events = page
            .events
            .iter()
            .filter(|e| e.envelope.payload["event"] == "bus.ask_expired")
            .count();
        assert_eq!(expired_events, 1);
    }

    #[test]
    fn refires_hook_for_stale_nonempty_inbox() {
        let (tmp, mut store) = test_store();
        let marker = tmp.path().join("refire.out");
        let opts = RegisterOpts {
            persistent: true,
            on_delivery: Some(format!("echo refired > {}", marker.display())),
            ..Default::default()
        };
        store.register("bot", &opts).unwrap();
        // Spool a message but simulate "sender crashed between append and
        // hook" by writing the spool directly (no hook fired).
        let env = crate::store::new_envelope(
            crate::envelope::Kind::Message,
            "alice",
            Some("bot"),
            json!({}),
        );
        crate::store::spool::append(tmp.path(), "bot", &env).unwrap();
        let report = store.sweep(&zero_grace(false)).unwrap();
        assert_eq!(report.rehooked, vec!["bot".to_string()]);
        assert!(marker.exists());
    }

    #[test]
    fn purge_orphans_removes_unregistered_inboxes() {
        let (tmp, mut store) = test_store();
        store.register("bob", &RegisterOpts::default()).unwrap();
        store.send("alice", "bob", json!({})).unwrap();
        store.unregister("bob").unwrap(); // inbox file survives unregister
        let inbox = crate::store::paths::inbox_dir(tmp.path()).join("bob.jsonl");
        assert!(inbox.exists());
        let without_purge = store.sweep(&zero_grace(false)).unwrap();
        assert!(without_purge.purged_inboxes.is_empty());
        assert!(inbox.exists());
        let with_purge = store.sweep(&zero_grace(true)).unwrap();
        assert_eq!(with_purge.purged_inboxes, vec!["bob".to_string()]);
        assert!(!inbox.exists());
    }
}
```

Note: `refires_hook_for_stale_nonempty_inbox` calls `crate::store::spool::append`; change `mod spool;` to `pub(crate) mod spool;` in `store/mod.rs`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p agentbus-core sweep::`
Expected: compile errors.

- [ ] **Step 3: Implement**

Prepend to `crates/agentbus-core/src/store/sweep.rs`:

```rust
//! Crash recovery (spec 6.8): a periodic CLI, not a resident. Without it the
//! same recovery happens lazily at the next send.

use std::collections::HashSet;
use std::time::Duration;

use rusqlite::params;
use serde::Serialize;
use serde_json::json;

use super::liveness::pid_alive;
use super::paths::inbox_dir;
use super::{Store, StoreError};

#[derive(Debug, Clone)]
pub struct SweepOpts {
    /// An inbox file must be non-empty and unmodified for this long before
    /// its on_delivery is re-fired.
    pub grace: Duration,
    pub purge_orphans: bool,
}

impl Default for SweepOpts {
    fn default() -> Self {
        SweepOpts { grace: Duration::from_secs(60), purge_orphans: false }
    }
}

#[derive(Debug, Default, Serialize)]
pub struct SweepReport {
    pub dead_instances: Vec<String>,
    pub rehooked: Vec<String>,
    pub expired_asks: Vec<String>,
    pub purged_inboxes: Vec<String>,
}

impl Store {
    pub fn sweep(&mut self, opts: &SweepOpts) -> Result<SweepReport, StoreError> {
        let mut report = SweepReport::default();
        self.sweep_dead_instances(&mut report)?;
        self.sweep_expired_asks(&mut report)?;
        self.sweep_stale_inboxes(opts.grace, &mut report)?;
        if opts.purge_orphans {
            self.sweep_orphan_inboxes(&mut report)?;
        }
        Ok(report)
    }

    fn sweep_dead_instances(&mut self, report: &mut SweepReport) -> Result<(), StoreError> {
        let dead: Vec<String> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id, pid FROM instances WHERE persistent = 0")?;
            let rows = stmt.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<i32>>(1)?))
            })?;
            let mut dead = Vec::new();
            for row in rows {
                let (id, pid) = row?;
                if !pid.map(pid_alive).unwrap_or(false) {
                    dead.push(id);
                }
            }
            dead
        };
        self.with_tx(|tx| {
            for id in &dead {
                tx.execute(
                    "DELETE FROM instances WHERE id = ?1 AND persistent = 0",
                    params![id],
                )?;
            }
            Ok(())
        })?;
        report.dead_instances = dead;
        Ok(())
    }

    fn sweep_expired_asks(&mut self, report: &mut SweepReport) -> Result<(), StoreError> {
        let now = crate::ids::now_utc();
        let candidates: Vec<(String, String)> = {
            let mut stmt = self.conn.prepare(
                "SELECT request_id, expires_at FROM asks
                 WHERE reply_payload IS NULL AND expired_notified = 0",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<Result<_, _>>()?
        };
        for (request_id, expires_at) in candidates {
            let is_expired = crate::ids::parse_rfc3339(&expires_at)
                .map(|t| t <= now)
                .unwrap_or(false);
            if !is_expired {
                continue;
            }
            self.publish_event(
                "bus",
                json!({"event": "bus.ask_expired", "request_id": request_id}),
            )?;
            self.with_tx(|tx| {
                tx.execute(
                    "UPDATE asks SET expired_notified = 1 WHERE request_id = ?1",
                    params![request_id],
                )?;
                Ok(())
            })?;
            report.expired_asks.push(request_id);
        }
        Ok(())
    }

    fn sweep_stale_inboxes(
        &mut self,
        grace: Duration,
        report: &mut SweepReport,
    ) -> Result<(), StoreError> {
        let hooked: Vec<(String, String)> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, on_delivery FROM instances WHERE on_delivery IS NOT NULL",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<Result<_, _>>()?
        };
        for (id, cmd) in hooked {
            let path = inbox_dir(&self.base).join(format!("{id}.jsonl"));
            let Ok(meta) = std::fs::metadata(&path) else { continue };
            if meta.len() == 0 {
                continue;
            }
            let stale = meta
                .modified()
                .ok()
                .and_then(|m| m.elapsed().ok())
                .map(|age| grace <= age)
                .unwrap_or(false);
            if stale {
                self.run_sweep_hook(&cmd, &id);
                report.rehooked.push(id);
            }
        }
        Ok(())
    }

    fn sweep_orphan_inboxes(&mut self, report: &mut SweepReport) -> Result<(), StoreError> {
        let known: HashSet<String> = self
            .list_instances()?
            .into_iter()
            .map(|r| r.id)
            .collect();
        for entry in std::fs::read_dir(inbox_dir(&self.base))? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(id) = name.strip_suffix(".jsonl") else { continue };
            if !known.contains(id) {
                std::fs::remove_file(entry.path())?;
                report.purged_inboxes.push(id.to_string());
            }
        }
        Ok(())
    }
}
```

- [ ] **Step 4: Verify**

Run: `cargo test -p agentbus-core sweep:: && cargo clippy -p agentbus-core --all-targets -- -D warnings`
Expected: 4 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/agentbus-core
git commit -m "feat: add sweep crash recovery"
```

---

### Task 9: Rewire agentbus-stdio onto the store (spec section 7)

**Files:**
- Modify: `crates/agentbus-stdio/Cargo.toml`
- Modify: `crates/agentbus-stdio/src/main.rs` (full rewrite, sync)
- Modify: `crates/agentbus-stdio/src/tools.rs` (full rewrite)
- Delete: `crates/agentbus-stdio/src/uds_client.rs`
- Delete: `crates/agentbus-stdio/src/lib.rs` (the binary no longer needs a lib target; if `lib.rs` only re-exports modules, fold everything into the binary)
- Delete: `crates/agentbus-stdio/tests/client.rs` (tested the UDS client against agentbusd)
- Create: `crates/agentbus-stdio/tests/rpc.rs`

- [ ] **Step 1: Write the failing subprocess test**

Create `crates/agentbus-stdio/tests/rpc.rs`:

```rust
//! Drives the shim binary over stdin/stdout exactly as an MCP client would.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct Shim {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Shim {
    fn spawn(dir: &std::path::Path) -> Shim {
        let mut child = Command::new(env!("CARGO_BIN_EXE_agentbus-stdio"))
            .env("AGENTBUS_DIR", dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Shim { child, stdin, stdout }
    }

    fn call(&mut self, req: serde_json::Value) -> serde_json::Value {
        let mut line = serde_json::to_string(&req).unwrap();
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).unwrap();
        self.stdin.flush().unwrap();
        let mut resp = String::new();
        self.stdout.read_line(&mut resp).unwrap();
        serde_json::from_str(&resp).unwrap()
    }

    fn tool(&mut self, name: &str, args: serde_json::Value) -> serde_json::Value {
        let resp = self.call(serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": name, "arguments": args}
        }));
        let text = resp["result"]["content"][0]["text"].as_str().unwrap_or_else(|| {
            panic!("tool error: {resp}");
        });
        serde_json::from_str(text).unwrap()
    }
}

impl Drop for Shim {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn initialize_and_tools_list() {
    let tmp = tempfile::tempdir().unwrap();
    let mut shim = Shim::spawn(tmp.path());
    let init = shim.call(serde_json::json!({
        "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {}
    }));
    assert_eq!(init["result"]["serverInfo"]["name"], "agentbus-stdio");
    let list = shim.call(serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}
    }));
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "register", "unregister", "list_instances", "await_message",
            "check_inbox", "send", "ask", "reply", "publish_event"
        ]
    );
}

#[test]
fn register_send_check_inbox_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let mut shim = Shim::spawn(tmp.path());
    let reg = shim.tool("register", serde_json::json!({"instance_id": "bob"}));
    assert_eq!(reg["ok"], true);
    let sent = shim.tool(
        "send",
        serde_json::json!({"from": "alice", "to": "bob", "payload": {"hi": 1}}),
    );
    assert!(sent["id"].as_str().unwrap().starts_with("msg_"));
    let inbox = shim.tool("check_inbox", serde_json::json!({"instance_id": "bob"}));
    assert_eq!(inbox["envelopes"][0]["payload"]["hi"], 1);
}

#[test]
fn send_to_unknown_instance_returns_coded_error() {
    let tmp = tempfile::tempdir().unwrap();
    let mut shim = Shim::spawn(tmp.path());
    let resp = shim.call(serde_json::json!({
        "jsonrpc": "2.0", "id": 9, "method": "tools/call",
        "params": {"name": "send",
                   "arguments": {"from": "a", "to": "ghost", "payload": {}}}
    }));
    assert_eq!(resp["error"]["message"], "unknown_instance");
}
```

Update `crates/agentbus-stdio/Cargo.toml` dependencies to:

```toml
[dependencies]
agentbus-core = { version = "0.1.0", path = "../agentbus-core" }
anyhow = { workspace = true }
serde_json = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

(Remove `[lib]` config if present; remove tokio, futures, thiserror, serde, and the agentbusd dev-dependency.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p agentbus-stdio`
Expected: FAIL (binary still speaks UDS; tools/list shape may pass but roundtrip fails with daemon_unavailable, and the build fails once Cargo.toml drops tokio).

- [ ] **Step 3: Rewrite the shim**

Replace `crates/agentbus-stdio/src/main.rs` entirely:

```rust
//! MCP stdio shim over the spool store (fr:08, v0.2): no daemon, no socket.
//! Single-threaded line loop; tool calls run synchronously against ~/.agentbus.

mod tools;

use std::io::{BufRead, Write};

use serde_json::json;

use agentbus_core::store::Store;
use tracing_subscriber::EnvFilter;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let mut store = Store::open()?;
    let mut session = tools::Session::default();
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let req: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "bad json from MCP client");
                continue;
            }
        };
        let id = req.get("id").cloned().unwrap_or(serde_json::Value::Null);
        let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");
        let params = req.get("params").cloned().unwrap_or(json!({}));
        let resp = handle(&mut store, &mut session, method, params);
        let envelope = match resp {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": e}),
        };
        let mut buf = serde_json::to_vec(&envelope)?;
        buf.push(b'\n');
        stdout.write_all(&buf)?;
        stdout.flush()?;
    }
    // Stdin closed: release this session's non-persistent registrations.
    // Pid liveness covers abrupt kills (spec section 12, item 4).
    session.cleanup(&mut store);
    Ok(())
}

fn handle(
    store: &mut Store,
    session: &mut tools::Session,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, serde_json::Value> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": "2024-11-05",
            "serverInfo": {"name": "agentbus-stdio", "version": env!("CARGO_PKG_VERSION")},
            "capabilities": {"tools": {}}
        })),
        "tools/list" => {
            let tools: Vec<_> = tools::specs()
                .into_iter()
                .map(|s| {
                    json!({
                        "name": s.name,
                        "description": s.description,
                        "inputSchema": s.input_schema
                    })
                })
                .collect();
            Ok(json!({"tools": tools}))
        }
        "tools/call" => {
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            let result = tools::call(store, session, name, args)?;
            Ok(json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string(&result).unwrap()
                }]
            }))
        }
        _ => Err(json!({"code": -32601, "message": "method not found"})),
    }
}
```

Replace `crates/agentbus-stdio/src/tools.rs` entirely:

```rust
//! MCP tool registry: name -> JSON Schema input + dispatch onto the Store.
//! Tool names match v0.1; `register` gains persistent/on_delivery and loses
//! mailbox_size (spool files are unbounded); await/check return envelope
//! BATCHES (fr:08 v0.2 surface).

use std::time::Duration;

use serde_json::{json, Value};

use agentbus_core::store::{RegisterOpts, Store, StoreError};

pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

/// Non-persistent ids registered through this shim process; released on EOF.
#[derive(Default)]
pub struct Session {
    registered: Vec<String>,
}

impl Session {
    pub fn cleanup(&mut self, store: &mut Store) {
        for id in self.registered.drain(..) {
            let _ = store.unregister(&id);
        }
    }
}

pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "register",
            description: "Register this session under an instance_id.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "instance_id": {"type": "string"},
                    "persistent": {"type": "boolean"},
                    "on_delivery": {"type": "string"}
                },
                "required": ["instance_id"]
            }),
        },
        ToolSpec {
            name: "unregister",
            description: "Release a previously registered instance_id.",
            input_schema: json!({
                "type": "object",
                "properties": {"instance_id": {"type": "string"}},
                "required": ["instance_id"]
            }),
        },
        ToolSpec {
            name: "list_instances",
            description: "List registered instances.",
            input_schema: json!({"type": "object"}),
        },
        ToolSpec {
            name: "await_message",
            description: "Block until messages arrive for instance_id, or time out (empty list).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "instance_id": {"type": "string"},
                    "timeout_ms": {"type": "integer", "minimum": 1}
                },
                "required": ["instance_id"]
            }),
        },
        ToolSpec {
            name: "check_inbox",
            description: "Drain pending messages for instance_id without blocking.",
            input_schema: json!({
                "type": "object",
                "properties": {"instance_id": {"type": "string"}},
                "required": ["instance_id"]
            }),
        },
        ToolSpec {
            name: "send",
            description: "Send a one-way message to another instance.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "from": {"type": "string"},
                    "to": {"type": "string"},
                    "payload": {}
                },
                "required": ["from", "to", "payload"]
            }),
        },
        ToolSpec {
            name: "ask",
            description: "Send a request to another instance and block until it replies or times out.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "from": {"type": "string"},
                    "to": {"type": "string"},
                    "payload": {},
                    "timeout_ms": {"type": "integer"}
                },
                "required": ["from", "to", "payload"]
            }),
        },
        ToolSpec {
            name: "reply",
            description: "Reply to an inbound ask; the asker reads it via the asks row.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "from": {"type": "string"},
                    "request_id": {"type": "string"},
                    "payload": {}
                },
                "required": ["from", "request_id", "payload"]
            }),
        },
        ToolSpec {
            name: "publish_event",
            description: "Append a broadcast event to the ordered event log.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "from": {"type": "string"},
                    "payload": {}
                },
                "required": ["from", "payload"]
            }),
        },
    ]
}

pub fn call(
    store: &mut Store,
    session: &mut Session,
    name: &str,
    mut args: Value,
) -> Result<Value, Value> {
    normalize_json_string_field(&mut args, "payload");
    match name {
        "register" => {
            let id = str_arg(&args, "instance_id")?;
            let opts = RegisterOpts {
                persistent: args.get("persistent").and_then(Value::as_bool).unwrap_or(false),
                on_delivery: args
                    .get("on_delivery")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                pid: None,
            };
            store.register(&id, &opts).map_err(store_err)?;
            if !opts.persistent {
                session.registered.push(id);
            }
            Ok(json!({"ok": true}))
        }
        "unregister" => {
            let id = str_arg(&args, "instance_id")?;
            let removed = store.unregister(&id).map_err(store_err)?;
            session.registered.retain(|r| *r != id);
            Ok(json!({"ok": removed}))
        }
        "list_instances" => {
            let rows = store.list_instances().map_err(store_err)?;
            Ok(json!({ "instances": rows }))
        }
        "await_message" => {
            let id = str_arg(&args, "instance_id")?;
            let timeout = timeout_arg(&args, 30_000);
            let envelopes = store.await_message(&id, timeout).map_err(store_err)?;
            Ok(json!({ "envelopes": envelopes }))
        }
        "check_inbox" => {
            let id = str_arg(&args, "instance_id")?;
            let envelopes = store.check_inbox(&id).map_err(store_err)?;
            Ok(json!({ "envelopes": envelopes }))
        }
        "send" => {
            let from = str_arg(&args, "from")?;
            let to = str_arg(&args, "to")?;
            let payload = args.get("payload").cloned().unwrap_or(Value::Null);
            let delivered = store.send(&from, &to, payload).map_err(store_err)?;
            Ok(json!({"id": delivered.envelope.id, "hook": delivered.hook}))
        }
        "ask" => {
            let from = str_arg(&args, "from")?;
            let to = str_arg(&args, "to")?;
            let payload = args.get("payload").cloned().unwrap_or(Value::Null);
            let timeout = timeout_arg(&args, 30_000);
            let reply = store.ask(&from, &to, payload, timeout).map_err(store_err)?;
            Ok(json!({"request_id": reply.request_id, "payload": reply.payload}))
        }
        "reply" => {
            let from = str_arg(&args, "from")?;
            let request_id = str_arg(&args, "request_id")?;
            let payload = args.get("payload").cloned().unwrap_or(Value::Null);
            store.reply(&from, &request_id, payload).map_err(store_err)?;
            Ok(json!({"ok": true}))
        }
        "publish_event" => {
            let from = str_arg(&args, "from")?;
            let payload = args.get("payload").cloned().unwrap_or(Value::Null);
            let id = store.publish_event(&from, payload).map_err(store_err)?;
            Ok(json!({ "id": id }))
        }
        other => Err(json!({"code": -32601, "message": format!("unknown tool `{other}`")})),
    }
}

fn str_arg(args: &Value, key: &str) -> Result<String, Value> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| json!({"code": -32602, "message": format!("missing `{key}`")}))
}

fn timeout_arg(args: &Value, default_ms: u64) -> Duration {
    Duration::from_millis(args.get("timeout_ms").and_then(Value::as_u64).unwrap_or(default_ms))
}

fn store_err(e: StoreError) -> Value {
    json!({"code": -32000, "message": e.code(), "data": e.to_string()})
}

fn normalize_json_string_field(args: &mut Value, key: &str) {
    let Some(obj) = args.as_object_mut() else { return };
    let Some(field) = obj.get_mut(key) else { return };
    if let Some(s) = field.as_str() {
        if let Ok(parsed) = serde_json::from_str::<Value>(s) {
            *field = parsed;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_replaces_json_string_with_parsed_value() {
        let mut args = json!({"payload": "{\"text\":\"hi\"}"});
        normalize_json_string_field(&mut args, "payload");
        assert_eq!(args["payload"], json!({"text": "hi"}));
    }

    #[test]
    fn normalize_keeps_non_json_string_as_is() {
        let mut args = json!({"payload": "plain text"});
        normalize_json_string_field(&mut args, "payload");
        assert_eq!(args["payload"], json!("plain text"));
    }
}
```

Delete `crates/agentbus-stdio/src/uds_client.rs`, `crates/agentbus-stdio/src/lib.rs`, and `crates/agentbus-stdio/tests/client.rs`.

- [ ] **Step 4: Verify**

Run: `cargo test -p agentbus-stdio && cargo clippy -p agentbus-stdio --all-targets -- -D warnings`
Expected: 3 rpc tests + 2 normalize tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/agentbus-stdio
git commit -m "feat: rewire mcp shim onto spool store"
```

---

### Task 10: Rewire agentbus-cli onto the store (spec section 7, fr:10)

**Files:**
- Modify: `crates/agentbus-cli/Cargo.toml`
- Modify: `crates/agentbus-cli/src/main.rs` (full rewrite)
- Modify: `crates/agentbus-cli/src/commands.rs` (full rewrite)
- Create: `crates/agentbus-cli/tests/cli.rs`

- [ ] **Step 1: Write the failing golden tests**

Create `crates/agentbus-cli/tests/cli.rs`:

```rust
//! Golden-output tests for each verb against a temp store (spec section 11).

use std::process::{Command, Output};

fn agentbus(dir: &std::path::Path, args: &[&str], stdin: Option<&str>) -> Output {
    use std::io::Write;
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_agentbus"));
    cmd.env("AGENTBUS_DIR", dir).args(args);
    match stdin {
        Some(input) => {
            cmd.stdin(std::process::Stdio::piped());
            cmd.stdout(std::process::Stdio::piped());
            cmd.stderr(std::process::Stdio::piped());
            let mut child = cmd.spawn().unwrap();
            child.stdin.as_mut().unwrap().write_all(input.as_bytes()).unwrap();
            child.wait_with_output().unwrap()
        }
        None => cmd.output().unwrap(),
    }
}

fn stdout_json(out: &Output) -> serde_json::Value {
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn register_ls_unregister() {
    let tmp = tempfile::tempdir().unwrap();
    let reg = agentbus(tmp.path(), &["register", "bot", "--persistent", "--on-delivery", "true"], None);
    assert_eq!(stdout_json(&reg)["ok"], true);
    let ls = stdout_json(&agentbus(tmp.path(), &["ls"], None));
    assert_eq!(ls["instances"][0]["id"], "bot");
    assert_eq!(ls["instances"][0]["persistent"], true);
    let un = stdout_json(&agentbus(tmp.path(), &["unregister", "bot"], None));
    assert_eq!(un["ok"], true);
}

#[test]
fn send_then_check_inbox() {
    let tmp = tempfile::tempdir().unwrap();
    agentbus(tmp.path(), &["register", "bob", "--persistent"], None);
    let sent = stdout_json(&agentbus(tmp.path(), &["send", "bob"], Some(r#"{"hi":1}"#)));
    assert!(sent["id"].as_str().unwrap().starts_with("msg_"));
    let inbox = stdout_json(&agentbus(tmp.path(), &["check-inbox", "bob"], None));
    assert_eq!(inbox["envelopes"][0]["payload"]["hi"], 1);
}

#[test]
fn send_to_unknown_prints_coded_error() {
    let tmp = tempfile::tempdir().unwrap();
    let out = agentbus(tmp.path(), &["send", "ghost"], Some("{}"));
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("error[unknown_instance]"), "stderr: {stderr}");
}

#[test]
fn ask_timeout_exits_2_with_ask_result_hint_then_late_reply_retrievable() {
    let tmp = tempfile::tempdir().unwrap();
    agentbus(tmp.path(), &["register", "bob", "--persistent"], None);
    let out = agentbus(tmp.path(), &["ask", "bob", "--timeout-ms", "150"], Some("{}"));
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("agentbus ask-result "), "stderr: {stderr}");
    let request_id = stderr
        .split("agentbus ask-result ")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();
    agentbus(tmp.path(), &["reply", &request_id, "bob"], Some(r#"{"late":1}"#));
    let res = stdout_json(&agentbus(tmp.path(), &["ask-result", &request_id], None));
    assert_eq!(res["status"], "replied");
    assert_eq!(res["payload"]["late"], 1);
}

#[test]
fn events_lists_seq_envelope_lines() {
    let tmp = tempfile::tempdir().unwrap();
    agentbus(tmp.path(), &["publish"], Some(r#"{"e":1}"#));
    agentbus(tmp.path(), &["publish"], Some(r#"{"e":2}"#));
    let out = agentbus(tmp.path(), &["events"], None);
    assert!(out.status.success());
    let lines: Vec<serde_json::Value> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["seq"], 1);
    assert_eq!(lines[1]["envelope"]["payload"]["e"], 2);
}

#[test]
fn await_returns_empty_on_timeout() {
    let tmp = tempfile::tempdir().unwrap();
    agentbus(tmp.path(), &["register", "bob", "--persistent"], None);
    let out = stdout_json(&agentbus(tmp.path(), &["await", "bob", "--timeout-ms", "120"], None));
    assert_eq!(out["envelopes"], serde_json::json!([]));
}

#[test]
fn sweep_prints_report() {
    let tmp = tempfile::tempdir().unwrap();
    let out = stdout_json(&agentbus(tmp.path(), &["sweep"], None));
    assert_eq!(out["dead_instances"], serde_json::json!([]));
    assert_eq!(out["purged_inboxes"], serde_json::json!([]));
}
```

Update `crates/agentbus-cli/Cargo.toml` dependencies to:

```toml
[dependencies]
agentbus-core = { version = "0.1.0", path = "../agentbus-core" }
anyhow = { workspace = true }
serde_json = { workspace = true }
clap = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

(Remove reqwest, tokio, futures, eventsource-stream, time, urlencoding, serde.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p agentbus-cli`
Expected: build FAIL (commands.rs still uses reqwest).

- [ ] **Step 3: Rewrite the CLI**

Replace `crates/agentbus-cli/src/main.rs` entirely:

```rust
mod commands;

use clap::Parser;

use agentbus_core::store::StoreError;

fn main() {
    let cli = commands::Cli::parse();
    if let Err(e) = commands::run(cli) {
        match e.downcast_ref::<StoreError>() {
            Some(se) => eprintln!("error[{}]: {se}", se.code()),
            None => eprintln!("error: {e}"),
        }
        std::process::exit(1);
    }
}
```

Replace `crates/agentbus-cli/src/commands.rs` entirely:

```rust
//! agentbus CLI (fr:10, v0.2): a thin wrapper over the spool store.
//! Single results print as pretty JSON; streams print one compact JSON
//! value per line.

use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};
use serde_json::Value;

use agentbus_core::envelope::Kind;
use agentbus_core::store::{EventFilter, RegisterOpts, Store, StoreError, SweepOpts};

#[derive(Parser)]
#[command(name = "agentbus", version, about = "agentbus CLI (daemonless spool store)")]
pub struct Cli {
    /// Store directory (default ~/.agentbus).
    #[arg(long, env = "AGENTBUS_DIR")]
    pub dir: Option<PathBuf>,
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// Register an instance id (non-persistent rows die with this process;
    /// pair with --persistent for durable addresses).
    Register {
        id: String,
        #[arg(long)]
        persistent: bool,
        #[arg(long)]
        on_delivery: Option<String>,
    },
    /// Remove a registration (the inbox file is kept).
    Unregister { id: String },
    /// List registered instances.
    Ls,
    /// Send a one-way message (payload from --file or stdin).
    Send {
        to: String,
        #[arg(long, default_value = "ext:cli")]
        from: String,
        #[arg(short, long)]
        file: Option<String>,
    },
    /// Send a request and wait for the reply.
    Ask {
        to: String,
        #[arg(long, default_value = "ext:cli")]
        from: String,
        #[arg(long, default_value_t = 30_000)]
        timeout_ms: u64,
        #[arg(short, long)]
        file: Option<String>,
    },
    /// Fetch the (possibly late) reply to an earlier ask.
    AskResult { request_id: String },
    /// Reply to an ask as <from>.
    Reply {
        request_id: String,
        from: String,
        #[arg(short, long)]
        file: Option<String>,
    },
    /// Drain an instance's inbox without blocking.
    CheckInbox { id: String },
    /// Block until messages arrive, or time out (empty list).
    Await {
        id: String,
        #[arg(long, default_value_t = 30_000)]
        timeout_ms: u64,
    },
    /// Publish a broadcast event.
    Publish {
        #[arg(long, default_value = "ext:cli")]
        from: String,
        #[arg(short, long)]
        file: Option<String>,
    },
    /// Read the event log as {"seq":..,"envelope":..} lines; --follow polls.
    Events {
        #[arg(long)]
        follow: bool,
        #[arg(long, default_value_t = 0)]
        since: i64,
        #[arg(long)]
        instance: Option<String>,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long, default_value_t = 500)]
        interval_ms: u64,
    },
    /// Stream envelopes addressed to one instance, one compact JSON per
    /// line, never consuming the inbox (spec 6.7; for harness monitor tools).
    Watch {
        id: String,
        #[arg(long, default_value_t = 500)]
        interval_ms: u64,
    },
    /// Crash recovery: prune dead registrations, re-fire stale hooks,
    /// report expired asks (spec 6.8).
    Sweep {
        #[arg(long)]
        purge_orphans: bool,
        #[arg(long, default_value_t = 60)]
        grace_secs: u64,
    },
}

pub fn run(cli: Cli) -> Result<()> {
    let mut store = match &cli.dir {
        Some(dir) => Store::open_at(dir)?,
        None => Store::open()?,
    };
    match cli.cmd {
        Cmd::Register { id, persistent, on_delivery } => {
            store.register(&id, &RegisterOpts { persistent, on_delivery, pid: None })?;
            println!("{}", serde_json::json!({"ok": true}));
        }
        Cmd::Unregister { id } => {
            let removed = store.unregister(&id)?;
            println!("{}", serde_json::json!({ "ok": removed }));
        }
        Cmd::Ls => {
            let rows = store.list_instances()?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({ "instances": rows }))?
            );
        }
        Cmd::Send { to, from, file } => {
            let delivered = store.send(&from, &to, read_payload(&file)?)?;
            warn_on_hook_failure(delivered.hook.as_ref());
            println!("{}", serde_json::json!({"id": delivered.envelope.id}));
        }
        Cmd::Ask { to, from, timeout_ms, file } => {
            let payload = read_payload(&file)?;
            match store.ask(&from, &to, payload, Duration::from_millis(timeout_ms)) {
                Ok(reply) => println!("{}", serde_json::to_string_pretty(&reply)?),
                Err(StoreError::Timeout(rid)) => {
                    eprintln!(
                        "error[timeout]: no reply within {timeout_ms} ms; \
                         retrieve a late reply with: agentbus ask-result {rid}"
                    );
                    std::process::exit(2);
                }
                Err(e) => return Err(e.into()),
            }
        }
        Cmd::AskResult { request_id } => {
            let status = store.ask_result(&request_id)?;
            println!("{}", serde_json::to_string_pretty(&status)?);
        }
        Cmd::Reply { request_id, from, file } => {
            store.reply(&from, &request_id, read_payload(&file)?)?;
            println!("{}", serde_json::json!({"ok": true}));
        }
        Cmd::CheckInbox { id } => {
            let envelopes = store.check_inbox(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({ "envelopes": envelopes }))?
            );
        }
        Cmd::Await { id, timeout_ms } => {
            let envelopes = store.await_message(&id, Duration::from_millis(timeout_ms))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({ "envelopes": envelopes }))?
            );
        }
        Cmd::Publish { from, file } => {
            let id = store.publish_event(&from, read_payload(&file)?)?;
            println!("{}", serde_json::json!({ "id": id }));
        }
        Cmd::Events { follow, since, instance, kind, interval_ms } => {
            let filter = EventFilter {
                instance,
                kind: parse_kind(kind.as_deref())?,
                to: None,
            };
            stream_events(&store, since, &filter, follow, Duration::from_millis(interval_ms), true)?;
        }
        Cmd::Watch { id, interval_ms } => {
            let filter = EventFilter { to: Some(id), ..Default::default() };
            let cursor = store.max_seq()?; // start live: no replay (spec 6.7)
            stream_events(&store, cursor, &filter, true, Duration::from_millis(interval_ms), false)?;
        }
        Cmd::Sweep { purge_orphans, grace_secs } => {
            let report = store.sweep(&SweepOpts {
                grace: Duration::from_secs(grace_secs),
                purge_orphans,
            })?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
    }
    Ok(())
}

fn parse_kind(kind: Option<&str>) -> Result<Option<Kind>> {
    match kind {
        None => Ok(None),
        Some(s) => s.parse::<Kind>().map(Some).map_err(anyhow::Error::msg),
    }
}

/// Drain rows from `since`; with `follow`, poll forever. `with_seq` selects
/// the {"seq":..,"envelope":..} line shape (events) vs bare envelopes (watch).
fn stream_events(
    store: &Store,
    mut cursor: i64,
    filter: &EventFilter,
    follow: bool,
    interval: Duration,
    with_seq: bool,
) -> Result<()> {
    use std::io::Write;
    loop {
        loop {
            let page = store.events_since(cursor, 1000, filter)?;
            let drained = page.events.is_empty() && page.cursor == cursor;
            for ev in &page.events {
                if with_seq {
                    println!("{}", serde_json::to_string(ev)?);
                } else {
                    println!("{}", serde_json::to_string(&ev.envelope)?);
                }
            }
            cursor = page.cursor;
            if drained {
                break;
            }
        }
        std::io::stdout().flush()?;
        if !follow {
            return Ok(());
        }
        std::thread::sleep(interval);
    }
}

fn warn_on_hook_failure(hook: Option<&agentbus_core::store::HookOutcome>) {
    if let Some(h) = hook {
        if !h.ok {
            eprintln!("warning: on_delivery hook failed: {}", h.detail);
        }
    }
}

fn read_payload(file: &Option<String>) -> Result<Value> {
    let raw = match file {
        Some(path) => std::fs::read_to_string(path)?,
        None => {
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            buf
        }
    };
    Ok(serde_json::from_str(&raw)?)
}
```

- [ ] **Step 4: Verify**

Run: `cargo test -p agentbus-cli && cargo clippy -p agentbus-cli --all-targets -- -D warnings`
Expected: 7 golden tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/agentbus-cli Cargo.lock
git commit -m "feat: rewire cli onto spool store"
```

---

### Task 11: watch streaming test (spec 6.7)

The `watch` verb was implemented in Task 10 (it shares `stream_events`). This task proves the streaming contract end to end: a watcher subprocess sees a message sent after it started, as one line, without consuming the inbox.

**Files:**
- Modify: `crates/agentbus-cli/tests/cli.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/agentbus-cli/tests/cli.rs`:

```rust
#[test]
fn watch_streams_new_envelopes_without_consuming() {
    use std::io::BufRead;

    let tmp = tempfile::tempdir().unwrap();
    agentbus(tmp.path(), &["register", "bob", "--persistent"], None);
    // Pre-existing traffic must NOT be replayed by watch.
    agentbus(tmp.path(), &["send", "bob"], Some(r#"{"old":1}"#));

    let mut watcher = Command::new(env!("CARGO_BIN_EXE_agentbus"))
        .env("AGENTBUS_DIR", tmp.path())
        .args(["watch", "bob", "--interval-ms", "50"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let stdout = watcher.stdout.take().unwrap();

    // Read one line on a thread so the test can time out instead of hanging.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(stdout);
        let mut line = String::new();
        let _ = reader.read_line(&mut line);
        let _ = tx.send(line);
    });

    // Give the watcher a moment to record its starting cursor.
    std::thread::sleep(std::time::Duration::from_millis(300));
    agentbus(tmp.path(), &["send", "bob"], Some(r#"{"fresh":1}"#));

    let line = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("watch line within 5s");
    let env: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(env["payload"]["fresh"], 1, "no replay of old traffic");
    assert_eq!(env["to"], "bob");

    watcher.kill().unwrap();
    watcher.wait().unwrap();

    // watch is a notifier only: both messages still consumable (spec 6.7).
    let inbox = stdout_json(&agentbus(tmp.path(), &["check-inbox", "bob"], None));
    assert_eq!(inbox["envelopes"].as_array().unwrap().len(), 2);
}
```

- [ ] **Step 2: Run to verify it passes (or fix)**

Run: `cargo test -p agentbus-cli watch_streams -- --nocapture`
Expected: PASS (implementation landed in Task 10). If it fails, debug `stream_events` — most likely a stdout flush ordering issue.

- [ ] **Step 3: Commit**

```bash
git add crates/agentbus-cli
git commit -m "test: prove watch streaming contract end to end"
```

---

### Task 12: Multi-process concurrency test (spec section 11)

**Files:**
- Create: `crates/agentbus-cli/tests/concurrency.rs`

- [ ] **Step 1: Write the test**

```rust
//! Spec section 11: N OS processes hammering send on one store. Assert no
//! lost envelopes, no duplicate seq, intact JSONL inbox lines.

use std::process::Command;

const WORKERS: u64 = 8;
const SENDS_PER_WORKER: u64 = 25;

#[test]
fn concurrent_senders_lose_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();

    let status = Command::new(env!("CARGO_BIN_EXE_agentbus"))
        .env("AGENTBUS_DIR", &dir)
        .args(["register", "sink", "--persistent"])
        .status()
        .unwrap();
    assert!(status.success());

    let mut handles = Vec::new();
    for w in 0..WORKERS {
        let dir = dir.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..SENDS_PER_WORKER {
                use std::io::Write;
                let mut child = Command::new(env!("CARGO_BIN_EXE_agentbus"))
                    .env("AGENTBUS_DIR", &dir)
                    .args(["send", "sink"])
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::inherit())
                    .spawn()
                    .unwrap();
                let payload = format!(r#"{{"w":{w},"i":{i}}}"#);
                child.stdin.as_mut().unwrap().write_all(payload.as_bytes()).unwrap();
                drop(child.stdin.take());
                assert!(child.wait().unwrap().success(), "send w={w} i={i}");
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    let total = WORKERS * SENDS_PER_WORKER;

    // Inbox: every line parses, count matches.
    let inbox = std::fs::read_to_string(dir.join("inbox/sink.jsonl")).unwrap();
    let envelopes: Vec<serde_json::Value> = inbox
        .lines()
        .map(|l| serde_json::from_str(l).expect("intact JSONL line"))
        .collect();
    assert_eq!(envelopes.len() as u64, total);

    // event_log: distinct seq per envelope, no gaps lost.
    let out = Command::new(env!("CARGO_BIN_EXE_agentbus"))
        .env("AGENTBUS_DIR", &dir)
        .args(["events"])
        .output()
        .unwrap();
    let seqs: Vec<i64> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["seq"].as_i64().unwrap())
        .collect();
    assert_eq!(seqs.len() as u64, total);
    let mut deduped = seqs.clone();
    deduped.dedup();
    assert_eq!(seqs, deduped, "duplicate seq");
}
```

- [ ] **Step 2: Run**

Run: `cargo test -p agentbus-cli concurrent_ -- --nocapture`
Expected: PASS in well under a minute (200 debug-binary spawns). If `store_locked` errors surface, the busy_timeout is being exhausted — investigate before weakening the test.

- [ ] **Step 3: Commit**

```bash
git add crates/agentbus-cli
git commit -m "test: add multi-process store concurrency test"
```

---

### Task 13: Delete the daemon and the in-memory bus (spec section 7)

**Files:**
- Delete: `crates/agentbusd/` (entire crate, including its `tests/`)
- Delete: `crates/agentbus-core/src/mailbox.rs`, `registry.rs`, `router.rs`, `eventlog.rs`
- Modify: `crates/agentbus-core/src/lib.rs`
- Modify: `crates/agentbus-core/Cargo.toml`
- Modify: `Cargo.toml` (workspace)
- Modify: `.github/workflows/release.yml`

- [ ] **Step 1: Delete the daemon crate and dead core modules**

```bash
git rm -r crates/agentbusd
git rm crates/agentbus-core/src/mailbox.rs \
       crates/agentbus-core/src/registry.rs \
       crates/agentbus-core/src/router.rs \
       crates/agentbus-core/src/eventlog.rs
```

- [ ] **Step 2: Trim the module tree and workspace**

`crates/agentbus-core/src/lib.rs` becomes exactly:

```rust
pub mod envelope;
pub mod ids;
pub mod store;
```

In `crates/agentbus-core/Cargo.toml`, the dependency section becomes:

```toml
[dependencies]
thiserror = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
tracing = { workspace = true }
ulid = { workspace = true }
time = { workspace = true }
rusqlite = { workspace = true }
libc = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

(Drop anyhow, tokio, proptest — confirm with the build; if `proptest` is still referenced by an envelope/ids test, keep it.)

In the root `Cargo.toml`:
- Remove `"crates/agentbusd"` from `[workspace.members]`.
- Remove from `[workspace.dependencies]`: `axum`, `tower`, `tower-http`, `reqwest`, `futures`, `eventsource-stream`, `tokio` (verify nothing references them first: `grep -rn "tokio" crates/ --include=Cargo.toml`).

- [ ] **Step 3: Update the release workflow**

In `.github/workflows/release.yml`, remove the `agentbusd` publish step so the order is: `agentbus-core` → `agentbus-cli` → `agentbus-stdio`. Leave `.github/workflows/ci.yml` untouched (workspace commands still apply).

- [ ] **Step 4: Verify the full gate**

Run:
```bash
cargo fmt --all -- --check && \
cargo clippy --workspace --all-targets -- -D warnings && \
cargo build --workspace --all-targets && \
cargo test --workspace
```
Expected: all green. Also run `cargo deny check 2>/dev/null || true` if cargo-deny is installed locally (rusqlite's bundled SQLite is public domain; expect no license failure).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat!: delete daemon and in-memory bus modules"
```

---

### Task 14: FR docs — tombstones and the registry rewrite

Read `docs/fr/README.md` and `docs/fr/01-envelope.md` first to mirror the house FR structure (frontmatter, section order, Boundaries notes). The post-edit hook runs `kusara validate` automatically; fix what it reports.

**Files:**
- Modify: `docs/fr/03-mailbox.md`, `docs/fr/06-rest-api.md`, `docs/fr/07-sse.md`, `docs/fr/11-daemon-lifecycle.md` (tombstones)
- Modify: `docs/fr/02-instance-registry.md` (rewrite)

- [ ] **Step 1: Tombstone the daemon-era FRs**

Each of the four files keeps its `refs:` id and kind (so no reference elsewhere dangles) but drops `modules:` (the paths no longer exist) and replaces the body. Template — adjust id/title/pointer per file:

```markdown
---
refs:
  id: fr:06-rest-api
  kind: fr
  title: "REST API surface (superseded)"
  related:
    - fr:12-store
---

# REST API surface (superseded)

Deleted in v0.2 (spool model). The bus is no longer a daemon; there is no
HTTP surface. The store operations that replaced this surface are
documented in [fr:12-store]. If remote access returns later, it returns as
a thin HTTP view over the same store (spec non-goal, 2026-06-05).
```

Pointers: `fr:03-mailbox` → "folded into fr:09-hook-inbox (inbox spool files are the mailbox)"; `fr:06-rest-api` → fr:12; `fr:07-sse` → fr:05 (events follow) and fr:14 (watch); `fr:11-daemon-lifecycle` → fr:12 and fr:15.

Note: `related:` entries pointing at fr:12/fr:14/fr:15 will dangle until Task 16 creates them. If `kusara validate` hard-errors on that, point the tombstones at `fr:09-hook-inbox`/`fr:05-eventlog` for now and update `related:` in Task 16.

- [ ] **Step 2: Rewrite fr:02 (instance registry)**

`docs/fr/02-instance-registry.md` — new frontmatter and content covering spec 6.1 and section 4:

```yaml
---
refs:
  id: fr:02-instance-registry
  kind: fr
  title: "Instance registration and identity"
  related:
    - fr:01-envelope
    - fr:09-hook-inbox
  modules:
    - crates/agentbus-core/src/store/instances.rs
    - crates/agentbus-core/src/store/liveness.rs
---
```

Body must document: registrations are `instances` rows (persistent or pid-scoped); the id charset `[A-Za-z0-9_.:-]{1,128}`; liveness = `kill(pid, 0)`, persistent rows exempt; collision rules (dead row replaced, same pid idempotent, live foreign pid → `instance_id_taken`, persistent rows upsert); unregister leaves the inbox file; registrations survive reboots only when persistent (pids are dead after reboot, swept lazily).

- [ ] **Step 3: Validate and commit**

Run `kusara validate` passes (hook does this on save), then:

```bash
git add docs/fr
git commit -m "docs: tombstone daemon-era frs, rewrite fr02"
```

---

### Task 15: FR docs — delivery, eventlog, shim, hook-inbox, cli

**Files:**
- Modify: `docs/fr/04-router.md`, `docs/fr/05-eventlog.md`, `docs/fr/08-mcp-shim.md`, `docs/fr/09-hook-inbox.md`, `docs/fr/10-cli.md`

- [ ] **Step 1: Rewrite fr:04 as sender-performed delivery**

Keep id `fr:04-router` (renaming would dangle references). New `modules: [crates/agentbus-core/src/store/messages.rs]`. Body documents spec 6.2–6.3: send transaction shape (recipient row must exist, event logged in-tx, spool append + hook post-commit); ask = send + `asks` row + 50→250 ms polling; reply = first-write-wins row update, both replies logged, no inbox write; late replies retrievable via `ask_result`; error codes `unknown_instance`, `timeout`, `unknown_request_id`.

- [ ] **Step 2: Rewrite fr:05 as the event_log table**

New `modules: [crates/agentbus-core/src/store/events.rs]`. Body: one ordered table; `seq` assigned transactionally (immune to wall-clock skew); cursor reads (`events_since` advances past filtered rows); no-gap/no-duplicate by construction; corrupt rows skipped with a warning; no payload size limit in v0.2 (Boundaries note: v0.1 had max_payload; reintroduce if abuse shows).

- [ ] **Step 3: Rewrite fr:08 for direct store access**

`modules: [crates/agentbus-stdio/src]`. Body: same nine tool names as v0.1; shim opens the store directly (no socket, no daemon); v0.2 surface changes — `register` gains `persistent`/`on_delivery` and loses `mailbox_size`; `await_message`/`check_inbox` return envelope batches (`envelopes` array); errors carry `code()` strings in the JSON-RPC `message` field; non-persistent registrations are released on stdin EOF, pid liveness covers abrupt kills.

- [ ] **Step 4: Edit fr:09 (minor)**

The contract is unchanged; the writer changed. Update: writer is now the **sender process** (`store/spool.rs`), not the daemon; default inbox dir is `~/.agentbus/inbox` (`AGENTBUS_INBOX_DIR` still overrides the hook script; the store uses `$AGENTBUS_DIR/inbox`); appends take an exclusive flock around one O_APPEND write; consumers never need the lock (the Rust consumer takes it once after rename as a belt-and-suspenders barrier). `modules:` becomes `[crates/agentbus-core/src/store/spool.rs, scripts/inject-inbox.sh]`.

- [ ] **Step 5: Rewrite fr:10 (cli)**

`modules: [crates/agentbus-cli/src]`. Body: verb table (register/unregister/ls/send/ask/ask-result/reply/check-inbox/await/publish/events/watch/sweep) with one-line semantics each; payload via `--file` or stdin; output conventions (pretty JSON for single results, compact JSON lines for streams); exit codes (0 ok, 1 coded error on stderr as `error[code]: message`, 2 ask timeout with ask-result hint); `AGENTBUS_DIR` override.

- [ ] **Step 6: Validate and commit**

```bash
git add docs/fr
git commit -m "docs: rewrite fr04/05/08/09/10 for spool model"
```

---

### Task 16: FR docs — new fr:12–fr:15 and index regeneration

**Files:**
- Create: `docs/fr/12-store.md`, `docs/fr/13-on-delivery.md`, `docs/fr/14-watch.md`, `docs/fr/15-sweep.md`
- Modify: tombstone `related:` entries from Task 14 if they were deferred
- Regenerate: `docs/fr/index.md` via `kusara index`

- [ ] **Step 1: fr:12 — store layout, schema, concurrency (spec section 5)**

```yaml
---
refs:
  id: fr:12-store
  kind: fr
  title: "Spool store layout, schema, and concurrency"
  related:
    - fr:01-envelope
    - fr:02-instance-registry
    - fr:09-hook-inbox
  modules:
    - crates/agentbus-core/src/store/mod.rs
    - crates/agentbus-core/src/store/paths.rs
    - crates/agentbus-core/src/store/error.rs
---
```

Body: directory layout (`~/.agentbus` 0700, `bus.db`, `inbox/<id>.jsonl`); `AGENTBUS_DIR` override; full schema (copy from `store/mod.rs::SCHEMA`, including `expired_notified`); pragmas (WAL, synchronous=NORMAL, busy_timeout=5000); concurrency rules (BEGIN IMMEDIATE per op; flock+O_APPEND appends; consumers rename); error model table (spec section 8 plus `unknown_request_id`); security posture (0700, single-user trust); "keep ~/.agentbus on a local volume" note (spec open question 2).

- [ ] **Step 2: fr:13 — on_delivery execution contract (spec 6.5)**

`modules: [crates/agentbus-core/src/store/hook.rs]`, related: fr:04-router, fr:15-sweep. Body: sender executes recipient's command via `sh -c`; env vars `AGENTBUS_INSTANCE`, `AGENTBUS_ENVELOPE_ID`, `AGENTBUS_KIND`, `AGENTBUS_FROM`; 15 s timeout, kill on expiry; non-fatal failure policy → `bus.delivery_hook_failed` event; security note (registered only by the same user; grants nothing the user lacks — document loudly per spec section 9); sweep re-fire runs with `AGENTBUS_INSTANCE` only.

- [ ] **Step 3: fr:14 — watch notifier contract (spec 6.7)**

`modules: [crates/agentbus-cli/src/commands.rs, crates/agentbus-core/src/store/events.rs]`, related: fr:05-eventlog, fr:09-hook-inbox, ref:watch-integration. Body: `agentbus watch <id>` tails event_log for envelopes addressed to the instance; starts at current max seq (no replay); one compact JSON envelope per line (newline-safe by construction); **never consumes the inbox** — consumption stays with check_inbox under fr:09; Boundaries: watcher lifecycle (session hooks, dedup, orphan cleanup) belongs to the integrating harness, not the bus.

- [ ] **Step 4: fr:15 — sweep (spec 6.8)**

`modules: [crates/agentbus-core/src/store/sweep.rs]`, related: fr:02-instance-registry, fr:13-on-delivery. Body: periodic CLI, optional; four actions (dead non-persistent rows; `bus.ask_expired` once per ask via `expired_notified`; stale-inbox hook re-fire after grace; `--purge-orphans`); without it recovery happens lazily at the next send.

- [ ] **Step 5: Fix deferred `related:` links, regenerate, validate**

Update Task 14 tombstones to point at the now-existing fr:12/fr:14/fr:15. Run:

```bash
kusara index && kusara validate
```

- [ ] **Step 6: Commit**

```bash
git add docs/fr
git commit -m "docs: add fr12-15 for store, hook, watch, sweep"
```

---

### Task 17: Reference docs, READMEs, version 0.2.0

**Files:**
- Modify: `docs/reference/protocol.md` (rewrite)
- Create: `docs/reference/watch-integration.md`
- Modify: `docs/README.md`, `README.md`
- Modify: `Cargo.toml` (version)
- Regenerate: `docs/reference/index.md`

- [ ] **Step 1: Rewrite protocol.md around store operations**

Keep `id: ref:protocol`. Sections: envelope wire format (unchanged, link fr:01); store operations table (register/send/ask/reply/ask-result/publish/events/check-inbox/await/watch/sweep — each with semantics and error codes); MCP tool surface (link fr:08, note v0.2 changes); CLI invocation examples for each verb with real JSON output; delivery modes summary (on_delivery hook, await_message, check_inbox, watch). Delete every REST/SSE section.

- [ ] **Step 2: Create watch-integration.md**

```yaml
---
refs:
  id: ref:watch-integration
  kind: reference
  title: "Recipient-side watch integration for interactive harnesses"
  related:
    - fr:14-watch
    - fr:13-on-delivery
---
```

Body: the watch-plus-monitor pattern — a session-start hook launches `agentbus watch <id>` under the harness's persistent monitor facility (e.g. Claude Code's Monitor tool with `persistent: true`); each stdout line re-invokes the idle agent; agent reacts by calling `check_inbox` (consume) and replies via `send`/`reply`. Include a concrete Claude Code hook example (directive emitting the Monitor invocation). State the division of responsibility: agentbus ships the verb; dedup across session restarts and orphan cleanup belong to the integrator. Mention `mode` alternatives for harnesses without a monitor facility (Stop-hook `check-inbox`). Prior art note: agmsg's monitor delivery mode.

- [ ] **Step 3: Update READMEs and bump the version**

- `docs/README.md` and root `README.md`: replace the four-surfaces description (MCP stdio / REST / SSE / hook-inbox) with the v0.2 model: an MCP-native message bus over shared local storage — MCP stdio shim, CLI, hook-driven inbox, and watch streaming; zero daemons. Update any quickstart that starts `agentbusd`.
- Root `Cargo.toml`: `[workspace.package] version = "0.2.0"`. Update the two path deps (`crates/agentbus-cli`, `crates/agentbus-stdio` depend on `agentbus-core = { version = "0.1.0", path = ... }`) to `version = "0.2.0"`.

- [ ] **Step 4: Final gate**

```bash
kusara index && kusara validate && \
cargo fmt --all -- --check && \
cargo clippy --workspace --all-targets -- -D warnings && \
cargo test --workspace
```
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "docs: rewrite reference docs, bump to 0.2.0"
```

---

## Done criteria

- `cargo test --workspace` green; fmt/clippy clean; CI workflow unchanged and passing.
- No `agentbusd` crate; no axum/tokio/reqwest in the dependency tree.
- `kusara validate` clean; `docs/fr/index.md` and `docs/reference/index.md` regenerated.
- The spec deltas listed at the top of this plan are folded back into `docs/superpowers/specs/2026-06-05-spool-model-design.md` (one small follow-up edit, can ride with Task 17).
