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
    // Abrupt kills and the I/O-error early returns above skip this; the
    // pid-liveness sweep reclaims those rows (spec section 12, item 4).
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
