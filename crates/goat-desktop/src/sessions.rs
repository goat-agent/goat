use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use goat_api::{
    Api, ConversationList, ConversationListParams, ResumeMode, SessionSubmit, SessionSubmitParams,
};
use goat_client::{AdminRequest, ApiSession, Attachment, Link};
use goat_command::{CommandEffect, CommandShape};
use goat_commands::CommandRegistry;
use goat_protocol::{Event, InputAttachment, NotifyKind, Op, TaskId};
use serde::Serialize;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::{Mutex as AsyncMutex, mpsc};
use tokio::task::JoinHandle;

#[path = "adapter.rs"]
mod adapter;
#[path = "state.rs"]
mod state;

pub use adapter::AdminInput;
use state::DesktopSession;

pub struct Sessions {
    link: Arc<Link>,
    next_id: AtomicU64,
    live: Mutex<HashMap<u64, Arc<Live>>>,
}

struct Live {
    id: u64,
    app: AppHandle,
    link: Arc<Link>,
    core: Mutex<Core>,
    dispatch: AsyncMutex<()>,
}

struct Core {
    registry: CommandRegistry,
    session: DesktopSession,
    generation: u64,
    channels: Option<Channels>,
    control: Option<ApiSession>,
}

struct Channels {
    ops: mpsc::Sender<Op>,
    admin: mpsc::Sender<AdminRequest>,
    pending: Option<Receivers>,
    forwarder: Option<tauri::async_runtime::JoinHandle<()>>,
    pump: JoinHandle<()>,
}

struct Receivers {
    events: mpsc::Receiver<Event>,
    presence: mpsc::Receiver<usize>,
}

#[derive(Serialize)]
pub struct SessionInfo {
    id: u64,
    cwd: String,
    daemon: Value,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommandOutcome {
    Show { screen: String },
    Done,
    Notice { level: NotifyKind, text: String },
    Quit,
}

#[derive(Serialize)]
pub struct CommandSpec {
    name: String,
    aliases: Vec<String>,
    description: String,
    usage: String,
}

struct ResolvedCommand {
    outcome: CommandOutcome,
    ops: Vec<Op>,
    admin: Vec<AdminRequest>,
    notifications: Vec<(NotifyKind, String)>,
}

impl Sessions {
    pub fn new(link: Arc<Link>) -> Self {
        Self {
            link,
            next_id: AtomicU64::new(1),
            live: Mutex::new(HashMap::new()),
        }
    }

    fn get(&self, id: u64) -> Result<Arc<Live>, String> {
        lock(&self.live)?
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("session {id} is not open"))
    }
}

impl Core {
    fn ensure_open(&self) -> Result<(), String> {
        if self.channels.is_some() {
            Ok(())
        } else {
            Err("session is closed".to_owned())
        }
    }

    fn close(&mut self) -> bool {
        self.control.take();
        self.channels.take().is_some()
    }
}

impl Live {
    async fn api(&self) -> Result<Api, String> {
        {
            let mut core = lock(&self.core)?;
            core.ensure_open()?;
            if let Some(control) = &core.control
                && !control.closed.is_cancelled()
            {
                return Ok(control.api.clone());
            }
            core.control.take();
        }
        let control = goat_client::open_api(&self.link, "goat-desktop-session")
            .await
            .map_err(|error| error.to_string())?;
        let mut core = lock(&self.core)?;
        core.ensure_open()?;
        let api = control.api.clone();
        core.control = Some(control);
        Ok(api)
    }

    async fn send_op(self: &Arc<Self>, mut op: Op) -> Result<Option<TaskId>, String> {
        match op {
            Op::Clear {} => return self.rebind(ResumeMode::New {}).await.map(|()| None),
            Op::ResumeLatest {} => return self.rebind(ResumeMode::Latest {}).await.map(|()| None),
            Op::Resume { conversation_id } => {
                return self
                    .rebind(ResumeMode::Conversation { conversation_id })
                    .await
                    .map(|()| None);
            }
            Op::SubmitMessage { .. } | Op::SubmitShell { .. } | Op::Compact { .. } => {
                return self.submit(op).await.map(Some);
            }
            _ => {}
        }
        let sender = lock(&self.core)?
            .channels
            .as_ref()
            .ok_or("session is closed")?
            .ops
            .clone();
        let permit = sender
            .reserve_owned()
            .await
            .map_err(|_| "session operation channel is closed".to_owned())?;
        let core = lock(&self.core)?;
        core.ensure_open()?;
        core.session.normalize(&mut op);
        permit.send(op);
        Ok(None)
    }

    async fn submit(&self, mut op: Op) -> Result<TaskId, String> {
        let kind = state::TaskKind::from_op(&op)?;
        let api = self.api().await?;
        let session = {
            let mut core = lock(&self.core)?;
            core.ensure_open()?;
            core.session.normalize(&mut op);
            core.session.submitting();
            goat_api::SessionId(core.session.session_id)
        };
        let result = api
            .call::<SessionSubmit>(SessionSubmitParams { session, op })
            .await;
        let mut core = lock(&self.core)?;
        let output = match result {
            Ok(output) => output,
            Err(error) => {
                core.session.submission_failed();
                return Err(error.to_string());
            }
        };
        core.ensure_open()?;
        core.session.submitted(kind, output.task);
        Ok(output.task)
    }

    async fn rebind(self: &Arc<Self>, resume: ResumeMode) -> Result<(), String> {
        let cwd = {
            let core = lock(&self.core)?;
            core.ensure_open()?;
            PathBuf::from(&core.session.cwd)
        };
        let (attachment, _) = goat_client::connect(self.link.clone(), cwd, resume)
            .await
            .map_err(|error| error.to_string())?;
        let session = DesktopSession::new(&attachment);
        let mut channels = Channels::from_attachment(attachment);
        let mut core = lock(&self.core)?;
        core.ensure_open()?;
        let listening = core
            .channels
            .as_ref()
            .is_some_and(|channels| channels.forwarder.is_some());
        core.generation += 1;
        core.session.rebind(session);
        if let ResumeMode::Conversation { conversation_id } = resume {
            core.session.conversation_id = Some(conversation_id);
        }
        core.registry.set_skills(&[]);
        core.channels.take();
        if listening {
            channels.start(self, core.generation);
        }
        core.channels = Some(channels);
        Ok(())
    }

    async fn send_admin(&self, request: AdminRequest) -> Result<(), String> {
        let sender = lock(&self.core)?
            .channels
            .as_ref()
            .ok_or("session is closed")?
            .admin
            .clone();
        let permit = sender
            .reserve_owned()
            .await
            .map_err(|_| "session admin channel is closed".to_owned())?;
        lock(&self.core)?.ensure_open()?;
        permit.send(request);
        Ok(())
    }

    async fn refresh_conversation(&self) -> Result<(), String> {
        let cwd = lock(&self.core)?.session.cwd.clone();
        let listed = self
            .api()
            .await?
            .call::<ConversationList>(ConversationListParams { cwd })
            .await
            .map_err(|error| error.to_string())?;
        let mut core = lock(&self.core)?;
        core.ensure_open()?;
        core.session.conversation_id = listed
            .conversations
            .iter()
            .find(|entry| entry.live.is_some_and(|id| id.0 == core.session.session_id))
            .map(|entry| entry.conversation_id);
        Ok(())
    }
}

impl Channels {
    fn from_attachment(attachment: Attachment) -> Self {
        let Attachment {
            ops,
            admin,
            events,
            presence,
            pump,
            ..
        } = attachment;
        Self {
            ops,
            admin,
            pending: Some(Receivers { events, presence }),
            forwarder: None,
            pump,
        }
    }

    fn start(&mut self, live: &Arc<Live>, generation: u64) {
        if let Some(receivers) = self.pending.take() {
            self.forwarder = Some(tauri::async_runtime::spawn(forward(
                live.app.clone(),
                Arc::downgrade(live),
                live.id,
                generation,
                receivers,
            )));
        }
    }
}

impl Drop for Channels {
    fn drop(&mut self) {
        self.pump.abort();
        if let Some(forwarder) = &self.forwarder {
            forwarder.abort();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, String> {
    mutex.lock().map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn session_open(
    app: AppHandle,
    sessions: State<'_, Sessions>,
    cwd: String,
    resume: ResumeMode,
) -> Result<SessionInfo, String> {
    let (attachment, _) = goat_client::connect(sessions.link.clone(), PathBuf::from(cwd), resume)
        .await
        .map_err(|error| error.to_string())?;
    let id = sessions.next_id.fetch_add(1, Ordering::Relaxed);
    let info = SessionInfo {
        id,
        cwd: attachment.cwd.clone(),
        daemon: json!({
            "wire": attachment.daemon.wire,
            "build": attachment.daemon.build,
            "version": attachment.daemon.version,
            "pid": attachment.daemon.pid,
            "started_at": attachment.daemon.started_at,
            "ready": attachment.daemon.ready,
            "busy": attachment.daemon.busy,
        }),
    };
    let mut session = DesktopSession::new(&attachment);
    if let ResumeMode::Conversation { conversation_id } = resume {
        session.conversation_id = Some(conversation_id);
    }
    let live = Arc::new(Live {
        id,
        app,
        link: sessions.link.clone(),
        core: Mutex::new(Core {
            registry: CommandRegistry::builtin(),
            session,
            generation: 1,
            channels: Some(Channels::from_attachment(attachment)),
            control: None,
        }),
        dispatch: AsyncMutex::new(()),
    });
    lock(&sessions.live)?.insert(id, live);
    Ok(info)
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn session_listen(sessions: State<'_, Sessions>, id: u64) -> Result<(), String> {
    let live = sessions.get(id)?;
    let mut core = lock(&live.core)?;
    let generation = core.generation;
    core.channels
        .as_mut()
        .ok_or("session is closed")?
        .start(&live, generation);
    Ok(())
}

async fn forward(
    app: AppHandle,
    live: Weak<Live>,
    id: u64,
    generation: u64,
    mut receivers: Receivers,
) {
    let event_name = format!("session:{id}");
    let presence_name = format!("session:{id}:presence");
    let mut events_open = true;
    let mut presence_open = true;
    while events_open || presence_open {
        tokio::select! {
            event = receivers.events.recv(), if events_open => {
                if let Some(event) = event {
                    let result = forward_event(&app, &live, generation, &event_name, &event);
                    match result {
                        Ok(true) => {}
                        Ok(false) => return,
                        Err(error) => {
                            tracing::warn!(session = id, %error, "desktop session event forwarding stopped");
                            break;
                        }
                    }
                } else {
                    events_open = false;
                }
            }
            count = receivers.presence.recv(), if presence_open => {
                if let Some(count) = count {
                    let result = forward_presence(&app, &live, generation, &presence_name, count);
                    match result {
                        Ok(true) => {}
                        Ok(false) => return,
                        Err(error) => {
                            tracing::warn!(session = id, %error, "desktop session presence forwarding stopped");
                            break;
                        }
                    }
                } else {
                    presence_open = false;
                }
            }
        }
    }
    if let Some(live) = live.upgrade()
        && let Ok(mut core) = live.core.lock()
        && core.generation == generation
    {
        if let Some(channels) = &mut core.channels {
            channels.forwarder.take();
        }
        if core.close()
            && let Err(error) = app.emit(&format!("session:{id}:closed"), ())
        {
            tracing::warn!(session = id, %error, "desktop session closure could not be emitted");
        }
    }
}

fn forward_event(
    app: &AppHandle,
    live: &Weak<Live>,
    generation: u64,
    name: &str,
    event: &Event,
) -> Result<bool, String> {
    let Some(live) = live.upgrade() else {
        return Ok(false);
    };
    let mut core = lock(&live.core)?;
    if core.generation != generation || core.channels.is_none() {
        return Ok(false);
    }
    if let Event::SkillsChanged { skills } = event {
        core.registry.set_skills(skills);
    }
    core.session.event(event);
    app.emit(name, event).map_err(|error| error.to_string())?;
    Ok(true)
}

fn forward_presence(
    app: &AppHandle,
    live: &Weak<Live>,
    generation: u64,
    name: &str,
    count: usize,
) -> Result<bool, String> {
    let Some(live) = live.upgrade() else {
        return Ok(false);
    };
    let mut core = lock(&live.core)?;
    if core.generation != generation || core.channels.is_none() {
        return Ok(false);
    }
    core.session.window_count = count;
    app.emit(name, count).map_err(|error| error.to_string())?;
    Ok(true)
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn session_close(sessions: State<'_, Sessions>, id: u64) -> Result<(), String> {
    let live = lock(&sessions.live)?
        .remove(&id)
        .ok_or_else(|| format!("session {id} is not open"))?;
    if lock(&live.core)?.close() {
        live.app
            .emit(&format!("session:{id}:closed"), ())
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn session_op(sessions: State<'_, Sessions>, id: u64, op: Op) -> Result<(), String> {
    let live = sessions.get(id)?;
    let _dispatch = live.dispatch.lock().await;
    live.send_op(op).await.map(|_| ())
}

#[tauri::command]
pub async fn session_submit(
    sessions: State<'_, Sessions>,
    id: u64,
    text: String,
    attachments: Vec<InputAttachment>,
) -> Result<u64, String> {
    let live = sessions.get(id)?;
    let _dispatch = live.dispatch.lock().await;
    live.submit(Op::SubmitMessage {
        id: TaskId(0),
        text,
        display: None,
        attachments,
    })
    .await
    .map(|task| task.0)
}

#[tauri::command]
pub async fn session_admin(
    sessions: State<'_, Sessions>,
    id: u64,
    request: AdminInput,
) -> Result<(), String> {
    let live = sessions.get(id)?;
    let _dispatch = live.dispatch.lock().await;
    live.send_admin(request.into()).await
}

#[tauri::command]
pub async fn session_command(
    sessions: State<'_, Sessions>,
    id: u64,
    line: String,
) -> Result<CommandOutcome, String> {
    let current = sessions.get(id)?;
    let _dispatch = current.dispatch.lock().await;
    if goat_command::parse_line(&line).is_ok_and(|line| line.name == "status") {
        current.refresh_conversation().await?;
    }
    let resolving = current.clone();
    let resolved = tokio::task::spawn_blocking(move || resolve_command(&resolving, &line))
        .await
        .map_err(|error| error.to_string())??;
    for op in resolved.ops {
        current.send_op(op).await?;
    }
    for request in resolved.admin {
        current.send_admin(request).await?;
    }
    for (kind, message) in resolved.notifications {
        current
            .app
            .emit(&format!("session:{id}"), Event::Notify { kind, message })
            .map_err(|error| error.to_string())?;
    }
    Ok(resolved.outcome)
}

fn resolve_command(current: &Live, line: &str) -> Result<ResolvedCommand, String> {
    let parsed = goat_command::parse_line(line).ok();
    if parsed.as_ref().is_some_and(|line| line.name == "status") {
        let cwd = lock(&current.core)?.session.cwd.clone();
        let workspace = goat_worktree::workspace(std::path::Path::new(&cwd)).ok();
        let pull_request = workspace.as_ref().and_then(|workspace| {
            goat_github::gh_available()
                .then(|| goat_github::pr_for_branch(&workspace.repo_root, &workspace.git_branch))
                .flatten()
        });
        let mut core = lock(&current.core)?;
        core.session.workspace = workspace;
        core.session.pull_request = pull_request;
    }
    let mut core = lock(&current.core)?;
    core.ensure_open()?;
    let Core {
        registry, session, ..
    } = &mut *core;
    session.notifications.clear();
    let screen = parsed
        .and_then(|line| registry.spec(&line.name))
        .map(|spec| match spec.name.as_str() {
            "provider" => "config".to_owned(),
            _ => spec.name,
        });
    let effect = registry.resolve_line(line, session);
    let mut resolved = ResolvedCommand {
        outcome: CommandOutcome::Done,
        ops: Vec::new(),
        admin: Vec::new(),
        notifications: std::mem::take(&mut session.notifications),
    };
    match effect {
        CommandEffect::Show(_) => {
            resolved.outcome = CommandOutcome::Show {
                screen: screen.ok_or("screen command has no registered name")?,
            };
        }
        CommandEffect::Dispatch(ops) => resolved.ops = ops,
        CommandEffect::Admin(requests) => resolved.admin = requests,
        CommandEffect::Submit { display, prompt } => {
            resolved.ops.push(Op::SubmitMessage {
                id: TaskId(0),
                text: prompt,
                display: Some(display),
                attachments: Vec::new(),
            });
        }
        CommandEffect::Noop => {
            if let Some((level, text)) = resolved.notifications.pop() {
                resolved.outcome = CommandOutcome::Notice { level, text };
            }
        }
        CommandEffect::Quit => resolved.outcome = CommandOutcome::Quit,
    }
    Ok(resolved)
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn command_specs(sessions: State<'_, Sessions>, id: u64) -> Result<Vec<CommandSpec>, String> {
    let live = sessions.get(id)?;
    let core = lock(&live.core)?;
    core.ensure_open()?;
    let specs = core.registry.specs();
    Ok(specs
        .into_iter()
        .map(|spec| {
            let mut usage = spec.usage();
            if let CommandShape::Branches(branches) = &spec.shape {
                for branch in branches {
                    usage.push('\n');
                    usage.push_str(&spec.branch_usage(branch));
                }
            }
            CommandSpec {
                usage,
                name: spec.name,
                aliases: spec.aliases,
                description: spec.description,
            }
        })
        .collect())
}

#[tauri::command]
pub async fn session_state(sessions: State<'_, Sessions>, id: u64) -> Result<Value, String> {
    let live = sessions.get(id)?;
    let _dispatch = live.dispatch.lock().await;
    live.refresh_conversation().await?;
    let core = lock(&live.core)?;
    core.ensure_open()?;
    serde_json::to_value(core.session.view()).map_err(|error| error.to_string())
}
