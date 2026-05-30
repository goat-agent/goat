//! Event-driven scheduler for goat's self-tick.
//!
//! Replaces the previous 1-minute polling loop with a `tokio::time::sleep_until`
//! based timer that wakes only at the next pending run's `run_at`. New
//! registrations and cancellations are propagated via an mpsc command channel
//! so the timer is woken immediately when state changes.
//!
//! Two important properties:
//!
//! 1. **Wall-clock re-anchor.** `tokio::time::Instant` is monotonic and
//!    ignores NTP adjustments. We therefore compute the deadline as
//!    `Instant::now() + (fire_at - Utc::now())` on every loop iteration,
//!    so the next wake follows wall-clock corrections.
//! 2. **DB is the source of truth.** The in-process heap only decides
//!    "when to look at the store". The actual fire decision is the
//!    atomic `claim_due_run` query, so cancelled tasks naturally
//!    tombstone themselves (a stale heap entry simply finds no pending
//!    row when the store is queried).

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Utc};
use goat_bus::EventBus;
use goat_store::{ScheduleKind, Store, StoreError};
use goat_types::Event;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::cron_expr;

/// Handle for tools and the runtime to nudge the scheduler.
#[derive(Clone)]
pub struct SchedulerHandle {
    tx: mpsc::UnboundedSender<DateTime<Utc>>,
}

impl SchedulerHandle {
    /// Notify the scheduler that a new pending run with the given
    /// `run_at` exists. The scheduler will wake at or before that time
    /// and consult the store. Cancellation does not need a separate
    /// command: tools update the store's `status` to `cancelled` and the
    /// scheduler naturally observes that the store returns no pending
    /// row when it next claims.
    pub fn schedule(&self, run_at: DateTime<Utc>) {
        let _ = self.tx.send(run_at);
    }

    /// Detached handle that silently drops every `schedule` notification.
    /// Useful in unit tests for tools that want a handle without running
    /// the actual scheduler loop.
    #[doc(hidden)]
    pub fn detached() -> Self {
        let (tx, _rx) = mpsc::unbounded_channel();
        Self { tx }
    }
}

/// Park interval when the heap is empty. The select! will still wake
/// instantly on a Register command, so this is just an upper bound on
/// idle sleep length.
const IDLE_PARK_SECS: u64 = 3600;

/// Prepares the scheduler: builds the command channel and pre-loads the
/// timer heap from durable storage, but does NOT start the loop yet. The
/// caller starts the loop after every subscriber (brain task) has been
/// spawned so that the first fire isn't lost on a still-empty bus.
pub async fn prepare_scheduler(
    store: Arc<dyn Store>,
    bus: EventBus,
) -> Result<(SchedulerHandle, PreparedScheduler), StoreError> {
    let (tx, rx) = mpsc::unbounded_channel();
    let handle = SchedulerHandle { tx };

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

/// The scheduler's state and resources at the moment of preparation.
/// Call [`PreparedScheduler::spawn`] after subscribers are attached.
pub struct PreparedScheduler {
    store: Arc<dyn Store>,
    bus: EventBus,
    rx: mpsc::UnboundedReceiver<DateTime<Utc>>,
    heap: BinaryHeap<Reverse<DateTime<Utc>>>,
}

impl PreparedScheduler {
    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(run_loop(self.store, self.bus, self.rx, self.heap))
    }
}

async fn run_loop(
    store: Arc<dyn Store>,
    bus: EventBus,
    mut rx: mpsc::UnboundedReceiver<DateTime<Utc>>,
    mut heap: BinaryHeap<Reverse<DateTime<Utc>>>,
) {
    loop {
        let deadline = next_deadline(&heap);
        tokio::select! {
            cmd = rx.recv() => {
                match cmd {
                    Some(run_at) => heap.push(Reverse(run_at)),
                    None => return, // all handles dropped
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                drain_due(&store, &bus, &mut heap).await;
            }
        }
    }
}

/// Computes the next wake `Instant` from the heap top using a wall-clock
/// re-anchor. `Instant::now() + (fire_at - Utc::now())` follows NTP
/// corrections every iteration.
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
            Ok(Some((run, task))) => {
                info!(
                    run_id = run.id,
                    task_id = run.task_id,
                    persona = %task.persona,
                    "scheduler dispatching self-tick"
                );
                bus.publish(Event::SelfTick {
                    persona: task.persona,
                    run_id: run.id,
                    task_id: run.task_id,
                });
                if let ScheduleKind::Cron(expr) = &task.schedule {
                    if let Some(next) = cron_next(expr, now) {
                        match store
                            .insert_task_run(task.id, next, task.task.clone())
                            .await
                        {
                            Ok(_) => heap.push(Reverse(next)),
                            // A dropped next-occurrence means this cron task
                            // silently never fires again — escalate past warn
                            // so it is not lost in the noise.
                            Err(e) => error!(
                                error = ?e,
                                task_id = task.id,
                                next = %next,
                                "cron re-schedule failed; task will NOT fire again until reboot",
                            ),
                        }
                    }
                }
            }
            Ok(None) => {
                // Stale heap entry — task was cancelled or no rows are
                // due. Move on to the next heap entry.
            }
            Err(e) => {
                warn!(error = ?e, "claim_due_run failed");
            }
        }
    }
}

fn cron_next(expr: &str, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
    match cron_expr::parse(expr) {
        Ok(schedule) => cron_expr::next_after(&schedule, after),
        Err(e) => {
            warn!(error = ?e, expr = %expr, "invalid cron in db");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as ChronoDuration;

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
