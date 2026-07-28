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
use crate::parse::{FetchPage, Mention, parse_page};
use crate::{ID, mcp};

pub const STREAM: &str = "mentions";
const POLL: Duration = Duration::from_mins(2);
const FETCH_LIMIT: usize = 50;
const MAX_BACKOFF_TICKS: u32 = 8;
const AUTH_ALERT_STREAK: u32 = 5;
const EVENT_CAP_PER_POLL: usize = 3;

pub trait FetchMentions: Send + Sync + 'static {
    fn fetch(&self) -> impl Future<Output = IntegrationResult<FetchPage>> + Send;
}

pub struct McpFetch {
    pub credentials: CredentialStore,
    pub account: String,
    pub query: String,
    pub search_tool: Option<String>,
    pub resolved_tool: OnceLock<String>,
}

impl McpFetch {
    async fn resolve_tool(&self, session: &mcp::SlackSession) -> IntegrationResult<String> {
        if let Some(tool) = &self.search_tool {
            return Ok(tool.clone());
        }
        if let Some(tool) = self.resolved_tool.get() {
            return Ok(tool.clone());
        }
        let names: Vec<String> = session
            .list_tools()
            .await?
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        let picked = mcp::pick_search_tool(names.iter().map(String::as_str)).ok_or_else(|| {
            IntegrationError::Service(format!(
                "slack mcp exposes no recognized search tool; set `search_tool` in the agent's \
                 slack binding (available: {})",
                names.join(", ")
            ))
        })?;
        let _ = self.resolved_tool.set(picked.clone());
        Ok(picked)
    }
}

impl FetchMentions for McpFetch {
    async fn fetch(&self) -> IntegrationResult<FetchPage> {
        let token = mcp::resolve_auth(&self.credentials, &self.account)?;
        let session = mcp::connect(&token).await?;
        let tool = match self.resolve_tool(&session).await {
            Ok(tool) => tool,
            Err(e) => {
                session.close().await;
                return Err(e);
            }
        };
        let result = session
            .call(&tool, json!({ "query": self.query, "limit": FETCH_LIMIT }))
            .await;
        session.close().await;
        parse_page(&result?)
    }
}

pub fn backoff_skips(error_streak: u32) -> u32 {
    2u32.saturating_pow(error_streak)
        .min(MAX_BACKOFF_TICKS)
        .saturating_sub(1)
}

pub async fn run<F: FetchMentions>(
    persona: ProfileId,
    runtime: IntegrationRuntime,
    account: String,
    self_id: String,
    fetch: F,
    cancel: CancellationToken,
) {
    run_with_poll(persona, runtime, account, self_id, fetch, cancel, POLL).await;
}

pub async fn run_with_poll<F: FetchMentions>(
    persona: ProfileId,
    runtime: IntegrationRuntime,
    account: String,
    self_id: String,
    fetch: F,
    cancel: CancellationToken,
    poll: Duration,
) {
    info!(profile = %persona, account = %account, "slack watcher running");
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
                    "slack poll failed; backing off",
                );
            }
            Ok(page) => {
                error_streak = 0;
                auth_streak = 0;
                auth_alerted = false;
                if let Err(e) = process(persona, &runtime, &account, &self_id, &page).await {
                    warn!(
                        profile = %persona,
                        account = %account,
                        error = %e,
                        "slack poll processing failed",
                    );
                }
            }
        }
    }
    info!(profile = %persona, account = %account, "slack watcher stopped");
}

fn auth_broken_event(persona: ProfileId, account: &str, error: &IntegrationError) -> Event {
    Event::IntegrationUpdate {
        profile: persona,
        integration: ID,
        account: account.to_string(),
        kind: IntegrationUpdateKind::AuthBroken,
        external_ref: format!("slack/{account}:auth"),
        summary: format!(
            "slack polling keeps failing to authenticate ({error}); \
             reconnect with `goat integration add slack`"
        ),
        observation: None,
    }
}

async fn process(
    persona: ProfileId,
    runtime: &IntegrationRuntime,
    account: &str,
    self_id: &str,
    page: &FetchPage,
) -> IntegrationResult<()> {
    let incoming: Vec<Mention> = page
        .mentions
        .iter()
        .filter(|mention| !mention.is_authored_by(self_id))
        .cloned()
        .collect();

    let prev: Option<WatchState> = runtime
        .load_state(persona, &ID, account, STREAM)
        .await?
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok());
    let (mut next, fresh) = diff(prev.as_ref(), &incoming);

    let mut events = Vec::new();
    for mention in &fresh {
        match observe(persona, runtime, account, mention).await {
            Ok(event) => events.push(event),
            Err(e) => {
                warn!(
                    mention = %mention.key,
                    error = %e,
                    "failed to record slack observation; retrying next poll",
                );
                revert_seen(&mut next, prev.as_ref(), &mention.key);
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
            let _ = write!(summary, " (+{overflow} more waiting on you)");
        }
        runtime.publish(event);
    }
    Ok(())
}

fn revert_seen(next: &mut WatchState, prev: Option<&WatchState>, key: &str) {
    match prev.and_then(|state| state.seen.get(key)) {
        Some(ts) => {
            next.seen.insert(key.to_string(), ts.clone());
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
    mention: &Mention,
) -> IntegrationResult<Event> {
    let external_ref = format!("slack/{account}:message:{}", mention.key);
    let observation = runtime
        .record_observation(
            persona,
            &ID,
            account,
            &external_ref,
            IntegrationUpdateKind::Updated.as_str(),
            mention.raw.clone(),
        )
        .await?;
    Ok(Event::IntegrationUpdate {
        profile: persona,
        integration: ID,
        account: account.to_string(),
        kind: IntegrationUpdateKind::Updated,
        external_ref,
        summary: mention.summary(),
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

    const SELF_ID: &str = "U0OWNER";

    struct ScriptedFetch {
        batches: Mutex<VecDeque<FetchPage>>,
        last: Mutex<Vec<Mention>>,
    }

    impl ScriptedFetch {
        fn new(batches: Vec<FetchPage>) -> Self {
            Self {
                batches: Mutex::new(batches.into()),
                last: Mutex::new(Vec::new()),
            }
        }
    }

    impl FetchMentions for ScriptedFetch {
        async fn fetch(&self) -> IntegrationResult<FetchPage> {
            let mut batches = self.batches.lock().unwrap();
            if let Some(page) = batches.pop_front() {
                *self.last.lock().unwrap() = page.mentions.clone();
                Ok(page)
            } else {
                Ok(FetchPage {
                    mentions: self.last.lock().unwrap().clone(),
                })
            }
        }
    }

    fn mention_from(channel: &str, ts: &str, user: &str) -> Mention {
        Mention {
            key: format!("{channel}:{ts}"),
            channel: channel.into(),
            channel_name: "eng".into(),
            ts: ts.into(),
            user: user.into(),
            text: "need you here".into(),
            raw: json!({ "ts": ts, "user": user, "channel": channel }),
        }
    }

    fn page(mentions: Vec<Mention>) -> FetchPage {
        FetchPage { mentions }
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
        let prev = diff(None, &[mention_from("C1", "1.1", "U1")]).0;
        let (mut next, _) = diff(Some(&prev), &[mention_from("C1", "2.2", "U1")]);
        revert_seen(&mut next, Some(&prev), "C1:1.1");
        assert_eq!(next.seen.get("C1:1.1").map(String::as_str), Some("1.1"));
        revert_seen(&mut next, None, "C1:2.2");
        assert!(!next.seen.contains_key("C1:2.2"));
    }

    #[tokio::test]
    async fn cold_start_briefs_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime_in(dir.path()).await;
        let persona = persona_in(&runtime).await;
        let mut sub = runtime.bus.subscribe(EventFilter::Persona(persona));

        let history: Vec<Mention> = (1..=40)
            .map(|n| mention_from("C1", &format!("{n}.0"), "U1"))
            .collect();
        process(persona, &runtime, "default", SELF_ID, &page(history))
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
    async fn messages_the_owner_wrote_are_never_observed() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime_in(dir.path()).await;
        let persona = persona_in(&runtime).await;
        let mut sub = runtime.bus.subscribe(EventFilter::Persona(persona));

        process(persona, &runtime, "default", SELF_ID, &page(vec![]))
            .await
            .unwrap();
        process(
            persona,
            &runtime,
            "default",
            SELF_ID,
            &page(vec![
                mention_from("C1", "1.1", SELF_ID),
                mention_from("C1", "2.2", "U1"),
            ]),
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
        assert_eq!(refs, vec!["slack/default:message:C1:2.2".to_string()]);
    }

    #[tokio::test]
    async fn burst_is_capped_with_overflow_note() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime_in(dir.path()).await;
        let persona = persona_in(&runtime).await;
        let mut sub = runtime.bus.subscribe(EventFilter::Persona(persona));

        process(persona, &runtime, "default", SELF_ID, &page(vec![]))
            .await
            .unwrap();
        let burst: Vec<Mention> = (1..=5)
            .map(|n| mention_from("C1", &format!("{n}.0"), "U1"))
            .collect();
        process(persona, &runtime, "default", SELF_ID, &page(burst))
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
    async fn watcher_publishes_once_per_mention_with_a_lossless_observation() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime_in(dir.path()).await;
        let persona = persona_in(&runtime).await;
        let mut sub = runtime.bus.subscribe(EventFilter::Persona(persona));
        let cancel = CancellationToken::new();

        let fetch = ScriptedFetch::new(vec![
            page(vec![mention_from("C1", "1.1", "U1")]),
            page(vec![
                mention_from("C1", "1.1", "U1"),
                mention_from("C2", "2.2", "U2"),
            ]),
        ]);
        let handle = tokio::spawn(run_with_poll(
            persona,
            runtime.clone(),
            "default".into(),
            SELF_ID.into(),
            fetch,
            cancel.clone(),
            Duration::from_millis(10),
        ));

        let event = tokio::time::timeout(Duration::from_secs(5), sub.recv())
            .await
            .expect("event before timeout")
            .expect("mention event");
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
        assert_eq!(external_ref, "slack/default:message:C2:2.2");

        cancel.cancel();
        handle.await.unwrap();

        let record = runtime
            .store
            .get_observation(observation.expect("observation id"))
            .await
            .unwrap()
            .expect("observation recorded");
        assert_eq!(record.payload["user"], "U2");
        assert_eq!(record.external_ref, "slack/default:message:C2:2.2");

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
