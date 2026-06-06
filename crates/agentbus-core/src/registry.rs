//! Instance registry.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::mailbox::Mailbox;

pub type OwnerToken = u64;

#[derive(Debug, Clone)]
pub struct InstanceRecord {
    pub id: String,
    pub owner: OwnerToken,
    pub mailbox: Mailbox,
}

#[derive(Debug, thiserror::Error)]
pub enum RegisterError {
    #[error("instance_id `{0}` already registered to another owner")]
    Collision(String),
    #[error("invalid instance_id (must match [A-Za-z0-9_.:-]{{1,128}})")]
    Invalid,
}

#[derive(Debug, Default)]
pub struct Registry {
    inner: Arc<RwLock<HashMap<String, InstanceRecord>>>,
    next_owner: std::sync::atomic::AtomicU64,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn issue_owner_token(&self) -> OwnerToken {
        self.next_owner
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    pub async fn register(
        &self,
        id: &str,
        owner: OwnerToken,
        mailbox_size: usize,
    ) -> Result<InstanceRecord, RegisterError> {
        if !is_valid_id(id) {
            return Err(RegisterError::Invalid);
        }
        let mut g = self.inner.write().await;
        if let Some(existing) = g.get(id) {
            if existing.owner == owner {
                return Ok(existing.clone()); // idempotent same-owner
            }
            return Err(RegisterError::Collision(id.into()));
        }
        let rec = InstanceRecord {
            id: id.into(),
            owner,
            mailbox: Mailbox::new(mailbox_size),
        };
        g.insert(id.into(), rec.clone());
        Ok(rec)
    }

    pub async fn unregister(&self, id: &str, owner: OwnerToken) -> bool {
        let mut g = self.inner.write().await;
        match g.get(id) {
            Some(rec) if rec.owner == owner => {
                let rec = g.remove(id).unwrap();
                rec.mailbox.close().await;
                true
            }
            _ => false,
        }
    }

    pub async fn unregister_owner(&self, owner: OwnerToken) -> Vec<String> {
        let mut g = self.inner.write().await;
        let victims: Vec<String> = g
            .iter()
            .filter_map(|(k, v)| {
                if v.owner == owner {
                    Some(k.clone())
                } else {
                    None
                }
            })
            .collect();
        let mut closed_ids = Vec::with_capacity(victims.len());
        for id in victims {
            if let Some(rec) = g.remove(&id) {
                rec.mailbox.close().await;
                closed_ids.push(id);
            }
        }
        closed_ids
    }

    pub async fn lookup(&self, id: &str) -> Option<InstanceRecord> {
        self.inner.read().await.get(id).cloned()
    }

    pub async fn list_ids(&self) -> Vec<String> {
        let g = self.inner.read().await;
        let mut v: Vec<String> = g.keys().cloned().collect();
        v.sort();
        v
    }
}

fn is_valid_id(id: &str) -> bool {
    let len = id.len();
    if !(1..=128).contains(&len) {
        return false;
    }
    id.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn register_then_lookup() {
        let r = Registry::new();
        let owner = r.issue_owner_token();
        r.register("alice", owner, 16).await.unwrap();
        assert!(r.lookup("alice").await.is_some());
    }

    #[tokio::test]
    async fn collision_rejected_for_different_owner() {
        let r = Registry::new();
        let a = r.issue_owner_token();
        let b = r.issue_owner_token();
        r.register("x", a, 16).await.unwrap();
        let err = r.register("x", b, 16).await.unwrap_err();
        assert!(matches!(err, RegisterError::Collision(_)));
    }

    #[tokio::test]
    async fn same_owner_reregister_is_idempotent() {
        let r = Registry::new();
        let a = r.issue_owner_token();
        r.register("x", a, 16).await.unwrap();
        r.register("x", a, 16).await.unwrap(); // OK
    }

    #[tokio::test]
    async fn unregister_owner_removes_all_and_closes_mailboxes() {
        let r = Registry::new();
        let owner = r.issue_owner_token();
        r.register("x", owner, 2).await.unwrap();
        r.register("y", owner, 2).await.unwrap();
        let ids = r.unregister_owner(owner).await;
        assert_eq!(ids.len(), 2);
        assert!(r.lookup("x").await.is_none());
    }

    #[tokio::test]
    async fn invalid_id_rejected() {
        let r = Registry::new();
        let owner = r.issue_owner_token();
        let err = r.register("space here", owner, 16).await.unwrap_err();
        assert!(matches!(err, RegisterError::Invalid));
    }
}
