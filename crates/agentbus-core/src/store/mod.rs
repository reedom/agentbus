//! The spool store (spec sections 5-6): SQLite for registry/asks/events,
//! per-instance JSONL inbox files for delivery. No daemon; every operation
//! is performed by the calling process.

mod error;
mod instances;
mod liveness;
pub(crate) mod paths;
mod spool;

pub use error::StoreError;
pub use instances::{InstanceRow, RegisterOpts};
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
        Ok(Store {
            base: base.to_path_buf(),
            conn,
        })
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
#[allow(dead_code)] // production use from Task 5 onward (tests use it now)
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
#[allow(dead_code)] // used from Task 3 onward
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
