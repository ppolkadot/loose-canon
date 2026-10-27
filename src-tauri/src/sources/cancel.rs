use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use tokio::sync::Notify;

/// Cooperative cancellation shared between the UI command that cancels and the tasks doing the
/// work. Cheap to clone; every clone observes the same state.
#[derive(Clone, Default)]
pub struct CancelToken {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    cancelled: AtomicBool,
    notify: Notify,
}

impl CancelToken {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::SeqCst);
        self.inner.notify.notify_waiters();
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// Resolves as soon as the token is cancelled (immediately if it already is).
    pub async fn cancelled(&self) {
        loop {
            // Register interest *before* checking the flag so a cancel between the check and the
            // await cannot be missed.
            let notified = self.inner.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn clones_share_state() {
        let a = CancelToken::new();
        let b = a.clone();
        assert!(!a.is_cancelled() && !b.is_cancelled());
        b.cancel();
        assert!(a.is_cancelled() && b.is_cancelled());
        b.cancel(); // idempotent
        assert!(a.is_cancelled());
    }

    #[tokio::test]
    async fn an_already_cancelled_token_resolves_immediately() {
        let t = CancelToken::new();
        t.cancel();
        tokio::time::timeout(Duration::from_millis(200), t.cancelled())
            .await
            .expect("must not wait");
    }

    #[tokio::test]
    async fn waiters_wake_when_another_task_cancels() {
        let t = CancelToken::new();
        let waiter = {
            let t = t.clone();
            tokio::spawn(async move { t.cancelled().await })
        };
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!waiter.is_finished(), "must keep waiting until cancelled");
        t.cancel();
        tokio::time::timeout(Duration::from_millis(500), waiter)
            .await
            .expect("woken")
            .unwrap();
    }

    #[tokio::test]
    async fn an_uncancelled_token_never_resolves() {
        let t = CancelToken::new();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), t.cancelled())
                .await
                .is_err()
        );
    }
}
