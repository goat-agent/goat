use std::fmt::Write as _;
use std::future::Future;
use std::time::Duration;

use goat_auth::CredentialStore;
use goat_integration::{IntegrationError, IntegrationResult, IntegrationRuntime};
use goat_types::{Event, IntegrationUpdateKind, ProfileId};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::diff::{WatchState, diff};
use crate::parse::{FetchPage, Issue, parse_page};
use crate::{ID, mcp};

pub const STREAM: &str = "issues";
pub const DEFAULT_QUERY: &str = "is:unresolved is:for_review";
pub const DEFAULT_SORT: &str = "new";
const POLL: Duration = Duration::from_mins(2);
const MAX_BACKOFF_TICKS: u32 = 8;
const AUTH_ALERT_STREAK: u32 = 5;
const EVENT_CAP_PER_POLL: usize = 3;

pub trait FetchIssues: Send + Sync + 'static {
    fn fetch(&self) -> impl Future<Output = IntegrationResult<FetchPage>> + Send;
}

pub struct McpFetch {
    pub credentials: CredentialStore,
    pub account: String,
    pub client_id: Option<String>,
    pub organization_slug: String,
    pub project: Option<String>,
    pub query: String,
    pub sort: String,
}

impl FetchIssues for McpFetch {
    async fn fetch(&self) -> IntegrationResult<FetchPage> {
        let auth = mcp::resolve_auth(&self.credentials, &self.account, self.client_id.as_deref())?;
        let session = mcp::connect(&auth).await?;
        let mut arguments = json!({
            "organizationSlug": self.organization_slug,
            "query": self.query,
            "sort": self.sort,
        });
        if let Some(project) = &self.project {
            arguments["projectSlugOrId"] = json!(project);
        }
        let result = session.call(mcp::TOOL_SEARCH_ISSUES, arguments).await;
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

pub async fn run<F: FetchIssues>(
    persona: ProfileId,
    runtime: IntegrationRuntime,
    account: String,
    fetch: F,
    cancel: CancellationToken,
) {
    run_with_poll(persona, runtime, account, fetch, cancel, POLL).await;
}

pub async fn run_with_poll<F: FetchIssues>(
    persona: ProfileId,
    runtime: IntegrationRuntime,
    account: String,
    fetch: F,
    cancel: CancellationToken,
    poll: Duration,
) {
    info!(profile = %persona, account = %account, "sentry watcher running");
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
                    "sentry poll failed; backing off",
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
                        "sentry poll processing failed",
                    );
                }
            }
        }
    }
    info!(profile = %persona, account = %account, "sentry watcher stopped");
}

fn auth_broken_event(persona: ProfileId, account: &str, error: &IntegrationError) -> Event {
    Event::IntegrationUpdate {
        profile: persona,
        integration: ID,
        account: account.to_string(),
        kind: IntegrationUpdateKind::AuthBroken,
        external_ref: format!("sentry/{account}:auth"),
        summary: format!(
            "sentry polling keeps failing to authenticate ({error}); \
             reconnect with `goat integration add sentry`"
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
    let prev: Option<WatchState> = runtime
        .load_state(persona, &ID, account, STREAM)
        .await?
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok());
    let (mut next, fresh) = diff(prev.as_ref(), &page.issues);

    let mut events = Vec::new();
    for issue in &fresh {
        match observe(persona, runtime, account, issue).await {
            Ok(event) => events.push(event),
            Err(e) => {
                warn!(
                    issue = %issue.key,
                    error = %e,
                    "failed to record sentry observation; retrying next poll",
                );
                revert_seen(&mut next, prev.as_ref(), &issue.key);
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
            let _ = write!(summary, " (+{overflow} more issues waiting)");
        }
        runtime.publish(event);
    }
    Ok(())
}

fn revert_seen(next: &mut WatchState, prev: Option<&WatchState>, key: &str) {
    match prev.and_then(|state| state.seen.get(key)) {
        Some(last_seen) => {
            next.seen.insert(key.to_string(), last_seen.clone());
        }
        None => {
            next.seen.remove(key);
        }
    }
}

async fn observe(
    persona: ProfileId,
    runtime: &IntegrationRuntime,
    account: &str,
    issue: &Issue,
) -> IntegrationResult<Event> {
    let external_ref = format!("sentry/{account}:issue:{}", issue.key);
    let observation = runtime
        .record_observation(
            persona,
            &ID,
            account,
            &external_ref,
            IntegrationUpdateKind::Updated.as_str(),
            issue.raw.clone(),
        )
        .await?;
    Ok(Event::IntegrationUpdate {
        profile: persona,
        integration: ID,
        account: account.to_string(),
        kind: IntegrationUpdateKind::Updated,
        external_ref,
        summary: issue.summary(),
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
        last: Mutex<Vec<Issue>>,
    }

    impl ScriptedFetch {
        fn new(batches: Vec<FetchPage>) -> Self {
            Self {
                batches: Mutex::new(batches.into()),
                last: Mutex::new(Vec::new()),
            }
        }
    }

    impl FetchIssues for ScriptedFetch {
        async fn fetch(&self) -> IntegrationResult<FetchPage> {
            let mut batches = self.batches.lock().unwrap();
            if let Some(page) = batches.pop_front() {
                *self.last.lock().unwrap() = page.issues.clone();
                Ok(page)
            } else {
                Ok(FetchPage {
                    issues: self.last.lock().unwrap().clone(),
                })
            }
        }
    }

    struct RejectingFetch;

    impl FetchIssues for RejectingFetch {
        async fn fetch(&self) -> IntegrationResult<FetchPage> {
            Err(IntegrationError::Auth(
                "sentry rejected the credential".into(),
            ))
        }
    }

    fn issue_at(key: &str, last_seen: &str) -> Issue {
        Issue {
            key: key.into(),
            short_id: key.into(),
            title: "TypeError: boom".into(),
            culprit: "app/handlers".into(),
            count: "3".into(),
            user_count: "2".into(),
            last_seen: last_seen.into(),
            raw: json!({ "shortId": key, "lastSeen": last_seen }),
        }
    }

    fn page(issues: Vec<Issue>) -> FetchPage {
        FetchPage { issues }
    }

    async fn runtime_in(dir: &std::path::Path) -> IntegrationRuntime {
        let store = SqliteStore::open(&dir.join("goat.db")).await.unwrap();
        IntegrationRuntime {
            credentials: CredentialStore::new(dir.join("credentials.json")),
            store: Arc::new(store),
            bus: EventBus::new(),
        }
    }

    async fn persona_in(runtime: &IntegrationRuntime) -> ProfileId {
        let persona = ProfileId::from_slug("test");
        runtime
            .store
            .ensure_persona(persona, "test", "test")
            .await
            .unwrap();
        persona
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
        let prev = diff(None, &[issue_at("BACKEND-1A", "1")]).0;
        let (mut next, _) = diff(Some(&prev), &[issue_at("BACKEND-2B", "2")]);
        revert_seen(&mut next, Some(&prev), "BACKEND-1A");
        assert_eq!(next.seen.get("BACKEND-1A").map(String::as_str), Some("1"));
        revert_seen(&mut next, None, "BACKEND-2B");
        assert!(!next.seen.contains_key("BACKEND-2B"));
    }

    #[tokio::test]
    async fn cold_start_briefs_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime_in(dir.path()).await;
        let persona = persona_in(&runtime).await;
        let mut sub = runtime.bus.subscribe(EventFilter::Persona(persona));

        let backlog: Vec<Issue> = (1..=40)
            .map(|n| issue_at(&format!("BACKEND-{n}"), &format!("{n}")))
            .collect();
        process(persona, &runtime, "default", &page(backlog))
            .await
            .unwrap();
        drop(runtime);

        let mut seen = 0;
        while let Some(event) = sub.recv().await {
            if matches!(event, Event::IntegrationUpdate { .. }) {
                seen += 1;
            }
        }
        assert_eq!(seen, 0, "the first poll must only baseline");
    }

    #[tokio::test]
    async fn burst_is_capped_with_overflow_note() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime_in(dir.path()).await;
        let persona = persona_in(&runtime).await;
        let mut sub = runtime.bus.subscribe(EventFilter::Persona(persona));

        process(persona, &runtime, "default", &page(vec![]))
            .await
            .unwrap();
        let burst: Vec<Issue> = (1..=5)
            .map(|n| issue_at(&format!("BACKEND-{n}"), &format!("{n}")))
            .collect();
        process(persona, &runtime, "default", &page(burst))
            .await
            .unwrap();
        drop(runtime);

        let mut summaries = Vec::new();
        while let Some(event) = sub.recv().await {
            if let Event::IntegrationUpdate { summary, .. } = event {
                summaries.push(summary);
            }
        }
        assert_eq!(summaries.len(), EVENT_CAP_PER_POLL);
        assert!(summaries.last().unwrap().contains("+2 more issues waiting"));
    }

    #[tokio::test]
    async fn watcher_publishes_once_per_issue_with_a_lossless_observation() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime_in(dir.path()).await;
        let persona = persona_in(&runtime).await;
        let mut sub = runtime.bus.subscribe(EventFilter::Persona(persona));
        let cancel = CancellationToken::new();

        let fetch = ScriptedFetch::new(vec![
            page(vec![issue_at("BACKEND-1A", "1")]),
            page(vec![
                issue_at("BACKEND-1A", "1"),
                issue_at("BACKEND-2B", "2"),
            ]),
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
            .expect("issue event");
        let Event::IntegrationUpdate {
            kind,
            external_ref,
            observation,
            ..
        } = event
        else {
            panic!("unexpected event type");
        };
        assert_eq!(kind, IntegrationUpdateKind::Updated);
        assert_eq!(external_ref, "sentry/default:issue:BACKEND-2B");

        cancel.cancel();
        handle.await.unwrap();

        let record = runtime
            .store
            .get_observation(observation.expect("observation id"))
            .await
            .unwrap()
            .expect("observation recorded");
        assert_eq!(record.payload["shortId"], "BACKEND-2B");
        assert_eq!(record.external_ref, "sentry/default:issue:BACKEND-2B");

        drop(runtime);
        let mut extra = 0;
        while let Some(event) = sub.recv().await {
            if matches!(event, Event::IntegrationUpdate { .. }) {
                extra += 1;
            }
        }
        assert_eq!(extra, 0, "re-polls must not duplicate events");
    }

    #[tokio::test]
    async fn repeated_auth_failures_alert_exactly_once() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime_in(dir.path()).await;
        let persona = persona_in(&runtime).await;
        let mut sub = runtime.bus.subscribe(EventFilter::Persona(persona));
        let cancel = CancellationToken::new();

        let handle = tokio::spawn(run_with_poll(
            persona,
            runtime.clone(),
            "default".into(),
            RejectingFetch,
            cancel.clone(),
            Duration::from_millis(5),
        ));

        let event = tokio::time::timeout(Duration::from_secs(10), sub.recv())
            .await
            .expect("event before timeout")
            .expect("auth event");
        let Event::IntegrationUpdate {
            kind, external_ref, ..
        } = event
        else {
            panic!("unexpected event type");
        };
        assert_eq!(kind, IntegrationUpdateKind::AuthBroken);
        assert_eq!(external_ref, "sentry/default:auth");

        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel.cancel();
        handle.await.unwrap();
        drop(runtime);

        let mut extra = 0;
        while let Some(event) = sub.recv().await {
            if matches!(event, Event::IntegrationUpdate { .. }) {
                extra += 1;
            }
        }
        assert_eq!(extra, 0, "a broken credential must alert only once");
    }
}
