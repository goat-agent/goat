use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Utc};
use goat_bus::EventBus;
use goat_store::{ScheduleKind, Store, StoreError};
use goat_types::Event;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::cron_expr;

#[derive(Clone)]
pub struct SchedulerHandle {
    tx: mpsc::UnboundedSender<DateTime<Utc>>,
}

impl SchedulerHandle {
    pub fn schedule(&self, run_at: DateTime<Utc>) {
        let _ = self.tx.send(run_at);
    }

    #[doc(hidden)]
    pub fn detached() -> Self {
        let (tx, _rx) = mpsc::unbounded_channel();
        Self { tx }
    }
}

const IDLE_PARK_SECS: u64 = 3600;

pub async fn prepare_scheduler(
    store: Arc<dyn Store>,
    bus: EventBus,
) -> Result<(SchedulerHandle, PreparedScheduler), StoreError> {
    let (tx, rx) = mpsc::unbounded_channel();
    let handle = SchedulerHandle { tx };

    match store.reclaim_stale_runs(Utc::now()).await {
        Ok(n) => info!(reclaimed = n, "boot-time stale run reclaim"),
        Err(e) => warn!(error = ?e, "boot-time reclaim_stale_runs failed; continuing"),
    }

    match store.cron_schedules_missing_next_run().await {
        Ok(schedules) => {
            let now = Utc::now();
            for schedule in schedules {
                let ScheduleKind::Cron(expr) = &schedule.schedule else {
                    continue;
                };
                let Some(next) = cron_next(expr, schedule.timezone.as_deref(), now) else {
                    warn!(schedule_id = schedule.id, expr = %expr, "cron repair: invalid schedule");
                    continue;
                };
                match store
                    .insert_schedule_run(schedule.id, next, schedule.instruction.clone())
                    .await
                {
                    Ok(_) => {
                        info!(schedule_id = schedule.id, next = %next, "cron repair: re-seeded next run");
                    }
                    Err(e) => {
                        warn!(error = ?e, schedule_id = schedule.id, "cron repair: insert failed");
                    }
                }
            }
        }
        Err(e) => warn!(error = ?e, "boot-time cron repair failed; continuing"),
    }

    let pending = store.all_pending_runs().await?;
    let mut heap: BinaryHeap<Reverse<DateTime<Utc>>> = BinaryHeap::new();
    for (_run_id, _task_id, run_at) in pending {
        heap.push(Reverse(run_at));
    }
    info!(
        initial_pending = heap.len(),
        "scheduler bootstrap from store"
    );

    let prepared = PreparedScheduler {
        store,
        bus,
        rx,
        heap,
    };
    Ok((handle, prepared))
}

pub struct PreparedScheduler {
    store: Arc<dyn Store>,
    bus: EventBus,
    rx: mpsc::UnboundedReceiver<DateTime<Utc>>,
    heap: BinaryHeap<Reverse<DateTime<Utc>>>,
}

impl PreparedScheduler {
    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        self.spawn_with_cancel(CancellationToken::new())
    }

    pub fn spawn_with_cancel(self, cancel: CancellationToken) -> tokio::task::JoinHandle<()> {
        tokio::spawn(run_loop(self.store, self.bus, self.rx, self.heap, cancel))
    }
}

async fn run_loop(
    store: Arc<dyn Store>,
    bus: EventBus,
    mut rx: mpsc::UnboundedReceiver<DateTime<Utc>>,
    mut heap: BinaryHeap<Reverse<DateTime<Utc>>>,
    cancel: CancellationToken,
) {
    let mut reclaim_ticker = tokio::time::interval(StdDuration::from_mins(5));
    reclaim_ticker.tick().await;

    loop {
        let deadline = next_deadline(&heap);
        tokio::select! {
            biased;
            () = cancel.cancelled() => return,
            cmd = rx.recv() => {
                match cmd {
                    Some(run_at) => heap.push(Reverse(run_at)),
                    None => return,
                }
            }
            () = tokio::time::sleep_until(deadline) => {
                drain_due(&store, &bus, &mut heap).await;
            }
            _ = reclaim_ticker.tick() => {
                let stale_before = Utc::now() - chrono::Duration::minutes(30);
                match store.reclaim_stale_runs(stale_before).await {
                    Ok(n) if n > 0 => info!(reclaimed = n, "periodic stale run reclaim"),
                    Ok(_) => {}
                    Err(e) => warn!(error = ?e, "periodic reclaim_stale_runs failed"),
                }
            }
        }
    }
}

fn next_deadline(heap: &BinaryHeap<Reverse<DateTime<Utc>>>) -> tokio::time::Instant {
    let now_instant = tokio::time::Instant::now();
    match heap.peek() {
        Some(Reverse(fire_at)) => {
            let delta = *fire_at - Utc::now();
            let dur = delta.to_std().unwrap_or(StdDuration::from_millis(0));
            now_instant + dur
        }
        None => now_instant + StdDuration::from_secs(IDLE_PARK_SECS),
    }
}

async fn drain_due(
    store: &Arc<dyn Store>,
    bus: &EventBus,
    heap: &mut BinaryHeap<Reverse<DateTime<Utc>>>,
) {
    loop {
        let now = Utc::now();
        let peek = heap.peek().map(|Reverse(at)| *at);
        match peek {
            Some(fire_at) if fire_at <= now => {
                let _ = heap.pop();
            }
            _ => break,
        }

        match store.claim_due_run(now).await {
            Ok(Some((run, schedule))) => {
                info!(
                    run_id = run.id,
                    schedule_id = run.schedule_id,
                    agent = %schedule.agent,
                    "scheduler dispatching schedule"
                );
                bus.publish(Event::Schedule {
                    agent: schedule.agent,
                    run_id: run.id,
                    schedule_id: run.schedule_id,
                });
                if let ScheduleKind::Cron(expr) = &schedule.schedule
                    && let Some(next) = cron_next(expr, schedule.timezone.as_deref(), now)
                {
                    match store
                        .insert_schedule_run(schedule.id, next, schedule.instruction.clone())
                        .await
                    {
                        Ok(_) => heap.push(Reverse(next)),
                        Err(e) => error!(
                            error = ?e,
                            schedule_id = schedule.id,
                            next = %next,
                            "cron re-schedule failed; task will NOT fire again until reboot",
                        ),
                    }
                }
            }
            Ok(None) => {}
            Err(e) => {
                warn!(error = ?e, "claim_due_run failed");
            }
        }
    }
}

fn cron_next(
    expr: &str,
    timezone_name: Option<&str>,
    after: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let schedule = cron_expr::parse(expr);
    let timezone = cron_expr::parse_timezone(timezone_name);
    match (schedule, timezone) {
        (Ok(schedule), Ok(timezone)) => cron_expr::next_after(&schedule, after, timezone),
        (Err(error), _) | (_, Err(error)) => {
            warn!(error = ?error, expr = %expr, timezone = ?timezone_name, "invalid cron in db");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration as ChronoDuration, Local, TimeZone};

    #[test]
    fn legacy_schedule_without_timezone_uses_host_local() {
        let after = Utc.with_ymd_and_hms(2026, 1, 1, 0, 1, 0).unwrap();
        let actual = cron_next("0 9 * * *", None, after).unwrap();
        let schedule = cron_expr::parse("0 9 * * *").unwrap();
        let expected = schedule
            .after(&after.with_timezone(&Local))
            .next()
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(actual, expected);
    }

    #[test]
    fn next_deadline_uses_wall_clock_offset() {
        let mut heap: BinaryHeap<Reverse<DateTime<Utc>>> = BinaryHeap::new();
        let fire = Utc::now() + ChronoDuration::seconds(2);
        heap.push(Reverse(fire));
        let deadline = next_deadline(&heap);
        let now = tokio::time::Instant::now();
        let dur = deadline.saturating_duration_since(now);
        assert!(
            dur >= StdDuration::from_millis(1500) && dur <= StdDuration::from_millis(2500),
            "expected ~2s, got {dur:?}"
        );
    }

    #[test]
    fn empty_heap_parks() {
        let heap: BinaryHeap<Reverse<DateTime<Utc>>> = BinaryHeap::new();
        let deadline = next_deadline(&heap);
        let now = tokio::time::Instant::now();
        let dur = deadline.saturating_duration_since(now);
        assert!(
            dur >= StdDuration::from_secs(IDLE_PARK_SECS - 1),
            "expected long park, got {dur:?}"
        );
    }

    #[test]
    fn past_fire_yields_immediate_deadline() {
        let mut heap: BinaryHeap<Reverse<DateTime<Utc>>> = BinaryHeap::new();
        heap.push(Reverse(Utc::now() - ChronoDuration::seconds(5)));
        let deadline = next_deadline(&heap);
        let now = tokio::time::Instant::now();
        assert!(deadline <= now + StdDuration::from_millis(50));
    }
}
