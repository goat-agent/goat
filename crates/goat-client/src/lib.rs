use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use goat_protocol::{Event, Op};
use goat_wire::{BuildId, Busy, ClientFrame, ResumeMode, ServerFrame, SessionId, WireError};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;

use crate::idmap::IdMap;
use crate::link::Conn;

mod idmap;
mod link;
mod session;

pub use link::{EnvelopeConn, LOCAL, Link};
pub use session::{ApiSession, open as open_api, open_serving};

pub const GREET_TIMEOUT: Duration = Duration::from_secs(2);
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub const STOP_TIMEOUT: Duration = Duration::from_secs(20);
pub const SPAWN_BUDGET: Duration = Duration::from_secs(45);

const SPAWN_POLL: Duration = Duration::from_millis(100);

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
pub enum Action {
    Attach,
    AttachStale,
    Replace,
    Refuse,
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
    Identity {
        wire: goat_wire::wire_fingerprint().to_owned(),
        build: BuildId::current(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        pid: std::process::id(),
        started_at: 0,
        ready: true,
        busy: Busy::default(),
    }
}

#[must_use]
pub fn decide(mine: &Identity, theirs: &Identity) -> Action {
    let incompatible = theirs.wire != mine.wire;
    let stale = match (mine.build.as_ref(), theirs.build.as_ref()) {
        (Some(ours), Some(other)) => ours != other,
        _ => false,
    };
    match (incompatible, stale, theirs.busy.is_idle()) {
        (false, false, _) => Action::Attach,
        (true, _, false) => Action::Refuse,
        (false, true, false) => Action::AttachStale,
        (_, _, true) => Action::Replace,
    }
}

pub struct Attachment {
    pub ops: mpsc::Sender<Op>,
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
    let (mut conn, identity, client_id, attached) = ensure(&link).await?;

    conn.send(ClientFrame::OpenSession {
        cwd: cwd.display().to_string(),
        resume,
    })
    .await?;
    let (session, opened_cwd) = match request(&mut conn).await? {
        ServerFrame::SessionOpened { session, cwd } => (session, cwd),
        ServerFrame::Error { message } => return Err(ClientError::OpenFailed(message)),
        _ => return Err(ClientError::Handshake),
    };

    Ok((
        spawn_pumps(conn, session, client_id, opened_cwd, link, identity),
        attached,
    ))
}

pub async fn start(socket_path: &Path, daemon_exe: &Path) -> Result<Attached, ClientError> {
    let link = Link::local(socket_path.to_path_buf(), daemon_exe.to_path_buf());
    let (_, _, _, attached) = ensure(&link).await?;
    Ok(attached)
}

pub async fn greet(socket_path: &Path) -> Daemon {
    let link = Link::local(socket_path.to_path_buf(), PathBuf::new());
    match open(&link).await {
        Ok((_, identity, _)) => Daemon::Reachable(identity),
        Err(ClientError::Timeout(_) | ClientError::Handshake) => Daemon::Silent,
        Err(_) => Daemon::Absent,
    }
}

async fn open(link: &Link) -> Result<(Conn, Identity, u64), ClientError> {
    let mut conn = tokio::time::timeout(GREET_TIMEOUT, link.dial())
        .await
        .map_err(|_| ClientError::Timeout(GREET_TIMEOUT))??;
    let frame = tokio::time::timeout(GREET_TIMEOUT, conn.recv())
        .await
        .map_err(|_| ClientError::Timeout(GREET_TIMEOUT))??;
    let ServerFrame::Welcome {
        wire,
        build,
        busy,
        version,
        pid,
        started_at,
        ready,
        client_id,
    } = frame
    else {
        return Err(ClientError::Handshake);
    };
    Ok((
        conn,
        Identity {
            wire,
            build,
            version,
            pid,
            started_at,
            ready,
            busy,
        },
        client_id.0,
    ))
}

async fn request(conn: &mut Conn) -> Result<ServerFrame, ClientError> {
    tokio::time::timeout(REQUEST_TIMEOUT, conn.recv())
        .await
        .map_err(|_| ClientError::Timeout(REQUEST_TIMEOUT))?
        .map_err(ClientError::Wire)
}

async fn ensure(link: &Link) -> Result<(Conn, Identity, u64, Attached), ClientError> {
    let ours = mine();
    let opened = open(link).await;
    if link.local_parts().is_none() {
        let (conn, identity, client_id) = opened?;
        if identity.wire != ours.wire {
            return Err(ClientError::RemoteIncompatible);
        }
        return Ok((conn, identity, client_id, Attached::Reused));
    }

    let opened = match opened {
        Ok(opened) => Some(opened),
        Err(err @ (ClientError::Timeout(_) | ClientError::Handshake)) => return Err(err),
        Err(_) => None,
    };
    let Some((conn, identity, client_id)) = opened else {
        let (conn, identity, client_id) = spawn_and_wait(link).await?;
        return Ok((conn, identity, client_id, Attached::Started));
    };

    match decide(&ours, &identity) {
        Action::Attach => Ok((conn, identity, client_id, Attached::Reused)),
        Action::AttachStale => {
            let stale = identity.clone();
            Ok((conn, identity, client_id, Attached::Stale(Box::new(stale))))
        }
        Action::Refuse => Err(ClientError::BusyIncompatible {
            sessions: identity.busy.sessions,
            turns: identity.busy.turns,
        }),
        Action::Replace => {
            let replaced = identity.clone();
            shutdown_and_wait(conn).await;
            let (conn, identity, client_id) = spawn_and_wait(link).await?;
            Ok((
                conn,
                identity,
                client_id,
                Attached::Replaced(Box::new(replaced)),
            ))
        }
    }
}

async fn shutdown_and_wait(mut conn: Conn) {
    if conn.send(ClientFrame::StopDaemon {}).await.is_err() {
        return;
    }
    let _ = tokio::time::timeout(STOP_TIMEOUT, async { while conn.recv().await.is_ok() {} }).await;
}

async fn spawn_and_wait(link: &Link) -> Result<(Conn, Identity, u64), ClientError> {
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

enum Outbound {
    Op(Op),
    ListConversations,
    ListFiles,
}

struct Shared {
    current: Mutex<SessionId>,
    current_conversation: Mutex<Option<i64>>,
    idmap: Mutex<IdMap>,
    cwd: String,
}

fn spawn_pumps(
    conn: Conn,
    session: SessionId,
    client_id: u64,
    cwd: String,
    link: Arc<Link>,
    daemon: Identity,
) -> Attachment {
    let (ops_tx, mut ops_rx) = mpsc::channel::<Op>(OPS_CAPACITY);
    let (events_tx, events_rx) = mpsc::channel::<Event>(EVENTS_CAPACITY);
    let (presence_tx, presence_rx) = mpsc::channel::<usize>(PRESENCE_CAPACITY);
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<Outbound>(OPS_CAPACITY + 8);

    let opened_cwd = cwd.clone();
    let shared = Arc::new(Shared {
        current: Mutex::new(session),
        current_conversation: Mutex::new(None),
        idmap: Mutex::new(IdMap::new()),
        cwd,
    });

    let cmd_for_ops = cmd_tx.clone();
    tokio::spawn(async move {
        while let Some(op) = ops_rx.recv().await {
            let cmd = match op {
                Op::ListConversations {} => Outbound::ListConversations,
                Op::ListFiles {} => Outbound::ListFiles,
                other => Outbound::Op(other),
            };
            if cmd_for_ops.send(cmd).await.is_err() {
                break;
            }
        }
    });

    let pump = tokio::spawn(async move {
        let mut conn = Some(conn);
        loop {
            let this_conn = match conn.take() {
                Some(c) => c,
                None => match reconnect(&link, &shared, &events_tx).await {
                    Some(c) => c,
                    None => break,
                },
            };
            let alive =
                run_connection(this_conn, &shared, &mut cmd_rx, &events_tx, &presence_tx).await;
            if !alive {
                break;
            }
        }
    });

    Attachment {
        ops: ops_tx,
        events: events_rx,
        presence: presence_rx,
        client_id,
        cwd: opened_cwd,
        daemon,
        session: session.0,
        pump,
    }
}

async fn reconnect(
    link: &Arc<Link>,
    shared: &Arc<Shared>,
    events_tx: &mpsc::Sender<Event>,
) -> Option<Conn> {
    for _ in 0..100 {
        let Ok((mut conn, identity, _)) = open(link).await else {
            tokio::time::sleep(Duration::from_millis(200)).await;
            continue;
        };
        if identity.wire != mine().wire {
            report_incompatible(events_tx).await;
            return None;
        }
        let resume = match *shared.current_conversation.lock().await {
            Some(conversation_id) => ResumeMode::Conversation { conversation_id },
            None => ResumeMode::New {},
        };
        if conn
            .send(ClientFrame::OpenSession {
                cwd: shared.cwd.clone(),
                resume,
            })
            .await
            .is_err()
        {
            continue;
        }
        if let Ok(ServerFrame::SessionOpened { session, .. }) = request(&mut conn).await {
            *shared.current.lock().await = session;
            shared.idmap.lock().await.reset();
            return Some(conn);
        }
    }
    None
}

async fn report_incompatible(events_tx: &mpsc::Sender<Event>) {
    let _ = events_tx
        .send(Event::Error {
            id: None,
            message: "the daemon protocol changed while reconnecting".to_owned(),
            hint: Some("restart the daemon with this goat build".to_owned()),
        })
        .await;
}

async fn run_connection(
    conn: Conn,
    shared: &Arc<Shared>,
    cmd_rx: &mut mpsc::Receiver<Outbound>,
    events_tx: &mpsc::Sender<Event>,
    presence_tx: &mpsc::Sender<usize>,
) -> bool {
    use futures::{SinkExt, StreamExt};
    let (mut sink, mut source) = conn.split();
    let mut expected_seq: Option<u64> = None;
    let mut replaying = false;

    loop {
        tokio::select! {
            biased;
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { return false };
                let frame = match cmd {
                    Outbound::ListConversations => ClientFrame::ListConversations {
                        cwd: shared.cwd.clone(),
                    },
                    Outbound::ListFiles => ClientFrame::ListDirectory {
                        path: shared.cwd.clone(),
                        recursive: true,
                    },
                    Outbound::Op(op) => {
                        let session = *shared.current.lock().await;
                        match op {
                            Op::Shutdown {} => {
                                let _ = sink.send(ClientFrame::Goodbye {}).await;
                                return false;
                            }
                            Op::Interrupt { .. }
                            | Op::Answer { .. }
                            | Op::DequeueMessage { .. }
                            | Op::ProcessKill { .. }
                            | Op::ProcessWatch { .. } => {
                                let mut op = op;
                                shared.idmap.lock().await.translate_outbound(&mut op);
                                ClientFrame::Control { session, op }
                            }
                            other => {
                                let correlation = submit_correlation(&other);
                                ClientFrame::Submit {
                                    session,
                                    correlation,
                                    op: other,
                                }
                            }
                        }
                    }
                };
                if sink.send(frame).await.is_err() {
                    return true;
                }
            }
            item = source.next() => {
                let Some(item) = item else { return true };
                let Ok(frame) = item else { return true };
                match &frame {
                    ServerFrame::SessionOpened { session: new, .. } => {
                        *shared.current.lock().await = *new;
                        *shared.current_conversation.lock().await = None;
                        continue;
                    }
                    ServerFrame::Detached { .. } => {
                        shared.idmap.lock().await.reset();
                        expected_seq = None;
                        continue;
                    }
                    ServerFrame::CorrelationAssigned { correlation, task, .. } => {
                        shared.idmap.lock().await.record_correlation(*correlation, *task);
                        continue;
                    }
                    ServerFrame::Presence { clients, .. } => {
                        let _ = presence_tx.try_send(clients.len());
                        continue;
                    }
                    ServerFrame::Snapshot { .. } => {
                        let _ = sink
                            .send(ClientFrame::ListDirectory {
                                path: shared.cwd.clone(),
                                recursive: true,
                            })
                            .await;
                    }
                    _ => {}
                }
                match sequenced_delivery(&mut expected_seq, &mut replaying, &frame) {
                    Delivery::Reconnect => return true,
                    Delivery::Skip => continue,
                    Delivery::Forward => {}
                }
                if let ServerFrame::Event {
                    event: Event::ConversationBound { conversation_id },
                    ..
                } = &frame
                {
                    *shared.current_conversation.lock().await = Some(*conversation_id);
                }
                for mut event in frame_to_events(frame) {
                    shared.idmap.lock().await.translate_inbound(&mut event);
                    if events_tx.send(event).await.is_err() {
                        return false;
                    }
                }
            }
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
enum Delivery {
    Forward,
    Skip,
    Reconnect,
}

fn sequenced_delivery(
    expected_seq: &mut Option<u64>,
    replaying: &mut bool,
    frame: &ServerFrame,
) -> Delivery {
    match frame {
        ServerFrame::Snapshot { watermark, .. } => {
            *expected_seq = Some(*watermark);
            *replaying = false;
            Delivery::Forward
        }
        ServerFrame::Event { seq, .. } if *replaying => match *expected_seq {
            Some(exp) if *seq < exp => Delivery::Skip,
            Some(exp) if *seq == exp => {
                *expected_seq = Some(*seq + 1);
                *replaying = false;
                Delivery::Forward
            }
            Some(_) | None => Delivery::Skip,
        },
        ServerFrame::Event { seq, .. } => match *expected_seq {
            Some(exp) if *seq < exp => Delivery::Skip,
            Some(exp) if *seq > exp => {
                *replaying = true;
                Delivery::Reconnect
            }
            _ => {
                *expected_seq = Some(*seq + 1);
                Delivery::Forward
            }
        },
        _ => Delivery::Forward,
    }
}

fn submit_correlation(op: &Op) -> u64 {
    match op {
        Op::SubmitMessage { id, .. } | Op::SubmitShell { id, .. } | Op::Compact { id, .. } => id.0,
        _ => 0,
    }
}

fn frame_to_events(frame: ServerFrame) -> Vec<Event> {
    match frame {
        ServerFrame::Event { event, .. } => vec![event],
        ServerFrame::Snapshot {
            target,
            transcript,
            pending,
            context_tokens,
            compaction_threshold,
            skills,
            accounts,
            model_list,
            selected,
            rate_limits,
            mode,
            processes,
            usage,
            active,
            retry,
            ..
        } => {
            let mut events = Vec::new();
            if let Some(target) = *target {
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
            events.push(Event::ModelListChanged {
                entries: model_list,
            });
            if let Some(target) = *selected {
                events.push(Event::ModelSelected { target });
            }
            events.push(Event::ModeChanged {
                mode: mode.mode,
                plan_path: mode.plan_path,
            });
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
            if let Some(retry) = *retry {
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
        ServerFrame::Conversations { conversations } => vec![Event::ConversationsListed {
            conversations: conversations
                .into_iter()
                .map(|t| goat_protocol::ConversationSummary {
                    id: t.conversation_id,
                    title: t.title.unwrap_or_default(),
                    model: t.model,
                    updated_at: t.updated_at,
                    live: t.live.is_some(),
                })
                .collect(),
        }],
        ServerFrame::Directory { children, .. } => vec![Event::FilesListed {
            entries: children
                .into_iter()
                .map(|entry| match entry.kind {
                    goat_wire::DirEntryKind::Directory {} => format!("{}/", entry.name),
                    goat_wire::DirEntryKind::File {} | goat_wire::DirEntryKind::Symlink {} => {
                        entry.name
                    }
                })
                .collect(),
        }],
        ServerFrame::Error { message } => vec![Event::Error {
            id: None,
            message,
            hint: None,
        }],
        _ => Vec::new(),
    }
}

pub async fn status(link: &Link) -> Result<Vec<goat_wire::SessionInfo>, ClientError> {
    match ask(link, ClientFrame::ListSessions {}).await? {
        ServerFrame::Sessions { sessions } => Ok(sessions),
        other => Err(refusal(other)),
    }
}

pub async fn list_conversations(
    link: &Link,
    cwd: &Path,
) -> Result<Vec<goat_wire::ConversationInfo>, ClientError> {
    let frame = ClientFrame::ListConversations {
        cwd: cwd.display().to_string(),
    };
    match ask(link, frame).await? {
        ServerFrame::Conversations { conversations } => Ok(conversations),
        other => Err(refusal(other)),
    }
}

pub async fn stop(link: &Link) -> Result<(), ClientError> {
    let (mut conn, _, _) = open(link).await?;
    conn.send(ClientFrame::StopDaemon {}).await?;
    tokio::time::timeout(STOP_TIMEOUT, async {
        loop {
            match conn.recv().await {
                Ok(ServerFrame::Error { message }) => return Err(ClientError::Refused(message)),
                Ok(_) => {}
                Err(WireError::Closed) => return Ok(()),
                Err(err) => return Err(ClientError::Wire(err)),
            }
        }
    })
    .await
    .map_err(|_| ClientError::Timeout(STOP_TIMEOUT))?
}

pub async fn reload(
    link: &Link,
    agent: Option<String>,
) -> Result<goat_wire::ReloadReport, ClientError> {
    match ask(link, ClientFrame::ReloadAgents { agent }).await? {
        ServerFrame::Reloaded { report } => Ok(report),
        other => Err(refusal(other)),
    }
}

pub async fn kill_session(link: &Link, session: u64) -> Result<(), ClientError> {
    let frame = ClientFrame::KillSession {
        session: SessionId(session),
    };
    tell(link, frame).await
}

pub struct PairingInfo {
    pub code: String,
    pub server_fingerprint: String,
    pub advertised: Vec<String>,
}

pub async fn pair_device(link: &Link, label: String) -> Result<PairingInfo, ClientError> {
    match ask(link, ClientFrame::PairDevice { label }).await? {
        ServerFrame::PairingCode {
            code,
            server_fingerprint,
            advertised,
        } => Ok(PairingInfo {
            code,
            server_fingerprint,
            advertised,
        }),
        other => Err(refusal(other)),
    }
}

pub async fn list_devices(link: &Link) -> Result<Vec<goat_wire::DeviceInfo>, ClientError> {
    match ask(link, ClientFrame::ListDevices {}).await? {
        ServerFrame::Devices { devices } => Ok(devices),
        other => Err(refusal(other)),
    }
}

pub async fn revoke_device(link: &Link, device: String) -> Result<bool, ClientError> {
    match ask(link, ClientFrame::RevokeDevice { device }).await? {
        ServerFrame::DeviceRevoked { ok } => Ok(ok),
        other => Err(refusal(other)),
    }
}

async fn tell(link: &Link, frame: ClientFrame) -> Result<(), ClientError> {
    let (mut conn, _, _) = open(link).await?;
    conn.send(frame).await?;
    Ok(())
}

async fn ask(link: &Link, frame: ClientFrame) -> Result<ServerFrame, ClientError> {
    let (mut conn, _, _) = open(link).await?;
    conn.send(frame).await?;
    request(&mut conn).await
}

fn refusal(frame: ServerFrame) -> ClientError {
    match frame {
        ServerFrame::Error { message } => ClientError::Refused(message),
        _ => ClientError::Handshake,
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, Delivery, Identity, decide, frame_to_events, sequenced_delivery};
    use goat_protocol::{Event, ModelTarget, SkillInfo, TaskId};
    use goat_wire::{BuildId, Busy, ServerFrame, SessionId};

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

    #[test]
    fn decision_table_preserves_busy_daemons() {
        let idle = Busy::default();
        let busy = Busy {
            sessions: 1,
            turns: 0,
        };
        let ours = peer("aaaa", 1, idle);
        for (wire, len, load, want) in [
            ("aaaa", 1, idle, Action::Attach),
            ("aaaa", 1, busy, Action::Attach),
            ("aaaa", 2, idle, Action::Replace),
            ("aaaa", 2, busy, Action::AttachStale),
            ("bbbb", 1, idle, Action::Replace),
            ("bbbb", 1, busy, Action::Refuse),
            ("bbbb", 2, busy, Action::Refuse),
        ] {
            assert_eq!(
                decide(&ours, &peer(wire, len, load)),
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
        assert_eq!(decide(&ours, &theirs), Action::Attach);
    }

    fn text(seq: u64) -> ServerFrame {
        ServerFrame::Event {
            session: SessionId(1),
            seq,
            event: Event::TextDelta {
                id: TaskId(1),
                chunk: "x".to_owned(),
            },
        }
    }

    #[test]
    fn gap_requests_one_reconnect_and_suppresses_until_snapshot() {
        let mut expected = Some(2);
        let mut replaying = false;
        assert_eq!(
            sequenced_delivery(&mut expected, &mut replaying, &text(4)),
            Delivery::Reconnect
        );
        assert!(replaying);
        assert_eq!(
            sequenced_delivery(&mut expected, &mut replaying, &text(5)),
            Delivery::Skip
        );
        assert_eq!(expected, Some(2));
        assert_eq!(
            sequenced_delivery(&mut expected, &mut replaying, &text(2)),
            Delivery::Forward
        );
        assert_eq!(expected, Some(3));
        assert!(!replaying);
    }

    #[test]
    fn snapshot_resets_replay_state() {
        let mut expected = Some(2);
        let mut replaying = true;
        let snapshot = ServerFrame::Snapshot {
            session: SessionId(1),
            watermark: 4,
            target: Box::new(None),
            transcript: Vec::new(),
            pending: Vec::new(),
            context_tokens: None,
            compaction_threshold: None,
            skills: Vec::new(),
            accounts: Vec::new(),
            model_list: Vec::new(),
            selected: Box::new(None),
            rate_limits: Vec::new(),
            mode: goat_wire::ModeEntry {
                mode: goat_protocol::Mode::Normal,
                plan_path: None,
            },
            processes: Vec::new(),
            usage: Vec::new(),
            active: None,
            retry: Box::new(None),
        };
        assert_eq!(
            sequenced_delivery(&mut expected, &mut replaying, &snapshot),
            Delivery::Forward
        );
        assert_eq!(expected, Some(4));
        assert!(!replaying);
        assert_eq!(
            sequenced_delivery(&mut expected, &mut replaying, &text(4)),
            Delivery::Forward
        );
        assert_eq!(expected, Some(5));
    }

    #[test]
    fn resumed_snapshot_expands_restore_first_then_state() {
        let snapshot = ServerFrame::Snapshot {
            session: SessionId(1),
            watermark: 4,
            target: Box::new(Some(ModelTarget {
                provider: "p".to_owned(),
                model: "m".to_owned(),
                account: "a".to_owned(),
                effort: None,
            })),
            transcript: Vec::new(),
            pending: Vec::new(),
            context_tokens: None,
            compaction_threshold: None,
            skills: vec![SkillInfo {
                name: "deploy".to_owned(),
                description: "ship".to_owned(),
                command: None,
            }],
            accounts: Vec::new(),
            model_list: Vec::new(),
            selected: Box::new(None),
            rate_limits: Vec::new(),
            mode: goat_wire::ModeEntry {
                mode: goat_protocol::Mode::Normal,
                plan_path: None,
            },
            processes: Vec::new(),
            usage: Vec::new(),
            active: None,
            retry: Box::new(None),
        };
        let events = frame_to_events(snapshot);
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
    fn new_session_snapshot_omits_restore_but_keeps_skills() {
        let snapshot = ServerFrame::Snapshot {
            session: SessionId(1),
            watermark: 2,
            target: Box::new(None),
            transcript: Vec::new(),
            pending: Vec::new(),
            context_tokens: None,
            compaction_threshold: None,
            skills: vec![SkillInfo {
                name: "deploy".to_owned(),
                description: "ship".to_owned(),
                command: None,
            }],
            accounts: Vec::new(),
            model_list: Vec::new(),
            selected: Box::new(None),
            rate_limits: Vec::new(),
            mode: goat_wire::ModeEntry {
                mode: goat_protocol::Mode::Normal,
                plan_path: None,
            },
            processes: Vec::new(),
            usage: Vec::new(),
            active: None,
            retry: Box::new(None),
        };
        let events = frame_to_events(snapshot);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::ConversationRestored { .. })),
            "a new session has no restore target"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::SkillsChanged { .. })),
            "skills still arrive for a new session"
        );
    }

    #[test]
    fn new_session_snapshot_watermark_keeps_seq_continuity() {
        let snapshot = ServerFrame::Snapshot {
            session: SessionId(1),
            watermark: 2,
            target: Box::new(None),
            transcript: Vec::new(),
            pending: Vec::new(),
            context_tokens: None,
            compaction_threshold: None,
            skills: Vec::new(),
            accounts: Vec::new(),
            model_list: Vec::new(),
            selected: Box::new(None),
            rate_limits: Vec::new(),
            mode: goat_wire::ModeEntry {
                mode: goat_protocol::Mode::Normal,
                plan_path: None,
            },
            processes: Vec::new(),
            usage: Vec::new(),
            active: None,
            retry: Box::new(None),
        };
        let mut expected = None;
        let mut replaying = false;
        assert_eq!(
            sequenced_delivery(&mut expected, &mut replaying, &snapshot),
            Delivery::Forward
        );
        assert_eq!(expected, Some(2));
        assert_eq!(
            sequenced_delivery(&mut expected, &mut replaying, &text(2)),
            Delivery::Forward
        );
        assert_eq!(expected, Some(3));
    }

    #[test]
    fn duplicate_event_is_skipped() {
        let mut expected = Some(4);
        let mut replaying = false;
        assert_eq!(
            sequenced_delivery(&mut expected, &mut replaying, &text(3)),
            Delivery::Skip
        );
        assert_eq!(expected, Some(4));
    }

    #[test]
    fn control_frames_forward_while_replaying() {
        let mut expected = Some(4);
        let mut replaying = true;
        assert_eq!(
            sequenced_delivery(
                &mut expected,
                &mut replaying,
                &ServerFrame::Error {
                    message: "err".to_owned(),
                },
            ),
            Delivery::Forward
        );
        assert_eq!(
            sequenced_delivery(
                &mut expected,
                &mut replaying,
                &ServerFrame::Conversations {
                    conversations: Vec::new()
                },
            ),
            Delivery::Forward
        );
    }
}
