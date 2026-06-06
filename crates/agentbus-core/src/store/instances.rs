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
#[allow(dead_code)] // used from Task 5 onward
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
            // Collision: only a non-persistent row with a live pid different
            // from ours. Persistent rows (pid = NULL) and dead rows always
            // yield to the caller (single-user trust model, spec 6.1).
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

    /// Liveness is checked per-row after the query and may lag by milliseconds.
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

#[cfg(test)]
mod tests {
    use crate::store::testutil::test_store;
    use crate::store::{RegisterOpts, StoreError};

    fn opts() -> RegisterOpts {
        RegisterOpts::default()
    }

    /// Kills the child even when the test panics before reaching cleanup.
    struct KillOnDrop(std::process::Child);

    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
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
        let child = KillOnDrop(
            std::process::Command::new("sleep")
                .arg("30")
                .spawn()
                .unwrap(),
        );
        let foreign = RegisterOpts {
            pid: Some(child.0.id() as i32),
            ..opts()
        };
        store.register("alice", &foreign).unwrap();
        let err = store.register("alice", &opts()).unwrap_err();
        assert!(matches!(err, StoreError::InstanceIdTaken(_)));
    }

    #[test]
    fn dead_pid_row_is_replaced() {
        // Theoretical pid-reuse race (spec open question 3): acceptable here.
        let (_tmp, mut store) = test_store();
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id() as i32;
        child.wait().unwrap();
        store
            .register(
                "alice",
                &RegisterOpts {
                    pid: Some(pid),
                    ..opts()
                },
            )
            .unwrap();
        store.register("alice", &opts()).unwrap(); // dead owner -> replace
        let rows = store.list_instances().unwrap();
        assert_eq!(rows[0].pid, Some(std::process::id() as i32));
    }

    #[test]
    fn persistent_row_has_no_pid_and_upserts() {
        let (_tmp, mut store) = test_store();
        let p = RegisterOpts {
            persistent: true,
            on_delivery: Some("true".into()),
            ..opts()
        };
        store.register("bot", &p).unwrap();
        let p2 = RegisterOpts {
            persistent: true,
            on_delivery: Some("false".into()),
            ..opts()
        };
        store.register("bot", &p2).unwrap();
        let rows = store.list_instances().unwrap();
        assert_eq!(rows[0].pid, None);
        assert!(rows[0].alive); // persistent rows are exempt from liveness
        assert_eq!(rows[0].on_delivery.as_deref(), Some("false"));
    }

    #[test]
    fn invalid_ids_are_rejected() {
        let (_tmp, mut store) = test_store();
        let long = "x".repeat(129);
        for bad in ["", "a/b", "a b", long.as_str()] {
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
