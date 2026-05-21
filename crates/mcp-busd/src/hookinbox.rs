//! Writes envelopes addressed to instance `id` to $INBOX_DIR/<id>.jsonl.
//! Append-only; never truncated by the daemon.

use std::path::{Path, PathBuf};
use tokio::fs::{create_dir_all, OpenOptions};
use tokio::io::AsyncWriteExt;

use mcp_bus_core::envelope::Envelope;

pub struct HookInbox {
    dir: PathBuf,
}

impl HookInbox {
    pub async fn new(dir: impl AsRef<Path>) -> std::io::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        create_dir_all(&dir).await?;
        Ok(Self { dir })
    }

    pub async fn write_for(&self, instance_id: &str, env: &Envelope) -> std::io::Result<()> {
        // Reject path traversal: instance_id is validated upstream but double-check.
        if instance_id.contains('/') || instance_id.contains("..") {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "bad id",
            ));
        }
        let path = self.dir.join(format!("{instance_id}.jsonl"));
        let mut line = serde_json::to_vec(env).expect("envelope serializes");
        line.push(b'\n');
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
        f.write_all(&line).await
    }
}
