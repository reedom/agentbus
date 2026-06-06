//! Ordered event log (spec 6.4): one table, transactional seq, cursor reads.
//! The v0.1 snapshot-replay-then-live guarantee is trivial here: no gap and
//! no duplicate by construction.

use rusqlite::params;
use serde::Serialize;
use serde_json::Value;

use crate::envelope::{Envelope, Kind};

use super::{append_event, new_envelope, Store, StoreError};

#[derive(Debug, Serialize)]
pub struct SeqEnvelope {
    pub seq: i64,
    pub envelope: Envelope,
}

/// One page of event_log rows. `cursor` advances past every SCANNED row,
/// including filtered-out ones, so follow loops never rescan.
#[derive(Debug)]
pub struct EventsPage {
    pub events: Vec<SeqEnvelope>,
    pub cursor: i64,
}

#[derive(Debug, Default, Clone)]
pub struct EventFilter {
    /// Envelopes whose `from` or `to` equals this id.
    pub instance: Option<String>,
    pub kind: Option<Kind>,
    /// Only envelopes addressed TO this id (watch mode, spec 6.7).
    pub to: Option<String>,
}

impl Store {
    pub fn publish_event(&mut self, from: &str, payload: Value) -> Result<String, StoreError> {
        let env = new_envelope(Kind::Event, from, None, payload);
        env.validate()?;
        self.with_tx(|tx| append_event(tx, &env))?;
        Ok(env.id)
    }

    /// Rows after `after_seq`, oldest first; filters apply post-deserialize.
    pub fn events_since(
        &self,
        after_seq: i64,
        limit: usize,
        filter: &EventFilter,
    ) -> Result<EventsPage, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT seq, envelope FROM event_log WHERE ?1 < seq ORDER BY seq LIMIT ?2")?;
        let rows = stmt.query_map(params![after_seq, limit as i64], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut page = EventsPage {
            events: Vec::new(),
            cursor: after_seq,
        };
        for row in rows {
            let (seq, text) = row?;
            page.cursor = seq;
            let Ok(envelope) = serde_json::from_str::<Envelope>(&text) else {
                tracing::warn!(seq, "skipping corrupt event_log row");
                continue;
            };
            if matches_filter(filter, &envelope) {
                page.events.push(SeqEnvelope { seq, envelope });
            }
        }
        Ok(page)
    }

    pub fn max_seq(&self) -> Result<i64, StoreError> {
        Ok(self
            .conn
            .query_row("SELECT COALESCE(MAX(seq), 0) FROM event_log", [], |r| {
                r.get(0)
            })?)
    }
}

fn matches_filter(filter: &EventFilter, env: &Envelope) -> bool {
    if let Some(kind) = &filter.kind {
        if env.kind != *kind {
            return false;
        }
    }
    if let Some(to) = &filter.to {
        if env.to.as_deref() != Some(to.as_str()) {
            return false;
        }
    }
    if let Some(id) = &filter.instance {
        let hit = env.from == *id || env.to.as_deref() == Some(id.as_str());
        if !hit {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::EventFilter;
    use crate::envelope::Kind;
    use crate::store::testutil::test_store;

    #[test]
    fn publish_assigns_monotonic_seq() {
        let (_tmp, mut store) = test_store();
        store.publish_event("alice", json!({"a": 1})).unwrap();
        store.publish_event("alice", json!({"a": 2})).unwrap();
        let page = store.events_since(0, 100, &EventFilter::default()).unwrap();
        assert_eq!(page.events.len(), 2);
        assert!(page.events[0].seq < page.events[1].seq);
        assert_eq!(page.cursor, page.events[1].seq);
    }

    #[test]
    fn cursor_resumes_without_gap_or_duplicate() {
        let (_tmp, mut store) = test_store();
        for i in 0..5 {
            store.publish_event("alice", json!({ "i": i })).unwrap();
        }
        let first = store.events_since(0, 2, &EventFilter::default()).unwrap();
        let rest = store
            .events_since(first.cursor, 100, &EventFilter::default())
            .unwrap();
        assert_eq!(first.events.len() + rest.events.len(), 5);
        let seqs: Vec<i64> = first
            .events
            .iter()
            .chain(rest.events.iter())
            .map(|e| e.seq)
            .collect();
        let mut deduped = seqs.clone();
        deduped.dedup();
        assert_eq!(seqs, deduped);
    }

    #[test]
    fn filters_match_kind_and_instance() {
        let (_tmp, mut store) = test_store();
        store.publish_event("alice", json!({"x": 1})).unwrap();
        store.publish_event("bob", json!({"x": 2})).unwrap();
        let alice_only = EventFilter {
            instance: Some("alice".into()),
            ..Default::default()
        };
        let page = store.events_since(0, 100, &alice_only).unwrap();
        assert_eq!(page.events.len(), 1);
        assert_eq!(page.events[0].envelope.from, "alice");
        // The cursor still advances past filtered-out rows.
        assert_eq!(page.cursor, store.max_seq().unwrap());
        let events_only = EventFilter {
            kind: Some(Kind::Event),
            ..Default::default()
        };
        assert_eq!(
            store
                .events_since(0, 100, &events_only)
                .unwrap()
                .events
                .len(),
            2
        );
    }

    #[test]
    fn to_filter_selects_addressed_envelopes() {
        let (_tmp, mut store) = test_store();
        store.publish_event("alice", json!({"x": 1})).unwrap();
        let to_bob = EventFilter {
            to: Some("bob".into()),
            ..Default::default()
        };
        assert!(store
            .events_since(0, 100, &to_bob)
            .unwrap()
            .events
            .is_empty());
    }

    #[test]
    fn max_seq_is_zero_on_empty_log() {
        let (_tmp, store) = test_store();
        assert_eq!(store.max_seq().unwrap(), 0);
    }
}
