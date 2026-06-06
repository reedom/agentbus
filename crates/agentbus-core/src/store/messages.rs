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
    /// Set when the recipient registered an on_delivery hook (Task 6 wires
    /// the actual execution; until then this stays None).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hook: Option<super::hook_outcome_placeholder::HookOutcome>,
}

impl Store {
    pub fn send(&mut self, from: &str, to: &str, payload: Value) -> Result<Delivered, StoreError> {
        let env = new_envelope(Kind::Message, from, Some(to), payload);
        env.validate()?;
        let _on_delivery = self.with_tx(|tx| {
            let cmd = on_delivery_of(tx, to)?;
            append_event(tx, &env)?;
            Ok(cmd)
        })?;
        spool::append(&self.base, to, &env)?;
        Ok(Delivered {
            envelope: env,
            hook: None,
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
