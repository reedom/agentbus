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
#[allow(dead_code)] // production use from Task 5 onward (tests use it now)
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
        let got = store
            .await_message("bob", Duration::from_millis(120))
            .unwrap();
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
            .env(
                "AGENTBUS_INBOX_DIR",
                crate::store::paths::inbox_dir(tmp.path()),
            )
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("agentbus inbox:"), "stdout: {stdout}");
        assert!(stdout.contains("42"), "stdout: {stdout}");
        assert!(!crate::store::paths::inbox_dir(tmp.path())
            .join("bob.jsonl")
            .exists());
    }
}
