//! Sender-executed on_delivery hooks (spec 6.5): sh -c, 15 s timeout,
//! non-fatal failure. This is how integrators wake agents without a daemon.

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::json;

use crate::envelope::Envelope;

use super::Store;

const HOOK_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Serialize)]
pub struct HookOutcome {
    pub ok: bool,
    pub detail: String,
}

impl Store {
    /// Best-effort: the send/ask already succeeded (the envelope is durably
    /// spooled); a failure only produces a bus.delivery_hook_failed event.
    pub(crate) fn run_delivery_hook(
        &mut self,
        cmd: &str,
        instance: &str,
        env: &Envelope,
    ) -> HookOutcome {
        let envs = [
            ("AGENTBUS_INSTANCE", instance.to_string()),
            ("AGENTBUS_ENVELOPE_ID", env.id.clone()),
            ("AGENTBUS_KIND", env.kind.as_str().to_string()),
            ("AGENTBUS_FROM", env.from.clone()),
        ];
        let outcome = run(cmd, &envs, HOOK_TIMEOUT);
        self.record_failure(instance, Some(&env.id), &outcome);
        outcome
    }

    /// Sweep re-fire (spec 6.8): no specific envelope, instance only.
    pub(crate) fn run_sweep_hook(&mut self, cmd: &str, instance: &str) -> HookOutcome {
        let envs = [("AGENTBUS_INSTANCE", instance.to_string())];
        let outcome = run(cmd, &envs, HOOK_TIMEOUT);
        self.record_failure(instance, None, &outcome);
        outcome
    }

    fn record_failure(&mut self, instance: &str, envelope_id: Option<&str>, outcome: &HookOutcome) {
        if outcome.ok {
            return;
        }
        let payload = json!({
            "event": "bus.delivery_hook_failed",
            "instance": instance,
            "envelope_id": envelope_id,
            "detail": outcome.detail,
        });
        if let Err(e) = self.publish_event("bus", payload) {
            tracing::warn!(error = %e, "could not record bus.delivery_hook_failed event");
        }
        tracing::warn!(instance, detail = %outcome.detail, "on_delivery hook failed");
    }
}

fn run(cmd: &str, envs: &[(&str, String)], timeout: Duration) -> HookOutcome {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // The child inherits the sender's full environment; the AGENTBUS_* vars
    // are additive. Hooks run as the same user, so this grants nothing the
    // user lacks (spec section 9).
    for (k, v) in envs {
        command.env(k, v);
    }
    match command.spawn() {
        Ok(mut child) => wait_with_timeout(&mut child, timeout),
        Err(e) => HookOutcome {
            ok: false,
            detail: format!("spawn: {e}"),
        },
    }
}

fn wait_with_timeout(child: &mut Child, timeout: Duration) -> HookOutcome {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                return HookOutcome {
                    ok: true,
                    detail: "ok".into(),
                };
            }
            Ok(Some(status)) => {
                return HookOutcome {
                    ok: false,
                    detail: format!("exit: {status}"),
                };
            }
            Ok(None) => {}
            Err(e) => {
                // try_wait failed, not the child: it may still be running.
                let _ = child.kill();
                let _ = child.wait();
                return HookOutcome {
                    ok: false,
                    detail: format!("wait: {e}"),
                };
            }
        }
        if deadline <= Instant::now() {
            let _ = child.kill();
            let _ = child.wait();
            return HookOutcome {
                ok: false,
                detail: "timeout".into(),
            };
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::store::testutil::test_store;
    use crate::store::{EventFilter, RegisterOpts};

    fn register_with_hook(store: &mut crate::store::Store, cmd: &str) {
        let opts = RegisterOpts {
            persistent: true,
            on_delivery: Some(cmd.to_string()),
            ..Default::default()
        };
        store.register("bob", &opts).unwrap();
    }

    #[test]
    fn hook_runs_with_envelope_env_vars() {
        let (tmp, mut store) = test_store();
        let marker = tmp.path().join("hook.out");
        let cmd = format!(
            "echo \"$AGENTBUS_INSTANCE $AGENTBUS_KIND $AGENTBUS_FROM $AGENTBUS_ENVELOPE_ID\" > {}",
            marker.display()
        );
        register_with_hook(&mut store, &cmd);
        let delivered = store.send("alice", "bob", json!({})).unwrap();
        let hook = delivered.hook.expect("hook ran");
        assert!(hook.ok, "{}", hook.detail);
        let written = std::fs::read_to_string(&marker).unwrap();
        assert_eq!(
            written.trim(),
            format!("bob message alice {}", delivered.envelope.id)
        );
    }

    #[test]
    fn hook_failure_is_nonfatal_and_emits_event() {
        let (_tmp, mut store) = test_store();
        register_with_hook(&mut store, "exit 3");
        let delivered = store.send("alice", "bob", json!({})).unwrap();
        let hook = delivered.hook.expect("hook ran");
        assert!(!hook.ok);
        // The send still succeeded: envelope is spooled.
        assert_eq!(store.check_inbox("bob").unwrap().len(), 1);
        // And a bus.delivery_hook_failed event was logged.
        let page = store.events_since(0, 100, &EventFilter::default()).unwrap();
        let failed = page.events.iter().any(|e| {
            e.envelope.from == "bus" && e.envelope.payload["event"] == "bus.delivery_hook_failed"
        });
        assert!(failed);
    }

    #[test]
    fn wait_with_timeout_kills_overrunning_hook() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let outcome = super::wait_with_timeout(&mut child, std::time::Duration::from_millis(200));
        assert!(!outcome.ok);
        assert_eq!(outcome.detail, "timeout");
    }
}
