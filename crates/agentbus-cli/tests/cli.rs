//! Golden-output tests for each verb against a temp store (spec section 11).

use std::process::{Command, Output};

fn agentbus(dir: &std::path::Path, args: &[&str], stdin: Option<&str>) -> Output {
    use std::io::Write;
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_agentbus"));
    cmd.env("AGENTBUS_DIR", dir).args(args);
    match stdin {
        Some(input) => {
            cmd.stdin(std::process::Stdio::piped());
            cmd.stdout(std::process::Stdio::piped());
            cmd.stderr(std::process::Stdio::piped());
            let mut child = cmd.spawn().unwrap();
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
            child.wait_with_output().unwrap()
        }
        None => cmd.output().unwrap(),
    }
}

fn stdout_json(out: &Output) -> serde_json::Value {
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn register_ls_unregister() {
    let tmp = tempfile::tempdir().unwrap();
    let reg = agentbus(
        tmp.path(),
        &["register", "bot", "--persistent", "--on-delivery", "true"],
        None,
    );
    assert_eq!(stdout_json(&reg)["ok"], true);
    let ls = stdout_json(&agentbus(tmp.path(), &["ls"], None));
    assert_eq!(ls["instances"][0]["id"], "bot");
    assert_eq!(ls["instances"][0]["persistent"], true);
    let un = stdout_json(&agentbus(tmp.path(), &["unregister", "bot"], None));
    assert_eq!(un["ok"], true);
}

#[test]
fn send_then_check_inbox() {
    let tmp = tempfile::tempdir().unwrap();
    agentbus(tmp.path(), &["register", "bob", "--persistent"], None);
    let sent = stdout_json(&agentbus(tmp.path(), &["send", "bob"], Some(r#"{"hi":1}"#)));
    assert!(sent["id"].as_str().unwrap().starts_with("msg_"));
    let inbox = stdout_json(&agentbus(tmp.path(), &["check-inbox", "bob"], None));
    assert_eq!(inbox["envelopes"][0]["payload"]["hi"], 1);
}

#[test]
fn send_to_unknown_prints_coded_error() {
    let tmp = tempfile::tempdir().unwrap();
    let out = agentbus(tmp.path(), &["send", "ghost"], Some("{}"));
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("error[unknown_instance]"),
        "stderr: {stderr}"
    );
}

#[test]
fn ask_timeout_exits_2_with_ask_result_hint_then_late_reply_retrievable() {
    let tmp = tempfile::tempdir().unwrap();
    agentbus(tmp.path(), &["register", "bob", "--persistent"], None);
    let out = agentbus(
        tmp.path(),
        &["ask", "bob", "--timeout-ms", "150"],
        Some("{}"),
    );
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("agentbus ask-result "), "stderr: {stderr}");
    let request_id = stderr
        .split("agentbus ask-result ")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();
    agentbus(
        tmp.path(),
        &["reply", &request_id, "bob"],
        Some(r#"{"late":1}"#),
    );
    let res = stdout_json(&agentbus(tmp.path(), &["ask-result", &request_id], None));
    assert_eq!(res["status"], "replied");
    assert_eq!(res["payload"]["late"], 1);
}

#[test]
fn events_lists_seq_envelope_lines() {
    let tmp = tempfile::tempdir().unwrap();
    agentbus(tmp.path(), &["publish"], Some(r#"{"e":1}"#));
    agentbus(tmp.path(), &["publish"], Some(r#"{"e":2}"#));
    let out = agentbus(tmp.path(), &["events"], None);
    assert!(out.status.success());
    let lines: Vec<serde_json::Value> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["seq"], 1);
    assert_eq!(lines[1]["envelope"]["payload"]["e"], 2);
}

#[test]
fn await_returns_empty_on_timeout() {
    let tmp = tempfile::tempdir().unwrap();
    agentbus(tmp.path(), &["register", "bob", "--persistent"], None);
    let out = stdout_json(&agentbus(
        tmp.path(),
        &["await", "bob", "--timeout-ms", "120"],
        None,
    ));
    assert_eq!(out["envelopes"], serde_json::json!([]));
}

#[test]
fn sweep_prints_report() {
    let tmp = tempfile::tempdir().unwrap();
    let out = stdout_json(&agentbus(tmp.path(), &["sweep"], None));
    assert_eq!(out["dead_instances"], serde_json::json!([]));
    assert_eq!(out["purged_inboxes"], serde_json::json!([]));
}
