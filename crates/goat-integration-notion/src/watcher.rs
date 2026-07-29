use std::fmt::Write as _;
use std::future::Future;
use std::sync::OnceLock;
use std::time::Duration;

use goat_auth::CredentialStore;
use goat_integration::{IntegrationError, IntegrationResult, IntegrationRuntime};
use goat_types::{Event, IntegrationUpdateKind, ProfileId};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::diff::{WatchState, diff};
use crate::parse::{FetchPage, ViewRow, parse_page};
use crate::{ID, mcp};

pub const STREAM: &str = "view";
const POLL: Duration = Duration::from_mins(2);
const FETCH_LIMIT: usize = 50;
const MAX_BACKOFF_TICKS: u32 = 8;
const AUTH_ALERT_STREAK: u32 = 5;
const EVENT_CAP_PER_POLL: usize = 3;

pub trait FetchView: Send + Sync + 'static {
    fn fetch(&self) -> impl Future<Output = IntegrationResult<FetchPage>> + Send;
}

pub struct McpFetch {
    pub credentials: CredentialStore,
    pub account: String,
    pub client_id: Option<String>,
    pub view_url: String,
    pub query_tool: Option<String>,
    pub resolved_tool: OnceLock<String>,
}

impl McpFetch {
    async fn resolve_tool(&self, session: &mcp::NotionSession) -> IntegrationResult<String> {
        if let Some(tool) = &self.query_tool {
            return Ok(tool.clone());
        }
        if let Some(tool) = self.resolved_tool.get() {
            return Ok(tool.clone());
        }
        let tools = session.list_tools().await?;
        let names: Vec<String> = tools.iter().map(|tool| tool.name.to_string()).collect();
        let picked = mcp::pick_tool(
            names.iter().map(String::as_str),
            mcp::VIEW_TOOL_CANDIDATES,
        )
        .ok_or_else(|| {
            IntegrationError::Service(format!(
                "notion mcp exposes no database query tool; set `query_tool` in the binding. available: {}",
                names.join(", ")
            ))
        })?;
        let _ = self.resolved_tool.set(picked.clone());
        Ok(picked)
    }
}

impl FetchView for McpFetch {
    async fn fetch(&self) -> IntegrationResult<FetchPage> {
        let auth = mcp::resolve_auth(&self.credentials, &self.account, self.client_id.as_deref())?;
        let session = mcp::connect(&auth).await?;
        let result = match self.resolve_tool(&session).await {
            Ok(tool) => {
                session
                    .call(
                        &tool,
                        json!({
                            "data": {
                                "mode": "view",
                                "view_url": self.view_url,
                                "page_size": FETCH_LIMIT,
                            }
                        }),
                    )
                    .await
            }
            Err(e) => Err(e),
        };
        mcp::persist_tokens(&self.credentials, &self.account, &session).await;
        session.close().await;
        parse_page(&result?)
    }
}

pub fn backoff_skips(error_streak: u32) -> u32 {
    2u32.saturating_pow(error_streak)
        .min(MAX_BACKOFF_TICKS)
        .saturating_sub(1)
}

pub async fn run<F: FetchView>(
    persona: ProfileId,
    runtime: IntegrationRuntime,
    account: String,
    fetch: F,
    cancel: CancellationToken,
) {
    run_with_poll(persona, runtime, account, fetch, cancel, POLL).await;
}

pub async fn run_with_poll<F: FetchView>(
    persona: ProfileId,
    runtime: IntegrationRuntime,
    account: String,
    fetch: F,
    cancel: CancellationToken,
    poll: Duration,
) {
    info!(profile = %persona, account = %account, "notion watcher running");
    let mut interval = tokio::time::interval(poll);
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
        match fetch.fetch().await {
            Err(e) => {
                error_streak += 1;
                skip_ticks = backoff_skips(error_streak);
                if matches!(e, IntegrationError::Auth(_)) {
                    auth_streak += 1;
                    if auth_streak >= AUTH_ALERT_STREAK && !auth_alerted {
                        auth_alerted = true;
                        runtime.publish(auth_broken_event(persona, &account, &e));
                    }
                } else {
                    auth_streak = 0;
                }
                warn!(
                    profile = %persona,
                    account = %account,
                    error = %e,
                    "notion poll failed; backing off",
                );
            }
            Ok(page) => {
                error_streak = 0;
                auth_streak = 0;
                auth_alerted = false;
                if let Err(e) = process(persona, &runtime, &account, &page).await {
                    warn!(
                        profile = %persona,
                        account = %account,
                        error = %e,
                        "notion poll processing failed",
                    );
                }
            }
        }
    }
    info!(profile = %persona, account = %account, "notion watcher stopped");
}

fn auth_broken_event(persona: ProfileId, account: &str, error: &IntegrationError) -> Event {
    Event::IntegrationUpdate {
        profile: persona,
        integration: ID,
        account: account.to_string(),
        kind: IntegrationUpdateKind::AuthBroken,
        external_ref: format!("notion/{account}:auth"),
        summary: format!(
            "notion polling keeps failing to authenticate ({error}); \
             reconnect with `goat integration add notion`"
        ),
        observation: None,
    }
}

async fn process(
    persona: ProfileId,
    runtime: &IntegrationRuntime,
    account: &str,
    page: &FetchPage,
) -> IntegrationResult<()> {
    if page.truncated {
        warn!(
            profile = %persona,
            limit = FETCH_LIMIT,
            "notion view page truncated; skipping removal detection this poll",
        );
    }
    let prev: Option<WatchState> = runtime
        .load_state(persona, &ID, account, STREAM)
        .await?
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok());
    let (mut next, entered) = diff(prev.as_ref(), &page.rows);
    if page.truncated
        && let Some(prev) = &prev
    {
        for (id, seen_at) in &prev.seen {
            next.seen
                .entry(id.clone())
                .or_insert_with(|| seen_at.clone());
        }
    }

    let mut events = Vec::new();
    for row in &entered {
        match observe(persona, runtime, account, row).await {
            Ok(event) => events.push(event),
            Err(e) => {
                warn!(
                    row = %row.id,
                    error = %e,
                    "failed to record notion observation; retrying next poll",
                );
                revert_seen(&mut next, prev.as_ref(), &row.id);
            }
        }
    }

    let raw = serde_json::to_string(&next).map_err(|e| IntegrationError::Store(e.to_string()))?;
    runtime
        .save_state(persona, &ID, account, STREAM, &raw)
        .await?;

    let overflow = events.len().saturating_sub(EVENT_CAP_PER_POLL);
    for (index, mut event) in events.into_iter().enumerate() {
        if index >= EVENT_CAP_PER_POLL {
            break;
        }
        if overflow > 0
            && index == EVENT_CAP_PER_POLL - 1
            && let Event::IntegrationUpdate { summary, .. } = &mut event
        {
            let _ = write!(summary, " (+{overflow} more in the view)");
        }
        runtime.publish(event);
    }
    Ok(())
}

fn revert_seen(next: &mut WatchState, prev: Option<&WatchState>, id: &str) {
    match prev.and_then(|p| p.seen.get(id)) {
        Some(seen_at) => {
            next.seen.insert(id.to_string(), seen_at.clone());
        }
        None => {
            next.seen.remove(id);
        }
    }
}

async fn observe(
    persona: ProfileId,
    runtime: &IntegrationRuntime,
    account: &str,
    row: &ViewRow,
) -> IntegrationResult<Event> {
    let external_ref = format!("notion/{account}:page:{}", row.id);
    let observation = runtime
        .record_observation(
            persona,
            &ID,
            account,
            &external_ref,
            IntegrationUpdateKind::Assigned.as_str(),
            row.raw.clone(),
        )
        .await?;
    Ok(Event::IntegrationUpdate {
        profile: persona,
        integration: ID,
        account: account.to_string(),
        kind: IntegrationUpdateKind::Assigned,
        external_ref,
        summary: row.summary(),
        observation: Some(observation),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use goat_bus::{EventBus, EventFilter};
    use goat_store::SqliteStore;

    struct ScriptedFetch {
        batches: Mutex<VecDeque<FetchPage>>,
        last: Mutex<Vec<ViewRow>>,
    }

    impl ScriptedFetch {
        fn new(batches: Vec<FetchPage>) -> Self {
            Self {
                batches: Mutex::new(batches.into()),
                last: Mutex::new(Vec::new()),
            }
        }
    }

    impl FetchView for ScriptedFetch {
        async fn fetch(&self) -> IntegrationResult<FetchPage> {
            let mut batches = self.batches.lock().unwrap();
            if let Some(page) = batches.pop_front() {
                *self.last.lock().unwrap() = page.rows.clone();
                Ok(page)
            } else {
                Ok(FetchPage {
                    rows: self.last.lock().unwrap().clone(),
                    truncated: false,
                })
            }
        }
    }

    fn row(id: &str, edited_at: &str) -> ViewRow {
        ViewRow {
            id: id.into(),
            title: format!("{id} title"),
            url: None,
            edited_at: edited_at.into(),
            raw: serde_json::json!({ "id": id }),
        }
    }

    fn page(rows: Vec<ViewRow>) -> FetchPage {
        FetchPage {
            rows,
            truncated: false,
        }
    }

    async fn runtime_in(dir: &std::path::Path) -> IntegrationRuntime {
        let store = SqliteStore::open(&dir.join("goat.db")).await.unwrap();
        IntegrationRuntime {
            credentials: CredentialStore::new(dir.join("credentials.json")),
            store: Arc::new(store),
            bus: EventBus::new(),
        }
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff_skips(0), 0);
        assert_eq!(backoff_skips(1), 1);
        assert_eq!(backoff_skips(2), 3);
        assert_eq!(backoff_skips(3), 7);
        assert_eq!(backoff_skips(10), 7);
    }

    #[test]
    fn revert_seen_restores_previous_entry() {
        let prev = diff(None, &[row("a", "t1")]).0;
        let (mut next, _) = diff(Some(&prev), &[row("a", "t2")]);
        revert_seen(&mut next, Some(&prev), "a");
        assert_eq!(next.seen.get("a").map(String::as_str), Some("t1"));
        revert_seen(&mut next, None, "a");
        assert!(!next.seen.contains_key("a"));
    }

    #[tokio::test]
    async fn truncated_page_keeps_unseen_entries() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime_in(dir.path()).await;
        let persona = ProfileId::from_slug("test");
        runtime
            .store
            .ensure_persona(persona, "test", "test")
            .await
            .unwrap();

        process(
            persona,
            &runtime,
            "default",
            &page(vec![row("a", "t1"), row("b", "t1")]),
        )
        .await
        .unwrap();

        process(
            persona,
            &runtime,
            "default",
            &FetchPage {
                rows: vec![row("a", "t1")],
                truncated: true,
            },
        )
        .await
        .unwrap();

        let state: WatchState = serde_json::from_str(
            &runtime
                .load_state(persona, &ID, "default", STREAM)
                .await
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert!(
            state.seen.contains_key("b"),
            "truncated poll must not treat missing rows as removed",
        );
    }

    #[tokio::test]
    async fn watcher_baselines_then_publishes_once_per_row() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime_in(dir.path()).await;
        let persona = ProfileId::from_slug("test");
        runtime
            .store
            .ensure_persona(persona, "test", "test")
            .await
            .unwrap();

        let mut sub = runtime.bus.subscribe(EventFilter::Persona(persona));
        let cancel = CancellationToken::new();
        let fetch = ScriptedFetch::new(vec![
            page(vec![row("a", "t1")]),
            page(vec![row("a", "t1"), row("b", "t1")]),
        ]);
        let handle = tokio::spawn(run_with_poll(
            persona,
            runtime.clone(),
            "default".into(),
            fetch,
            cancel.clone(),
            Duration::from_millis(10),
        ));

        let event = tokio::time::timeout(Duration::from_secs(5), sub.recv())
            .await
            .expect("event before timeout")
            .expect("view event");
        let Event::IntegrationUpdate {
            kind,
            external_ref,
            summary,
            observation,
            ..
        } = event
        else {
            panic!("unexpected event type");
        };
        assert_eq!(kind, IntegrationUpdateKind::Assigned);
        assert_eq!(external_ref, "notion/default:page:b");
        assert_eq!(summary, "b title");

        cancel.cancel();
        handle.await.unwrap();

        let record = runtime
            .store
            .get_observation(observation.expect("observation id"))
            .await
            .unwrap()
            .expect("observation recorded");
        assert_eq!(record.payload["id"], "b");
        assert_eq!(record.external_ref, "notion/default:page:b");

        drop(runtime);
        let mut extra = 0;
        while let Some(ev) = sub.recv().await {
            if matches!(ev, Event::IntegrationUpdate { .. }) {
                extra += 1;
            }
        }
        assert_eq!(extra, 0, "re-polls must not duplicate events");
    }

    #[tokio::test]
    async fn burst_is_capped_with_overflow_note() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime_in(dir.path()).await;
        let persona = ProfileId::from_slug("test");
        runtime
            .store
            .ensure_persona(persona, "test", "test")
            .await
            .unwrap();
        let mut sub = runtime.bus.subscribe(EventFilter::Persona(persona));

        process(persona, &runtime, "default", &page(vec![]))
            .await
            .unwrap();
        let burst: Vec<ViewRow> = (1..=5).map(|n| row(&format!("r{n}"), "t1")).collect();
        process(persona, &runtime, "default", &page(burst))
            .await
            .unwrap();
        drop(runtime);

        let mut summaries = Vec::new();
        while let Some(ev) = sub.recv().await {
            if let Event::IntegrationUpdate { summary, .. } = ev {
                summaries.push(summary);
            }
        }
        assert_eq!(summaries.len(), EVENT_CAP_PER_POLL);
        assert!(summaries.last().unwrap().contains("+2 more in the view"));
    }
}
