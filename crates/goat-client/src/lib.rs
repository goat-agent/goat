use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use goat_api::{
    Api, ConversationList, ConversationListParams, DaemonStatus, Empty, FsList, FsListParams,
    ResumeMode, SessionControl, SessionControlParams, SessionKill, SessionKillParams, SessionList,
    SessionOpen, SessionOpenParams, SessionSnapshot, SessionSubmit, SessionSubmitParams,
    SessionWatch, SessionWatchParams, StreamEvent, WatchFrom, WatchItem,
};
use goat_api::{BuildId, Busy};
use goat_protocol::{Event, Op};
use goat_wire::WireError;
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;

mod link;
mod session;

pub use link::{EnvelopeConn, LOCAL, Link};
pub use session::{ApiSession, open as open_api, open_serving};

pub const GREET_TIMEOUT: Duration = Duration::from_secs(2);
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub const STOP_TIMEOUT: Duration = Duration::from_secs(20);
pub const SPAWN_BUDGET: Duration = Duration::from_secs(45);

const SPAWN_POLL: Duration = Duration::from_millis(100);
const AGENT: &str = concat!("goat-client/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("wire error: {0}")]
    Wire(#[from] WireError),
    #[error("remote error: {0}")]
    Remote(#[from] goat_remote::RemoteError),
    #[error("the daemon did not answer within {0:?}")]
    Timeout(Duration),
    #[error("unexpected daemon response during handshake")]
    Handshake,
    #[error(
        "the running daemon speaks a different protocol and is busy ({sessions} session(s), {turns} agent turn(s))"
    )]
    BusyIncompatible { sessions: usize, turns: usize },
    #[error("the remote daemon speaks a different protocol")]
    RemoteIncompatible,
    #[error(
        "the running daemon speaks a different protocol; run `goat daemon start` to replace it"
    )]
    Incompatible,
    #[error("daemon did not open a session: {0}")]
    OpenFailed(String),
    #[error("could not start daemon: {0}")]
    SpawnFailed(String),
    #[error("daemon refused the request: {0}")]
    Refused(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub wire: String,
    pub build: Option<BuildId>,
    pub version: String,
    pub pid: u32,
    pub started_at: i64,
    pub ready: bool,
    pub busy: Busy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Daemon {
    Reachable(Identity),
    Silent,
    Absent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OnStale {
    Attach,
    ReplaceIfIdle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resolution {
    Reused,
    Stale,
    Replace,
    Refuse,
}

#[must_use]
pub fn is_current(mine: &Identity, theirs: &Identity) -> bool {
    theirs.wire == mine.wire
        && match (mine.build.as_ref(), theirs.build.as_ref()) {
            (Some(ours), Some(other)) => ours == other,
            _ => true,
        }
}

fn resolve(ours: &Identity, theirs: &Identity, on_stale: OnStale) -> Resolution {
    if is_current(ours, theirs) {
        return Resolution::Reused;
    }
    if on_stale == OnStale::ReplaceIfIdle && theirs.busy.is_idle() {
        return Resolution::Replace;
    }
    if theirs.wire != ours.wire {
        return Resolution::Refuse;
    }
    Resolution::Stale
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attached {
    Reused,
    Started,
    Replaced(Box<Identity>),
    Stale(Box<Identity>),
}

#[must_use]
pub fn mine() -> Identity {
    identity_of(BuildId::current())
}

#[must_use]
pub fn mine_for(daemon_exe: Option<&Path>) -> Identity {
    identity_of(match daemon_exe {
        Some(path) => BuildId::of(path),
        None => BuildId::current(),
    })
}

fn identity_of(build: Option<BuildId>) -> Identity {
    Identity {
        wire: goat_wire::envelope_fingerprint().to_owned(),
        build,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        pid: std::process::id(),
        started_at: 0,
        ready: true,
        busy: Busy::default(),
    }
}

pub struct Attachment {
    pub ops: mpsc::Sender<Op>,
    pub edits: mpsc::Sender<Vec<goat_api::ConfigEdit>>,
    pub events: mpsc::Receiver<Event>,
    pub presence: mpsc::Receiver<usize>,
    pub client_id: u64,
    pub cwd: String,
    pub daemon: Identity,
    session: u64,
    pub pump: JoinHandle<()>,
}

impl Attachment {
    #[must_use]
    pub fn session(&self) -> u64 {
        self.session
    }
}

const OPS_CAPACITY: usize = 32;
const EVENTS_CAPACITY: usize = 512;
const PRESENCE_CAPACITY: usize = 16;

pub async fn connect(
    link: Arc<Link>,
    cwd: PathBuf,
    resume: ResumeMode,
) -> Result<(Attachment, Attached), ClientError> {
    let (session, identity, client_id, attached) = ensure(&link, OnStale::Attach).await?;

    let opened = session
        .api
        .call::<SessionOpen>(SessionOpenParams {
            cwd: cwd.display().to_string(),
            resume,
        })
        .await
        .map_err(|err| ClientError::OpenFailed(err.message))?;

    Ok((
        spawn_pumps(session, &opened, client_id, link, identity),
        attached,
    ))
}

pub async fn start(socket_path: &Path, daemon_exe: &Path) -> Result<Attached, ClientError> {
    let link = Link::local(socket_path.to_path_buf(), daemon_exe.to_path_buf());
    let (session, _, _, attached) = ensure(&link, OnStale::ReplaceIfIdle).await?;
    session.shutdown();
    Ok(attached)
}

pub async fn greet(socket_path: &Path) -> Daemon {
    let link = Link::local(socket_path.to_path_buf(), PathBuf::new());
    match open(&link).await {
        Ok((session, identity, _)) => {
            session.shutdown();
            Daemon::Reachable(identity)
        }
        Err(ClientError::Timeout(_) | ClientError::Handshake) => Daemon::Silent,
        Err(_) => Daemon::Absent,
    }
}

async fn open(link: &Link) -> Result<(ApiSession, Identity, u64), ClientError> {
    let session = tokio::time::timeout(GREET_TIMEOUT, session::open(link, AGENT))
        .await
        .map_err(|_| ClientError::Timeout(GREET_TIMEOUT))??;

    let info = session.daemon.info.clone();
    let client_id = info
        .get("client_id")
        .and_then(|value| value.as_str())
        .and_then(|text| text.parse::<u64>().ok())
        .unwrap_or(0);
    let build = info
        .get("build")
        .cloned()
        .and_then(|value| serde_json::from_value::<Option<BuildId>>(value).ok())
        .flatten();

    let status = session
        .api
        .call::<DaemonStatus>(Empty {})
        .await
        .map_err(|_| ClientError::Handshake)?;

    let identity = Identity {
        wire: session.daemon.envelope.clone(),
        build,
        version: status.version,
        pid: status.pid,
        started_at: status.started_at,
        ready: status.ready,
        busy: Busy {
            sessions: status.sessions,
            turns: status.turns,
        },
    };
    Ok((session, identity, client_id))
}

async fn ensure(
    link: &Link,
    on_stale: OnStale,
) -> Result<(ApiSession, Identity, u64, Attached), ClientError> {
    let ours = mine_for(link.local_parts().map(|(_, daemon_exe)| daemon_exe));
    let opened = open(link).await;
    if link.local_parts().is_none() {
        let (session, identity, client_id) = opened?;
        if identity.wire != ours.wire {
            return Err(ClientError::RemoteIncompatible);
        }
        return Ok((session, identity, client_id, Attached::Reused));
    }

    let opened = match opened {
        Ok(opened) => Some(opened),
        Err(err @ (ClientError::Timeout(_) | ClientError::Handshake)) => return Err(err),
        Err(_) => None,
    };
    let Some((session, identity, client_id)) = opened else {
        let (session, identity, client_id) = spawn_and_wait(link).await?;
        return Ok((session, identity, client_id, Attached::Started));
    };

    match resolve(&ours, &identity, on_stale) {
        Resolution::Reused => Ok((session, identity, client_id, Attached::Reused)),
        Resolution::Stale => {
            let stale = identity.clone();
            Ok((
                session,
                identity,
                client_id,
                Attached::Stale(Box::new(stale)),
            ))
        }
        Resolution::Refuse => match on_stale {
            OnStale::Attach => Err(ClientError::Incompatible),
            OnStale::ReplaceIfIdle => Err(ClientError::BusyIncompatible {
                sessions: identity.busy.sessions,
                turns: identity.busy.turns,
            }),
        },
        Resolution::Replace => {
            let replaced = identity.clone();
            stop_daemon_and_wait(session).await;
            let (session, identity, client_id) = spawn_and_wait(link).await?;
            Ok((
                session,
                identity,
                client_id,
                Attached::Replaced(Box::new(replaced)),
            ))
        }
    }
}

async fn stop_daemon_and_wait(session: ApiSession) {
    let _ = tokio::time::timeout(
        STOP_TIMEOUT,
        session
            .api
            .call::<goat_api::AdminDaemonStop>(goat_api::AdminDaemonStopParams { if_idle: false }),
    )
    .await;
    session.shutdown();
    tokio::time::sleep(SPAWN_POLL).await;
}

async fn spawn_and_wait(link: &Link) -> Result<(ApiSession, Identity, u64), ClientError> {
    link.spawn_local()?;
    let deadline = tokio::time::Instant::now() + SPAWN_BUDGET;
    loop {
        match open(link).await {
            Ok(opened) => return Ok(opened),
            Err(err) if tokio::time::Instant::now() >= deadline => {
                return Err(ClientError::SpawnFailed(format!(
                    "daemon did not become reachable: {err}"
                )));
            }
            Err(_) => tokio::time::sleep(SPAWN_POLL).await,
        }
    }
}

struct Shared {
    session: Mutex<goat_api::SessionId>,
    cwd: String,
}

fn spawn_pumps(
    session: ApiSession,
    opened: &goat_api::SessionOpenOutput,
    client_id: u64,
    link: Arc<Link>,
    daemon: Identity,
) -> Attachment {
    let (ops_tx, ops_rx) = mpsc::channel::<Op>(OPS_CAPACITY);
    let (edits_tx, edits_rx) = mpsc::channel::<Vec<goat_api::ConfigEdit>>(OPS_CAPACITY);
    let (events_tx, events_rx) = mpsc::channel::<Event>(EVENTS_CAPACITY);
    let (presence_tx, presence_rx) = mpsc::channel::<usize>(PRESENCE_CAPACITY);

    let session_id = opened.session;
    let cwd = opened.cwd.clone();
    let shared = Arc::new(Shared {
        session: Mutex::new(session_id),
        cwd: cwd.clone(),
    });

    let pump = tokio::spawn(run_pump(
        session,
        shared,
        link,
        ops_rx,
        edits_rx,
        events_tx,
        presence_tx,
    ));

    Attachment {
        ops: ops_tx,
        edits: edits_tx,
        events: events_rx,
        presence: presence_rx,
        client_id,
        cwd,
        daemon,
        session: session_id.0,
        pump,
    }
}

async fn run_pump(
    mut session: ApiSession,
    shared: Arc<Shared>,
    link: Arc<Link>,
    mut ops_rx: mpsc::Receiver<Op>,
    mut edits_rx: mpsc::Receiver<Vec<goat_api::ConfigEdit>>,
    events_tx: mpsc::Sender<Event>,
    presence_tx: mpsc::Sender<usize>,
) {
    let mut from = WatchFrom::Snapshot {};
    loop {
        let watching = *shared.session.lock().await;
        let Ok(mut watch) = session
            .api
            .open::<SessionWatch>(SessionWatchParams {
                session: watching,
                from: from.clone(),
            })
            .await
        else {
            break;
        };

        let keep_going = loop {
            tokio::select! {
                biased;
                edits = edits_rx.recv() => {
                    let Some(edits) = edits else { break false };
                    let _ = session
                        .api
                        .call::<goat_api::AdminConfigEdit>(goat_api::AdminConfigEditParams { edits })
                        .await;
                }
                op = ops_rx.recv() => {
                    let Some(op) = op else { break false };
                    match handle_op(&session.api, &shared, op, &events_tx).await {
                        OpOutcome::Continue => {}
                        OpOutcome::Rewatch => {
                            from = WatchFrom::Snapshot {};
                            break true;
                        }
                        OpOutcome::Stop => break false,
                    }
                }
                item = watch.recv() => {
                    match item {
                        Some(StreamEvent::Item { item, .. }) => {
                            if !deliver(item, &mut from, &events_tx, &presence_tx).await {
                                break false;
                            }
                        }
                        Some(StreamEvent::End(_)) | None => break true,
                    }
                }
            }
        };

        if !keep_going {
            break;
        }
        if session.closed.is_cancelled() {
            match reopen(&link).await {
                Some(fresh) => session = fresh,
                None => break,
            }
        }
    }
    session.shutdown();
}

async fn reopen(link: &Link) -> Option<ApiSession> {
    for _ in 0..100 {
        if let Ok(session) = session::open(link, AGENT).await {
            return Some(session);
        }
        tokio::time::sleep(SPAWN_POLL).await;
    }
    None
}

enum OpOutcome {
    Continue,
    Rewatch,
    Stop,
}

async fn handle_op(
    api: &Api,
    shared: &Arc<Shared>,
    op: Op,
    events_tx: &mpsc::Sender<Event>,
) -> OpOutcome {
    let session = *shared.session.lock().await;
    match op {
        Op::Shutdown {} => OpOutcome::Stop,
        Op::Clear {} => rebind(api, shared, ResumeMode::New {}).await,
        Op::ResumeLatest {} => rebind(api, shared, ResumeMode::Latest {}).await,
        Op::Resume { conversation_id } => {
            rebind(api, shared, ResumeMode::Conversation { conversation_id }).await
        }
        Op::ListConversations {} => {
            if let Ok(listed) = api
                .call::<ConversationList>(ConversationListParams {
                    cwd: shared.cwd.clone(),
                })
                .await
            {
                let _ = events_tx
                    .send(Event::ConversationsListed {
                        conversations: listed
                            .conversations
                            .into_iter()
                            .map(|entry| goat_protocol::ConversationSummary {
                                id: entry.conversation_id,
                                title: entry.title.unwrap_or_default(),
                                model: entry.model,
                                updated_at: entry.updated_at,
                                live: entry.live.is_some(),
                            })
                            .collect(),
                    })
                    .await;
            }
            OpOutcome::Continue
        }
        Op::ListFiles {} => {
            if let Ok(listed) = api
                .call::<FsList>(FsListParams {
                    path: shared.cwd.clone(),
                    recursive: true,
                })
                .await
            {
                let _ = events_tx
                    .send(Event::FilesListed {
                        entries: listed
                            .entries
                            .into_iter()
                            .map(|entry| match entry.kind {
                                goat_api::DirEntryKind::Directory {} => {
                                    format!("{}/", entry.name)
                                }
                                goat_api::DirEntryKind::File {}
                                | goat_api::DirEntryKind::Symlink {} => entry.name,
                            })
                            .collect(),
                    })
                    .await;
            }
            OpOutcome::Continue
        }
        Op::Interrupt { .. }
        | Op::Answer { .. }
        | Op::DequeueMessage { .. }
        | Op::ProcessKill { .. }
        | Op::ProcessWatch { .. } => {
            let _ = api
                .call::<SessionControl>(SessionControlParams { session, op })
                .await;
            OpOutcome::Continue
        }
        other => {
            let _ = api
                .call::<SessionSubmit>(SessionSubmitParams { session, op: other })
                .await;
            OpOutcome::Continue
        }
    }
}

async fn rebind(api: &Api, shared: &Arc<Shared>, resume: ResumeMode) -> OpOutcome {
    let Ok(opened) = api
        .call::<SessionOpen>(SessionOpenParams {
            cwd: shared.cwd.clone(),
            resume,
        })
        .await
    else {
        return OpOutcome::Continue;
    };
    *shared.session.lock().await = opened.session;
    OpOutcome::Rewatch
}

async fn deliver(
    item: WatchItem,
    from: &mut WatchFrom,
    events_tx: &mpsc::Sender<Event>,
    presence_tx: &mpsc::Sender<usize>,
) -> bool {
    let (WatchItem::Presence { cursor, .. }
    | WatchItem::Event { cursor, .. }
    | WatchItem::Snapshot { cursor, .. }) = &item;
    *from = WatchFrom::Cursor {
        cursor: cursor.clone(),
    };

    match item {
        WatchItem::Presence { clients, .. } => {
            let _ = presence_tx.try_send(clients);
            true
        }
        WatchItem::Event { event, .. } => events_tx.send(*event).await.is_ok(),
        WatchItem::Snapshot { state, .. } => {
            for event in snapshot_to_events(*state) {
                if events_tx.send(event).await.is_err() {
                    return false;
                }
            }
            true
        }
    }
}

fn snapshot_to_events(snapshot: SessionSnapshot) -> Vec<Event> {
    let SessionSnapshot {
        target,
        transcript,
        pending,
        context_tokens,
        compaction_threshold,
        skills,
        accounts,
        models,
        selected,
        mode,
        plan_path,
        processes,
        usage,
        rate_limits,
        active,
        retry,
        ..
    } = snapshot;

    let mut events = Vec::new();
    if let Some(target) = target {
        events.push(Event::ConversationRestored {
            target,
            entries: transcript,
            context_tokens,
            compaction_threshold,
        });
    }
    events.push(Event::SkillsChanged { skills });
    events.push(Event::AccountsChanged {
        providers: accounts,
    });
    events.push(Event::ModelListChanged { entries: models });
    if let Some(target) = selected {
        events.push(Event::ModelSelected { target });
    }
    events.push(Event::ModeChanged { mode, plan_path });
    events.push(Event::ProcessListChanged { processes });
    if let Some(id) = active {
        events.push(Event::TaskStarted { id });
    }
    events.extend(pending);
    for entry in usage {
        events.push(Event::Usage {
            id: active.unwrap_or(goat_protocol::TaskId(0)),
            provider: entry.provider,
            account: entry.account,
            usage: entry.usage,
            context_window: entry.context_window,
            compaction_threshold: entry.compaction_threshold,
        });
    }
    if let Some(retry) = retry {
        events.push(Event::Retrying {
            id: retry.id,
            attempt: retry.attempt,
            max_attempts: retry.max_attempts,
            delay_ms: retry.delay_ms,
            reason: retry.reason,
            resets_at: retry.resets_at,
        });
    }
    for entry in rate_limits {
        events.push(Event::RateLimits {
            provider: entry.provider,
            account: entry.account,
            snapshot: entry.snapshot,
            cached_at: entry.cached_at,
        });
    }
    events
}

async fn one_shot<T, F, Fut>(link: &Link, call: F) -> Result<T, ClientError>
where
    F: FnOnce(Api) -> Fut,
    Fut: std::future::Future<Output = Result<T, goat_wire::envelope::CallError>>,
{
    let session = session::open(link, AGENT).await?;
    let result = call(session.api.clone()).await;
    session.shutdown();
    result.map_err(|err| ClientError::Refused(err.message))
}

pub async fn status(link: &Link) -> Result<Vec<goat_api::SessionInfo>, ClientError> {
    one_shot(link, |api| async move {
        api.call::<SessionList>(Empty {})
            .await
            .map(|out| out.sessions)
    })
    .await
}

pub async fn list_conversations(
    link: &Link,
    cwd: &Path,
) -> Result<Vec<goat_api::ConversationInfo>, ClientError> {
    let cwd = cwd.display().to_string();
    one_shot(link, |api| async move {
        api.call::<ConversationList>(ConversationListParams { cwd })
            .await
            .map(|out| out.conversations)
    })
    .await
}

pub async fn stop(link: &Link) -> Result<(), ClientError> {
    stop_if(link, false).await
}

pub async fn stop_if_idle(link: &Link) -> Result<Option<Busy>, ClientError> {
    match stop_if(link, true).await {
        Ok(()) => Ok(None),
        Err(ClientError::BusyIncompatible { sessions, turns }) => {
            Ok(Some(Busy { sessions, turns }))
        }
        Err(err) => Err(err),
    }
}

async fn stop_if(link: &Link, if_idle: bool) -> Result<(), ClientError> {
    let session = session::open(link, AGENT).await?;
    let answered = session
        .api
        .call::<goat_api::AdminDaemonStop>(goat_api::AdminDaemonStopParams { if_idle })
        .await;
    session.shutdown();
    match answered {
        Ok(goat_api::AdminDaemonStopOutput::Busy { sessions, turns }) => {
            Err(ClientError::BusyIncompatible { sessions, turns })
        }
        Ok(goat_api::AdminDaemonStopOutput::Stopping) | Err(_) => Ok(()),
    }
}

pub async fn reload(
    link: &Link,
    agent: Option<String>,
) -> Result<goat_api::AdminAgentReloadOutput, ClientError> {
    one_shot(link, |api| async move {
        api.call::<goat_api::AdminAgentReload>(goat_api::AdminAgentReloadParams { agent })
            .await
    })
    .await
}

pub async fn edit_config(
    link: &Link,
    edits: Vec<goat_api::ConfigEdit>,
) -> Result<bool, ClientError> {
    let (session, _, _, _) = ensure(link, OnStale::Attach).await?;
    let answered = session
        .api
        .call::<goat_api::AdminConfigEdit>(goat_api::AdminConfigEditParams { edits })
        .await;
    session.shutdown();
    answered
        .map(|out| out.changed)
        .map_err(|err| ClientError::Refused(err.message))
}

pub async fn kill_session(link: &Link, session: u64) -> Result<(), ClientError> {
    one_shot(link, |api| async move {
        api.call::<SessionKill>(SessionKillParams {
            session: goat_api::SessionId(session),
        })
        .await
        .map(|_| ())
    })
    .await
}

pub struct PairingInfo {
    pub code: String,
    pub server_fingerprint: String,
    pub advertised: Vec<String>,
}

pub async fn pair_device(link: &Link, label: String) -> Result<PairingInfo, ClientError> {
    let out = one_shot(link, |api| async move {
        api.call::<goat_api::AdminDevicePair>(goat_api::AdminDevicePairParams { label })
            .await
    })
    .await?;
    Ok(PairingInfo {
        code: out.code,
        server_fingerprint: out.server_fingerprint,
        advertised: out.advertised,
    })
}

pub async fn list_devices(link: &Link) -> Result<Vec<goat_api::DeviceInfo>, ClientError> {
    one_shot(link, |api| async move {
        api.call::<goat_api::AdminDeviceList>(Empty {})
            .await
            .map(|out| out.devices)
    })
    .await
}

pub async fn revoke_device(link: &Link, device: String) -> Result<bool, ClientError> {
    one_shot(link, |api| async move {
        api.call::<goat_api::AdminDeviceRevoke>(goat_api::AdminDeviceRevokeParams { device })
            .await
            .map(|out| out.ok)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::{Identity, OnStale, Resolution, is_current, resolve, snapshot_to_events};
    use goat_api::{BuildId, Busy};
    use goat_api::{SessionId, SessionSnapshot};
    use goat_protocol::{Event, ModelTarget, SkillInfo};

    fn build(len: u64) -> BuildId {
        BuildId {
            path: "/bin/goat".to_owned(),
            len,
            mtime: 1,
        }
    }

    fn peer(wire: &str, len: u64, busy: Busy) -> Identity {
        Identity {
            wire: wire.to_owned(),
            build: Some(build(len)),
            version: "0.1.27".to_owned(),
            pid: 9,
            started_at: 0,
            ready: true,
            busy,
        }
    }

    fn snapshot(target: Option<ModelTarget>, skills: Vec<SkillInfo>) -> SessionSnapshot {
        SessionSnapshot {
            session: SessionId(1),
            cwd: "/tmp".to_owned(),
            target,
            transcript: Vec::new(),
            pending: Vec::new(),
            context_tokens: None,
            compaction_threshold: None,
            skills,
            accounts: Vec::new(),
            models: Vec::new(),
            selected: None,
            mode: goat_protocol::Mode::Normal,
            plan_path: None,
            processes: Vec::new(),
            usage: Vec::new(),
            rate_limits: Vec::new(),
            active: None,
            retry: None,
        }
    }

    #[test]
    fn a_second_client_binary_reports_the_daemon_exe_not_its_own() {
        let dir = std::env::temp_dir().join(format!("goat-buildid-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let daemon_exe = dir.join("goat");
        std::fs::write(&daemon_exe, b"daemon binary").unwrap();

        let desktop = super::mine_for(Some(&daemon_exe));
        let cli = super::mine_for(Some(&daemon_exe));
        assert_eq!(
            desktop.build, cli.build,
            "two different client binaries must agree on the daemon build"
        );

        let daemon = Identity {
            build: goat_api::BuildId::of(&daemon_exe),
            ..desktop.clone()
        };
        assert!(
            is_current(&desktop, &daemon),
            "a desktop client must attach to the daemon it would have spawned"
        );

        let own_exe = super::mine();
        assert_ne!(
            own_exe.build, desktop.build,
            "the fix only matters because the two differ"
        );
        assert!(
            !is_current(&own_exe, &daemon),
            "reporting its own exe is what used to replace an idle daemon every launch"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_daemon_exe_falls_back_without_claiming_a_build() {
        let absent = std::path::Path::new("/definitely/not/a/goat/binary");
        assert_eq!(super::mine_for(Some(absent)).build, None);
    }

    #[test]
    fn opening_a_session_never_replaces_the_daemon() {
        let idle = Busy::default();
        let busy = Busy {
            sessions: 1,
            turns: 0,
        };
        let ours = peer("aaaa", 1, idle);
        for (wire, len, load, want) in [
            ("aaaa", 1, idle, Resolution::Reused),
            ("aaaa", 1, busy, Resolution::Reused),
            ("aaaa", 2, idle, Resolution::Stale),
            ("aaaa", 2, busy, Resolution::Stale),
            ("bbbb", 1, idle, Resolution::Refuse),
            ("bbbb", 1, busy, Resolution::Refuse),
        ] {
            assert_eq!(
                resolve(&ours, &peer(wire, len, load), OnStale::Attach),
                want,
                "wire={wire} len={len} busy={load:?}"
            );
        }
    }

    #[test]
    fn daemon_start_replaces_an_idle_leftover_and_preserves_a_busy_one() {
        let idle = Busy::default();
        let busy = Busy {
            sessions: 1,
            turns: 0,
        };
        let ours = peer("aaaa", 1, idle);
        for (wire, len, load, want) in [
            ("aaaa", 1, idle, Resolution::Reused),
            ("aaaa", 1, busy, Resolution::Reused),
            ("aaaa", 2, idle, Resolution::Replace),
            ("aaaa", 2, busy, Resolution::Stale),
            ("bbbb", 1, idle, Resolution::Replace),
            ("bbbb", 1, busy, Resolution::Refuse),
            ("bbbb", 2, busy, Resolution::Refuse),
        ] {
            assert_eq!(
                resolve(&ours, &peer(wire, len, load), OnStale::ReplaceIfIdle),
                want,
                "wire={wire} len={len} busy={load:?}"
            );
        }
    }

    #[test]
    fn unknown_build_is_not_stale() {
        let ours = peer("aaaa", 1, Busy::default());
        let mut theirs = peer("aaaa", 2, Busy::default());
        theirs.build = None;
        assert!(is_current(&ours, &theirs));
    }

    #[test]
    fn a_resumed_snapshot_restores_before_it_reports_state() {
        let events = snapshot_to_events(snapshot(
            Some(ModelTarget {
                provider: "p".to_owned(),
                model: "m".to_owned(),
                account: "a".to_owned(),
                effort: None,
            }),
            vec![SkillInfo {
                name: "deploy".to_owned(),
                description: "ship".to_owned(),
                command: None,
            }],
        ));
        assert!(
            matches!(events.first(), Some(Event::ConversationRestored { .. })),
            "restore must land before state events"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::SkillsChanged { skills } if skills.len() == 1)),
            "skills must be delivered from the snapshot"
        );
    }

    #[test]
    fn a_new_session_snapshot_omits_restore_but_keeps_skills() {
        let events = snapshot_to_events(snapshot(
            None,
            vec![SkillInfo {
                name: "deploy".to_owned(),
                description: "ship".to_owned(),
                command: None,
            }],
        ));
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::ConversationRestored { .. })),
            "a fresh session has nothing to restore"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::SkillsChanged { skills } if skills.len() == 1)),
            "skills must still arrive"
        );
    }

    #[test]
    fn a_retrying_snapshot_replays_the_retry_state() {
        let mut state = snapshot(None, Vec::new());
        state.retry = Some(goat_api::RetryEntry {
            id: goat_protocol::TaskId(3),
            attempt: 2,
            max_attempts: 5,
            delay_ms: 1000,
            reason: "overloaded".to_owned(),
            resets_at: None,
        });
        let events = snapshot_to_events(state);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::Retrying { attempt: 2, .. })),
            "a client that reattaches mid-retry must still see the retry"
        );
    }
}
