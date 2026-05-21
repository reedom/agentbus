mod tools;
mod uds_client;

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Stdin};
use tracing_subscriber::EnvFilter;

use uds_client::{ClientError, UdsClient};

fn socket_path() -> PathBuf {
    if let Ok(p) = std::env::var("MCP_BUS_SOCKET") {
        return PathBuf::from(p);
    }
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("mcp-bus.sock")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let client = Arc::new(UdsClient::new(socket_path()));
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    run(stdin, stdout, client).await
}

async fn run(
    stdin: Stdin,
    mut stdout: tokio::io::Stdout,
    client: Arc<UdsClient>,
) -> anyhow::Result<()> {
    let mut lines = BufReader::new(stdin).lines();
    while let Some(line) = lines.next_line().await? {
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
        let resp = handle(client.clone(), method, params).await;
        let envelope = match resp {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": e}),
        };
        let mut buf = serde_json::to_vec(&envelope)?;
        buf.push(b'\n');
        stdout.write_all(&buf).await?;
        stdout.flush().await?;
    }
    Ok(())
}

async fn handle(
    client: Arc<UdsClient>,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, serde_json::Value> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": "2024-11-05",
            "serverInfo": {"name": "mcp-bus-stdio", "version": env!("CARGO_PKG_VERSION")},
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
            match tools::call(&client, name, args).await {
                Ok(result) => Ok(json!({
                    "content": [{
                        "type": "text",
                        "text": serde_json::to_string(&result).unwrap()
                    }]
                })),
                Err(ClientError::Unavailable(msg)) => Err(json!({
                    "code": -32000,
                    "message": "daemon_unavailable",
                    "data": msg
                })),
                Err(ClientError::Rpc {
                    code,
                    message,
                    data,
                }) => Err(json!({"code": code, "message": message, "data": data})),
                Err(e) => Err(json!({"code": -32603, "message": e.to_string()})),
            }
        }
        _ => Err(json!({"code": -32601, "message": "method not found"})),
    }
}
