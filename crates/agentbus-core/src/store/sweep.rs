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
        SweepOpts {
            grace: Duration::from_secs(60),
            purge_orphans: false,
        }
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
        let dead: Vec<(String, Option<i32>)> = {
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
                    dead.push((id, pid));
                }
            }
            dead
        };
        self.with_tx(|tx| {
            for (id, pid) in &dead {
                // The pid predicate closes the liveness-check-to-delete
                // window: a row re-registered under a new live pid since the
                // check above is left alone.
                tx.execute(
                    "DELETE FROM instances WHERE id = ?1 AND persistent = 0 AND pid IS ?2",
                    params![id, pid],
                )?;
            }
            Ok(())
        })?;
        report.dead_instances = dead.into_iter().map(|(id, _)| id).collect();
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
            // Publish before flipping the flag: at-least-once on crash, but
            // never silent suppression. A duplicate event beats a lost one.
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
            let mut stmt = self
                .conn
                .prepare("SELECT id, on_delivery FROM instances WHERE on_delivery IS NOT NULL")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<Result<_, _>>()?
        };
        for (id, cmd) in hooked {
            let path = inbox_dir(&self.base).join(format!("{id}.jsonl"));
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
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
        let known: HashSet<String> = self.list_instances()?.into_iter().map(|r| r.id).collect();
        for entry in std::fs::read_dir(inbox_dir(&self.base))? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(id) = name.strip_suffix(".jsonl") else {
                continue;
            };
            if !known.contains(id) {
                std::fs::remove_file(entry.path())?;
                report.purged_inboxes.push(id.to_string());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::SweepOpts;
    use crate::store::testutil::test_store;
    use crate::store::{EventFilter, RegisterOpts};

    fn zero_grace(purge: bool) -> SweepOpts {
        SweepOpts {
            grace: Duration::ZERO,
            purge_orphans: purge,
        }
    }

    #[test]
    fn removes_dead_nonpersistent_rows_only() {
        let (_tmp, mut store) = test_store();
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let dead_pid = child.id() as i32;
        child.wait().unwrap();
        store
            .register(
                "dead",
                &RegisterOpts {
                    pid: Some(dead_pid),
                    ..Default::default()
                },
            )
            .unwrap();
        store.register("live", &RegisterOpts::default()).unwrap();
        store
            .register(
                "bot",
                &RegisterOpts {
                    persistent: true,
                    ..Default::default()
                },
            )
            .unwrap();
        let report = store.sweep(&zero_grace(false)).unwrap();
        assert_eq!(report.dead_instances, vec!["dead".to_string()]);
        let ids: Vec<String> = store
            .list_instances()
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(ids, vec!["bot".to_string(), "live".to_string()]);
    }

    #[test]
    fn reports_expired_asks_exactly_once() {
        let (_tmp, mut store) = test_store();
        store.register("bob", &RegisterOpts::default()).unwrap();
        let err = store
            .ask("alice", "bob", json!({}), Duration::from_millis(60))
            .unwrap_err();
        let crate::store::StoreError::Timeout(rid) = err else {
            panic!()
        };
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
