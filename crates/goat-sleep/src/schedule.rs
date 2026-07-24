use std::future::Future;
use std::time::Duration;

use chrono::Utc;
use goat_loop::cron_expr;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

const PARK: Duration = Duration::from_hours(1);

#[derive(Clone, Debug)]
pub struct SleepConfig {
    pub cron: String,
}

impl Default for SleepConfig {
    fn default() -> Self {
        Self {
            cron: "0 4 * * *".to_string(),
        }
    }
}

pub fn spawn<F, Fut>(
    config: SleepConfig,
    cancel: CancellationToken,
    run: F,
) -> tokio::task::JoinHandle<()>
where
    F: Fn() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        let schedule = match cron_expr::parse(&config.cron) {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, cron = %config.cron, "sleep: invalid cron; loop disabled");
                return;
            }
        };
        info!(cron = %config.cron, "sleep-job loop started");
        loop {
            let now = Utc::now();
            let wait = match cron_expr::next_after(&schedule, now) {
                Some(next) => (next - now).to_std().unwrap_or(PARK),
                None => PARK,
            };
            tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                () = tokio::time::sleep(wait) => {
                    run().await;
                }
            }
        }
        info!("sleep-job loop stopped");
    })
}
