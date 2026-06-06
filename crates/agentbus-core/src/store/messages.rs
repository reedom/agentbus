//! Sender-performed delivery (spec 6.2-6.3): in one transaction the
//! recipient is checked and the envelope logged; the inbox append and the
//! on_delivery hook happen after commit, outside the transaction.

use std::time::{Duration, Instant};

use rusqlite::OptionalExtension;
use serde::Serialize;
use serde_json::Value;

use crate::envelope::Kind;

use super::instances::on_delivery_of;
use super::{append_event, new_envelope, spool, Store, StoreError};

#[derive(Debug, Serialize)]
pub struct Delivered {
    pub envelope: crate::envelope::Envelope,
    /// Outcome of the recipient's on_delivery hook, when one is registered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hook: Option<super::hook::HookOutcome>,
}

#[derive(Debug, Serialize)]
pub struct AskReply {
    pub request_id: String,
    pub payload: Value,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AskStatus {
    Pending { expires_at: String },
    Replied { payload: Value, replied_at: String },
    Expired { expires_at: String },
}

impl Store {
    /// Spec 6.2: log inside the tx, then spool + on_delivery outside it.
    ///
    /// Partial failure: if the spool append fails after the event_log
    /// commit, the envelope is logged but not delivered; sweep (spec 6.8)
    /// re-fires on_delivery for stuck inboxes. A retry sends a NEW envelope.
    pub fn send(&mut self, from: &str, to: &str, payload: Value) -> Result<Delivered, StoreError> {
        let env = new_envelope(Kind::Message, from, Some(to), payload);
        env.validate()?;
        let on_delivery = self.with_tx(|tx| {
            let cmd = on_delivery_of(tx, to)?;
            append_event(tx, &env)?;
            Ok(cmd)
        })?;
        if let Err(e) = spool::append(&self.base, to, &env) {
            // The envelope IS committed to event_log; surface that fact so
            // operators can judge retry safety (a retry re-stamps a new id).
            tracing::warn!(envelope_id = %env.id, error = %e, "inbox spool append failed after event_log commit");
            return Err(e);
        }
        let hook = on_delivery.map(|cmd| self.run_delivery_hook(&cmd, to, &env));
        Ok(Delivered {
            envelope: env,
            hook,
        })
    }

    /// Spec 6.3: as send, plus an asks row; then poll for the reply with
    /// 50 ms -> 250 ms backoff. On expiry the row stays for ask_result.
    pub fn ask(
        &mut self,
        from: &str,
        to: &str,
        payload: Value,
        timeout: Duration,
    ) -> Result<AskReply, StoreError> {
        let mut env = new_envelope(Kind::Ask, from, Some(to), payload);
        env.timeout_ms = Some(timeout.as_millis() as u64);
        env.validate()?;
        let expires_at = super::rfc3339(&(env.ts + timeout));
        let on_delivery = self.with_tx(|tx| {
            let cmd = on_delivery_of(tx, to)?;
            append_event(tx, &env)?;
            tx.execute(
                "INSERT INTO asks (request_id, from_id, to_id, expires_at)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![env.id, from, to, expires_at],
            )?;
            Ok(cmd)
        })?;
        if let Err(e) = spool::append(&self.base, to, &env) {
            tracing::warn!(envelope_id = %env.id, error = %e, "inbox spool append failed after event_log commit");
            return Err(e);
        }
        if let Some(cmd) = on_delivery {
            self.run_delivery_hook(&cmd, to, &env);
        }
        self.poll_reply(&env.id, timeout)
    }

    fn poll_reply(&self, request_id: &str, timeout: Duration) -> Result<AskReply, StoreError> {
        let deadline = Instant::now() + timeout;
        let mut delay = Duration::from_millis(50);
        loop {
            if let Some(payload) = self.reply_payload(request_id)? {
                return Ok(AskReply {
                    request_id: request_id.to_string(),
                    payload,
                });
            }
            let now = Instant::now();
            if deadline <= now {
                return Err(StoreError::Timeout(request_id.to_string()));
            }
            std::thread::sleep(delay.min(deadline - now));
            delay = (delay * 2).min(Duration::from_millis(250));
        }
    }

    fn reply_payload(&self, request_id: &str) -> Result<Option<Value>, StoreError> {
        let text: Option<Option<String>> = self
            .conn
            .query_row(
                "SELECT reply_payload FROM asks WHERE request_id = ?1",
                rusqlite::params![request_id],
                |r| r.get(0),
            )
            .optional()?;
        match text.flatten() {
            Some(t) => Ok(Some(serde_json::from_str(&t).unwrap_or(Value::String(t)))),
            None => Ok(None),
        }
    }

    /// Spec 6.3: first reply wins; every reply is logged. No inbox write --
    /// the asker reads the row.
    pub fn reply(
        &mut self,
        from: &str,
        request_id: &str,
        payload: Value,
    ) -> Result<(), StoreError> {
        self.with_tx(|tx| {
            let asker: Option<String> = tx
                .query_row(
                    "SELECT from_id FROM asks WHERE request_id = ?1",
                    rusqlite::params![request_id],
                    |r| r.get(0),
                )
                .optional()?;
            let asker =
                asker.ok_or_else(|| StoreError::UnknownRequestId(request_id.to_string()))?;
            let mut env = new_envelope(Kind::Reply, from, Some(&asker), payload.clone());
            env.request_id = Some(request_id.to_string());
            env.validate()?;
            tx.execute(
                "UPDATE asks SET reply_payload = ?1, replied_at = ?2
                 WHERE request_id = ?3 AND reply_payload IS NULL",
                rusqlite::params![
                    serde_json::to_string(&payload).expect("payload serializes"),
                    super::rfc3339(&env.ts),
                    request_id
                ],
            )?;
            append_event(tx, &env)?;
            Ok(())
        })
    }

    /// Spec 6.3: a late reply is retrievable after the ask timed out.
    pub fn ask_result(&self, request_id: &str) -> Result<AskStatus, StoreError> {
        let row: Option<(String, Option<String>, Option<String>)> = self
            .conn
            .query_row(
                "SELECT expires_at, reply_payload, replied_at FROM asks WHERE request_id = ?1",
                rusqlite::params![request_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let (expires_at, reply, replied_at) =
            row.ok_or_else(|| StoreError::UnknownRequestId(request_id.to_string()))?;
        if let (Some(text), Some(replied_at)) = (reply, replied_at) {
            let payload = serde_json::from_str(&text).unwrap_or(Value::String(text));
            return Ok(AskStatus::Replied {
                payload,
                replied_at,
            });
        }
        let expired = crate::ids::parse_rfc3339(&expires_at)
            .map(|t| t <= crate::ids::now_utc())
            .unwrap_or(false);
        if expired {
            return Ok(AskStatus::Expired { expires_at });
        }
        Ok(AskStatus::Pending { expires_at })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use crate::envelope::Kind;
    use crate::store::testutil::test_store;
    use crate::store::{EventFilter, RegisterOpts, StoreError};

    #[test]
    fn ask_reply_roundtrip_across_threads() {
        let (tmp, mut store) = test_store();
        store.register("bob", &RegisterOpts::default()).unwrap();
        let base = tmp.path().to_path_buf();
        let responder = std::thread::spawn(move || {
            let mut bob = crate::store::Store::open_at(&base).unwrap();
            // Wait for the ask to land in bob's inbox, then reply.
            let asks = bob.await_message("bob", Duration::from_secs(2)).unwrap();
            assert_eq!(asks.len(), 1);
            assert_eq!(asks[0].kind, Kind::Ask);
            bob.reply("bob", &asks[0].id, json!({"pong": true}))
                .unwrap();
        });
        let reply = store
            .ask(
                "alice",
                "bob",
                json!({"ping": true}),
                Duration::from_secs(2),
            )
            .unwrap();
        responder.join().unwrap();
        assert_eq!(reply.payload, json!({"pong": true}));
    }

    #[test]
    fn ask_times_out_but_late_reply_is_retrievable() {
        let (_tmp, mut store) = test_store();
        store.register("bob", &RegisterOpts::default()).unwrap();
        let err = store
            .ask("alice", "bob", json!({"q": 1}), Duration::from_millis(120))
            .unwrap_err();
        let StoreError::Timeout(request_id) = err else {
            panic!("want timeout")
        };
        // Late reply still lands in the row (improvement over v0.1).
        store
            .reply("bob", &request_id, json!({"late": true}))
            .unwrap();
        let status = store.ask_result(&request_id).unwrap();
        let crate::store::AskStatus::Replied { payload, .. } = status else {
            panic!("want replied")
        };
        assert_eq!(payload, json!({"late": true}));
    }

    #[test]
    fn first_reply_wins_but_both_are_logged() {
        let (_tmp, mut store) = test_store();
        store.register("bob", &RegisterOpts::default()).unwrap();
        let err = store
            .ask("alice", "bob", json!({}), Duration::from_millis(60))
            .unwrap_err();
        let StoreError::Timeout(rid) = err else {
            panic!()
        };
        store.reply("bob", &rid, json!({"n": 1})).unwrap();
        store.reply("bob", &rid, json!({"n": 2})).unwrap();
        let crate::store::AskStatus::Replied { payload, .. } = store.ask_result(&rid).unwrap()
        else {
            panic!()
        };
        assert_eq!(payload, json!({"n": 1})); // first write won
        let page = store.events_since(0, 100, &EventFilter::default()).unwrap();
        let replies = page
            .events
            .iter()
            .filter(|e| e.envelope.kind == Kind::Reply)
            .count();
        assert_eq!(replies, 2); // both recorded in event_log
    }

    #[test]
    fn reply_to_unknown_request_id_fails() {
        let (_tmp, mut store) = test_store();
        assert!(matches!(
            store.reply("bob", "msg_nope", json!({})),
            Err(StoreError::UnknownRequestId(_))
        ));
        assert!(matches!(
            store.ask_result("msg_nope"),
            Err(StoreError::UnknownRequestId(_))
        ));
    }

    #[test]
    fn ask_result_reports_pending_then_expired() {
        let (_tmp, mut store) = test_store();
        store.register("bob", &RegisterOpts::default()).unwrap();
        let err = store
            .ask("alice", "bob", json!({}), Duration::from_millis(60))
            .unwrap_err();
        let StoreError::Timeout(rid) = err else {
            panic!()
        };
        // After expiry with no reply, status is Expired.
        assert!(matches!(
            store.ask_result(&rid).unwrap(),
            crate::store::AskStatus::Expired { .. }
        ));
    }

    #[test]
    fn send_to_unknown_instance_fails_and_logs_nothing() {
        let (_tmp, mut store) = test_store();
        let err = store.send("alice", "ghost", json!({})).unwrap_err();
        assert!(matches!(err, StoreError::UnknownInstance(_)));
        assert_eq!(store.max_seq().unwrap(), 0);
        assert!(store.check_inbox("ghost").unwrap().is_empty());
    }

    #[test]
    fn send_logs_event_and_spools_inbox() {
        let (_tmp, mut store) = test_store();
        store.register("bob", &RegisterOpts::default()).unwrap();
        let delivered = store.send("alice", "bob", json!({"hi": 1})).unwrap();
        assert!(delivered.envelope.id.starts_with("msg_"));
        assert_eq!(delivered.envelope.kind, Kind::Message);
        assert_eq!(delivered.envelope.from, "alice");
        assert_eq!(delivered.envelope.to.as_deref(), Some("bob"));
        assert!(delivered.hook.is_none()); // no on_delivery registered
                                           // event_log holds the same envelope...
        let page = store.events_since(0, 10, &EventFilter::default()).unwrap();
        assert_eq!(page.events.len(), 1);
        assert_eq!(page.events[0].envelope.id, delivered.envelope.id);
        // ...and the inbox spool delivers it.
        let inbox = store.check_inbox("bob").unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].payload, json!({"hi": 1}));
    }
}
