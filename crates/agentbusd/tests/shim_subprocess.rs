mod common;
use common::*;
use serde_json::{json, Value};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};

fn shim_bin() -> std::path::PathBuf {
    let mut p = std::env::current_exe().unwrap();
    // target/debug/deps/<test>-<hash> -> target/debug/agentbus-stdio
    p.pop();
    p.pop();
    p.push("agentbus-stdio");
    p
}

async fn call(
    child_stdin: &mut ChildStdin,
    reader: &mut BufReader<ChildStdout>,
    id: u64,
    method: &str,
    params: Value,
) -> Value {
    let req = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
    let mut buf = serde_json::to_vec(&req).unwrap();
    buf.push(b'\n');
    child_stdin.write_all(&buf).await.unwrap();
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut line))
        .await
        .expect("response did not arrive in time")
        .expect("read_line failed");
    serde_json::from_str(&line).unwrap()
}

/// Convenience: invoke a `tools/call` and parse the inner JSON text payload.
async fn tool_call(
    child_stdin: &mut ChildStdin,
    reader: &mut BufReader<ChildStdout>,
    id: u64,
    name: &str,
    arguments: Value,
) -> Value {
    let resp = call(
        child_stdin,
        reader,
        id,
        "tools/call",
        json!({"name": name, "arguments": arguments}),
    )
    .await;
    let text = resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("tool {name} returned non-text result: {resp}"))
        .to_string();
    serde_json::from_str(&text).unwrap()
}

#[tokio::test]
async fn shim_lists_tools_and_round_trips() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let app = spawn_with_ipc().await;

        // Spawn two shim subprocesses that share the daemon's UDS socket.
        let mut alice = Command::new(shim_bin())
            .env("AGENTBUS_SOCKET", &app.ipc_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut bob = Command::new(shim_bin())
            .env("AGENTBUS_SOCKET", &app.ipc_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();

        let mut alice_stdin = alice.stdin.take().unwrap();
        let mut alice_reader = BufReader::new(alice.stdout.take().unwrap());
        let mut bob_stdin = bob.stdin.take().unwrap();
        let mut bob_reader = BufReader::new(bob.stdout.take().unwrap());

        // Sanity-check that tools/list works through the shim before exercising
        // the full ask/reply roundtrip.
        let list = call(
            &mut alice_stdin,
            &mut alice_reader,
            1,
            "tools/list",
            json!({}),
        )
        .await;
        let tools = list["result"]["tools"].as_array().unwrap();
        assert!(tools.iter().any(|t| t["name"] == "register"));
        assert!(tools.iter().any(|t| t["name"] == "ask"));
        assert!(tools.iter().any(|t| t["name"] == "reply"));

        // Register both instances against the shared daemon.
        let alice_reg = tool_call(
            &mut alice_stdin,
            &mut alice_reader,
            2,
            "register",
            json!({"instance_id": "alice"}),
        )
        .await;
        assert_eq!(alice_reg["ok"], true);

        let bob_reg = tool_call(
            &mut bob_stdin,
            &mut bob_reader,
            2,
            "register",
            json!({"instance_id": "bob"}),
        )
        .await;
        assert_eq!(bob_reg["ok"], true);

        // Drive alice's `ask` and bob's `await_message` in parallel: the ask
        // blocks until bob's reply arrives, and await_message returns as soon
        // as the ask hits bob's mailbox.
        let ask_fut = async {
            tool_call(
                &mut alice_stdin,
                &mut alice_reader,
                3,
                "ask",
                json!({
                    "from": "alice",
                    "to": "bob",
                    "payload": {"q": "ping"},
                    "timeout_ms": 2000,
                }),
            )
            .await
        };

        let bob_fut = async {
            // Wait for the ask envelope, then send back a reply tied to the
            // original request_id. We use the envelope's own `id` field as the
            // request id since that's what `router.reply` matches against.
            let inbound = tool_call(
                &mut bob_stdin,
                &mut bob_reader,
                3,
                "await_message",
                json!({"instance_id": "bob", "timeout_ms": 2000}),
            )
            .await;
            let envelope = &inbound["envelope"];
            assert_eq!(envelope["kind"], "ask");
            assert_eq!(envelope["from"], "alice");
            assert_eq!(envelope["payload"]["q"], "ping");
            let request_id = envelope["id"].as_str().unwrap().to_string();
            let from = envelope["from"].as_str().unwrap().to_string();

            let reply = tool_call(
                &mut bob_stdin,
                &mut bob_reader,
                4,
                "reply",
                json!({
                    "from": "bob",
                    "to": from,
                    "request_id": request_id,
                    "payload": {"a": "pong"},
                }),
            )
            .await;
            assert_eq!(reply["ok"], true);
        };

        let (ask_result, _) = tokio::join!(ask_fut, bob_fut);
        assert_eq!(ask_result["reply"]["kind"], "reply");
        assert_eq!(ask_result["reply"]["from"], "bob");
        assert_eq!(ask_result["reply"]["payload"]["a"], "pong");

        // Clean shutdown: closing stdin makes the shims exit their read loops.
        drop(alice_stdin);
        drop(bob_stdin);
        for child in [&mut alice, &mut bob] {
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
        }
    })
    .await
    .expect("shim roundtrip test timed out");
}
