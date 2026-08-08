use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use goat_types::{AgentId, Event, IntegrationId, IntegrationUpdateKind, WorkflowItem};
use rand::RngExt;
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
    pub reference: Option<String>,
    pub stamp: String,
    pub summary: String,
    pub payload: Value,
}

impl Observed {
    pub fn new(
        key: impl Into<String>,
        stamp: impl Into<String>,
        summary: impl Into<String>,
        payload: Value,
    ) -> Self {
        Self {
            key: key.into(),
            reference: None,
            stamp: stamp.into(),
            summary: summary.into(),
            payload,
        }
    }

    #[must_use]
    pub fn with_reference(mut self, reference: impl Into<String>) -> Self {
        self.reference = Some(reference.into());
        self
    }

    pub fn reference(&self) -> &str {
        self.reference.as_deref().unwrap_or(&self.key)
    }
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

pub trait DynWatchSource: Send + Sync + 'static {
    fn fetch_dyn(&self) -> Pin<Box<dyn Future<Output = IntegrationResult<WatchPage>> + Send + '_>>;
}

impl<S: WatchSource> DynWatchSource for S {
    fn fetch_dyn(&self) -> Pin<Box<dyn Future<Output = IntegrationResult<WatchPage>> + Send + '_>> {
        Box::pin(WatchSource::fetch(self))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchSpec {
    pub stream: String,
    pub query: String,
}

pub struct CompiledWatch {
    pub kind: IntegrationUpdateKind,
    pub entity: &'static str,
    pub diff: DiffOps,
    pub source: Box<dyn DynWatchSource>,
}

pub struct WorkflowSource {
    pub integration: IntegrationId,
    pub account: String,
    pub stream: String,
    pub compiled: CompiledWatch,
}

pub struct Workflow {
    pub name: String,
    pub poll: Duration,
    pub event_cap: usize,
    pub sources: Vec<WorkflowSource>,
}

impl Workflow {
    pub fn new(name: impl Into<String>, sources: Vec<WorkflowSource>) -> Self {
        Self {
            name: name.into(),
            poll: POLL,
            event_cap: EVENT_CAP_PER_POLL,
            sources,
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
}

pub fn backoff_skips(error_streak: u32) -> u32 {
    2u32.saturating_pow(error_streak)
        .min(MAX_BACKOFF_TICKS)
        .saturating_sub(1)
}

fn startup_jitter(poll: Duration, sample: f64) -> Duration {
    poll.mul_f64(sample)
}

fn tick_jitter(poll: Duration, sample: f64) -> Duration {
    poll.mul_f64(0.75 + sample * 0.5)
}

fn random_sample() -> f64 {
    rand::rng().random_range(0.0..=1.0)
}

#[derive(Default)]
struct SourceHealth {
    error_streak: u32,
    auth_streak: u32,
    auth_alerted: bool,
    skip_ticks: u32,
}

pub async fn run_workflow(
    workflow: Workflow,
    agent: AgentId,
    runtime: IntegrationRuntime,
    cancel: CancellationToken,
) {
    info!(
        agent = %agent,
        workflow = %workflow.name,
        sources = workflow.sources.len(),
        "workflow running",
    );
    let mut next_tick =
        tokio::time::Instant::now() + startup_jitter(workflow.poll, random_sample());
    let mut health: Vec<SourceHealth> = workflow
        .sources
        .iter()
        .map(|_| SourceHealth::default())
        .collect();

    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            () = tokio::time::sleep_until(next_tick) => {}
        }
        next_tick = tokio::time::Instant::now() + tick_jitter(workflow.poll, random_sample());
        if runtime.paused().await {
            continue;
        }
        let mut items: Vec<WorkflowItem> = Vec::new();
        for (source, health) in workflow.sources.iter().zip(health.iter_mut()) {
            if health.skip_ticks > 0 {
                health.skip_ticks -= 1;
                continue;
            }
            match source.compiled.source.fetch_dyn().await {
                Err(e) => {
                    health.error_streak += 1;
                    health.skip_ticks = backoff_skips(health.error_streak);
                    if matches!(e, IntegrationError::Auth(_)) {
                        health.auth_streak += 1;
                        if health.auth_streak >= AUTH_ALERT_STREAK && !health.auth_alerted {
                            health.auth_alerted = true;
                            runtime.publish(auth_broken_event(
                                agent,
                                &source.integration,
                                &source.account,
                                &e,
                            ));
                        }
                    } else {
                        health.auth_streak = 0;
                    }
                    warn!(
                        agent = %agent,
                        workflow = %workflow.name,
                        integration = %source.integration,
                        account = %source.account,
                        stream = %source.stream,
                        error = %e,
                        "poll failed; backing off",
                    );
                }
                Ok(page) => {
                    health.error_streak = 0;
                    health.auth_streak = 0;
                    health.auth_alerted = false;
                    match process_source(source, agent, &runtime, page).await {
                        Ok(mut fresh) => items.append(&mut fresh),
                        Err(e) => warn!(
                            agent = %agent,
                            workflow = %workflow.name,
                            integration = %source.integration,
                            account = %source.account,
                            stream = %source.stream,
                            error = %e,
                            "poll processing failed",
                        ),
                    }
                }
            }
        }
        publish(&workflow, &runtime, agent, items);
    }
    info!(
        agent = %agent,
        workflow = %workflow.name,
        "workflow stopped",
    );
}

fn auth_broken_event(
    agent: AgentId,
    integration: &IntegrationId,
    account: &str,
    error: &IntegrationError,
) -> Event {
    let name = integration.as_str();
    Event::IntegrationUpdate {
        agent,
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
    agent: AgentId,
    integration: &IntegrationId,
    account: &str,
    stream: &str,
) -> IntegrationResult<Option<WatchState>> {
    let Some(raw) = runtime
        .load_state(agent, integration, account, stream)
        .await?
    else {
        return Ok(None);
    };
    match serde_json::from_str::<WatchState>(&raw) {
        Ok(state) => Ok(Some(state)),
        Err(e) => {
            warn!(
                agent = %agent,
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

async fn process_source(
    source: &WorkflowSource,
    agent: AgentId,
    runtime: &IntegrationRuntime,
    page: WatchPage,
) -> IntegrationResult<Vec<WorkflowItem>> {
    let prev = load_state(
        runtime,
        agent,
        &source.integration,
        &source.account,
        &source.stream,
    )
    .await?;

    let (mut next, fresh) = (source.compiled.diff.diff)(prev.as_ref(), &page.items);

    if page.truncated == Some(true)
        && let Some(prev) = prev.as_ref()
    {
        warn!(
            agent = %agent,
            account = %source.account,
            integration = %source.integration,
            stream = %source.stream,
            "page was truncated; carrying prior state forward",
        );
        carry_forward_truncated_state(&mut next, prev);
    }

    let mut items = Vec::new();
    for item in &fresh {
        match observe(source, agent, runtime, item).await {
            Ok(observed) => items.push(observed),
            Err(e) => {
                warn!(
                    agent = %agent,
                    account = %source.account,
                    integration = %source.integration,
                    key = %item.key,
                    error = %e,
                    "failed to record observation; retrying next poll",
                );
                (source.compiled.diff.hold_back)(&mut next, prev.as_ref(), item);
            }
        }
    }

    let raw = serde_json::to_string(&next).map_err(|e| IntegrationError::Store(e.to_string()))?;
    runtime
        .save_state(
            agent,
            &source.integration,
            &source.account,
            &source.stream,
            &raw,
        )
        .await?;

    Ok(items)
}

fn carry_forward_truncated_state(next: &mut WatchState, prev: &WatchState) {
    for (key, stamp) in &prev.seen {
        next.seen
            .entry(key.clone())
            .or_insert_with(|| stamp.clone());
    }
}

async fn observe(
    source: &WorkflowSource,
    agent: AgentId,
    runtime: &IntegrationRuntime,
    item: &Observed,
) -> IntegrationResult<WorkflowItem> {
    let external_ref = format!(
        "{}/{}:{}:{}",
        source.integration.as_str(),
        source.account,
        source.compiled.entity,
        item.reference()
    );
    let observation = runtime
        .record_observation(
            agent,
            &source.integration,
            &source.account,
            &external_ref,
            source.compiled.kind.as_str(),
            item.payload.clone(),
        )
        .await?;
    Ok(WorkflowItem {
        integration: source.integration.clone(),
        account: source.account.clone(),
        stream: source.stream.clone(),
        kind: source.compiled.kind,
        external_ref,
        summary: item.summary.clone(),
        observation: Some(observation),
    })
}

fn publish(
    workflow: &Workflow,
    runtime: &IntegrationRuntime,
    agent: AgentId,
    mut items: Vec<WorkflowItem>,
) {
    if items.is_empty() {
        return;
    }
    let overflow = items.len().saturating_sub(workflow.event_cap);
    items.truncate(workflow.event_cap);
    runtime.publish(Event::WorkflowUpdate {
        agent,
        workflow: workflow.name.clone(),
        items,
        overflow,
    });
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
    fn workflow_deadlines_have_startup_and_recurring_jitter() {
        let poll = Duration::from_mins(2);
        assert_eq!(startup_jitter(poll, 0.0), Duration::ZERO);
        assert_eq!(startup_jitter(poll, 1.0), poll);
        assert_eq!(tick_jitter(poll, 0.0), Duration::from_secs(90));
        assert_eq!(tick_jitter(poll, 0.5), poll);
        assert_eq!(tick_jitter(poll, 1.0), Duration::from_secs(150));
    }

    #[test]
    fn truncated_page_carry_forward_suppresses_false_reemission() {
        let original = Observed::new("a", "1", "s", Value::Null);
        let (first, _) = (crate::diff::REBUILD.diff)(None, std::slice::from_ref(&original));
        let (mut truncated, _) = (crate::diff::REBUILD.diff)(Some(&first), &[]);
        carry_forward_truncated_state(&mut truncated, &first);
        let (_, fresh) =
            (crate::diff::REBUILD.diff)(Some(&truncated), std::slice::from_ref(&original));
        assert!(fresh.is_empty());
    }

    #[test]
    fn an_item_references_its_own_key_unless_told_otherwise() {
        let plain = Observed::new("uuid-9", "1", "s", Value::Null);
        assert_eq!(plain.reference(), "uuid-9");
        let aliased = plain.with_reference("US-1");
        assert_eq!(aliased.key, "uuid-9");
        assert_eq!(aliased.reference(), "US-1");
    }

    #[test]
    fn the_auth_alert_names_the_connect_command_not_the_bind_command() {
        let event = auth_broken_event(
            AgentId::from_slug("t"),
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

#[cfg(all(test, feature = "test-support"))]
mod contract_tests {
    use crate::diff::{REBUILD, RETAIN, SETTLE};
    use crate::test_support::{WatchContract, assert_bundle_contract, assert_watch_contract};
    use goat_types::{IntegrationId, IntegrationUpdateKind};

    fn contract(name: &'static str, diff: crate::diff::DiffOps) -> WatchContract {
        WatchContract {
            integration: IntegrationId::from_static(name),
            stream: "items".to_owned(),
            kind: IntegrationUpdateKind::Assigned,
            entity: "issue",
            diff,
        }
    }

    #[tokio::test]
    async fn the_rebuild_policy_honours_the_watch_contract() {
        assert_watch_contract(&contract("rebuilder", REBUILD)).await;
    }

    #[tokio::test]
    async fn the_retain_policy_honours_the_watch_contract() {
        assert_watch_contract(&contract("retainer", RETAIN)).await;
    }

    #[tokio::test]
    async fn the_settle_policy_honours_the_watch_contract() {
        assert_watch_contract(&contract("settler", SETTLE)).await;
    }

    #[tokio::test]
    async fn a_two_source_workflow_honours_the_bundle_contract() {
        assert_bundle_contract().await;
    }
}
