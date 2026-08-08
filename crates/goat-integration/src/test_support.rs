use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use goat_auth::CredentialStore;
use goat_bus::{EventBus, EventFilter};
use goat_store::SqliteStore;
use goat_types::{AgentId, Event, IntegrationId, IntegrationUpdateKind};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::diff::{DiffOps, REBUILD, RETAIN};
use crate::watch::{
    CompiledWatch, Observed, WatchPage, WatchSource, Workflow, WorkflowSource, run_workflow,
};
use crate::{IntegrationError, IntegrationResult, IntegrationRuntime};

const TICK: Duration = Duration::from_millis(10);
const PATIENCE: Duration = Duration::from_secs(5);

pub async fn runtime_in(dir: &Path) -> IntegrationRuntime {
    let store = SqliteStore::open(&dir.join("goat.db")).await.unwrap();
    let mut runtime = IntegrationRuntime::new(
        CredentialStore::new(dir.join("credentials.json")),
        Arc::new(store),
        EventBus::new(),
    );
    runtime.poll_budget = crate::watch::PollBudget::new(Duration::ZERO);
    runtime
}

pub async fn agent_in(runtime: &IntegrationRuntime) -> AgentId {
    let agent = AgentId::from_slug("test");
    runtime
        .store
        .ensure_agent(agent, "test", "test")
        .await
        .unwrap();
    agent
}

pub fn observed(key: &str, stamp: &str) -> Observed {
    Observed::new(
        key,
        stamp,
        format!("{key} needs you"),
        json!({ "key": key, "stamp": stamp }),
    )
}

pub struct ScriptedSource {
    pages: Mutex<VecDeque<IntegrationResult<WatchPage>>>,
    last: Mutex<Option<WatchPage>>,
}

impl ScriptedSource {
    pub fn new(pages: Vec<IntegrationResult<WatchPage>>) -> Self {
        Self {
            pages: Mutex::new(pages.into()),
            last: Mutex::new(None),
        }
    }

    pub fn pages(items: Vec<Vec<Observed>>) -> Self {
        Self::new(items.into_iter().map(|i| Ok(WatchPage::new(i))).collect())
    }

    pub fn always_failing(error: &IntegrationError) -> Self {
        Self {
            pages: Mutex::new([Err(clone_error(error))].into_iter().collect()),
            last: Mutex::new(None),
        }
    }
}

impl WatchSource for ScriptedSource {
    async fn fetch(&self) -> IntegrationResult<WatchPage> {
        let next = self.pages.lock().unwrap().pop_front();
        match next {
            Some(Ok(page)) => {
                *self.last.lock().unwrap() = Some(page.clone());
                Ok(page)
            }
            Some(Err(e)) => {
                self.pages.lock().unwrap().push_back(Err(clone_error(&e)));
                Err(e)
            }
            None => {
                let last = self.last.lock().unwrap().clone();
                Ok(last.unwrap_or_default())
            }
        }
    }
}

fn clone_error(error: &IntegrationError) -> IntegrationError {
    match error {
        IntegrationError::Auth(m) => IntegrationError::Auth(m.clone()),
        IntegrationError::Config(m) => IntegrationError::Config(m.clone()),
        IntegrationError::Store(m) => IntegrationError::Store(m.clone()),
        other => IntegrationError::Service(other.to_string()),
    }
}

pub struct WatchContract {
    pub integration: IntegrationId,
    pub stream: String,
    pub kind: IntegrationUpdateKind,
    pub entity: &'static str,
    pub diff: DiffOps,
}

impl WatchContract {
    fn source(&self, source: ScriptedSource) -> WorkflowSource {
        WorkflowSource {
            integration: self.integration.clone(),
            account: "default".to_owned(),
            state_key: self.stream.clone(),
            stream: self.stream.clone(),
            compiled: CompiledWatch {
                kind: self.kind,
                entity: self.entity,
                diff: self.diff,
                source: Box::new(source),
            },
        }
    }

    fn workflow(&self, source: ScriptedSource) -> Workflow {
        Workflow::new(self.stream.clone(), vec![self.source(source)]).with_poll(TICK)
    }
}

pub async fn assert_watch_contract(contract: &WatchContract) {
    cold_start_briefs_nothing(contract).await;
    a_burst_is_capped_with_an_overflow_count(contract).await;
    repeated_auth_failures_alert_exactly_once(contract).await;
    an_observation_round_trips_losslessly(contract).await;
    unreadable_state_starts_cold_instead_of_replaying(contract).await;
}

async fn cold_start_briefs_nothing(contract: &WatchContract) {
    let dir = tempfile::tempdir().unwrap();
    let runtime = runtime_in(dir.path()).await;
    let agent = agent_in(&runtime).await;
    let mut sub = runtime.bus.subscribe(EventFilter::Agent(agent));
    let cancel = CancellationToken::new();

    let backlog: Vec<Observed> = (0..40)
        .map(|n| observed(&format!("old{n}"), &format!("2026-01-{n:02}")))
        .collect();
    let source = ScriptedSource::pages(vec![backlog]);
    let handle = tokio::spawn(run_workflow(
        contract.workflow(source),
        agent,
        runtime.clone(),
        cancel.clone(),
    ));

    let quiet = tokio::time::timeout(Duration::from_millis(200), sub.recv()).await;
    cancel.cancel();
    handle.await.unwrap();
    assert!(
        quiet.is_err(),
        "a cold start must not brief the existing backlog"
    );
}

async fn a_burst_is_capped_with_an_overflow_count(contract: &WatchContract) {
    let dir = tempfile::tempdir().unwrap();
    let runtime = runtime_in(dir.path()).await;
    let agent = agent_in(&runtime).await;
    let mut sub = runtime.bus.subscribe(EventFilter::Agent(agent));
    let cancel = CancellationToken::new();

    let burst: Vec<Observed> = (0..5)
        .map(|n| observed(&format!("new{n}"), &format!("2026-02-{n:02}")))
        .collect();
    let source = ScriptedSource::new(vec![
        Ok(WatchPage::new(Vec::new())),
        Ok(WatchPage::new(burst.clone())),
        Ok(WatchPage::new(burst)),
    ]);
    let handle = tokio::spawn(run_workflow(
        contract.workflow(source),
        agent,
        runtime.clone(),
        cancel.clone(),
    ));

    let event = tokio::time::timeout(PATIENCE, sub.recv())
        .await
        .expect("an event before timeout")
        .expect("event");
    let Event::WorkflowUpdate {
        workflow,
        items,
        overflow,
        ..
    } = event
    else {
        panic!("expected a workflow update");
    };
    let extra = tokio::time::timeout(Duration::from_millis(200), sub.recv()).await;
    cancel.cancel();
    handle.await.unwrap();

    assert_eq!(workflow, contract.stream);
    assert_eq!(items.len(), 3, "a burst must be capped");
    assert_eq!(overflow, 2, "the cap must report what it dropped");
    assert!(
        extra.is_err(),
        "everything fetched is seen; nothing beyond the cap may re-fire"
    );
}

async fn repeated_auth_failures_alert_exactly_once(contract: &WatchContract) {
    let dir = tempfile::tempdir().unwrap();
    let runtime = runtime_in(dir.path()).await;
    let agent = agent_in(&runtime).await;
    let mut sub = runtime.bus.subscribe(EventFilter::Agent(agent));
    let cancel = CancellationToken::new();

    let source = ScriptedSource::always_failing(&IntegrationError::Auth("401".into()));
    let handle = tokio::spawn(run_workflow(
        contract.workflow(source),
        agent,
        runtime.clone(),
        cancel.clone(),
    ));

    let event = tokio::time::timeout(PATIENCE, sub.recv())
        .await
        .expect("an auth alert before timeout")
        .expect("event");
    let Event::IntegrationUpdate {
        kind, external_ref, ..
    } = event
    else {
        panic!("unexpected event type");
    };
    assert_eq!(kind, IntegrationUpdateKind::AuthBroken);
    assert_eq!(
        external_ref,
        format!("{}/default:auth", contract.integration.as_str())
    );

    let second = tokio::time::timeout(Duration::from_millis(400), sub.recv()).await;
    cancel.cancel();
    handle.await.unwrap();
    assert!(second.is_err(), "the auth alert must fire exactly once");
}

async fn an_observation_round_trips_losslessly(contract: &WatchContract) {
    let dir = tempfile::tempdir().unwrap();
    let runtime = runtime_in(dir.path()).await;
    let agent = agent_in(&runtime).await;
    let mut sub = runtime.bus.subscribe(EventFilter::Agent(agent));
    let cancel = CancellationToken::new();

    let source = ScriptedSource::new(vec![
        Ok(WatchPage::new(Vec::new())),
        Ok(WatchPage::new(vec![observed("K-1", "2026-03-01")])),
    ]);
    let handle = tokio::spawn(run_workflow(
        contract.workflow(source),
        agent,
        runtime.clone(),
        cancel.clone(),
    ));

    let event = tokio::time::timeout(PATIENCE, sub.recv())
        .await
        .expect("an event before timeout")
        .expect("event");
    let Event::WorkflowUpdate { items, .. } = event else {
        panic!("expected a workflow update");
    };
    cancel.cancel();
    handle.await.unwrap();

    let item = items.first().expect("one item");
    assert_eq!(item.kind, contract.kind);
    assert_eq!(item.stream, contract.stream);
    assert_eq!(
        item.external_ref,
        format!(
            "{}/default:{}:K-1",
            contract.integration.as_str(),
            contract.entity
        )
    );
    let record = runtime
        .store
        .get_observation(item.observation.expect("an observation id"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.external_ref, item.external_ref);
    assert_eq!(
        record.payload,
        json!({ "key": "K-1", "stamp": "2026-03-01" })
    );
}

async fn unreadable_state_starts_cold_instead_of_replaying(contract: &WatchContract) {
    let dir = tempfile::tempdir().unwrap();
    let runtime = runtime_in(dir.path()).await;
    let agent = agent_in(&runtime).await;
    runtime
        .save_state(
            agent,
            &contract.integration,
            "default",
            &contract.stream,
            "{not json",
        )
        .await
        .unwrap();

    let loaded = crate::watch::load_state(
        &runtime,
        agent,
        &contract.integration,
        "default",
        &contract.stream,
        &contract.stream,
    )
    .await
    .unwrap();
    assert!(
        loaded.is_none(),
        "unreadable state must resolve to a cold start, not a panic or a replay"
    );
}

pub async fn assert_bundle_contract() {
    two_sources_merge_into_one_capped_event().await;
    one_failing_source_does_not_silence_the_other().await;
}

fn bundle_source(name: &'static str, diff: DiffOps, source: ScriptedSource) -> WorkflowSource {
    WorkflowSource {
        integration: IntegrationId::from_static(name),
        account: "default".to_owned(),
        state_key: "inbox".to_owned(),
        stream: "inbox".to_owned(),
        compiled: CompiledWatch {
            kind: IntegrationUpdateKind::Assigned,
            entity: "item",
            diff,
            source: Box::new(source),
        },
    }
}

async fn two_sources_merge_into_one_capped_event() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = runtime_in(dir.path()).await;
    let agent = agent_in(&runtime).await;
    let mut sub = runtime.bus.subscribe(EventFilter::Agent(agent));
    let cancel = CancellationToken::new();

    let alpha = ScriptedSource::pages(vec![
        Vec::new(),
        vec![observed("a1", "1"), observed("a2", "2")],
    ]);
    let beta = ScriptedSource::pages(vec![
        Vec::new(),
        vec![observed("b1", "1"), observed("b2", "2")],
    ]);
    let workflow = Workflow::new(
        "inbox",
        vec![
            bundle_source("alpha", REBUILD, alpha),
            bundle_source("beta", RETAIN, beta),
        ],
    )
    .with_poll(TICK);
    let handle = tokio::spawn(run_workflow(
        workflow,
        agent,
        runtime.clone(),
        cancel.clone(),
    ));

    let event = tokio::time::timeout(PATIENCE, sub.recv())
        .await
        .expect("an event before timeout")
        .expect("event");
    let Event::WorkflowUpdate {
        workflow,
        items,
        overflow,
        ..
    } = event
    else {
        panic!("expected a workflow update");
    };
    let extra = tokio::time::timeout(Duration::from_millis(200), sub.recv()).await;
    cancel.cancel();
    handle.await.unwrap();

    assert_eq!(workflow, "inbox");
    assert_eq!(items.len(), 3, "the cap applies across the bundle");
    assert_eq!(overflow, 1);
    assert_eq!(items[0].integration.as_str(), "alpha");
    assert_eq!(items[2].integration.as_str(), "beta");
    assert!(
        extra.is_err(),
        "capped items are seen, not replayed on the next tick"
    );
}

async fn one_failing_source_does_not_silence_the_other() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = runtime_in(dir.path()).await;
    let agent = agent_in(&runtime).await;
    let mut sub = runtime.bus.subscribe(EventFilter::Agent(agent));
    let cancel = CancellationToken::new();

    let alpha = ScriptedSource::new(vec![
        Ok(WatchPage::new(Vec::new())),
        Ok(WatchPage::new(vec![observed("a1", "1")])),
    ]);
    let beta = ScriptedSource::always_failing(&IntegrationError::Service("boom".into()));
    let workflow = Workflow::new(
        "inbox",
        vec![
            bundle_source("alpha", REBUILD, alpha),
            bundle_source("beta", RETAIN, beta),
        ],
    )
    .with_poll(TICK);
    let handle = tokio::spawn(run_workflow(
        workflow,
        agent,
        runtime.clone(),
        cancel.clone(),
    ));

    let event = tokio::time::timeout(PATIENCE, sub.recv())
        .await
        .expect("an event before timeout")
        .expect("event");
    let Event::WorkflowUpdate { items, .. } = event else {
        panic!("expected a workflow update");
    };
    cancel.cancel();
    handle.await.unwrap();

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].integration.as_str(), "alpha");
    assert_eq!(items[0].summary, "a1 needs you");
}

pub fn sample_payload() -> Value {
    json!({ "key": "K-1", "stamp": "2026-03-01" })
}
