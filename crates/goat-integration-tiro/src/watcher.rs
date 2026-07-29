use std::fmt::Write as _;
use std::time::Duration;

use goat_auth::CredentialStore;
use goat_integration::{IntegrationError, IntegrationResult, IntegrationRuntime};
use goat_types::{Event, IntegrationUpdateKind, ProfileId};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::diff::{WatchState, diff, hold_back};
use crate::parse::{FetchPage, Note, parse_page};
use crate::{ID, mcp};

pub const STREAM: &str = "notes";
const PAGE_SIZE: u64 = 50;
const POLL: Duration = Duration::from_mins(2);
const MAX_BACKOFF_TICKS: u32 = 8;
const AUTH_ALERT_STREAK: u32 = 5;
const EVENT_CAP_PER_POLL: usize = 3;

pub trait FetchNotes: Send + Sync + 'static {
    fn fetch(&self) -> impl Future<Output = IntegrationResult<FetchPage>> + Send;
}

pub struct McpFetch {
    pub credentials: CredentialStore,
    pub account: String,
    pub client_id: Option<String>,
    pub workspace: Option<String>,
    pub folder_id: Option<String>,
}

impl FetchNotes for McpFetch {
    async fn fetch(&self) -> IntegrationResult<FetchPage> {
        let auth = mcp::resolve_auth(&self.credentials, &self.account, self.client_id.as_deref())?;
        let session = mcp::connect(&auth).await?;
        let mut arguments = json!({ "pagination": { "size": PAGE_SIZE } });
        if let Some(workspace) = &self.workspace {
            arguments["workspaceGuid"] = json!(workspace);
        }
        if let Some(folder_id) = &self.folder_id {
            arguments["filter"] = json!({ "folderId": folder_id });
        }
        let result = session.call(mcp::TOOL_LIST_NOTES, arguments).await;
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

pub async fn run<F: FetchNotes>(
    persona: ProfileId,
    runtime: IntegrationRuntime,
    account: String,
    fetch: F,
    cancel: CancellationToken,
) {
    run_with_poll(persona, runtime, account, fetch, cancel, POLL).await;
}

pub async fn run_with_poll<F: FetchNotes>(
    persona: ProfileId,
    runtime: IntegrationRuntime,
    account: String,
    fetch: F,
    cancel: CancellationToken,
    poll: Duration,
) {
    info!(profile = %persona, account = %account, "tiro watcher running");
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
                    "tiro poll failed; backing off",
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
                        "tiro poll processing failed",
                    );
                }
            }
        }
    }
    info!(profile = %persona, account = %account, "tiro watcher stopped");
}

fn auth_broken_event(persona: ProfileId, account: &str, error: &IntegrationError) -> Event {
    Event::IntegrationUpdate {
        profile: persona,
        integration: ID,
        account: account.to_string(),
        kind: IntegrationUpdateKind::AuthBroken,
        external_ref: format!("tiro/{account}:auth"),
        summary: format!(
            "tiro polling keeps failing to authenticate ({error}); \
             reconnect with `goat integration add tiro`"
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
    let (mut next, fresh) = diff(prev.as_ref(), &page.notes);

    let mut events = Vec::new();
    for note in &fresh {
        match observe(persona, runtime, account, note).await {
            Ok(event) => events.push(event),
            Err(e) => {
                warn!(
                    note = %note.key,
                    error = %e,
                    "failed to record tiro observation; retrying next poll",
                );
                hold_back(&mut next, &note.key, &note.updated_at);
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
            let _ = write!(summary, " (+{overflow} more notes waiting)");
        }
        runtime.publish(event);
    }
    Ok(())
}

async fn observe(
    persona: ProfileId,
    runtime: &IntegrationRuntime,
    account: &str,
    note: &Note,
) -> IntegrationResult<Event> {
    let external_ref = format!("tiro/{account}:note:{}", note.key);
    let observation = runtime
        .record_observation(
            persona,
            &ID,
            account,
            &external_ref,
            IntegrationUpdateKind::Updated.as_str(),
            note.raw.clone(),
        )
        .await?;
    Ok(Event::IntegrationUpdate {
        profile: persona,
        integration: ID,
        account: account.to_string(),
        kind: IntegrationUpdateKind::Updated,
        external_ref,
        summary: note.summary(),
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
        last: Mutex<Vec<Note>>,
    }

    impl ScriptedFetch {
        fn new(batches: Vec<FetchPage>) -> Self {
            Self {
                batches: Mutex::new(batches.into()),
                last: Mutex::new(Vec::new()),
            }
        }
    }

    impl FetchNotes for ScriptedFetch {
        async fn fetch(&self) -> IntegrationResult<FetchPage> {
            let mut batches = self.batches.lock().unwrap();
            if let Some(page) = batches.pop_front() {
                *self.last.lock().unwrap() = page.notes.clone();
                Ok(page)
            } else {
                Ok(FetchPage {
                    notes: self.last.lock().unwrap().clone(),
                })
            }
        }
    }

    struct RejectingFetch;

    impl FetchNotes for RejectingFetch {
        async fn fetch(&self) -> IntegrationResult<FetchPage> {
            Err(IntegrationError::Auth(
                "tiro rejected the credential".into(),
            ))
        }
    }

    fn note_at(key: &str, updated_at: &str) -> Note {
        Note {
            key: key.into(),
            title: "OKR Q2 Planning".into(),
            updated_at: updated_at.into(),
            duration_seconds: 3600,
            source_type: "live-voice".into(),
            participants: vec!["Alice Kim".into()],
            raw: json!({ "noteGuid": key, "updatedAt": updated_at }),
        }
    }

    fn page(notes: Vec<Note>) -> FetchPage {
        FetchPage { notes }
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

    #[tokio::test]
    async fn cold_start_briefs_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime_in(dir.path()).await;
        let persona = persona_in(&runtime).await;
        let mut sub = runtime.bus.subscribe(EventFilter::Persona(persona));

        let backlog: Vec<Note> = (1..=40)
            .map(|n| note_at(&format!("n-{n}"), &format!("t{n}")))
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
    async fn a_note_stays_quiet_until_its_summary_stops_moving() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime_in(dir.path()).await;
        let persona = persona_in(&runtime).await;
        let mut sub = runtime.bus.subscribe(EventFilter::Persona(persona));

        process(persona, &runtime, "default", &page(vec![]))
            .await
            .unwrap();
        process(
            persona,
            &runtime,
            "default",
            &page(vec![note_at("n-1", "t1")]),
        )
        .await
        .unwrap();
        process(
            persona,
            &runtime,
            "default",
            &page(vec![note_at("n-1", "t2")]),
        )
        .await
        .unwrap();
        drop(runtime);

        let mut seen = 0;
        while let Some(event) = sub.recv().await {
            if matches!(event, Event::IntegrationUpdate { .. }) {
                seen += 1;
            }
        }
        assert_eq!(seen, 0, "a note being written must not be briefed");
    }

    #[tokio::test]
    async fn burst_is_capped_with_overflow_note() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime_in(dir.path()).await;
        let persona = persona_in(&runtime).await;
        let mut sub = runtime.bus.subscribe(EventFilter::Persona(persona));

        let burst: Vec<Note> = (1..=5)
            .map(|n| note_at(&format!("n-{n}"), &format!("t{n}")))
            .collect();
        process(persona, &runtime, "default", &page(vec![]))
            .await
            .unwrap();
        process(persona, &runtime, "default", &page(burst.clone()))
            .await
            .unwrap();
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
        assert!(summaries.last().unwrap().contains("+2 more notes waiting"));
    }

    #[tokio::test]
    async fn watcher_publishes_once_per_note_with_a_lossless_observation() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime_in(dir.path()).await;
        let persona = persona_in(&runtime).await;
        let mut sub = runtime.bus.subscribe(EventFilter::Persona(persona));
        let cancel = CancellationToken::new();

        let fetch = ScriptedFetch::new(vec![
            page(vec![note_at("n-1", "t1")]),
            page(vec![note_at("n-1", "t1"), note_at("n-2", "t2")]),
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
            .expect("note event");
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
        assert_eq!(kind, IntegrationUpdateKind::Updated);
        assert_eq!(external_ref, "tiro/default:note:n-2");
        assert!(summary.contains("OKR Q2 Planning"));

        cancel.cancel();
        handle.await.unwrap();

        let record = runtime
            .store
            .get_observation(observation.expect("observation id"))
            .await
            .unwrap()
            .expect("observation recorded");
        assert_eq!(record.payload["noteGuid"], "n-2");
        assert_eq!(record.external_ref, "tiro/default:note:n-2");

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
        assert_eq!(external_ref, "tiro/default:auth");

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
