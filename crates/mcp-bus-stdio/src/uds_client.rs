//! Unix-domain socket JSON-RPC client used by the MCP stdio shim to talk to
//! the daemon. Maintains a single persistent connection with simple
//! reconnect-with-backoff semantics on first connect.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
use tokio::sync::{Mutex, MutexGuard};

/// Long-lived split connection state.
///
/// We keep the `BufReader` for the lifetime of the connection because
/// `read_line` may pre-buffer bytes past the `\n` delimiter; if we recreated
/// the reader per call those buffered bytes would be lost and subsequent
/// reads would silently desynchronize from the framing.
struct Conn {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
}

pub struct UdsClient {
    path: PathBuf,
    stream: Mutex<Option<Conn>>,
    next_id: std::sync::atomic::AtomicU64,
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("daemon unavailable: {0}")]
    Unavailable(String),
    #[error("rpc error {code}: {message}")]
    Rpc {
        code: i64,
        message: String,
        data: Option<serde_json::Value>,
    },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("decode: {0}")]
    Decode(#[from] serde_json::Error),
}

impl UdsClient {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            stream: Mutex::new(None),
            next_id: 0.into(),
        }
    }

    /// Ensure the guarded slot holds a live `Conn`, reconnecting if needed.
    ///
    /// Takes the guard by mutable reference so the caller can hold the lock
    /// across the connect-then-use sequence, eliminating the TOCTOU window
    /// where another task could clear the slot between checks.
    async fn ensure_connected_locked(
        &self,
        guard: &mut MutexGuard<'_, Option<Conn>>,
    ) -> Result<(), ClientError> {
        if guard.is_some() {
            return Ok(());
        }
        let mut delay = Duration::from_millis(200);
        // Cap retries; on failure return Unavailable so the shim surfaces a
        // structured MCP error rather than crashing the loop.
        for _ in 0..5 {
            match UnixStream::connect(&self.path).await {
                Ok(s) => {
                    let (rd, wr) = s.into_split();
                    **guard = Some(Conn {
                        reader: BufReader::new(rd),
                        writer: wr,
                    });
                    return Ok(());
                }
                Err(e) => {
                    tracing::warn!(error = %e, "uds connect failed; retrying");
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_secs(3));
                }
            }
        }
        Err(ClientError::Unavailable(format!(
            "could not connect to {}",
            self.path.display()
        )))
    }

    pub async fn call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, ClientError> {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let req = json!({"id": id, "method": method, "params": params});
        let mut buf = serde_json::to_vec(&req)?;
        buf.push(b'\n');

        // Acquire the connection lock exactly once for the whole
        // write+read cycle. This both eliminates the TOCTOU window around
        // reconnects and prevents interleaved responses on the shared
        // stream when multiple callers race.
        let mut g = self.stream.lock().await;
        self.ensure_connected_locked(&mut g).await?;

        let conn = match g.as_mut() {
            Some(c) => c,
            // Should be unreachable because ensure_connected_locked returned
            // Ok above, but stay defensive instead of panicking.
            None => return Err(ClientError::Unavailable("not connected".into())),
        };

        if let Err(e) = conn.writer.write_all(&buf).await {
            *g = None;
            return Err(ClientError::Unavailable(e.to_string()));
        }

        let mut line = String::new();
        match conn.reader.read_line(&mut line).await {
            Ok(0) => {
                *g = None;
                Err(ClientError::Unavailable("eof".into()))
            }
            Err(e) => {
                *g = None;
                Err(ClientError::Unavailable(e.to_string()))
            }
            Ok(_) => {
                let v: serde_json::Value = serde_json::from_str(&line)?;
                if let Some(err) = v.get("error") {
                    return Err(ClientError::Rpc {
                        code: err.get("code").and_then(|x| x.as_i64()).unwrap_or(0),
                        message: err
                            .get("message")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .into(),
                        data: err.get("data").cloned(),
                    });
                }
                Ok(v.get("result").cloned().unwrap_or(serde_json::Value::Null))
            }
        }
    }
}
