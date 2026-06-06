//! Routes envelopes between mailboxes and resolves ask/reply correlations.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{oneshot, Mutex};

use crate::envelope::{Envelope, Kind};
use crate::ids::{new_envelope_id, now_utc};
use crate::registry::Registry;

#[derive(Debug, thiserror::Error)]
pub enum RouteError {
    #[error("unknown instance `{0}`")]
    UnknownInstance(String),
    #[error("ask timed out after {ms} ms")]
    AskTimeout { ms: u64 },
    #[error("instance `{0}` disconnected before replying")]
    InstanceDisconnected(String),
    #[error("unknown request_id `{0}`")]
    UnknownRequestId(String),
    #[error("envelope validation: {0}")]
    Validation(#[from] crate::envelope::ValidationError),
}

pub struct Router {
    registry: Arc<Registry>,
    pending: Mutex<HashMap<String, Pending>>,
}

struct Pending {
    tx: oneshot::Sender<Result<Envelope, RouteError>>,
    to: String,
    asker: String,
}

impl Router {
    pub fn new(registry: Arc<Registry>) -> Arc<Self> {
        Arc::new(Self {
            registry,
            pending: Mutex::new(HashMap::new()),
        })
    }

    /// Stamp `id` and `ts`, validate, push to recipient mailbox.
    pub async fn send(&self, mut env: Envelope) -> Result<String, RouteError> {
        env.id = new_envelope_id();
        env.ts = now_utc();
        env.validate()?;
        let to = env
            .to
            .as_deref()
            .ok_or_else(|| RouteError::UnknownInstance(String::new()))?;
        let rec = self
            .registry
            .lookup(to)
            .await
            .ok_or_else(|| RouteError::UnknownInstance(to.into()))?;
        let id = env.id.clone();
        rec.mailbox.push(env).await;
        Ok(id)
    }

    /// Send `ask` and wait for a matching `reply` or timeout.
    pub async fn ask(&self, mut env: Envelope, timeout: Duration) -> Result<Envelope, RouteError> {
        env.id = new_envelope_id();
        env.ts = now_utc();
        env.kind = Kind::Ask;
        env.validate()?;
        let to = env
            .to
            .clone()
            .ok_or_else(|| RouteError::UnknownInstance(String::new()))?;
        let rec = self
            .registry
            .lookup(&to)
            .await
            .ok_or_else(|| RouteError::UnknownInstance(to.clone()))?;

        let (tx, rx) = oneshot::channel();
        let req_id = env.id.clone();
        let asker = env.from.clone();
        {
            let mut g = self.pending.lock().await;
            g.insert(
                req_id.clone(),
                Pending {
                    tx,
                    to: to.clone(),
                    asker,
                },
            );
        }
        rec.mailbox.push(env).await;

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(res)) => res,
            Ok(Err(_canceled)) => Err(RouteError::InstanceDisconnected(to)),
            Err(_) => {
                self.pending.lock().await.remove(&req_id);
                Err(RouteError::AskTimeout {
                    ms: timeout.as_millis() as u64,
                })
            }
        }
    }

    /// Resolve a pending ask. Returns Err if no matching request_id.
    ///
    /// If `env.to` is missing, auto-fills it from the pending ask's asker
    /// so callers do not need to remember who they are replying to.
    pub async fn reply(&self, mut env: Envelope) -> Result<(), RouteError> {
        env.id = new_envelope_id();
        env.ts = now_utc();
        env.kind = Kind::Reply;
        let req_id = env
            .request_id
            .clone()
            .ok_or_else(|| RouteError::UnknownRequestId(String::new()))?;
        let mut g = self.pending.lock().await;
        let asker_opt = g.get(&req_id).map(|p| p.asker.clone());
        let Some(asker) = asker_opt else {
            return Err(RouteError::UnknownRequestId(req_id));
        };
        if env.to.is_none() {
            env.to = Some(asker);
        }
        env.validate()?;
        // Validation passed: consume the pending entry and deliver.
        let pending = g.remove(&req_id).expect("re-checked under same lock");
        drop(g);
        let _ = pending.tx.send(Ok(env));
        Ok(())
    }

    /// Cancel all pending asks whose target is `instance_id`.
    pub async fn cancel_pending_for(&self, instance_id: &str) {
        let mut g = self.pending.lock().await;
        let victims: Vec<String> = g
            .iter()
            .filter_map(|(k, v)| {
                if v.to == instance_id {
                    Some(k.clone())
                } else {
                    None
                }
            })
            .collect();
        for k in victims {
            if let Some(p) = g.remove(&k) {
                let _ =
                    p.tx.send(Err(RouteError::InstanceDisconnected(instance_id.into())));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::Kind;
    use serde_json::json;
    use time::macros::datetime;

    fn mk(kind: Kind, from: &str, to: Option<&str>) -> Envelope {
        Envelope {
            id: String::new(),
            kind,
            from: from.into(),
            to: to.map(Into::into),
            request_id: None,
            timeout_ms: None,
            ts: datetime!(2026-05-21 08:00:00 UTC),
            payload: json!({}),
        }
    }

    #[tokio::test]
    async fn send_to_unknown_fails() {
        let reg = Arc::new(Registry::new());
        let router = Router::new(reg);
        let err = router
            .send(mk(Kind::Message, "a", Some("ghost")))
            .await
            .unwrap_err();
        assert!(matches!(err, RouteError::UnknownInstance(_)));
    }

    #[tokio::test]
    async fn ask_reply_roundtrip() {
        let reg = Arc::new(Registry::new());
        let owner = reg.issue_owner_token();
        let bob = reg.register("bob", owner, 4).await.unwrap();
        let router = Router::new(reg);

        // Spawn a fake bob that drains and replies.
        let r2 = router.clone();
        tokio::spawn(async move {
            let req = bob.mailbox.pop().await.unwrap();
            let mut reply = mk(Kind::Reply, "bob", Some(&req.from));
            reply.request_id = Some(req.id.clone());
            reply.payload = json!({"ok": true});
            r2.reply(reply).await.unwrap();
        });

        let mut ask = mk(Kind::Ask, "alice", Some("bob"));
        ask.payload = json!({"q": "ping"});
        let reply = router.ask(ask, Duration::from_secs(1)).await.unwrap();
        assert_eq!(reply.payload, json!({"ok": true}));
    }

    #[tokio::test]
    async fn ask_times_out() {
        let reg = Arc::new(Registry::new());
        let owner = reg.issue_owner_token();
        reg.register("bob", owner, 4).await.unwrap();
        let router = Router::new(reg);

        let ask = mk(Kind::Ask, "alice", Some("bob"));
        let err = router
            .ask(ask, Duration::from_millis(30))
            .await
            .unwrap_err();
        assert!(matches!(err, RouteError::AskTimeout { .. }));
    }

    #[tokio::test]
    async fn cancel_pending_returns_disconnected() {
        let reg = Arc::new(Registry::new());
        let owner = reg.issue_owner_token();
        reg.register("bob", owner, 4).await.unwrap();
        let router = Router::new(reg);

        let r2 = router.clone();
        let handle = tokio::spawn(async move {
            let ask = mk(Kind::Ask, "alice", Some("bob"));
            r2.ask(ask, Duration::from_secs(5)).await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        router.cancel_pending_for("bob").await;
        let err = handle.await.unwrap().unwrap_err();
        assert!(matches!(err, RouteError::InstanceDisconnected(_)));
    }

    #[tokio::test]
    async fn unknown_request_id_rejected() {
        let reg = Arc::new(Registry::new());
        let router = Router::new(reg);
        let mut reply = mk(Kind::Reply, "bob", Some("alice"));
        reply.request_id = Some("nope".into());
        let err = router.reply(reply).await.unwrap_err();
        assert!(matches!(err, RouteError::UnknownRequestId(_)));
    }

    #[tokio::test]
    async fn reply_auto_fills_to_from_pending_asker() {
        let reg = Arc::new(Registry::new());
        let owner = reg.issue_owner_token();
        let bob = reg.register("bob", owner, 4).await.unwrap();
        let router = Router::new(reg);

        // Fake bob drains the ask, replies WITHOUT setting `to`.
        let r2 = router.clone();
        tokio::spawn(async move {
            let req = bob.mailbox.pop().await.unwrap();
            let mut reply = mk(Kind::Reply, "bob", None); // no `to`
            reply.request_id = Some(req.id.clone());
            reply.payload = json!({"ok": true});
            r2.reply(reply).await.unwrap();
        });

        let ask = mk(Kind::Ask, "alice", Some("bob"));
        let reply = router.ask(ask, Duration::from_secs(1)).await.unwrap();
        assert_eq!(reply.to.as_deref(), Some("alice"));
        assert_eq!(reply.payload, json!({"ok": true}));
    }
}
