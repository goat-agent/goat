use std::fmt::Write as _;
use std::future::Future;
use std::time::Duration;

use goat_types::{Event, IntegrationId, IntegrationUpdateKind, ProfileId};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::diff::{DiffOps, WatchState};
use crate::{IntegrationError, IntegrationResult, IntegrationRuntime};

pub const POLL: Duration = Duration::from_mins(2);
pub const EVENT_CAP_PER_POLL: usize = 3;
const MAX_BACKOFF_TICKS: u32 = 8;
const AUTH_ALERT_STREAK: u32 = 5;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observed {
    pub key: String,
    pub stamp: String,
    pub summary: String,
    pub payload: Value,
}

#[derive(Clone, Debug, Default)]
pub struct WatchPage {
    pub items: Vec<Observed>,
    pub truncated: Option<bool>,
}

impl WatchPage {
    pub fn new(items: Vec<Observed>) -> Self {
        Self {
            items,
            truncated: None,
        }
    }

    #[must_use]
    pub fn with_truncated(mut self, truncated: bool) -> Self {
        self.truncated = Some(truncated);
        self
    }
}

pub trait WatchSource: Send + Sync + 'static {
    fn fetch(&self) -> impl Future<Output = IntegrationResult<WatchPage>> + Send;
}

pub struct Watch<S> {
    pub integration: IntegrationId,
    pub stream: String,
    pub kind: IntegrationUpdateKind,
    pub entity: &'static str,
    pub overflow_tail: &'static str,
    pub poll: Duration,
    pub event_cap: usize,
    pub diff: DiffOps,
    pub keep: fn(&Observed) -> bool,
    pub source: S,
}

impl<S> Watch<S> {
    pub fn new(
        integration: IntegrationId,
        stream: impl Into<String>,
        kind: IntegrationUpdateKind,
        entity: &'static str,
        overflow_tail: &'static str,
        diff: DiffOps,
        source: S,
    ) -> Self {
        Self {
            integration,
            stream: stream.into(),
            kind,
            entity,
            overflow_tail,
            poll: POLL,
            event_cap: EVENT_CAP_PER_POLL,
            diff,
            keep: keep_all,
            source,
        }
    }

    #[must_use]
    pub fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    #[must_use]
    pub fn with_event_cap(mut self, cap: usize) -> Self {
        self.event_cap = cap;
        self
    }

    #[must_use]
    pub fn with_keep(mut self, keep: fn(&Observed) -> bool) -> Self {
        self.keep = keep;
        self
    }
}

fn keep_all(_: &Observed) -> bool {
    true
}

pub fn backoff_skips(error_streak: u32) -> u32 {
    2u32.saturating_pow(error_streak)
        .min(MAX_BACKOFF_TICKS)
        .saturating_sub(1)
}

pub async fn run<S: WatchSource>(
    watch: Watch<S>,
    persona: ProfileId,
    runtime: IntegrationRuntime,
    account: String,
    cancel: CancellationToken,
) {
    let integration = watch.integration.clone();
    info!(
        profile = %persona,
        account = %account,
        integration = %integration,
        stream = %watch.stream,
        "watcher running",
    );
    let mut interval = tokio::time::interval(watch.poll);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut error_streak: u32 = 0;
    let mut auth_streak: u32 = 0;
    let mut auth_alerted = false;
    let mut skip_ticks: u32 = 0;

    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            _ = interval.tick() => {}
        }
        if skip_ticks > 0 {
            skip_ticks -= 1;
            continue;
        }
        if runtime.paused().await {
            continue;
        }
        match watch.source.fetch().await {
            Err(e) => {
                error_streak += 1;
                skip_ticks = backoff_skips(error_streak);
                if matches!(e, IntegrationError::Auth(_)) {
                    auth_streak += 1;
                    if auth_streak >= AUTH_ALERT_STREAK && !auth_alerted {
                        auth_alerted = true;
                        runtime.publish(auth_broken_event(persona, &integration, &account, &e));
                    }
                } else {
                    auth_streak = 0;
                }
                warn!(
                    profile = %persona,
                    account = %account,
                    integration = %integration,
                    stream = %watch.stream,
                    error = %e,
                    "poll failed; backing off",
                );
            }
            Ok(page) => {
                error_streak = 0;
                auth_streak = 0;
                auth_alerted = false;
                if let Err(e) = process(&watch, persona, &runtime, &account, page).await {
                    warn!(
                        profile = %persona,
                        account = %account,
                        integration = %integration,
                        stream = %watch.stream,
                        error = %e,
                        "poll processing failed",
                    );
                }
            }
        }
    }
    info!(
        profile = %persona,
        account = %account,
        integration = %integration,
        stream = %watch.stream,
        "watcher stopped",
    );
}

fn auth_broken_event(
    persona: ProfileId,
    integration: &IntegrationId,
    account: &str,
    error: &IntegrationError,
) -> Event {
    let name = integration.as_str();
    Event::IntegrationUpdate {
        profile: persona,
        integration: integration.clone(),
        account: account.to_string(),
        kind: IntegrationUpdateKind::AuthBroken,
        external_ref: format!("{name}/{account}:auth"),
        summary: format!(
            "{name} polling keeps failing to authenticate ({error}); \
             reconnect with `goat integration add {name}`"
        ),
        observation: None,
    }
}

pub async fn load_state(
    runtime: &IntegrationRuntime,
    persona: ProfileId,
    integration: &IntegrationId,
    account: &str,
    stream: &str,
) -> IntegrationResult<Option<WatchState>> {
    let Some(raw) = runtime
        .load_state(persona, integration, account, stream)
        .await?
    else {
        return Ok(None);
    };
    match serde_json::from_str::<WatchState>(&raw) {
        Ok(state) => Ok(Some(state)),
        Err(e) => {
            warn!(
                profile = %persona,
                account = %account,
                integration = %integration,
                stream = %stream,
                error = %e,
                "stored watcher state is unreadable; starting cold and briefing nothing this poll",
            );
            Ok(None)
        }
    }
}

async fn process<S: WatchSource>(
    watch: &Watch<S>,
    persona: ProfileId,
    runtime: &IntegrationRuntime,
    account: &str,
    page: WatchPage,
) -> IntegrationResult<()> {
    let prev = load_state(runtime, persona, &watch.integration, account, &watch.stream).await?;

    let kept: Vec<Observed> = page.items.into_iter().filter(|i| (watch.keep)(i)).collect();
    let (mut next, fresh) = (watch.diff.diff)(prev.as_ref(), &kept);

    if page.truncated == Some(true)
        && let Some(prev) = prev.as_ref()
    {
        warn!(
            profile = %persona,
            account = %account,
            integration = %watch.integration,
            stream = %watch.stream,
            "page was truncated; carrying prior state forward",
        );
        for (key, stamp) in &prev.seen {
            next.seen
                .entry(key.clone())
                .or_insert_with(|| stamp.clone());
        }
    }

    let mut events = Vec::new();
    for item in &fresh {
        match observe(watch, persona, runtime, account, item).await {
            Ok(event) => events.push(event),
            Err(e) => {
                warn!(
                    profile = %persona,
                    account = %account,
                    integration = %watch.integration,
                    key = %item.key,
                    error = %e,
                    "failed to record observation; retrying next poll",
                );
                (watch.diff.hold_back)(&mut next, prev.as_ref(), item);
            }
        }
    }

    let raw = serde_json::to_string(&next).map_err(|e| IntegrationError::Store(e.to_string()))?;
    runtime
        .save_state(persona, &watch.integration, account, &watch.stream, &raw)
        .await?;

    publish(watch, runtime, events);
    Ok(())
}

fn publish<S: WatchSource>(watch: &Watch<S>, runtime: &IntegrationRuntime, events: Vec<Event>) {
    let overflow = events.len().saturating_sub(watch.event_cap);
    for (index, mut event) in events.into_iter().enumerate() {
        if index >= watch.event_cap {
            break;
        }
        if overflow > 0
            && index == watch.event_cap - 1
            && let Event::IntegrationUpdate { summary, .. } = &mut event
        {
            let _ = write!(summary, " (+{overflow} more {})", watch.overflow_tail);
        }
        runtime.publish(event);
    }
}

async fn observe<S: WatchSource>(
    watch: &Watch<S>,
    persona: ProfileId,
    runtime: &IntegrationRuntime,
    account: &str,
    item: &Observed,
) -> IntegrationResult<Event> {
    let external_ref = format!(
        "{}/{account}:{}:{}",
        watch.integration.as_str(),
        watch.entity,
        item.key
    );
    let observation = runtime
        .record_observation(
            persona,
            &watch.integration,
            account,
            &external_ref,
            watch.kind.as_str(),
            item.payload.clone(),
        )
        .await?;
    Ok(Event::IntegrationUpdate {
        profile: persona,
        integration: watch.integration.clone(),
        account: account.to_string(),
        kind: watch.kind,
        external_ref,
        summary: item.summary.clone(),
        observation: Some(observation),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff_skips(0), 0);
        assert_eq!(backoff_skips(1), 1);
        assert_eq!(backoff_skips(2), 3);
        assert_eq!(backoff_skips(3), 7);
        assert_eq!(backoff_skips(9), 7);
    }

    #[test]
    fn the_auth_alert_names_the_connect_command_not_the_bind_command() {
        let event = auth_broken_event(
            ProfileId::from_slug("t"),
            &IntegrationId::from_static("linear"),
            "default",
            &IntegrationError::Auth("401".into()),
        );
        let Event::IntegrationUpdate {
            summary,
            external_ref,
            kind,
            ..
        } = event
        else {
            panic!("expected an integration update");
        };
        assert_eq!(kind, IntegrationUpdateKind::AuthBroken);
        assert_eq!(external_ref, "linear/default:auth");
        assert!(summary.contains("goat integration add linear"));
        assert!(!summary.contains("goat agent integration add"));
    }
}
