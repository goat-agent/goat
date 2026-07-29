use std::fmt::Write as _;
use std::future::Future;
use std::time::Duration;

use goat_integration::{IntegrationError, IntegrationResult, IntegrationRuntime};
use goat_types::{Event, IntegrationUpdateKind, ProfileId};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::ID;
use crate::diff::{WatchState, diff};
use crate::gh;
use crate::parse::{FetchPage, Item, parse_page};

pub const DEFAULT_LIMIT: usize = 50;
const POLL: Duration = Duration::from_mins(2);
const MAX_BACKOFF_TICKS: u32 = 8;
const AUTH_ALERT_STREAK: u32 = 5;
const EVENT_CAP_PER_POLL: usize = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchQuery {
    pub stream: String,
    pub query: String,
}

pub fn default_queries() -> Vec<WatchQuery> {
    vec![
        WatchQuery {
            stream: "review".into(),
            query: "is:open is:pr review-requested:@me".into(),
        },
        WatchQuery {
            stream: "assigned".into(),
            query: "is:open assignee:@me".into(),
        },
    ]
}

pub trait FetchItems: Send + Sync + 'static {
    fn fetch(&self, query: &str) -> impl Future<Output = IntegrationResult<FetchPage>> + Send;
}

pub struct GhFetch {
    pub limit: usize,
}

impl FetchItems for GhFetch {
    async fn fetch(&self, query: &str) -> IntegrationResult<FetchPage> {
        parse_page(&gh::search(query, self.limit).await?)
    }
}

pub fn backoff_skips(error_streak: u32) -> u32 {
    2u32.saturating_pow(error_streak)
        .min(MAX_BACKOFF_TICKS)
        .saturating_sub(1)
}

pub async fn run<F: FetchItems>(
    persona: ProfileId,
    runtime: IntegrationRuntime,
    account: String,
    queries: Vec<WatchQuery>,
    fetch: F,
    cancel: CancellationToken,
) {
    run_with_poll(persona, runtime, account, queries, fetch, cancel, POLL).await;
}

pub async fn run_with_poll<F: FetchItems>(
    persona: ProfileId,
    runtime: IntegrationRuntime,
    account: String,
    queries: Vec<WatchQuery>,
    fetch: F,
    cancel: CancellationToken,
    poll: Duration,
) {
    info!(
        profile = %persona,
        account = %account,
        streams = queries.len(),
        "github watcher running",
    );
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

        let mut failure: Option<IntegrationError> = None;
        for watch in &queries {
            match fetch.fetch(&watch.query).await {
                Err(e) => {
                    warn!(
                        profile = %persona,
                        account = %account,
                        stream = %watch.stream,
                        error = %e,
                        "github poll failed",
                    );
                    if failure.is_none() || matches!(e, IntegrationError::Auth(_)) {
                        failure = Some(e);
                    }
                }
                Ok(page) => {
                    if let Err(e) = process(persona, &runtime, &account, &watch.stream, &page).await
                    {
                        warn!(
                            profile = %persona,
                            account = %account,
                            stream = %watch.stream,
                            error = %e,
                            "github poll processing failed",
                        );
                    }
                }
            }
        }

        if let Some(e) = failure {
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
        } else {
            error_streak = 0;
            auth_streak = 0;
            auth_alerted = false;
        }
    }
    info!(profile = %persona, account = %account, "github watcher stopped");
}

fn auth_broken_event(persona: ProfileId, account: &str, error: &IntegrationError) -> Event {
    Event::IntegrationUpdate {
        profile: persona,
        integration: ID,
        account: account.to_string(),
        kind: IntegrationUpdateKind::AuthBroken,
        external_ref: format!("github/{account}:auth"),
        summary: format!(
            "github polling keeps failing to authenticate ({error}); \
             sign the `gh` cli back in with `gh auth login`"
        ),
        observation: None,
    }
}

async fn process(
    persona: ProfileId,
    runtime: &IntegrationRuntime,
    account: &str,
    stream: &str,
    page: &FetchPage,
) -> IntegrationResult<()> {
    let prev: Option<WatchState> = runtime
        .load_state(persona, &ID, account, stream)
        .await?
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok());
    let (mut next, fresh) = diff(prev.as_ref(), &page.items);
    if page.truncated
        && let Some(prev) = &prev
    {
        warn!(
            profile = %persona,
            stream = %stream,
            "github result page truncated; keeping previously seen items",
        );
        for (key, seen_at) in &prev.seen {
            next.seen
                .entry(key.clone())
                .or_insert_with(|| seen_at.clone());
        }
    }

    let mut events = Vec::new();
    for item in &fresh {
        match observe(persona, runtime, account, stream, item).await {
            Ok(event) => events.push(event),
            Err(e) => {
                warn!(
                    item = %item.key,
                    error = %e,
                    "failed to record github observation; retrying next poll",
                );
                revert_seen(&mut next, prev.as_ref(), &item.key);
            }
        }
    }

    let raw = serde_json::to_string(&next).map_err(|e| IntegrationError::Store(e.to_string()))?;
    runtime
        .save_state(persona, &ID, account, stream, &raw)
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
            let _ = write!(summary, " (+{overflow} more waiting on you)");
        }
        runtime.publish(event);
    }
    Ok(())
}

fn revert_seen(next: &mut WatchState, prev: Option<&WatchState>, key: &str) {
    match prev.and_then(|state| state.seen.get(key)) {
        Some(seen_at) => {
            next.seen.insert(key.to_string(), seen_at.clone());
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
    stream: &str,
    item: &Item,
) -> IntegrationResult<Event> {
    let external_ref = format!("github/{account}:{stream}:{}", item.key);
    let observation = runtime
        .record_observation(
            persona,
            &ID,
            account,
            &external_ref,
            IntegrationUpdateKind::Assigned.as_str(),
            item.raw.clone(),
        )
        .await?;
    Ok(Event::IntegrationUpdate {
        profile: persona,
        integration: ID,
        account: account.to_string(),
        kind: IntegrationUpdateKind::Assigned,
        external_ref,
        summary: item.summary(),
        observation: Some(observation),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use goat_auth::CredentialStore;
    use goat_bus::{EventBus, EventFilter};
    use goat_store::SqliteStore;
    use serde_json::json;

    struct ScriptedFetch {
        batches: Mutex<HashMap<String, VecDeque<FetchPage>>>,
        last: Mutex<HashMap<String, Vec<Item>>>,
    }

    impl ScriptedFetch {
        fn new(batches: Vec<(&str, Vec<FetchPage>)>) -> Self {
            Self {
                batches: Mutex::new(
                    batches
                        .into_iter()
                        .map(|(query, pages)| (query.to_string(), pages.into()))
                        .collect(),
                ),
                last: Mutex::new(HashMap::new()),
            }
        }
    }

    impl FetchItems for ScriptedFetch {
        async fn fetch(&self, query: &str) -> IntegrationResult<FetchPage> {
            let next = self
                .batches
                .lock()
                .unwrap()
                .get_mut(query)
                .and_then(VecDeque::pop_front);
            match next {
                Some(page) => {
                    self.last
                        .lock()
                        .unwrap()
                        .insert(query.to_string(), page.items.clone());
                    Ok(page)
                }
                None => Ok(page(
                    self.last
                        .lock()
                        .unwrap()
                        .get(query)
                        .cloned()
                        .unwrap_or_default(),
                )),
            }
        }
    }

    fn item(repo: &str, number: u64, updated_at: &str) -> Item {
        Item {
            key: format!("{repo}#{number}"),
            repo: repo.into(),
            number,
            title: "needs your review".into(),
            updated_at: updated_at.into(),
            is_pr: true,
            raw: json!({ "number": number, "repository_url": format!("https://api.github.com/repos/{repo}") }),
        }
    }

    fn page(items: Vec<Item>) -> FetchPage {
        FetchPage {
            items,
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
    fn the_default_streams_watch_what_is_waiting_on_the_owner() {
        let queries = default_queries();
        assert_eq!(queries.len(), 2);
        assert!(queries.iter().all(|watch| watch.query.contains("@me")));
        assert!(
            queries
                .iter()
                .any(|watch| watch.query.contains("review-requested:@me"))
        );
    }

    #[test]
    fn revert_seen_restores_the_previous_entry() {
        let prev = diff(None, &[item("acme/a", 1, "t1")]).0;
        let (mut next, _) = diff(Some(&prev), &[item("acme/a", 1, "t2")]);
        revert_seen(&mut next, Some(&prev), "acme/a#1");
        assert_eq!(next.seen.get("acme/a#1").map(String::as_str), Some("t1"));
        revert_seen(&mut next, None, "acme/a#1");
        assert!(!next.seen.contains_key("acme/a#1"));
    }

    #[tokio::test]
    async fn cold_start_briefs_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime_in(dir.path()).await;
        let persona = persona_in(&runtime).await;
        let mut sub = runtime.bus.subscribe(EventFilter::Persona(persona));

        let history: Vec<Item> = (1..=40)
            .map(|number| item("acme/a", number, "t1"))
            .collect();
        process(persona, &runtime, "default", "review", &page(history))
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
    async fn streams_keep_independent_state() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime_in(dir.path()).await;
        let persona = persona_in(&runtime).await;
        let mut sub = runtime.bus.subscribe(EventFilter::Persona(persona));

        let shared = vec![item("acme/a", 1, "t1")];
        process(
            persona,
            &runtime,
            "default",
            "review",
            &page(shared.clone()),
        )
        .await
        .unwrap();
        process(persona, &runtime, "default", "assigned", &page(vec![]))
            .await
            .unwrap();
        process(persona, &runtime, "default", "assigned", &page(shared))
            .await
            .unwrap();
        drop(runtime);

        let mut refs = Vec::new();
        while let Some(event) = sub.recv().await {
            if let Event::IntegrationUpdate { external_ref, .. } = event {
                refs.push(external_ref);
            }
        }
        assert_eq!(
            refs,
            vec!["github/default:assigned:acme/a#1".to_string()],
            "the same item is news once per stream, and review was still cold",
        );
    }

    #[tokio::test]
    async fn a_truncated_page_does_not_forget_what_scrolled_off() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime_in(dir.path()).await;
        let persona = persona_in(&runtime).await;
        let mut sub = runtime.bus.subscribe(EventFilter::Persona(persona));

        process(
            persona,
            &runtime,
            "default",
            "review",
            &page(vec![item("acme/a", 1, "t1")]),
        )
        .await
        .unwrap();
        process(
            persona,
            &runtime,
            "default",
            "review",
            &FetchPage {
                items: vec![item("acme/a", 2, "t2")],
                truncated: true,
            },
        )
        .await
        .unwrap();
        process(
            persona,
            &runtime,
            "default",
            "review",
            &page(vec![item("acme/a", 1, "t1")]),
        )
        .await
        .unwrap();
        drop(runtime);

        let mut refs = Vec::new();
        while let Some(event) = sub.recv().await {
            if let Event::IntegrationUpdate { external_ref, .. } = event {
                refs.push(external_ref);
            }
        }
        assert_eq!(
            refs,
            vec!["github/default:review:acme/a#2".to_string()],
            "#1 fell off a truncated page and must not re-fire",
        );
    }

    #[tokio::test]
    async fn burst_is_capped_with_an_overflow_note() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime_in(dir.path()).await;
        let persona = persona_in(&runtime).await;
        let mut sub = runtime.bus.subscribe(EventFilter::Persona(persona));

        process(persona, &runtime, "default", "review", &page(vec![]))
            .await
            .unwrap();
        let burst: Vec<Item> = (1..=5).map(|number| item("acme/a", number, "t1")).collect();
        process(persona, &runtime, "default", "review", &page(burst))
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
        assert!(summaries.last().unwrap().contains("+2 more waiting on you"));
    }

    #[tokio::test]
    async fn watcher_publishes_once_per_item_with_a_lossless_observation() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime_in(dir.path()).await;
        let persona = persona_in(&runtime).await;
        let mut sub = runtime.bus.subscribe(EventFilter::Persona(persona));
        let cancel = CancellationToken::new();

        let fetch = ScriptedFetch::new(vec![(
            "review-requested:@me",
            vec![
                page(vec![item("acme/a", 1, "t1")]),
                page(vec![item("acme/a", 1, "t1"), item("acme/b", 2, "t2")]),
            ],
        )]);
        let handle = tokio::spawn(run_with_poll(
            persona,
            runtime.clone(),
            "default".into(),
            vec![WatchQuery {
                stream: "review".into(),
                query: "review-requested:@me".into(),
            }],
            fetch,
            cancel.clone(),
            Duration::from_millis(10),
        ));

        let event = tokio::time::timeout(Duration::from_secs(5), sub.recv())
            .await
            .expect("event before timeout")
            .expect("item event");
        let Event::IntegrationUpdate {
            kind,
            external_ref,
            observation,
            ..
        } = event
        else {
            panic!("unexpected event type");
        };
        assert_eq!(kind, IntegrationUpdateKind::Assigned);
        assert_eq!(external_ref, "github/default:review:acme/b#2");

        cancel.cancel();
        handle.await.unwrap();

        let record = runtime
            .store
            .get_observation(observation.expect("observation id"))
            .await
            .unwrap()
            .expect("observation recorded");
        assert_eq!(record.payload["number"], 2);
        assert_eq!(record.external_ref, "github/default:review:acme/b#2");

        drop(runtime);
        let mut extra = 0;
        while let Some(event) = sub.recv().await {
            if matches!(event, Event::IntegrationUpdate { .. }) {
                extra += 1;
            }
        }
        assert_eq!(extra, 0, "re-polls must not duplicate events");
    }
}
