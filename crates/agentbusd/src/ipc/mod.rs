//! Unix-socket JSON-RPC IPC server for shim/CLI clients.

pub mod handler;
pub mod proto;

use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

use self::handler::{dispatch, make_response, ConnCtx};
use self::proto::RpcRequest;
use crate::state::AppState;

pub async fn serve(state: AppState, path: impl AsRef<Path>) -> std::io::Result<()> {
    let path = path.as_ref().to_path_buf();
    if path.exists() {
        let _ = tokio::fs::remove_file(&path).await;
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            tokio::fs::create_dir_all(parent).await?;
        }
    }
    let listener = UnixListener::bind(&path)?;
    // Tighten perms so only the owner can connect.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = tokio::fs::metadata(&path).await?.permissions();
        perms.set_mode(0o600);
        tokio::fs::set_permissions(&path, perms).await?;
    }
    tracing::info!(?path, "ipc listening");
    loop {
        let (stream, _) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(handle_conn(state, stream));
    }
}

async fn handle_conn(state: AppState, stream: UnixStream) {
    let owner = state.registry.issue_owner_token();
    let mut ctx = ConnCtx {
        owner,
        claimed_ids: Vec::new(),
    };
    let (rd, mut wr) = stream.into_split();
    let mut lines = BufReader::new(rd).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.is_empty() {
            continue;
        }
        let req: RpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "ipc bad json");
                continue;
            }
        };
        let res = dispatch(&state, &mut ctx, &req.method, &req.params).await;
        let resp = make_response(req.id, res);
        let mut buf = match serde_json::to_vec(&resp) {
            Ok(b) => b,
            Err(e) => {
                tracing::error!(error = %e, "ipc serialize response failed");
                break;
            }
        };
        buf.push(b'\n');
        if wr.write_all(&buf).await.is_err() {
            break;
        }
    }
    // Connection dropped: unregister everything this owner claimed and
    // cancel any in-flight asks targeting those instances.
    let closed = state.registry.unregister_owner(owner).await;
    for id in &closed {
        state.router.cancel_pending_for(id).await;
    }
    tracing::info!(owner, ?closed, "ipc conn closed; auto-unregistered");
}
