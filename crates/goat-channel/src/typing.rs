use std::future::Future;
use std::time::Duration;

use tokio::sync::oneshot;
use tokio::time::sleep;

pub struct TypingGuard {
    stop: Option<oneshot::Sender<()>>,
}

impl TypingGuard {
    pub fn new(stop: oneshot::Sender<()>) -> Self {
        Self { stop: Some(stop) }
    }

    pub fn noop() -> Self {
        Self { stop: None }
    }

    pub fn is_noop(&self) -> bool {
        self.stop.is_none()
    }
}

impl Drop for TypingGuard {
    fn drop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
    }
}

pub fn spawn_typing<F, Fut>(refresh: Duration, on_tick: F) -> TypingGuard
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let (stop_tx, mut stop_rx) = oneshot::channel();
    tokio::spawn(async move {
        on_tick().await;
        loop {
            tokio::select! {
                _ = &mut stop_rx => break,
                _ = sleep(refresh) => on_tick().await,
            }
        }
    });
    TypingGuard::new(stop_tx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn ticks_at_least_once_then_stops_on_drop() {
        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        let guard = spawn_typing(Duration::from_millis(20), move || {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
            }
        });
        sleep(Duration::from_millis(50)).await;
        drop(guard);
        let after_drop = count.load(Ordering::SeqCst);
        sleep(Duration::from_millis(80)).await;
        assert!(
            after_drop >= 1,
            "expected at least one tick, got {after_drop}"
        );
        assert_eq!(count.load(Ordering::SeqCst), after_drop, "stopped on drop");
    }

    #[test]
    fn noop_drop_is_silent() {
        drop(TypingGuard::noop());
    }
}
