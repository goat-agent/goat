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

use crate::diff::DiffOps;
use crate::watch::{Observed, Watch, WatchPage, WatchSource, run};
use crate::{IntegrationError, IntegrationResult, IntegrationRuntime};

const TICK: Duration = Duration::from_millis(10);
const PATIENCE: Duration = Duration::from_secs(5);

pub async fn runtime_in(dir: &Path) -> IntegrationRuntime {
    let store = SqliteStore::open(&dir.join("goat.db")).await.unwrap();
    IntegrationRuntime {
        credentials: CredentialStore::new(dir.join("credentials.json")),
        store: Arc::new(store),
        bus: EventBus::new(),
    }
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
    pub overflow_tail: &'static str,
    pub diff: DiffOps,
}

impl WatchContract {
    fn watch<S: WatchSource>(&self, source: S) -> Watch<S> {
        Watch::new(
            self.integration.clone(),
            self.stream.clone(),
            self.kind,
            self.entity,
            self.overflow_tail,
            self.diff,
            source,
        )
        .with_poll(TICK)
    }
}

pub async fn assert_watch_contract(contract: &WatchContract) {
    cold_start_briefs_nothing(contract).await;
    a_burst_is_capped_with_an_overflow_note(contract).await;
    repeated_auth_failures_alert_exactly_once(contract).await;
    an_observation_round_trips_losslessly(contract).await;
    unreadable_state_starts_cold_instead_of_replaying(contract).await;
}

async fn cold_start_briefs_nothing(contract: &WatchContract) {
    let dir = tempfile::tempdir().unwrap();
    let runtime = runtime_in(dir.path()).await;
    let agent = agent_in(&runtime).await;
    let mut sub = runtime.bus.subscribe(EventFilter::Persona(agent));
    let cancel = CancellationToken::new();

    let backlog: Vec<Observed> = (0..40)
        .map(|n| observed(&format!("old{n}"), &format!("2026-01-{n:02}")))
        .collect();
    let source = ScriptedSource::pages(vec![backlog]);
    let handle = tokio::spawn(run(
        contract.watch(source),
        agent,
        runtime.clone(),
        "default".into(),
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

async fn a_burst_is_capped_with_an_overflow_note(contract: &WatchContract) {
    let dir = tempfile::tempdir().unwrap();
    let runtime = runtime_in(dir.path()).await;
    let agent = agent_in(&runtime).await;
    let mut sub = runtime.bus.subscribe(EventFilter::Persona(agent));
    let cancel = CancellationToken::new();

    let burst: Vec<Observed> = (0..5)
        .map(|n| observed(&format!("new{n}"), &format!("2026-02-{n:02}")))
        .collect();
    let source = ScriptedSource::new(vec![
        Ok(WatchPage::new(Vec::new())),
        Ok(WatchPage::new(burst.clone())),
        Ok(WatchPage::new(burst)),
    ]);
    let handle = tokio::spawn(run(
        contract.watch(source),
        agent,
        runtime.clone(),
        "default".into(),
        cancel.clone(),
    ));

    let mut summaries = Vec::new();
    while summaries.len() < 3 {
        let event = tokio::time::timeout(PATIENCE, sub.recv())
            .await
            .expect("events before timeout")
            .expect("event");
        if let Event::IntegrationUpdate { summary, kind, .. } = event
            && kind != IntegrationUpdateKind::AuthBroken
        {
            summaries.push(summary);
        }
    }
    let extra = tokio::time::timeout(Duration::from_millis(200), sub.recv()).await;
    cancel.cancel();
    handle.await.unwrap();

    assert_eq!(summaries.len(), 3, "a burst must be capped");
    assert!(extra.is_err(), "nothing beyond the cap may be published");
    let last = summaries.last().unwrap();
    assert!(
        last.contains("+2 more") && last.contains(contract.overflow_tail),
        "the last briefing must carry the overflow note, got {last}"
    );
}

async fn repeated_auth_failures_alert_exactly_once(contract: &WatchContract) {
    let dir = tempfile::tempdir().unwrap();
    let runtime = runtime_in(dir.path()).await;
    let agent = agent_in(&runtime).await;
    let mut sub = runtime.bus.subscribe(EventFilter::Persona(agent));
    let cancel = CancellationToken::new();

    let source = ScriptedSource::always_failing(&IntegrationError::Auth("401".into()));
    let handle = tokio::spawn(run(
        contract.watch(source),
        agent,
        runtime.clone(),
        "default".into(),
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
    let mut sub = runtime.bus.subscribe(EventFilter::Persona(agent));
    let cancel = CancellationToken::new();

    let source = ScriptedSource::new(vec![
        Ok(WatchPage::new(Vec::new())),
        Ok(WatchPage::new(vec![observed("K-1", "2026-03-01")])),
    ]);
    let handle = tokio::spawn(run(
        contract.watch(source),
        agent,
        runtime.clone(),
        "default".into(),
        cancel.clone(),
    ));

    let event = tokio::time::timeout(PATIENCE, sub.recv())
        .await
        .expect("an event before timeout")
        .expect("event");
    let Event::IntegrationUpdate {
        kind,
        external_ref,
        observation,
        ..
    } = event
    else {
        panic!("unexpected event type");
    };
    cancel.cancel();
    handle.await.unwrap();

    assert_eq!(kind, contract.kind);
    assert_eq!(
        external_ref,
        format!(
            "{}/default:{}:K-1",
            contract.integration.as_str(),
            contract.entity
        )
    );
    let record = runtime
        .store
        .get_observation(observation.expect("an observation id"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.external_ref, external_ref);
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
    )
    .await
    .unwrap();
    assert!(
        loaded.is_none(),
        "unreadable state must resolve to a cold start, not a panic or a replay"
    );
}

pub fn sample_payload() -> Value {
    json!({ "key": "K-1", "stamp": "2026-03-01" })
}
