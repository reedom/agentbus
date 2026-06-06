//! Spec section 11: N OS processes hammering send on one store. Assert no
//! lost envelopes, no duplicate seq, intact JSONL inbox lines.

use std::process::Command;

const WORKERS: u64 = 8;
const SENDS_PER_WORKER: u64 = 25;

#[test]
fn concurrent_senders_lose_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();

    let status = Command::new(env!("CARGO_BIN_EXE_agentbus"))
        .env("AGENTBUS_DIR", &dir)
        .args(["register", "sink", "--persistent"])
        .status()
        .unwrap();
    assert!(status.success());

    let mut handles = Vec::new();
    for w in 0..WORKERS {
        let dir = dir.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..SENDS_PER_WORKER {
                use std::io::Write;
                let mut child = Command::new(env!("CARGO_BIN_EXE_agentbus"))
                    .env("AGENTBUS_DIR", &dir)
                    .args(["send", "sink"])
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::inherit())
                    .spawn()
                    .unwrap();
                let payload = format!(r#"{{"w":{w},"i":{i}}}"#);
                child.stdin.as_mut().unwrap().write_all(payload.as_bytes()).unwrap();
                drop(child.stdin.take());
                assert!(child.wait().unwrap().success(), "send w={w} i={i}");
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    let total = WORKERS * SENDS_PER_WORKER;

    // Inbox: every line parses, count matches.
    let inbox = std::fs::read_to_string(dir.join("inbox/sink.jsonl")).unwrap();
    let envelopes: Vec<serde_json::Value> = inbox
        .lines()
        .map(|l| serde_json::from_str(l).expect("intact JSONL line"))
        .collect();
    assert_eq!(envelopes.len() as u64, total);

    // event_log: distinct seq per envelope, no gaps lost.
    let out = Command::new(env!("CARGO_BIN_EXE_agentbus"))
        .env("AGENTBUS_DIR", &dir)
        .args(["events"])
        .output()
        .unwrap();
    let seqs: Vec<i64> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["seq"].as_i64().unwrap())
        .collect();
    assert_eq!(seqs.len() as u64, total);
    let mut deduped = seqs.clone();
    deduped.dedup();
    assert_eq!(seqs, deduped, "duplicate seq");
}
