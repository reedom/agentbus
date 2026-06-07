//! Drives the shim binary over stdin/stdout exactly as an MCP client would.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct Shim {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Shim {
    fn spawn(dir: &std::path::Path) -> Shim {
        let mut child = Command::new(env!("CARGO_BIN_EXE_agentbus-stdio"))
            .env("AGENTBUS_DIR", dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Shim {
            child,
            stdin,
            stdout,
        }
    }

    fn call(&mut self, req: serde_json::Value) -> serde_json::Value {
        let mut line = serde_json::to_string(&req).unwrap();
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).unwrap();
        self.stdin.flush().unwrap();
        let mut resp = String::new();
        self.stdout.read_line(&mut resp).unwrap();
        serde_json::from_str(&resp).unwrap()
    }

    fn tool(&mut self, name: &str, args: serde_json::Value) -> serde_json::Value {
        let resp = self.call(serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": name, "arguments": args}
        }));
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("tool error: {resp}");
            });
        serde_json::from_str(text).unwrap()
    }
}

impl Drop for Shim {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn initialize_and_tools_list() {
    let tmp = tempfile::tempdir().unwrap();
    let mut shim = Shim::spawn(tmp.path());
    let init = shim.call(serde_json::json!({
        "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {}
    }));
    assert_eq!(init["result"]["serverInfo"]["name"], "agentbus-stdio");
    let list = shim.call(serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}
    }));
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "register",
            "unregister",
            "list_instances",
            "await_message",
            "check_inbox",
            "send",
            "ask",
            "reply",
            "publish_event"
        ]
    );
}

#[test]
fn register_send_check_inbox_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let mut shim = Shim::spawn(tmp.path());
    let reg = shim.tool("register", serde_json::json!({"instance_id": "bob"}));
    assert_eq!(reg["ok"], true);
    let sent = shim.tool(
        "send",
        serde_json::json!({"from": "alice", "to": "bob", "payload": {"hi": 1}}),
    );
    assert!(sent["id"].as_str().unwrap().starts_with("msg_"));
    let inbox = shim.tool("check_inbox", serde_json::json!({"instance_id": "bob"}));
    assert_eq!(inbox["envelopes"][0]["payload"]["hi"], 1);
}

#[test]
fn send_to_unknown_instance_returns_coded_error() {
    let tmp = tempfile::tempdir().unwrap();
    let mut shim = Shim::spawn(tmp.path());
    let resp = shim.call(serde_json::json!({
        "jsonrpc": "2.0", "id": 9, "method": "tools/call",
        "params": {"name": "send",
                   "arguments": {"from": "a", "to": "ghost", "payload": {}}}
    }));
    assert_eq!(resp["error"]["message"], "unknown_instance");
}
