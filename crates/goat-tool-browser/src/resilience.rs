use std::future::Future;
use std::time::Duration;

use crate::error::BrowserError;

pub const OP_META: Duration = Duration::from_secs(3);
pub const OP_EVAL: Duration = Duration::from_secs(6);
pub const OP_FIND: Duration = Duration::from_secs(4);
pub const OP_CLICK: Duration = Duration::from_secs(6);
pub const OP_FILL: Duration = Duration::from_secs(8);
pub const OP_SCREENSHOT: Duration = Duration::from_secs(10);
pub const OP_NAV_ACK: Duration = Duration::from_secs(6);
pub const OP_OPEN: Duration = Duration::from_secs(12);
pub const OP_HEALTH: Duration = Duration::from_millis(2500);

pub async fn with_timeout<F, T, E>(
    budget: Duration,
    op: &'static str,
    fut: F,
) -> Result<T, BrowserError>
where
    F: Future<Output = Result<T, E>>,
    BrowserError: From<E>,
{
    match tokio::time::timeout(budget, fut).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(BrowserError::from(err)),
        Err(_) => Err(BrowserError::Timeout {
            op,
            ms: budget.as_millis(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{OP_META, with_timeout};
    use crate::error::BrowserError;

    #[tokio::test(start_paused = true)]
    async fn times_out_a_pending_future() {
        let never = std::future::pending::<Result<(), BrowserError>>();
        let result = with_timeout(OP_META, "never", never).await;
        assert!(matches!(
            result,
            Err(BrowserError::Timeout { op: "never", .. })
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn passes_through_ready_ok() {
        let ready = std::future::ready(Ok::<_, BrowserError>(7));
        let value = with_timeout(Duration::from_secs(1), "ready", ready)
            .await
            .unwrap();
        assert_eq!(value, 7);
    }
}
