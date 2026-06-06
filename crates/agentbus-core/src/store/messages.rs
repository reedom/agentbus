//! Sender-performed delivery (spec 6.2-6.3): in one transaction the
//! recipient is checked and the envelope logged; the inbox append and the
//! on_delivery hook happen after commit, outside the transaction.

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
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::envelope::Kind;
    use crate::store::testutil::test_store;
    use crate::store::{EventFilter, RegisterOpts, StoreError};

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
