//! Per-instance bounded mailbox with drop-oldest overflow semantics.

use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};

use crate::envelope::Envelope;

#[derive(Debug, Clone)]
pub struct Mailbox {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    capacity: usize,
    queue: Mutex<VecDeque<Envelope>>,
    notify: Notify,
    closed: Mutex<bool>,
}

#[derive(Debug)]
pub struct Pushed {
    pub dropped: Option<String>, // id of evicted envelope, if any
}

#[derive(Debug, thiserror::Error)]
pub enum RecvError {
    #[error("mailbox closed")]
    Closed,
    #[error("recv timed out")]
    Timeout,
}

impl Mailbox {
    pub fn new(capacity: usize) -> Self {
        assert!(0 < capacity);
        Self {
            inner: Arc::new(Inner {
                capacity,
                queue: Mutex::new(VecDeque::with_capacity(capacity)),
                notify: Notify::new(),
                closed: Mutex::new(false),
            }),
        }
    }

    pub async fn push(&self, env: Envelope) -> Pushed {
        let mut q = self.inner.queue.lock().await;
        let dropped = if q.len() == self.inner.capacity {
            q.pop_front().map(|e| e.id)
        } else {
            None
        };
        q.push_back(env);
        drop(q);
        self.inner.notify.notify_waiters();
        Pushed { dropped }
    }

    pub async fn pop(&self) -> Result<Envelope, RecvError> {
        loop {
            // Register the Notified future BEFORE checking state so a concurrent
            // push()/close() that calls notify_waiters() between our check and
            // the await cannot drop the wakeup on the floor.
            let notified = self.inner.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let closed = *self.inner.closed.lock().await;
                let mut q = self.inner.queue.lock().await;
                if let Some(env) = q.pop_front() {
                    return Ok(env);
                }
                if closed {
                    return Err(RecvError::Closed);
                }
            }
            notified.await;
        }
    }

    pub async fn pop_with_timeout(&self, dur: std::time::Duration) -> Result<Envelope, RecvError> {
        match tokio::time::timeout(dur, self.pop()).await {
            Ok(res) => res,
            Err(_) => Err(RecvError::Timeout),
        }
    }

    pub async fn drain(&self) -> Vec<Envelope> {
        let mut q = self.inner.queue.lock().await;
        q.drain(..).collect()
    }

    pub async fn close(&self) {
        *self.inner.closed.lock().await = true;
        self.inner.notify.notify_waiters();
    }

    pub fn capacity(&self) -> usize {
        self.inner.capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::Kind;
    use serde_json::json;
    use std::time::Duration;
    use time::macros::datetime;

    fn env(id: &str) -> Envelope {
        Envelope {
            id: id.into(),
            kind: Kind::Message,
            from: "a".into(),
            to: Some("b".into()),
            request_id: None,
            timeout_ms: None,
            ts: datetime!(2026-05-21 08:00:00 UTC),
            payload: json!({}),
        }
    }

    #[tokio::test]
    async fn push_pop_fifo() {
        let m = Mailbox::new(4);
        m.push(env("1")).await;
        m.push(env("2")).await;
        assert_eq!(m.pop().await.unwrap().id, "1");
        assert_eq!(m.pop().await.unwrap().id, "2");
    }

    #[tokio::test]
    async fn overflow_drops_oldest() {
        let m = Mailbox::new(2);
        assert!(m.push(env("1")).await.dropped.is_none());
        assert!(m.push(env("2")).await.dropped.is_none());
        let p = m.push(env("3")).await;
        assert_eq!(p.dropped.as_deref(), Some("1"));
        assert_eq!(m.pop().await.unwrap().id, "2");
        assert_eq!(m.pop().await.unwrap().id, "3");
    }

    #[tokio::test]
    async fn close_unblocks_pop_with_error() {
        let m = Mailbox::new(2);
        let m2 = m.clone();
        let handle = tokio::spawn(async move { m2.pop().await });
        // Give the spawn time to suspend.
        tokio::time::sleep(Duration::from_millis(20)).await;
        m.close().await;
        assert!(matches!(handle.await.unwrap(), Err(RecvError::Closed)));
    }

    #[tokio::test]
    async fn pop_timeout_returns_timeout_error() {
        let m = Mailbox::new(2);
        let err = m
            .pop_with_timeout(Duration::from_millis(20))
            .await
            .unwrap_err();
        assert!(matches!(err, RecvError::Timeout));
    }

    // Regression: pop() previously checked state then awaited notify, so a
    // push() that fired notify_waiters() in between would lose the wakeup
    // and pop would hang. Stressing many producers/consumers in parallel
    // reliably exercises that race window.
    #[tokio::test]
    async fn parallel_producers_consumers_deliver_all_messages() {
        const PRODUCERS: usize = 10;
        const CONSUMERS: usize = 10;
        const PER_PRODUCER: usize = 50;
        const TOTAL: usize = PRODUCERS * PER_PRODUCER;

        let m = Mailbox::new(TOTAL);
        let mut consumer_handles = Vec::with_capacity(CONSUMERS);
        for _ in 0..CONSUMERS {
            let mc = m.clone();
            consumer_handles.push(tokio::spawn(async move {
                let mut got = Vec::new();
                loop {
                    match mc.pop().await {
                        Ok(env) => got.push(env.id),
                        Err(RecvError::Closed) => break,
                        Err(RecvError::Timeout) => unreachable!(),
                    }
                }
                got
            }));
        }

        let mut producer_handles = Vec::with_capacity(PRODUCERS);
        for p in 0..PRODUCERS {
            let mp = m.clone();
            producer_handles.push(tokio::spawn(async move {
                for i in 0..PER_PRODUCER {
                    mp.push(env(&format!("p{p}-{i}"))).await;
                    // Yield to let consumers race the notify_waiters() call.
                    tokio::task::yield_now().await;
                }
            }));
        }

        for h in producer_handles {
            h.await.unwrap();
        }

        // Drain any remaining queued messages before closing so consumers
        // can observe Closed only after the queue is empty.
        loop {
            let mp = m.clone();
            let pending = tokio::time::timeout(Duration::from_millis(50), async move {
                let q = mp.inner.queue.lock().await;
                q.len()
            })
            .await
            .unwrap();
            if pending == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
        m.close().await;

        let mut received = Vec::new();
        for h in consumer_handles {
            received.extend(h.await.unwrap());
        }
        assert_eq!(received.len(), TOTAL);
        received.sort();
        received.dedup();
        assert_eq!(received.len(), TOTAL, "expected unique deliveries");
    }
}
