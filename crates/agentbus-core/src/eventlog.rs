//! Append-only JSONL event log for replay.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

use crate::envelope::Envelope;

#[derive(Debug, thiserror::Error)]
pub enum LogError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("payload exceeds max size {max} bytes")]
    PayloadTooLarge { max: usize },
}

pub struct EventLog {
    path: PathBuf,
    writer: Mutex<File>,
    max_payload: usize,
}

impl EventLog {
    pub async fn open(path: impl AsRef<Path>, max_payload: usize) -> Result<Arc<Self>, LogError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;
        Ok(Arc::new(Self {
            path,
            writer: Mutex::new(f),
            max_payload,
        }))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn append(&self, env: &Envelope) -> Result<(), LogError> {
        let mut line = serde_json::to_vec(env).expect("envelope serializes");
        if self.max_payload + 4096 < line.len() {
            return Err(LogError::PayloadTooLarge {
                max: self.max_payload,
            });
        }
        line.push(b'\n');
        let mut w = self.writer.lock().await;
        w.write_all(&line).await?;
        w.flush().await?;
        Ok(())
    }

    /// Snapshot current end-of-file size, used by SSE to define replay boundary.
    pub async fn snapshot_offset(&self) -> Result<u64, LogError> {
        let meta = tokio::fs::metadata(&self.path).await?;
        Ok(meta.len())
    }

    /// Read all envelopes whose `ts` is at or after `since`, scanning up to `until_offset` bytes.
    pub async fn replay_since(
        &self,
        since: Option<time::OffsetDateTime>,
        until_offset: u64,
    ) -> Result<Vec<Envelope>, LogError> {
        let f = File::open(&self.path).await?;
        // Truncation guard: if file grew/shrank between snapshot and read, clamp.
        let cur_len = f.metadata().await?.len();
        let cap = until_offset.min(cur_len);
        let mut reader = BufReader::new(f).take(cap);
        let mut out = Vec::new();
        let mut buf = String::new();
        loop {
            buf.clear();
            let n = reader.read_line(&mut buf).await?;
            if n == 0 {
                break;
            }
            let trimmed = buf.trim_end_matches(['\n', '\r']);
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<Envelope>(trimmed) {
                Ok(env) => {
                    if let Some(s) = since {
                        if env.ts < s {
                            continue;
                        }
                    }
                    out.push(env);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "skipping corrupt event log line");
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::Kind;
    use crate::ids::{new_envelope_id, now_utc};
    use serde_json::json;
    use tempfile::tempdir;

    fn env() -> Envelope {
        Envelope {
            id: new_envelope_id(),
            kind: Kind::Event,
            from: "x".into(),
            to: None,
            request_id: None,
            timeout_ms: None,
            ts: now_utc(),
            payload: json!({"k": "v"}),
        }
    }

    #[tokio::test]
    async fn append_and_replay() {
        let dir = tempdir().unwrap();
        let log = EventLog::open(dir.path().join("e.jsonl"), 65536)
            .await
            .unwrap();
        log.append(&env()).await.unwrap();
        log.append(&env()).await.unwrap();
        let off = log.snapshot_offset().await.unwrap();
        let replay = log.replay_since(None, off).await.unwrap();
        assert_eq!(replay.len(), 2);
    }

    #[tokio::test]
    async fn replay_since_filters_old() {
        let dir = tempdir().unwrap();
        let log = EventLog::open(dir.path().join("e.jsonl"), 65536)
            .await
            .unwrap();
        let mut e1 = env();
        e1.ts = time::macros::datetime!(2026-01-01 00:00:00 UTC);
        let mut e2 = env();
        e2.ts = time::macros::datetime!(2026-06-01 00:00:00 UTC);
        log.append(&e1).await.unwrap();
        log.append(&e2).await.unwrap();
        let off = log.snapshot_offset().await.unwrap();
        let replay = log
            .replay_since(Some(time::macros::datetime!(2026-03-01 00:00:00 UTC)), off)
            .await
            .unwrap();
        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].ts, e2.ts);
    }

    #[tokio::test]
    async fn corrupt_lines_are_skipped() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("e.jsonl");
        tokio::fs::write(&path, "not-json\n").await.unwrap();
        let log = EventLog::open(&path, 65536).await.unwrap();
        log.append(&env()).await.unwrap();
        let off = log.snapshot_offset().await.unwrap();
        let replay = log.replay_since(None, off).await.unwrap();
        assert_eq!(replay.len(), 1);
    }
}
