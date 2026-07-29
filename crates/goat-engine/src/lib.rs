use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, atomic::AtomicU64},
};

use std::path::PathBuf;

use goat_auth::CredentialStore;
use goat_core::Engine;
use goat_protocol::{Event, ModelTarget, Op, SkillInfo, TaskId, ToolCallId};
use goat_provider::{Provider, ToolDefinition};
use goat_providers::{DEFAULT_ACCOUNT, Registry};
use goat_store::CodeStore as Store;
use goat_tool::SandboxPolicy;
use goat_tools::ToolRegistry;
use tokio::{
    sync::{Mutex, Semaphore, mpsc, oneshot},
    task::JoinHandle,
};

mod accounts;
mod agent;
mod ask;
mod compaction;
mod conversation;
mod delegate;
mod instructions;
mod mcp_tools;
mod persist;
mod process;
mod process_tools;
mod prompt;
mod rate_limit_cache;
mod retry;
mod rounds;
mod shell;
mod threads;
mod tool_recovery;
mod tools_exec;
mod turn;
mod websearch;

pub use agent::{AgentRegistry, AgentSpec, ToolSelection};

pub async fn model_list_entries(credentials: &CredentialStore) -> Vec<goat_protocol::ModelEntry> {
    let registry = Registry::new(credentials);
    accounts::discover_ready(&registry, credentials).await
}

const CHILD_ID_BASE: u64 = 1 << 32;
const WAKE_ID_BASE: u64 = 1 << 48;

pub struct GoatAgent {
    registry: Registry,
    tools: ToolRegistry,
    store: Store,
    credentials: CredentialStore,
    target: Option<ModelTarget>,
    mcp: Arc<goat_mcp::McpManager>,
    cwd: PathBuf,
    meter: Option<goat_proxy::Meter>,
}

impl GoatAgent {
    pub async fn new(
        registry: Registry,
        store: Store,
        credentials: CredentialStore,
        target: Option<ModelTarget>,
        cwd: PathBuf,
        meter: Option<goat_proxy::Meter>,
    ) -> Self {
        let config = goat_config::Config::load();
        let mcp = goat_mcp::load_manager(goat_config::mcp_config_path().as_deref(), &cwd).await;
        let mcp_tools = mcp_tools::adapt(&mcp);
        let tool_count = mcp_tools.len();
        let mut tools = ToolRegistry::builtin().with_many(mcp_tools);
        if tool_count > 0 {
            tracing::info!(tool_count, "registered mcp tools");
        }
        if config.computer_use_enabled {
            match goat_tool_computer::desktop_tool() {
                Ok(ct) => tools = tools.with(Box::new(ct)),
                Err(err) => tracing::warn!("computer use unavailable: {err}"),
            }
        }
        if config.browser_enabled {
            tools = tools.with(Box::new(goat_tool_browser::browser_tool()));
        }
        Self {
            registry,
            tools,
            store,
            credentials,
            target,
            mcp,
            cwd,
            meter,
        }
    }
}

impl Engine for GoatAgent {
    fn spawn(self, ops: mpsc::Receiver<Op>, events: mpsc::Sender<Event>) -> JoinHandle<()> {
        tokio::spawn(run(self, ops, events))
    }
}

pub(crate) struct Ctx<'a> {
    pub(crate) registry: &'a Registry,
    pub(crate) account_registries: &'a std::sync::Mutex<HashMap<String, Arc<Registry>>>,
    pub(crate) credentials: &'a CredentialStore,
    pub(crate) tools: &'a ToolRegistry,
    pub(crate) agents: &'a AgentRegistry,
    pub(crate) store: &'a Store,
    pub(crate) events: &'a mpsc::Sender<Event>,
    pub(crate) skills: &'a [SkillInfo],
    pub(crate) instructions: Option<&'a str>,
    pub(crate) semaphore: &'a Arc<Semaphore>,
    pub(crate) child_ids: &'a AtomicU64,
    pub(crate) wake_ids: &'a AtomicU64,
    pub(crate) processes: &'a Arc<process::ProcessRegistry>,
    pub(crate) asks: &'a Mutex<HashMap<ToolCallId, oneshot::Sender<Vec<String>>>>,
    pub(crate) rl_cache: &'a std::sync::Mutex<rate_limit_cache::RateLimitCache>,
    pub(crate) rl_path: Option<&'a std::path::Path>,
    pub(crate) meter: &'a Option<goat_proxy::Meter>,
    pub(crate) cwd: &'a std::path::Path,
    pub(crate) date: &'a str,
}

pub(crate) enum Flow {
    Continue,
    Shutdown,
}

pub(crate) struct SessionState {
    pub(crate) target: Option<ModelTarget>,
    pub(crate) conversation: conversation::Conversation,
    pub(crate) tracker: compaction::ContextTracker,
    pub(crate) thread_id: Option<i64>,
}

pub(crate) struct TurnIds {
    pub(crate) stored_thread: Option<i64>,
    pub(crate) turn_db_id: Option<i64>,
    pub(crate) user_message_db_id: Option<i64>,
}

pub(crate) struct UserInput {
    pub(crate) id: TaskId,
    pub(crate) text: String,
    pub(crate) display: Option<String>,
    pub(crate) attachments: Vec<goat_protocol::InputAttachment>,
}

pub(crate) type SteeringQueue = std::sync::Mutex<std::collections::VecDeque<UserInput>>;

enum Report<'a> {
    Top {
        ids: &'a TurnIds,
        steering: &'a SteeringQueue,
    },
    Child,
}

pub(crate) struct Run<'a> {
    pub(crate) id: TaskId,
    report: Report<'a>,
}

impl<'a> Run<'a> {
    pub(crate) fn top(id: TaskId, ids: &'a TurnIds, steering: &'a SteeringQueue) -> Self {
        Self {
            id,
            report: Report::Top { ids, steering },
        }
    }

    pub(crate) fn child(id: TaskId) -> Self {
        Self {
            id,
            report: Report::Child,
        }
    }

    pub(crate) fn ids(&self) -> Option<&TurnIds> {
        match &self.report {
            Report::Top { ids, .. } => Some(ids),
            Report::Child => None,
        }
    }

    pub(crate) fn steering(&self) -> Option<&SteeringQueue> {
        match &self.report {
            Report::Top { steering, .. } => Some(steering),
            Report::Child => None,
        }
    }

    pub(crate) fn steering_pending(&self) -> bool {
        self.steering().is_some_and(|queue| {
            !queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        })
    }

    pub(crate) fn is_top(&self) -> bool {
        matches!(self.report, Report::Top { .. })
    }
}

pub(crate) struct LoopEnv<'a> {
    pub(crate) provider: &'a dyn Provider,
    pub(crate) target: &'a ModelTarget,
    pub(crate) tool_defs: &'a [ToolDefinition],
    pub(crate) cwd: &'a Path,
    pub(crate) allow_delegate: bool,
    pub(crate) allow_ask: bool,
    pub(crate) exec_policy: SandboxPolicy,
}

#[allow(clippy::too_many_lines)]
async fn run(agent: GoatAgent, mut ops: mpsc::Receiver<Op>, events: mpsc::Sender<Event>) {
    let GoatAgent {
        mut registry,
        tools,
        store,
        credentials,
        target,
        mcp,
        cwd,
        meter,
    } = agent;
    let mut state = SessionState {
        target,
        conversation: conversation::Conversation::new(),
        tracker: compaction::ContextTracker::new(),
        thread_id: None,
    };

    if state.target.is_none() {
        state.target = accounts::restore_target(&store, &credentials, &cwd).await;
    }
    accounts::announce_startup(&events, &registry, &credentials, state.target.as_ref()).await;

    let skills = prompt::load_skill_infos(&cwd);
    let agents = AgentRegistry::load(&cwd);
    let project_instructions = instructions::load_project_instructions(&cwd);
    let session_date = prompt::current_utc_date();
    let semaphore = Arc::new(Semaphore::new(delegate::MAX_CONCURRENT_AGENTS));
    let child_ids = AtomicU64::new(CHILD_ID_BASE);
    let wake_ids = AtomicU64::new(WAKE_ID_BASE);
    let wake = Arc::new(tokio::sync::Notify::new());
    let processes =
        process::ProcessRegistry::new(events.clone(), wake.clone(), Some(store.clone()));
    let asks: Mutex<HashMap<ToolCallId, oneshot::Sender<Vec<String>>>> = Mutex::new(HashMap::new());
    let _ = events
        .send(Event::SkillsChanged {
            skills: skills.clone(),
        })
        .await;

    let rl_path = goat_config::rate_limits_path();
    let rl_cache_data = rl_path
        .as_deref()
        .map(rate_limit_cache::RateLimitCache::load)
        .unwrap_or_default();
    for (provider, account, entry) in rl_cache_data.entries() {
        let _ = events
            .send(Event::RateLimits {
                provider: provider.to_owned(),
                account: account.to_owned(),
                snapshot: goat_protocol::RateLimitSnapshot {
                    windows: entry.windows.clone(),
                    representative: None,
                },
                cached_at: entry.cached_at,
            })
            .await;
    }
    let rl_cache = std::sync::Mutex::new(rl_cache_data);
    let account_registries: std::sync::Mutex<HashMap<String, Arc<Registry>>> =
        std::sync::Mutex::new(HashMap::new());

    macro_rules! ctx {
        () => {
            Ctx {
                registry: &registry,
                account_registries: &account_registries,
                credentials: &credentials,
                tools: &tools,
                agents: &agents,
                store: &store,
                events: &events,
                skills: &skills,
                instructions: project_instructions.as_deref(),
                semaphore: &semaphore,
                child_ids: &child_ids,
                wake_ids: &wake_ids,
                processes: &processes,
                asks: &asks,
                rl_cache: &rl_cache,
                rl_path: rl_path.as_deref(),
                meter: &meter,
                cwd: &cwd,
                date: &session_date,
            }
        };
    }

    loop {
        let op = tokio::select! {
            biased;
            maybe_op = ops.recv() => match maybe_op {
                Some(op) => op,
                None => break,
            },
            () = wake.notified() => {
                let ctx = ctx!();
                if let Flow::Shutdown =
                    turn::handle_wake(&ctx, &mut state, &mut ops).await
                {
                    break;
                }
                continue;
            }
        };
        match op {
            Op::SubmitMessage {
                id,
                text,
                display,
                attachments,
            } => {
                let ctx = ctx!();
                if let Flow::Shutdown =
                    turn::handle_turn(&ctx, id, text, display, attachments, &mut state, &mut ops)
                        .await
                {
                    break;
                }
            }
            Op::Interrupt { .. } | Op::Answer { .. } | Op::DequeueMessage { .. } | Op::Clear {} => {
            }
            Op::ProcessKill { process } => {
                let _ = processes.kill(process).await;
            }
            Op::ProcessWatch { process, on } => {
                let _ = processes.set_watch(process, on).await;
            }
            Op::Compact { id, instructions } => {
                let ctx = ctx!();
                if let Flow::Shutdown =
                    turn::handle_compact(&ctx, id, instructions, &mut state, &mut ops).await
                {
                    break;
                }
            }
            Op::SubmitShell { id, command } => {
                let ctx = ctx!();
                if let Flow::Shutdown =
                    turn::handle_shell(&ctx, id, &command, &mut state, &mut ops).await
                {
                    break;
                }
            }
            Op::SelectModel { .. } => {
                turn::handle_idle_op(
                    op,
                    &store,
                    &cwd,
                    state.thread_id,
                    &mut state.target,
                    &events,
                    &processes,
                )
                .await;
            }
            Op::Login {
                provider,
                credential,
            } => {
                let ctx = accounts::LoginCtx {
                    credentials: &credentials,
                    registry: &mut registry,
                    events: &events,
                };
                accounts::handle_login(
                    ctx,
                    provider,
                    DEFAULT_ACCOUNT.to_owned(),
                    credential,
                    false,
                )
                .await;
                accounts::clear_account_registries(&account_registries);
            }
            Op::AddAccount {
                provider,
                name,
                credential,
            } => {
                let ctx = accounts::LoginCtx {
                    credentials: &credentials,
                    registry: &mut registry,
                    events: &events,
                };
                accounts::handle_login(ctx, provider, name, credential, true).await;
                accounts::clear_account_registries(&account_registries);
            }
            Op::RemoveAccount { provider, name } => {
                accounts::handle_remove_account(
                    provider,
                    name,
                    &credentials,
                    &mut registry,
                    &events,
                )
                .await;
                accounts::clear_account_registries(&account_registries);
            }
            Op::ListThreads {} => {
                threads::handle_list_threads(&store, &cwd, &events).await;
            }
            Op::Resume { thread_id: tid } => {
                threads::handle_resume(
                    &store,
                    &skills,
                    &tools,
                    project_instructions.as_deref(),
                    &session_date,
                    tid,
                    &mut state,
                    &events,
                )
                .await;
                accounts::refresh_model_list(&events, &registry, &credentials).await;
            }
            Op::ResumeLatest {} => {
                threads::handle_resume_latest(
                    &store,
                    &skills,
                    &tools,
                    project_instructions.as_deref(),
                    &session_date,
                    &cwd,
                    &mut state,
                    &events,
                )
                .await;
                accounts::refresh_model_list(&events, &registry, &credentials).await;
            }
            Op::RenameThread { title } => {
                threads::handle_rename(&store, state.thread_id, title, &events).await;
            }
            Op::Shutdown {} => break,
        }
    }
    processes.shutdown_all().await;
    mcp.shutdown().await;
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use goat_auth::CredentialStore;
    use goat_core::Session;
    use goat_protocol::{Event, ModelTarget, Op, TaskId};
    use goat_provider::{
        AuthMethod, Capabilities, ChunkStream, Model, Provider, ProviderId, Request, StreamChunk,
        StreamError,
    };
    use goat_providers::Registry;
    use goat_store::CodeStore as Store;
    use tokio::{sync::mpsc, task::JoinHandle};

    use super::GoatAgent;

    struct MockProvider {
        id: String,
        reply: String,
        delay_ms: u64,
    }

    #[async_trait::async_trait]
    impl Provider for MockProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from(self.id.as_str())
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities {
                tools: false,
                auth: AuthMethod::None,
                images: false,
            }
        }

        async fn stream(&self, _req: Request) -> Result<ChunkStream, StreamError> {
            let reply = self.reply.clone();
            let delay = self.delay_ms;
            Ok(Box::pin(async_stream::try_stream! {
                if delay > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                }
                yield StreamChunk::TextDelta { text: reply };
            }))
        }

        fn discover(&self, out: mpsc::Sender<Model>) -> JoinHandle<()> {
            tokio::spawn(async move {
                drop(out);
            })
        }
    }

    struct ScriptedProvider {
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl Provider for ScriptedProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from("mock")
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities {
                tools: true,
                auth: AuthMethod::None,
                images: true,
            }
        }

        async fn stream(&self, _req: Request) -> Result<ChunkStream, StreamError> {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Box::pin(async_stream::try_stream! {
                match n {
                    0 => {
                        yield StreamChunk::ToolCall {
                            id: "call-1".to_owned(),
                            name: "Agent".to_owned(),
                            input: "{\"agent_type\":\"explore\",\"prompt\":\"look into it\"}"
                                .to_owned(),
                        };
                    }
                    1 => {
                        yield StreamChunk::TextDelta {
                            text: "child findings".to_owned(),
                        };
                    }
                    _ => {
                        yield StreamChunk::TextDelta {
                            text: "final answer".to_owned(),
                        };
                    }
                }
            }))
        }

        fn discover(&self, out: mpsc::Sender<Model>) -> JoinHandle<()> {
            tokio::spawn(async move {
                drop(out);
            })
        }
    }

    struct SeqTextProvider {
        calls: Arc<std::sync::atomic::AtomicUsize>,
        delay_ms: u64,
    }

    #[async_trait::async_trait]
    impl Provider for SeqTextProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from("mock")
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities {
                tools: false,
                auth: AuthMethod::None,
                images: false,
            }
        }

        async fn stream(&self, _req: Request) -> Result<ChunkStream, StreamError> {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let delay = self.delay_ms;
            Ok(Box::pin(async_stream::try_stream! {
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                yield StreamChunk::TextDelta {
                    text: format!("reply {n}"),
                };
            }))
        }

        fn discover(&self, out: mpsc::Sender<Model>) -> JoinHandle<()> {
            tokio::spawn(async move {
                drop(out);
            })
        }
    }

    struct CapturingProvider {
        calls: Arc<std::sync::atomic::AtomicUsize>,
        captured: Arc<std::sync::Mutex<Vec<Request>>>,
    }

    #[async_trait::async_trait]
    impl Provider for CapturingProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from("mock")
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities {
                tools: true,
                auth: AuthMethod::None,
                images: true,
            }
        }

        async fn stream(&self, req: Request) -> Result<ChunkStream, StreamError> {
            self.captured.lock().unwrap().push(req);
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Box::pin(async_stream::try_stream! {
                if n == 0 {
                    yield StreamChunk::ToolCall {
                        id: "call-1".to_owned(),
                        name: "Read".to_owned(),
                        input: "{\"path\":\"does-not-exist.txt\"}".to_owned(),
                    };
                } else {
                    yield StreamChunk::TextDelta {
                        text: "final answer".to_owned(),
                    };
                }
            }))
        }

        fn discover(&self, out: mpsc::Sender<Model>) -> JoinHandle<()> {
            tokio::spawn(async move {
                drop(out);
            })
        }
    }

    async fn seq_agent(delay_ms: u64) -> (GoatAgent, Arc<std::sync::atomic::AtomicUsize>) {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider = SeqTextProvider {
            calls: calls.clone(),
            delay_ms,
        };
        let registry = Registry::from_providers(vec![Arc::new(provider)]);
        let store = Store::open_in_memory().await.unwrap();
        let credentials =
            CredentialStore::new(std::env::temp_dir().join("goat-agent-steering.json"));
        (
            GoatAgent::new(
                registry,
                store,
                credentials,
                Some(target("mock")),
                std::env::temp_dir(),
                None,
            )
            .await,
            calls,
        )
    }

    #[tokio::test]
    async fn steering_extends_the_turn_and_injects_message() {
        let (agent, calls) = seq_agent(150).await;
        let session = Session::spawn(agent);
        let (ops, mut events, _handle) = session.into_parts();
        ops.send(Op::SubmitMessage {
            id: TaskId(1),
            text: "first".to_owned(),
            display: None,
            attachments: Vec::new(),
        })
        .await
        .unwrap();
        ops.send(Op::SubmitMessage {
            id: TaskId(2),
            text: "also do this".to_owned(),
            display: None,
            attachments: Vec::new(),
        })
        .await
        .unwrap();

        let mut user_messages = Vec::new();
        let mut text_dones = Vec::new();
        let mut task_starts = 0;
        let mut task_dones = 0;
        while let Some(event) = events.recv().await {
            match event {
                Event::TaskStarted { .. } => task_starts += 1,
                Event::UserMessage { id, text, .. } => user_messages.push((id, text)),
                Event::TextDone { text, .. } => text_dones.push(text),
                Event::TaskDone { interrupted, .. } => {
                    assert!(!interrupted);
                    task_dones += 1;
                    break;
                }
                _ => {}
            }
        }
        assert_eq!(
            user_messages,
            vec![
                (TaskId(1), "first".to_owned()),
                (TaskId(2), "also do this".to_owned())
            ],
            "first message and steering message are both echoed with their task ids"
        );
        assert_eq!(text_dones, vec!["reply 0".to_owned(), "reply 1".to_owned()]);
        assert_eq!(
            task_starts, 1,
            "steering must extend the turn, not start a new one"
        );
        assert_eq!(task_dones, 1);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn dequeue_removes_pending_steering_message() {
        let (agent, calls) = seq_agent(300).await;
        let session = Session::spawn(agent);
        let (ops, mut events, _handle) = session.into_parts();
        ops.send(Op::SubmitMessage {
            id: TaskId(1),
            text: "first".to_owned(),
            display: None,
            attachments: Vec::new(),
        })
        .await
        .unwrap();
        ops.send(Op::SubmitMessage {
            id: TaskId(2),
            text: "typo message".to_owned(),
            display: None,
            attachments: Vec::new(),
        })
        .await
        .unwrap();
        ops.send(Op::DequeueMessage { id: TaskId(2) })
            .await
            .unwrap();

        let mut dequeued = Vec::new();
        let mut user_messages = Vec::new();
        while let Some(event) = events.recv().await {
            match event {
                Event::MessageDequeued { id, text, .. } => dequeued.push((id, text)),
                Event::UserMessage { id, .. } => user_messages.push(id),
                Event::TaskDone { .. } => break,
                _ => {}
            }
        }
        assert_eq!(dequeued, vec![(TaskId(2), "typo message".to_owned())]);
        assert_eq!(
            user_messages,
            vec![TaskId(1)],
            "only the first message echoes; the dequeued message never injects"
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn first_message_echoes_user_message_before_task_started() {
        let (agent, _calls) = seq_agent(10).await;
        let session = Session::spawn(agent);
        let (ops, mut events, _handle) = session.into_parts();
        ops.send(Op::SubmitMessage {
            id: TaskId(1),
            text: "first".to_owned(),
            display: None,
            attachments: Vec::new(),
        })
        .await
        .unwrap();

        let mut order = Vec::new();
        while let Some(event) = events.recv().await {
            match event {
                Event::UserMessage { id, text, .. } => {
                    order.push(("user", id, Some(text)));
                }
                Event::TaskStarted { id } => order.push(("started", id, None)),
                Event::TaskDone { .. } => break,
                _ => {}
            }
        }
        assert_eq!(
            order,
            vec![
                ("user", TaskId(1), Some("first".to_owned())),
                ("started", TaskId(1), None),
            ],
            "UserMessage must precede TaskStarted for the first message"
        );
    }

    #[tokio::test]
    async fn followup_message_mid_turn_is_never_lost() {
        let (agent, _calls) = seq_agent(10).await;
        let session = Session::spawn(agent);
        let (ops, mut events, _handle) = session.into_parts();
        ops.send(Op::SubmitMessage {
            id: TaskId(1),
            text: "first".to_owned(),
            display: None,
            attachments: Vec::new(),
        })
        .await
        .unwrap();
        let mut sent_followup = false;
        let mut text_dones = 0;
        loop {
            let event = tokio::time::timeout(std::time::Duration::from_secs(10), events.recv())
                .await
                .expect("engine stalled before processing the follow-up")
                .expect("engine closed");
            match event {
                Event::TaskStarted { .. } if !sent_followup => {
                    sent_followup = true;
                    ops.send(Op::SubmitMessage {
                        id: TaskId(2),
                        text: "follow up".to_owned(),
                        display: None,
                        attachments: Vec::new(),
                    })
                    .await
                    .unwrap();
                }
                Event::TextDone { .. } => text_dones += 1,
                Event::TaskDone { interrupted, .. } => {
                    assert!(!interrupted);
                    if text_dones >= 2 {
                        break;
                    }
                }
                _ => {}
            }
        }
        assert_eq!(text_dones, 2, "follow-up must produce its own response");
    }

    struct OverflowThenRecoverProvider {
        calls: Arc<std::sync::atomic::AtomicUsize>,
        requests: Arc<std::sync::Mutex<Vec<Request>>>,
    }

    #[async_trait::async_trait]
    impl Provider for OverflowThenRecoverProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from("mock")
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities {
                tools: false,
                auth: AuthMethod::None,
                images: false,
            }
        }

        async fn stream(&self, req: Request) -> Result<ChunkStream, StreamError> {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(req);
            Ok(Box::pin(async_stream::try_stream! {
                match n {
                    0 => {
                        Err(StreamError::context_overflow("prompt is too long"))?;
                    }
                    1 => {
                        yield StreamChunk::TextDelta {
                            text:
                                "<analysis>walk</analysis><summary>## Task\nthe work</summary>"
                                    .to_owned(),
                        };
                    }
                    _ => {
                        yield StreamChunk::TextDelta {
                            text: "recovered after compaction".to_owned(),
                        };
                    }
                }
            }))
        }

        fn discover(&self, out: mpsc::Sender<Model>) -> JoinHandle<()> {
            tokio::spawn(async move {
                drop(out);
            })
        }
    }

    #[tokio::test]
    async fn context_overflow_compacts_and_retries_the_round() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let provider = OverflowThenRecoverProvider {
            calls: calls.clone(),
            requests: requests.clone(),
        };
        let registry = Registry::from_providers(vec![Arc::new(provider)]);
        let store = Store::open_in_memory().await.unwrap();
        let credentials =
            CredentialStore::new(std::env::temp_dir().join("goat-agent-overflow.json"));
        let agent = GoatAgent::new(
            registry,
            store.clone(),
            credentials,
            Some(target("mock")),
            std::env::temp_dir(),
            None,
        )
        .await;
        let session = Session::spawn(agent);
        let (ops, mut events, _handle) = session.into_parts();
        ops.send(Op::SubmitMessage {
            id: TaskId(1),
            text: "long running work".to_owned(),
            display: None,
            attachments: Vec::new(),
        })
        .await
        .unwrap();

        let mut compaction_started = false;
        let mut compaction_done_ok = None;
        let mut final_text = String::new();
        while let Some(event) = events.recv().await {
            match event {
                Event::CompactionStarted { .. } => compaction_started = true,
                Event::CompactionDone { ok, .. } => compaction_done_ok = Some(ok),
                Event::TextDone { text, .. } => final_text = text,
                Event::TaskDone { interrupted, .. } => {
                    assert!(!interrupted);
                    break;
                }
                _ => {}
            }
        }
        assert!(compaction_started);
        assert_eq!(compaction_done_ok, Some(true));
        assert_eq!(final_text, "recovered after compaction");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 3);

        let captured = requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(
            captured[1].tool_choice,
            goat_provider::ToolChoice::None,
            "summarization call must structurally forbid tool use"
        );
        assert!(
            captured[1]
                .messages
                .last()
                .unwrap()
                .text_content()
                .contains("checkpoint summary"),
            "summarization call must append the prompt"
        );
        let retried = &captured[2];
        assert!(
            retried
                .messages
                .iter()
                .any(|message| message.text_content().contains("## Task")),
            "retried round must see the summary"
        );
        assert!(
            retried
                .messages
                .iter()
                .any(|message| message.text_content() == "long running work"),
            "the in-flight user prompt must survive verbatim"
        );

        let compactions = store.compactions_for_thread(1).await.unwrap();
        assert_eq!(compactions.len(), 1, "compaction must be persisted");
        assert!(compactions[0].summary.contains("## Task"));
    }

    #[tokio::test]
    async fn resume_after_compaction_rebuilds_compacted_history() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let provider = OverflowThenRecoverProvider {
            calls: calls.clone(),
            requests: requests.clone(),
        };
        let registry = Registry::from_providers(vec![Arc::new(provider)]);
        let store = Store::open_in_memory().await.unwrap();
        let credentials =
            CredentialStore::new(std::env::temp_dir().join("goat-agent-resume-compact.json"));
        let agent = GoatAgent::new(
            registry,
            store.clone(),
            credentials.clone(),
            Some(target("mock")),
            std::env::temp_dir(),
            None,
        )
        .await;
        let session = Session::spawn(agent);
        let (ops, mut events, _handle) = session.into_parts();
        ops.send(Op::SubmitMessage {
            id: TaskId(1),
            text: "long running work".to_owned(),
            display: None,
            attachments: Vec::new(),
        })
        .await
        .unwrap();
        while let Some(event) = events.recv().await {
            if matches!(event, Event::TaskDone { .. }) {
                break;
            }
        }
        ops.send(Op::Shutdown {}).await.unwrap();

        let provider2 = MockProvider {
            id: "mock".to_owned(),
            reply: "resumed".to_owned(),
            delay_ms: 0,
        };
        let registry2 = Registry::from_providers(vec![Arc::new(provider2)]);
        let agent2 = GoatAgent::new(
            registry2,
            store.clone(),
            credentials,
            Some(target("mock")),
            std::env::temp_dir(),
            None,
        )
        .await;
        let session2 = Session::spawn(agent2);
        let (ops2, mut events2, _handle2) = session2.into_parts();
        ops2.send(Op::Resume { thread_id: 1 }).await.unwrap();

        let mut saw_marker = false;
        while let Some(event) = events2.recv().await {
            if let Event::ConversationRestored {
                entries,
                context_tokens,
                ..
            } = event
            {
                saw_marker = entries.iter().any(|entry| {
                    matches!(entry, goat_protocol::TranscriptEntry::Compaction { .. })
                });
                assert!(context_tokens.is_some());
                break;
            }
        }
        assert!(
            saw_marker,
            "restored transcript must carry the compaction marker"
        );
    }

    struct FailingProvider {
        calls: Arc<std::sync::atomic::AtomicUsize>,
        failures: usize,
        error: StreamError,
    }

    #[async_trait::async_trait]
    impl Provider for FailingProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from("mock")
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities {
                tools: false,
                auth: AuthMethod::None,
                images: false,
            }
        }

        async fn stream(&self, _req: Request) -> Result<ChunkStream, StreamError> {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let failures = self.failures;
            let error = self.error.clone();
            Ok(Box::pin(async_stream::try_stream! {
                if n < failures {
                    Err(error)?;
                }
                yield StreamChunk::TextDelta {
                    text: "recovered".to_owned(),
                };
            }))
        }

        fn discover(&self, out: mpsc::Sender<Model>) -> JoinHandle<()> {
            tokio::spawn(async move {
                drop(out);
            })
        }
    }

    async fn failing_agent(
        failures: usize,
        error: StreamError,
    ) -> (GoatAgent, Arc<std::sync::atomic::AtomicUsize>) {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider = FailingProvider {
            calls: calls.clone(),
            failures,
            error,
        };
        let registry = Registry::from_providers(vec![Arc::new(provider)]);
        let store = Store::open_in_memory().await.unwrap();
        let credentials = CredentialStore::new(std::env::temp_dir().join("goat-agent-retry.json"));
        (
            GoatAgent::new(
                registry,
                store,
                credentials,
                Some(target("mock")),
                std::env::temp_dir(),
                None,
            )
            .await,
            calls,
        )
    }

    #[tokio::test]
    async fn retries_transient_failures_until_success() {
        let (agent, calls) = failing_agent(2, StreamError::overloaded("busy")).await;
        tokio::time::pause();
        let session = Session::spawn(agent);
        let (ops, mut events, _handle) = session.into_parts();
        ops.send(Op::SubmitMessage {
            id: TaskId(1),
            text: "go".to_owned(),
            display: None,
            attachments: Vec::new(),
        })
        .await
        .unwrap();

        let mut retries = Vec::new();
        let mut final_text = String::new();
        let mut interrupted = true;
        while let Some(event) = events.recv().await {
            match event {
                Event::Retrying {
                    attempt, reason, ..
                } => retries.push((attempt, reason)),
                Event::TextDone { text, .. } => final_text = text,
                Event::TaskDone {
                    interrupted: was, ..
                } => {
                    interrupted = was;
                    break;
                }
                _ => {}
            }
        }
        assert_eq!(retries.len(), 2);
        assert_eq!(retries[0], (1, "overloaded".to_owned()));
        assert_eq!(retries[1], (2, "overloaded".to_owned()));
        assert_eq!(final_text, "recovered");
        assert!(!interrupted);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn auth_failure_aborts_without_retry() {
        let (agent, calls) = failing_agent(usize::MAX, StreamError::auth("expired")).await;
        let session = Session::spawn(agent);
        let (ops, mut events, _handle) = session.into_parts();
        ops.send(Op::SubmitMessage {
            id: TaskId(1),
            text: "go".to_owned(),
            display: None,
            attachments: Vec::new(),
        })
        .await
        .unwrap();

        let mut saw_retry = false;
        let mut error_message = String::new();
        let mut error_hint = String::new();
        while let Some(event) = events.recv().await {
            match event {
                Event::Retrying { .. } => saw_retry = true,
                Event::Error { message, hint, .. } => {
                    error_message = message;
                    error_hint = hint.unwrap_or_default();
                }
                Event::TaskDone { interrupted, .. } => {
                    assert!(interrupted);
                    break;
                }
                _ => {}
            }
        }
        assert!(!saw_retry, "auth failures must not retry");
        assert!(
            error_message.contains("authentication failed"),
            "{error_message}"
        );
        assert!(error_hint.contains("/config to re-login"), "{error_hint}");
        assert!(error_hint.contains("progress saved"), "{error_hint}");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn interrupt_cancels_retry_backoff_promptly() {
        let (agent, _calls) = failing_agent(
            usize::MAX,
            StreamError::rate_limited("slow", Some(std::time::Duration::from_secs(30))),
        )
        .await;
        let session = Session::spawn(agent);
        let (ops, mut events, _handle) = session.into_parts();
        ops.send(Op::SubmitMessage {
            id: TaskId(4),
            text: "go".to_owned(),
            display: None,
            attachments: Vec::new(),
        })
        .await
        .unwrap();

        let started = std::time::Instant::now();
        let mut interrupted = false;
        while let Some(event) = events.recv().await {
            match event {
                Event::Retrying { .. } => {
                    ops.send(Op::Interrupt { id: TaskId(4) }).await.unwrap();
                }
                Event::TaskDone {
                    interrupted: was, ..
                } => {
                    interrupted = was;
                    break;
                }
                _ => {}
            }
        }
        assert!(interrupted);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "interrupt during backoff must cancel promptly, took {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn delegates_to_agent_and_returns_result() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider = ScriptedProvider {
            calls: calls.clone(),
        };
        let registry = Registry::from_providers(vec![Arc::new(provider)]);
        let store = Store::open_in_memory().await.unwrap();
        let credentials =
            CredentialStore::new(std::env::temp_dir().join("goat-agent-delegate.json"));
        let agent = GoatAgent::new(
            registry,
            store,
            credentials,
            Some(target("mock")),
            std::env::temp_dir(),
            None,
        )
        .await;
        let session = Session::spawn(agent);
        let (ops, mut events, _handle) = session.into_parts();
        ops.send(Op::SubmitMessage {
            id: TaskId(1),
            text: "do it".to_owned(),
            display: None,
            attachments: Vec::new(),
        })
        .await
        .unwrap();

        let mut agent_started = false;
        let mut agent_done_ok = false;
        let mut final_text = String::new();
        while let Some(event) = events.recv().await {
            match event {
                Event::ToolStarted { call, .. } if call.name == "Agent" => agent_started = true,
                Event::ToolDone { outcome, .. } => agent_done_ok = outcome.ok,
                Event::TextDone { text, .. } => final_text = text,
                Event::TaskDone { interrupted, .. } => {
                    assert!(!interrupted);
                    break;
                }
                _ => {}
            }
        }
        assert!(agent_started, "expected the Agent tool to start");
        assert!(agent_done_ok, "expected the Agent delegation to succeed");
        assert_eq!(final_text, "final answer");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn tool_round_message_carries_language_anchor() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let captured = Arc::new(std::sync::Mutex::new(Vec::<Request>::new()));
        let provider = CapturingProvider {
            calls: calls.clone(),
            captured: captured.clone(),
        };
        let registry = Registry::from_providers(vec![Arc::new(provider)]);
        let store = Store::open_in_memory().await.unwrap();
        let credentials = CredentialStore::new(std::env::temp_dir().join("goat-agent-anchor.json"));
        let agent = GoatAgent::new(
            registry,
            store,
            credentials,
            Some(target("mock")),
            std::env::temp_dir(),
            None,
        )
        .await;
        let session = Session::spawn(agent);
        let (ops, mut events, _handle) = session.into_parts();
        ops.send(Op::SubmitMessage {
            id: TaskId(1),
            text: "do it".to_owned(),
            display: None,
            attachments: Vec::new(),
        })
        .await
        .unwrap();

        while let Some(event) = events.recv().await {
            if let Event::TaskDone { .. } = event {
                break;
            }
        }

        let requests = captured.lock().unwrap();
        assert!(
            requests.len() >= 2,
            "expected a second round after the tool call, got {} requests",
            requests.len()
        );
        let second = &requests[1];
        let last = second.messages.last().expect("second request has messages");
        let has_anchor = last.content.iter().any(|block| {
            matches!(
                block,
                goat_provider::ContentBlock::Text { text } if text == crate::prompt::LANGUAGE_REMINDER
            )
        });
        assert!(
            has_anchor,
            "the tool-result message handed to round 2 must end with the language anchor"
        );
    }

    fn target(provider: &str) -> ModelTarget {
        ModelTarget {
            provider: provider.to_owned(),
            model: "m".to_owned(),
            account: "default".to_owned(),
            effort: None,
        }
    }

    async fn agent_with(reply: &str, delay_ms: u64) -> GoatAgent {
        let provider = MockProvider {
            id: "mock".to_owned(),
            reply: reply.to_owned(),
            delay_ms,
        };
        let registry = Registry::from_providers(vec![Arc::new(provider)]);
        let store = Store::open_in_memory().await.unwrap();
        let credentials = CredentialStore::new(std::env::temp_dir().join("goat-agent-test.json"));
        GoatAgent::new(
            registry,
            store,
            credentials,
            Some(target("mock")),
            std::env::temp_dir(),
            None,
        )
        .await
    }

    #[tokio::test]
    async fn bridges_text_to_protocol_events() {
        let session = Session::spawn(agent_with("hello", 0).await);
        let (ops, mut events, _handle) = session.into_parts();
        ops.send(Op::SubmitMessage {
            id: TaskId(1),
            text: "hi".to_owned(),
            display: None,
            attachments: Vec::new(),
        })
        .await
        .unwrap();

        let mut started = false;
        let mut deltas = String::new();
        let mut done = false;
        let mut user_echo = None;
        while let Some(event) = events.recv().await {
            match event {
                Event::ModelListChanged { .. }
                | Event::ModelSelected { .. }
                | Event::LoginProviders { .. }
                | Event::LoginStatus { .. }
                | Event::AccountsChanged { .. }
                | Event::SkillsChanged { .. }
                | Event::TextDone { .. }
                | Event::Usage { .. }
                | Event::ThreadBound { .. }
                | Event::RateLimits { .. } => {}
                Event::TaskStarted { .. } => started = true,
                Event::UserMessage { id, text, .. } => user_echo = Some((id, text)),
                Event::TextDelta { chunk, .. } => deltas.push_str(&chunk),
                Event::TaskDone { interrupted, .. } => {
                    assert!(!interrupted);
                    done = true;
                    break;
                }
                other => panic!("unexpected event: {other:?}"),
            }
        }
        assert!(started);
        assert_eq!(deltas, "hello");
        assert!(done);
        assert_eq!(user_echo, Some((TaskId(1), "hi".to_owned())));
    }

    struct SlowOpenProvider {
        open_delay_ms: u64,
    }

    #[async_trait::async_trait]
    impl Provider for SlowOpenProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from("mock")
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities {
                tools: false,
                auth: AuthMethod::None,
                images: false,
            }
        }

        async fn stream(&self, _req: Request) -> Result<ChunkStream, StreamError> {
            tokio::time::sleep(std::time::Duration::from_millis(self.open_delay_ms)).await;
            Ok(Box::pin(async_stream::try_stream! {
                yield StreamChunk::TextDelta { text: "late".to_owned() };
            }))
        }

        fn discover(&self, out: mpsc::Sender<Model>) -> JoinHandle<()> {
            tokio::spawn(async move {
                drop(out);
            })
        }
    }

    #[tokio::test]
    async fn interrupt_cancels_pending_stream_open_promptly() {
        let provider = SlowOpenProvider {
            open_delay_ms: 30_000,
        };
        let registry = Registry::from_providers(vec![Arc::new(provider)]);
        let store = Store::open_in_memory().await.unwrap();
        let credentials =
            CredentialStore::new(std::env::temp_dir().join("goat-agent-slow-open.json"));
        let agent = GoatAgent::new(
            registry,
            store,
            credentials,
            Some(target("mock")),
            std::env::temp_dir(),
            None,
        )
        .await;
        let session = Session::spawn(agent);
        let (ops, mut events, _handle) = session.into_parts();
        ops.send(Op::SubmitMessage {
            id: TaskId(3),
            text: "go".to_owned(),
            display: None,
            attachments: Vec::new(),
        })
        .await
        .unwrap();

        let mut started = false;
        while let Some(event) = events.recv().await {
            if matches!(event, Event::TaskStarted { .. }) {
                started = true;
                break;
            }
        }
        assert!(started);
        ops.send(Op::Interrupt { id: TaskId(3) }).await.unwrap();

        let at = std::time::Instant::now();
        let mut interrupted = false;
        while let Some(event) = events.recv().await {
            match event {
                Event::TextDone { .. } => panic!("interrupted turn must not finalize text"),
                Event::TaskDone {
                    interrupted: was, ..
                } => {
                    interrupted = was;
                    break;
                }
                _ => {}
            }
        }
        assert!(interrupted);
        assert!(
            at.elapsed() < std::time::Duration::from_secs(5),
            "interrupt during stream open must cancel promptly, took {:?}",
            at.elapsed()
        );
    }

    #[tokio::test]
    async fn interrupt_ends_turn() {
        let session = Session::spawn(agent_with("late", 5_000).await);
        let (ops, mut events, _handle) = session.into_parts();
        ops.send(Op::SubmitMessage {
            id: TaskId(9),
            text: "hi".to_owned(),
            display: None,
            attachments: Vec::new(),
        })
        .await
        .unwrap();
        ops.send(Op::Interrupt { id: TaskId(9) }).await.unwrap();

        let mut interrupted = false;
        while let Some(event) = events.recv().await {
            match event {
                Event::TextDone { .. } => panic!("interrupted turn must not finalize text"),
                Event::TaskDone {
                    interrupted: was, ..
                } => {
                    interrupted = was;
                    break;
                }
                _ => {}
            }
        }
        assert!(interrupted);
    }

    #[tokio::test]
    async fn unknown_provider_errors() {
        let registry = Registry::from_providers(vec![]);
        let store = Store::open_in_memory().await.unwrap();
        let credentials = CredentialStore::new(std::env::temp_dir().join("goat-agent-ghost.json"));
        let agent = GoatAgent::new(
            registry,
            store,
            credentials,
            Some(target("ghost")),
            std::env::temp_dir(),
            None,
        )
        .await;
        let session = Session::spawn(agent);
        let (ops, mut events, _handle) = session.into_parts();
        ops.send(Op::SubmitMessage {
            id: TaskId(1),
            text: "hi".to_owned(),
            display: None,
            attachments: Vec::new(),
        })
        .await
        .unwrap();

        let mut saw_error = false;
        while let Some(event) = events.recv().await {
            match event {
                Event::Error { .. } => saw_error = true,
                Event::TaskDone { .. } => break,
                _ => {}
            }
        }
        assert!(saw_error);
    }

    #[tokio::test]
    async fn shell_runs_and_reports_output() {
        let session = Session::spawn(agent_with("unused", 0).await);
        let (ops, mut events, _handle) = session.into_parts();
        ops.send(Op::SubmitShell {
            id: TaskId(7),
            command: "echo hello".to_owned(),
        })
        .await
        .unwrap();

        let mut started = false;
        let mut output = None;
        while let Some(event) = events.recv().await {
            match event {
                Event::TaskStarted { .. } => started = true,
                Event::ShellDone { output: text, .. } => output = Some(text),
                Event::TaskDone { interrupted, .. } => {
                    assert!(!interrupted);
                    break;
                }
                _ => {}
            }
        }
        assert!(started);
        assert!(
            output
                .expect("ShellDone must precede TaskDone")
                .contains("hello")
        );
    }

    #[tokio::test]
    async fn shell_interrupt_kills_command() {
        let session = Session::spawn(agent_with("unused", 0).await);
        let (ops, mut events, _handle) = session.into_parts();
        ops.send(Op::SubmitShell {
            id: TaskId(8),
            command: "sleep 999".to_owned(),
        })
        .await
        .unwrap();
        ops.send(Op::Interrupt { id: TaskId(8) }).await.unwrap();

        let mut output = None;
        while let Some(event) = events.recv().await {
            match event {
                Event::ShellDone { output: text, .. } => output = Some(text),
                Event::TaskDone { interrupted, .. } => {
                    assert!(!interrupted);
                    break;
                }
                _ => {}
            }
        }
        assert_eq!(output.as_deref(), Some(crate::turn::SHELL_INTERRUPTED));
    }

    #[tokio::test]
    async fn shell_persists_role_and_resumes() {
        let provider = MockProvider {
            id: "mock".to_owned(),
            reply: "ok".to_owned(),
            delay_ms: 0,
        };
        let registry = Registry::from_providers(vec![Arc::new(provider)]);
        let store = Store::open_in_memory().await.unwrap();
        let credentials = CredentialStore::new(std::env::temp_dir().join("goat-agent-shell.json"));
        let agent = GoatAgent::new(
            registry,
            store.clone(),
            credentials,
            Some(target("mock")),
            std::env::temp_dir(),
            None,
        )
        .await;
        let session = Session::spawn(agent);
        let (ops, mut events, _handle) = session.into_parts();

        ops.send(Op::SubmitShell {
            id: TaskId(1),
            command: "echo persisted".to_owned(),
        })
        .await
        .unwrap();
        drain_until_task_done(&mut events).await;

        let thread = store.get_thread(1).await.unwrap().expect("thread created");
        assert_eq!(thread.title.as_deref(), Some("! echo persisted"));
        let messages = store.get_messages(1).await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "shell");

        ops.send(Op::Resume { thread_id: 1 }).await.unwrap();
        let mut restored = false;
        let mut refreshed = false;
        while let Some(event) = events.recv().await {
            match event {
                Event::ConversationRestored { entries, .. } => {
                    assert!(entries.iter().any(|entry| matches!(
                        entry,
                        goat_protocol::TranscriptEntry::Shell { command, output }
                            if command == "echo persisted" && output.contains("persisted")
                    )));
                    restored = true;
                }
                Event::ModelListChanged { .. } if restored => {
                    refreshed = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(restored, "expected ConversationRestored");
        assert!(refreshed, "expected ModelListChanged after resume");
    }

    #[tokio::test]
    async fn resume_latest_restores_most_recent_thread() {
        let provider = MockProvider {
            id: "mock".to_owned(),
            reply: "ok".to_owned(),
            delay_ms: 0,
        };
        let registry = Registry::from_providers(vec![Arc::new(provider)]);
        let store = Store::open_in_memory().await.unwrap();
        let credentials =
            CredentialStore::new(std::env::temp_dir().join("goat-agent-resume-latest.json"));
        let agent = GoatAgent::new(
            registry,
            store.clone(),
            credentials,
            Some(target("mock")),
            std::env::temp_dir(),
            None,
        )
        .await;
        let session = Session::spawn(agent);
        let (ops, mut events, _handle) = session.into_parts();

        ops.send(Op::SubmitMessage {
            id: TaskId(1),
            text: "hello there".to_owned(),
            display: None,
            attachments: Vec::new(),
        })
        .await
        .unwrap();
        drain_until_task_done(&mut events).await;

        ops.send(Op::ResumeLatest {}).await.unwrap();
        while let Some(event) = events.recv().await {
            if let Event::ConversationRestored { entries, .. } = event {
                assert!(entries.iter().any(|entry| matches!(
                    entry,
                    goat_protocol::TranscriptEntry::User { text, .. } if text == "hello there"
                )));
                return;
            }
        }
        panic!("expected ConversationRestored");
    }

    #[tokio::test]
    async fn resume_latest_without_history_notifies() {
        let provider = MockProvider {
            id: "mock".to_owned(),
            reply: "ok".to_owned(),
            delay_ms: 0,
        };
        let registry = Registry::from_providers(vec![Arc::new(provider)]);
        let store = Store::open_in_memory().await.unwrap();
        let credentials =
            CredentialStore::new(std::env::temp_dir().join("goat-agent-resume-latest-empty.json"));
        let agent = GoatAgent::new(
            registry,
            store.clone(),
            credentials,
            Some(target("mock")),
            std::env::temp_dir(),
            None,
        )
        .await;
        let session = Session::spawn(agent);
        let (ops, mut events, _handle) = session.into_parts();

        ops.send(Op::ResumeLatest {}).await.unwrap();
        ops.send(Op::Shutdown {}).await.unwrap();

        let mut saw_notify = false;
        while let Some(event) = events.recv().await {
            match event {
                Event::Notify {
                    kind: goat_protocol::NotifyKind::Info,
                    ..
                } => saw_notify = true,
                Event::ConversationRestored { .. } => {
                    panic!("nothing to restore in an empty store");
                }
                _ => {}
            }
        }
        assert!(saw_notify, "empty resume must emit an Info notify");
    }

    async fn drain_until_task_done(events: &mut mpsc::Receiver<Event>) {
        while let Some(event) = events.recv().await {
            if matches!(event, Event::TaskDone { .. }) {
                return;
            }
        }
    }

    #[tokio::test]
    async fn clear_is_noop_in_engine_now_that_daemon_owns_rebind() {
        let provider = MockProvider {
            id: "mock".to_owned(),
            reply: "ok".to_owned(),
            delay_ms: 0,
        };
        let registry = Registry::from_providers(vec![Arc::new(provider)]);
        let store = Store::open_in_memory().await.unwrap();
        let credentials = CredentialStore::new(std::env::temp_dir().join("goat-agent-clear.json"));
        let agent = GoatAgent::new(
            registry,
            store.clone(),
            credentials,
            Some(target("mock")),
            std::env::temp_dir(),
            None,
        )
        .await;
        let session = Session::spawn(agent);
        let (ops, mut events, _handle) = session.into_parts();

        ops.send(Op::SubmitMessage {
            id: TaskId(1),
            text: "first".to_owned(),
            display: None,
            attachments: Vec::new(),
        })
        .await
        .unwrap();
        drain_until_task_done(&mut events).await;

        ops.send(Op::Clear {}).await.unwrap();

        ops.send(Op::SubmitMessage {
            id: TaskId(2),
            text: "second".to_owned(),
            display: None,
            attachments: Vec::new(),
        })
        .await
        .unwrap();
        drain_until_task_done(&mut events).await;

        assert!(store.get_thread(1).await.unwrap().is_some());
    }
}
