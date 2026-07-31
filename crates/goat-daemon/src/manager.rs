use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use goat_auth::CredentialStore;
use goat_core::Session;
use goat_engine::GoatAgent;
use goat_protocol::Op;
use goat_store::CodeStore as Store;
use goat_wire::{ClientId, ResumeMode, ServerFrame, SessionId, SessionInfo};
use tokio::sync::Mutex;
use tokio::sync::{mpsc, oneshot};

const SUBSCRIBER_QUEUE: usize = 1024;

use crate::session::{LiveSession, SessionInner, SessionTable};

#[derive(Clone)]
pub struct Manager {
    inner: Arc<ManagerInner>,
}

struct ManagerInner {
    auth_path: PathBuf,
    user_providers: goat_config::UserProviders,
    db_path: PathBuf,
    sessions: Mutex<SessionTable>,
    threads: Mutex<HashMap<i64, SessionId>>,
    next_session: AtomicU64,
    next_client: AtomicU64,
    remote: Mutex<Option<RemoteControls>>,
    meter: std::sync::OnceLock<goat_proxy::Meter>,
    reload: std::sync::OnceLock<mpsc::Sender<ReloadRequest>>,
}

pub struct ReloadRequest {
    pub agent: Option<String>,
    pub reply: oneshot::Sender<goat_wire::ReloadReport>,
}

struct RemoteControls {
    pairing: goat_remote::Pairing,
    devices: goat_remote::Devices,
    server_fingerprint: String,
    advertised: Vec<String>,
}

impl Manager {
    pub fn new(
        auth_path: PathBuf,
        user_providers: goat_config::UserProviders,
        db_path: PathBuf,
    ) -> Self {
        Self {
            inner: Arc::new(ManagerInner {
                auth_path,
                user_providers,
                db_path,
                sessions: Mutex::new(HashMap::new()),
                threads: Mutex::new(HashMap::new()),
                next_session: AtomicU64::new(1),
                next_client: AtomicU64::new(1),
                remote: Mutex::new(None),
                meter: std::sync::OnceLock::new(),
                reload: std::sync::OnceLock::new(),
            }),
        }
    }

    pub fn set_meter(&self, meter: goat_proxy::Meter) {
        let _ = self.inner.meter.set(meter);
    }

    pub fn set_reload(&self, sender: mpsc::Sender<ReloadRequest>) {
        let _ = self.inner.reload.set(sender);
    }

    pub async fn reload_agents(
        &self,
        agent: Option<String>,
    ) -> Result<goat_wire::ReloadReport, String> {
        let Some(sender) = self.inner.reload.get() else {
            return Err("no agent runtime is attached to this daemon".to_owned());
        };
        let (reply, answer) = oneshot::channel();
        sender
            .send(ReloadRequest { agent, reply })
            .await
            .map_err(|_| "the agent runtime is no longer accepting reloads".to_owned())?;
        answer
            .await
            .map_err(|_| "the agent runtime dropped the reload request".to_owned())
    }

    pub(crate) fn set_remote(
        &self,
        pairing: goat_remote::Pairing,
        devices: goat_remote::Devices,
        server_fingerprint: String,
        advertised: Vec<String>,
    ) {
        let inner = self.inner.clone();
        tokio::spawn(async move {
            *inner.remote.lock().await = Some(RemoteControls {
                pairing,
                devices,
                server_fingerprint,
                advertised,
            });
        });
    }

    pub(crate) async fn pair_device(
        &self,
        label: String,
    ) -> Result<(String, String, Vec<String>), String> {
        let guard = self.inner.remote.lock().await;
        let controls = guard.as_ref().ok_or("remote is not enabled")?;
        let code = controls.pairing.mint(label).await;
        Ok((
            code,
            controls.server_fingerprint.clone(),
            controls.advertised.clone(),
        ))
    }

    pub(crate) async fn list_devices(&self) -> Result<Vec<goat_wire::DeviceInfo>, String> {
        let guard = self.inner.remote.lock().await;
        let controls = guard.as_ref().ok_or("remote is not enabled")?;
        let devices = controls
            .devices
            .list()
            .await
            .into_iter()
            .map(|d| goat_wire::DeviceInfo {
                id: d.id,
                label: d.label,
                paired_at: d.paired_at,
            })
            .collect();
        Ok(devices)
    }

    pub(crate) async fn revoke_device(&self, id: &str) -> Result<bool, String> {
        let guard = self.inner.remote.lock().await;
        let controls = guard.as_ref().ok_or("remote is not enabled")?;
        controls
            .devices
            .revoke(id)
            .await
            .map_err(|e| format!("revoke: {e}"))
    }

    pub(crate) fn next_client_id(&self) -> ClientId {
        ClientId(self.inner.next_client.fetch_add(1, Ordering::Relaxed))
    }

    fn now_ms() -> i64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
    }

    fn normalize_cwd(cwd: &std::path::Path) -> String {
        std::fs::canonicalize(cwd)
            .unwrap_or_else(|_| cwd.to_path_buf())
            .display()
            .to_string()
    }

    async fn register_thread(&self, thread_id: i64, session: SessionId) {
        let mut threads = self.inner.threads.lock().await;
        thread_register(&mut threads, thread_id, session);
    }

    async fn unregister_thread_if_owner(&self, session: SessionId) {
        let mut threads = self.inner.threads.lock().await;
        thread_unregister_owner(&mut threads, session);
    }

    pub(crate) async fn open_or_attach(
        &self,
        cwd: PathBuf,
        resume: ResumeMode,
    ) -> Result<SessionId, String> {
        let normalized = Self::normalize_cwd(&cwd);
        let thread_id = self.resolve_thread_id(&normalized, resume).await;
        if let Some(tid) = thread_id {
            let threads = self.inner.threads.lock().await;
            if let Some(existing) = threads.get(&tid).copied() {
                return Ok(existing);
            }
        }
        self.open_session(cwd, normalized, thread_id).await
    }

    async fn resolve_thread_id(&self, normalized: &str, resume: ResumeMode) -> Option<i64> {
        match resume {
            ResumeMode::New {} => None,
            ResumeMode::Thread { thread_id } => Some(thread_id),
            ResumeMode::Latest {} => {
                let store = Store::open(&self.inner.db_path).await.ok()?;
                store
                    .latest_thread_in(normalized.to_owned())
                    .await
                    .ok()
                    .flatten()
                    .map(|t| t.id)
            }
        }
    }

    async fn open_session(
        &self,
        cwd: PathBuf,
        normalized: String,
        thread_id: Option<i64>,
    ) -> Result<SessionId, String> {
        let credentials = CredentialStore::new(self.inner.auth_path.clone());
        let store = Store::open(&self.inner.db_path)
            .await
            .map_err(|e| format!("store: {e}"))?;
        let store_for_pump = store.clone();
        let registry = goat_providers::Registry::load_metered(
            &credentials,
            &self.inner.user_providers,
            goat_providers::DEFAULT_ACCOUNT,
            self.inner.meter.get().cloned(),
        );
        let agent = GoatAgent::new(
            registry,
            store,
            credentials,
            self.inner.user_providers.clone(),
            None,
            cwd.clone(),
            self.inner.meter.get().cloned(),
        )
        .await;
        let session = Session::spawn(agent);
        let (ops, events, handle) = session.into_parts();

        let id = SessionId(self.inner.next_session.fetch_add(1, Ordering::Relaxed));
        let ready = Arc::new(tokio::sync::Notify::new());
        let inner = Arc::new(Mutex::new(SessionInner {
            id,
            cwd: normalized,
            created_at: Self::now_ms(),
            ops: ops.clone(),
            log: std::collections::VecDeque::new(),
            next_seq: 0,
            next_task: 1,
            subscribers: Vec::new(),
            state: goat_wire::SessionLiveState::Idle {},
            snapshot: None,
            tokens: 0,
            open_asks: 0,
            live_processes: 0,
            thread_id,
            awaits_restore: thread_id.is_some(),
            ready,
            resurrected: std::collections::HashSet::new(),
            pending_attaches: 0,
            skills: Vec::new(),
            accounts: Vec::new(),
            model_list: Vec::new(),
            selected_target: None,
            rate_limits: std::collections::HashMap::new(),
            state_ready: false,
            state_watermark: 0,
        }));

        let id = {
            let mut table = self.inner.sessions.lock().await;
            if let Some(tid) = thread_id {
                let mut threads = self.inner.threads.lock().await;
                if let Some(existing) = threads.get(&tid).copied() {
                    let _ = ops.send(Op::Shutdown {}).await;
                    return Ok(existing);
                }
                threads.insert(tid, id);
            }
            table.insert(
                id,
                LiveSession {
                    inner: inner.clone(),
                },
            );
            id
        };

        spawn_pump(self.clone(), id, inner, events, handle, store_for_pump);

        if let Some(thread_id) = thread_id {
            let _ = ops.send(Op::Resume { thread_id }).await;
        }
        Ok(id)
    }

    pub(crate) async fn subscribe(
        &self,
        session: SessionId,
        client: ClientId,
        sender: mpsc::Sender<ServerFrame>,
    ) -> Result<(), String> {
        let live = {
            let table = self.inner.sessions.lock().await;
            let live = table.get(&session).cloned();
            if let Some(live) = &live {
                live.inner.lock().await.pending_attaches += 1;
            }
            live
        };
        let live = live.ok_or("unknown session")?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let notified = {
                let inner = live.inner.lock().await;
                if inner.subscribe_ready() {
                    break;
                }
                inner.ready.clone()
            };
            let wait = notified.notified();
            tokio::pin!(wait);
            wait.as_mut().enable();
            {
                let inner = live.inner.lock().await;
                if inner.subscribe_ready() {
                    break;
                }
            }
            if tokio::time::timeout_at(deadline, wait).await.is_err() {
                break;
            }
        }
        let (backlog, live_rx) = {
            let mut inner = live.inner.lock().await;
            let snapshot = inner.build_snapshot();
            let ServerFrame::Snapshot { watermark, .. } = &snapshot else {
                unreachable!("build_snapshot always returns a Snapshot frame")
            };
            let watermark = *watermark;
            let mut backlog = vec![snapshot];
            for (seq, event) in &inner.log {
                if *seq < watermark {
                    continue;
                }
                backlog.push(ServerFrame::Event {
                    session,
                    seq: *seq,
                    event: event.clone(),
                });
            }
            let (bridge_tx, bridge_rx) = mpsc::channel(SUBSCRIBER_QUEUE);
            crate::session::subscriber_upsert(&mut inner.subscribers, client, bridge_tx);
            inner.pending_attaches = inner.pending_attaches.saturating_sub(1);
            let clients = inner.presence();
            broadcast_presence(&mut inner, clients);
            (backlog, bridge_rx)
        };
        let replay_sent = spawn_subscriber_bridge(sender, backlog, live_rx);
        let _ = replay_sent.await;
        Ok(())
    }

    pub(crate) async fn unsubscribe(&self, session: SessionId, client: ClientId) {
        let live = {
            let table = self.inner.sessions.lock().await;
            table.get(&session).cloned()
        };
        let Some(live) = live else {
            return;
        };
        let evictable = {
            let mut inner = live.inner.lock().await;
            crate::session::subscriber_map_remove(&mut inner.subscribers, client);
            let clients = inner.presence();
            broadcast_presence(&mut inner, clients);
            inner.evictable()
        };
        if evictable {
            self.evict_if_idle(session).await;
        }
    }

    pub(crate) async fn drop_client(&self, client: ClientId) {
        let lives: Vec<(SessionId, LiveSession)> = {
            let table = self.inner.sessions.lock().await;
            table.iter().map(|(id, live)| (*id, live.clone())).collect()
        };
        let mut candidates = Vec::new();
        for (id, live) in lives {
            let mut inner = live.inner.lock().await;
            crate::session::subscriber_map_remove(&mut inner.subscribers, client);
            let clients = inner.presence();
            broadcast_presence(&mut inner, clients);
            if inner.evictable() {
                candidates.push(id);
            }
        }
        for id in candidates {
            self.evict_if_idle(id).await;
        }
    }

    async fn evict_if_idle(&self, session: SessionId) {
        let ops = {
            let mut table = self.inner.sessions.lock().await;
            let Some(live) = table.get(&session) else {
                return;
            };
            let inner = live.inner.lock().await;
            if !inner.evictable() {
                return;
            }
            let ops = inner.ops.clone();
            drop(inner);
            table.remove(&session);
            ops
        };
        self.unregister_thread_if_owner(session).await;
        let _ = ops.send(Op::Shutdown {}).await;
        tracing::info!(session = session.0, "evicted idle session with no windows");
    }

    pub(crate) async fn shutdown_all_sessions(&self) {
        let all_ops: Vec<mpsc::Sender<Op>> = {
            let table = self.inner.sessions.lock().await;
            let mut ops = Vec::new();
            for live in table.values() {
                let inner = live.inner.lock().await;
                ops.push(inner.ops.clone());
            }
            ops
        };
        for ops in &all_ops {
            let _ = ops.send(Op::Shutdown {}).await;
        }
        if !all_ops.is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }
    }

    pub(crate) async fn rebind(
        &self,
        client: ClientId,
        from: SessionId,
        client_sender: &mpsc::Sender<ServerFrame>,
        resume: ResumeMode,
    ) -> Result<(), String> {
        let cwd = {
            let table = self.inner.sessions.lock().await;
            let live = table.get(&from).ok_or("unknown session")?;
            let inner = live.inner.lock().await;
            inner.cwd.clone()
        };
        let new = self.open_or_attach(PathBuf::from(cwd), resume).await?;
        self.unsubscribe(from, client).await;
        let _ = client_sender
            .send(ServerFrame::Detached { session: from })
            .await;
        let _ = client_sender
            .send(ServerFrame::SessionOpened { session: new })
            .await;
        self.subscribe(new, client, client_sender.clone()).await
    }

    pub(crate) async fn submit(
        &self,
        session: SessionId,
        client_sender: &mpsc::Sender<ServerFrame>,
        correlation: u64,
        mut op: Op,
    ) -> Result<(), String> {
        let live = {
            let table = self.inner.sessions.lock().await;
            table.get(&session).cloned()
        };
        let live = live.ok_or("unknown session")?;
        let (ops, task) = {
            let mut inner = live.inner.lock().await;
            let task = inner.allocate_task();
            (inner.ops.clone(), task)
        };
        match &mut op {
            Op::SubmitMessage { id, .. } | Op::SubmitShell { id, .. } | Op::Compact { id, .. } => {
                *id = task;
            }
            _ => {}
        }
        let _ = client_sender
            .send(ServerFrame::CorrelationAssigned {
                session,
                correlation,
                task,
            })
            .await;
        ops.send(op).await.map_err(|_| "engine closed".to_owned())
    }

    pub async fn delegate_code(
        &self,
        cwd: PathBuf,
        prompt: String,
    ) -> Result<mpsc::Receiver<goat_protocol::Event>, String> {
        let session = self.open_or_attach(cwd, ResumeMode::New {}).await?;
        let client = self.next_client_id();
        let (frame_tx, mut frame_rx) = mpsc::channel::<ServerFrame>(SUBSCRIBER_QUEUE);
        self.subscribe(session, client, frame_tx.clone()).await?;
        self.submit(
            session,
            &frame_tx,
            1,
            Op::SubmitMessage {
                id: goat_protocol::TaskId(0),
                text: prompt,
                display: None,
                attachments: Vec::new(),
            },
        )
        .await?;

        let (event_tx, event_rx) = mpsc::channel::<goat_protocol::Event>(SUBSCRIBER_QUEUE);
        let manager = self.clone();
        tokio::spawn(async move {
            while let Some(frame) = frame_rx.recv().await {
                let ServerFrame::Event { event, .. } = frame else {
                    continue;
                };
                let terminal = matches!(
                    event,
                    goat_protocol::Event::TaskDone { .. } | goat_protocol::Event::Error { .. }
                );
                if event_tx.send(event).await.is_err() {
                    break;
                }
                if terminal {
                    break;
                }
            }
            manager.unsubscribe(session, client).await;
            manager.drop_client(client).await;
        });
        Ok(event_rx)
    }

    pub(crate) async fn control(&self, session: SessionId, op: Op) -> Result<(), String> {
        let live = {
            let table = self.inner.sessions.lock().await;
            table.get(&session).cloned()
        };
        let live = live.ok_or("unknown session")?;
        let (ops, thread_id, rewritten) = {
            let mut inner = live.inner.lock().await;
            let rewritten = rewrite_resurrected_answer(&mut inner, &op);
            (inner.ops.clone(), inner.thread_id, rewritten)
        };
        if let Some((call, message)) = rewritten {
            if let Some(tid) = thread_id {
                let store = Store::open(&self.inner.db_path)
                    .await
                    .map_err(|e| format!("store: {e}"))?;
                let _ = store.clear_open_prompt(tid, format!("{call}")).await;
            }
            return ops
                .send(Op::SubmitMessage {
                    id: goat_protocol::TaskId(0),
                    text: message,
                    display: None,
                    attachments: Vec::new(),
                })
                .await
                .map_err(|_| "engine closed".to_owned());
        }
        ops.send(op).await.map_err(|_| "engine closed".to_owned())
    }

    pub(crate) fn list_directory(path: &str) -> Result<Vec<goat_wire::DirEntry>, String> {
        let dir = std::fs::read_dir(path).map_err(|e| format!("read_dir: {e}"))?;
        let mut children = Vec::new();
        for entry in dir.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let file_type = entry.file_type().map_err(|e| format!("file_type: {e}"))?;
            let kind = if file_type.is_symlink() {
                goat_wire::DirEntryKind::Symlink {}
            } else if file_type.is_dir() {
                goat_wire::DirEntryKind::Directory {}
            } else {
                goat_wire::DirEntryKind::File {}
            };
            children.push(goat_wire::DirEntry { name, kind });
        }
        children.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(children)
    }

    pub(crate) async fn list_threads(&self, cwd: &str) -> Vec<goat_wire::ThreadInfo> {
        let normalized = Self::normalize_cwd(std::path::Path::new(cwd));
        let live: HashMap<i64, SessionId> = {
            let threads = self.inner.threads.lock().await;
            threads.clone()
        };
        let mut states: HashMap<SessionId, goat_wire::SessionLiveState> = HashMap::new();
        {
            let table = self.inner.sessions.lock().await;
            for session in live.values() {
                if let Some(found) = table.get(session) {
                    let inner = found.inner.lock().await;
                    states.insert(*session, inner.state);
                }
            }
        }
        let Ok(store) = Store::open(&self.inner.db_path).await else {
            return Vec::new();
        };
        let Ok(threads) = store.list_threads_in(normalized, 100).await else {
            return Vec::new();
        };
        threads
            .into_iter()
            .map(|t| thread_info(t, &live, &states))
            .collect()
    }

    pub(crate) async fn list_sessions(&self) -> Vec<SessionInfo> {
        let lives: Vec<LiveSession> = {
            let table = self.inner.sessions.lock().await;
            table.values().cloned().collect()
        };
        let mut out = Vec::new();
        let now = Self::now_ms();
        for live in lives {
            let inner = live.inner.lock().await;
            out.push(SessionInfo {
                session: inner.id,
                cwd: inner.cwd.clone(),
                state: inner.state,
                windows: inner.subscribers.len(),
                age_ms: now - inner.created_at,
                tokens: inner.tokens,
            });
        }
        out
    }

    pub(crate) async fn kill_session(&self, session: SessionId) -> Result<(), String> {
        let live = {
            let mut table = self.inner.sessions.lock().await;
            table.remove(&session)
        };
        let live = live.ok_or("unknown session")?;
        self.unregister_thread_if_owner(session).await;
        let ops = {
            let inner = live.inner.lock().await;
            inner.ops.clone()
        };
        let _ = ops.send(Op::Shutdown {}).await;
        Ok(())
    }

    async fn remove_session(&self, session: SessionId) {
        {
            let mut table = self.inner.sessions.lock().await;
            table.remove(&session);
        }
        self.unregister_thread_if_owner(session).await;
    }
}

fn spawn_subscriber_bridge(
    sender: mpsc::Sender<ServerFrame>,
    backlog: Vec<ServerFrame>,
    mut live_rx: mpsc::Receiver<ServerFrame>,
) -> oneshot::Receiver<()> {
    let (ready_tx, ready_rx) = oneshot::channel();
    tokio::spawn(async move {
        for frame in backlog {
            if sender.send(frame).await.is_err() {
                let _ = ready_tx.send(());
                return;
            }
        }
        let _ = ready_tx.send(());
        while let Some(frame) = live_rx.recv().await {
            if sender.send(frame).await.is_err() {
                break;
            }
        }
    });
    ready_rx
}

fn rewrite_resurrected_answer(inner: &mut SessionInner, op: &Op) -> Option<(u64, String)> {
    let (call, message) = match op {
        Op::Answer { call, answers, .. } => (call.0, format!("My answer: {}", answers.join(", "))),
        _ => return None,
    };
    if inner.resurrected.remove(&call) {
        inner.open_asks = inner.open_asks.saturating_sub(1);
        Some((call, message))
    } else {
        None
    }
}

fn broadcast_presence(inner: &mut SessionInner, clients: Vec<ClientId>) {
    let frame = ServerFrame::Presence {
        session: inner.id,
        clients,
    };
    inner
        .subscribers
        .retain(|sub| sub.sender.try_send(frame.clone()).is_ok());
}

fn spawn_pump(
    manager: Manager,
    session: SessionId,
    inner: Arc<Mutex<SessionInner>>,
    mut events: mpsc::Receiver<goat_protocol::Event>,
    handle: tokio::task::JoinHandle<()>,
    store: Store,
) {
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            let bound = matches!(event, goat_protocol::Event::ThreadBound { .. });
            let persist = {
                let mut guard = inner.lock().await;
                guard.record_and_fanout(event)
            };
            if let Some(persist) = persist {
                let now = Manager::now_ms();
                match persist.prompt {
                    Some(crate::session::PromptAction::Open {
                        call_id,
                        kind,
                        payload,
                        task_id,
                    }) => {
                        let _ = store
                            .record_open_prompt(
                                persist.thread_id,
                                call_id,
                                kind,
                                payload,
                                task_id,
                                now,
                            )
                            .await;
                    }
                    Some(crate::session::PromptAction::Close { call_id }) => {
                        let _ = store.clear_open_prompt(persist.thread_id, call_id).await;
                    }
                    None => {}
                }
                if bound {
                    manager.register_thread(persist.thread_id, session).await;
                    resurrect_open_prompts(&inner, &store, persist.thread_id).await;
                }
            }
            if inner.lock().await.evictable() {
                manager.evict_if_idle(session).await;
            }
        }
        {
            let mut guard = inner.lock().await;
            let frame = ServerFrame::Error {
                message: "session engine stopped".to_owned(),
            };
            guard
                .subscribers
                .retain(|sub| sub.sender.try_send(frame.clone()).is_ok());
        }
        manager.remove_session(session).await;
        handle.abort();
    });
}

async fn resurrect_open_prompts(inner: &Arc<Mutex<SessionInner>>, store: &Store, thread_id: i64) {
    let already_live = {
        let guard = inner.lock().await;
        guard.open_asks > 0
    };
    if already_live {
        return;
    }
    let Ok(prompts) = store.open_prompts(thread_id).await else {
        return;
    };
    for prompt in prompts {
        let Ok(call) = prompt.call_id.parse::<u64>() else {
            continue;
        };
        let event = match prompt.kind.as_str() {
            "ask" => {
                let Ok(questions) =
                    serde_json::from_str::<Vec<goat_protocol::AskQuestion>>(&prompt.payload)
                else {
                    continue;
                };
                goat_protocol::Event::AskStarted {
                    id: goat_protocol::TaskId(prompt.task_id),
                    call: goat_protocol::ToolCallId(call),
                    questions,
                }
            }
            _ => continue,
        };
        let mut guard = inner.lock().await;
        guard.resurrected.insert(call);
        let _ = guard.record_and_fanout(event);
    }
}

fn thread_info(
    t: goat_store::Thread,
    live: &HashMap<i64, SessionId>,
    states: &HashMap<SessionId, goat_wire::SessionLiveState>,
) -> goat_wire::ThreadInfo {
    let live_session = live.get(&t.id).copied();
    let state = live_session.and_then(|s| states.get(&s).copied());
    goat_wire::ThreadInfo {
        thread_id: t.id,
        cwd: t.cwd,
        title: t.title,
        model: t.model,
        updated_at: t.updated_at,
        live: live_session,
        state,
    }
}

fn thread_register(threads: &mut HashMap<i64, SessionId>, thread_id: i64, session: SessionId) {
    threads.entry(thread_id).or_insert(session);
}

fn thread_unregister_owner(threads: &mut HashMap<i64, SessionId>, session: SessionId) {
    threads.retain(|_, owner| *owner != session);
}

#[cfg(test)]
mod tests {
    use super::{
        HashMap, SessionId, spawn_subscriber_bridge, thread_register, thread_unregister_owner,
    };
    use goat_wire::ServerFrame;
    use tokio::sync::mpsc;

    #[test]
    fn register_is_first_writer_wins() {
        let mut threads: HashMap<i64, SessionId> = HashMap::new();
        thread_register(&mut threads, 42, SessionId(1));
        thread_register(&mut threads, 42, SessionId(2));
        assert_eq!(threads.get(&42), Some(&SessionId(1)));
    }

    #[test]
    fn unregister_only_removes_owned_entries() {
        let mut threads: HashMap<i64, SessionId> = HashMap::new();
        threads.insert(7, SessionId(1));
        threads.insert(8, SessionId(2));
        thread_unregister_owner(&mut threads, SessionId(1));
        assert_eq!(threads.get(&7), None);
        assert_eq!(threads.get(&8), Some(&SessionId(2)));
    }

    #[test]
    fn unregister_keeps_entry_reassigned_to_other_session() {
        let mut threads: HashMap<i64, SessionId> = HashMap::new();
        threads.insert(7, SessionId(2));
        thread_unregister_owner(&mut threads, SessionId(1));
        assert_eq!(threads.get(&7), Some(&SessionId(2)));
    }

    fn sample_thread(id: i64) -> goat_store::Thread {
        goat_store::Thread {
            id,
            cwd: "/tmp".to_owned(),
            title: Some("t".to_owned()),
            provider: "p".to_owned(),
            model: "m".to_owned(),
            account: "a".to_owned(),
            effort: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn thread_info_marks_live_threads() {
        let mut live: HashMap<i64, SessionId> = HashMap::new();
        live.insert(5, SessionId(9));
        let mut states: HashMap<SessionId, goat_wire::SessionLiveState> = HashMap::new();
        states.insert(SessionId(9), goat_wire::SessionLiveState::Active {});

        let info = super::thread_info(sample_thread(5), &live, &states);
        assert_eq!(info.live, Some(SessionId(9)));
        assert_eq!(info.state, Some(goat_wire::SessionLiveState::Active {}));
    }

    #[test]
    fn thread_info_marks_dead_threads_with_no_live_session() {
        let live: HashMap<i64, SessionId> = HashMap::new();
        let states: HashMap<SessionId, goat_wire::SessionLiveState> = HashMap::new();

        let info = super::thread_info(sample_thread(5), &live, &states);
        assert_eq!(info.live, None);
        assert_eq!(info.state, None);
        assert_eq!(info.thread_id, 5);
    }
    #[tokio::test]
    async fn subscriber_bridge_sends_backlog_before_live() {
        let (target_tx, mut target_rx) = mpsc::channel(8);
        let (live_tx, live_rx) = mpsc::channel(8);
        let replay_sent = spawn_subscriber_bridge(
            target_tx,
            vec![ServerFrame::Error {
                message: "backlog".to_owned(),
            }],
            live_rx,
        );
        live_tx
            .send(ServerFrame::Error {
                message: "live".to_owned(),
            })
            .await
            .unwrap();
        replay_sent.await.unwrap();
        match target_rx.recv().await.unwrap() {
            ServerFrame::Error { message } => assert_eq!(message, "backlog"),
            _ => panic!("expected backlog frame"),
        }
        match target_rx.recv().await.unwrap() {
            ServerFrame::Error { message } => assert_eq!(message, "live"),
            _ => panic!("expected live frame"),
        }
    }
}
