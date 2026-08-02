use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use goat_protocol::{Event, Op};
use goat_wire::transport::{self, Stream};
use goat_wire::{
    BuildId, Busy, ClientConn, ClientFrame, ResumeMode, ServerFrame, SessionId, WireError,
};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;

use crate::idmap::IdMap;

mod idmap;

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
    #[error("the daemon did not answer within {0:?}")]
    Timeout(Duration),
    #[error("unexpected daemon response during handshake")]
    Handshake,
    #[error(
        "the running daemon speaks a different protocol and is busy ({sessions} session(s), {turns} agent turn(s))"
    )]
    BusyIncompatible { sessions: usize, turns: usize },
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
    pub pump: JoinHandle<()>,
}

const OPS_CAPACITY: usize = 32;
const EVENTS_CAPACITY: usize = 512;
const PRESENCE_CAPACITY: usize = 16;

async fn open(socket_path: &Path) -> Result<(ClientConn<Stream>, Identity, u64), ClientError> {
    let stream = transport::connect(socket_path).await?;
    let mut conn: ClientConn<Stream> = ClientConn::new(stream);
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
    let identity = Identity {
        wire,
        build,
        version,
        pid,
        started_at,
        ready,
        busy,
    };
    Ok((conn, identity, client_id.0))
}

async fn request(conn: &mut ClientConn<Stream>) -> Result<ServerFrame, ClientError> {
    tokio::time::timeout(REQUEST_TIMEOUT, conn.recv())
        .await
        .map_err(|_| ClientError::Timeout(REQUEST_TIMEOUT))?
        .map_err(ClientError::Wire)
}

pub async fn greet(socket_path: &Path) -> Daemon {
    match open(socket_path).await {
        Ok((_, identity, _)) => Daemon::Reachable(identity),
        Err(ClientError::Timeout(_) | ClientError::Handshake) => Daemon::Silent,
        Err(_) => Daemon::Absent,
    }
}

pub async fn ensure(
    socket_path: &Path,
    daemon_exe: &Path,
) -> Result<(ClientConn<Stream>, Identity, u64, Attached), ClientError> {
    let ours = mine();
    let opened = match open(socket_path).await {
        Ok(opened) => Some(opened),
        Err(err @ (ClientError::Timeout(_) | ClientError::Handshake)) => return Err(err),
        Err(_) => None,
    };

    let Some((conn, identity, client_id)) = opened else {
        let (conn, identity, client_id) = spawn_and_wait(socket_path, daemon_exe).await?;
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
            let (conn, identity, client_id) = spawn_and_wait(socket_path, daemon_exe).await?;
            Ok((
                conn,
                identity,
                client_id,
                Attached::Replaced(Box::new(replaced)),
            ))
        }
    }
}

async fn shutdown_and_wait(mut conn: ClientConn<Stream>) {
    if conn.send(&ClientFrame::StopDaemon {}).await.is_err() {
        return;
    }
    let _ = tokio::time::timeout(STOP_TIMEOUT, async { while conn.recv().await.is_ok() {} }).await;
}

async fn spawn_and_wait(
    socket_path: &Path,
    daemon_exe: &Path,
) -> Result<(ClientConn<Stream>, Identity, u64), ClientError> {
    spawn_daemon(daemon_exe)?;
    let deadline = tokio::time::Instant::now() + SPAWN_BUDGET;
    loop {
        match open(socket_path).await {
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

pub async fn connect(
    socket_path: &Path,
    daemon_exe: &Path,
    cwd: PathBuf,
    resume: ResumeMode,
) -> Result<(Attachment, Attached), ClientError> {
    let (mut conn, _identity, client_id, attached) = ensure(socket_path, daemon_exe).await?;

    conn.send(&ClientFrame::OpenSession {
        cwd: cwd.display().to_string(),
        resume,
    })
    .await?;
    let session = match tokio::time::timeout(REQUEST_TIMEOUT, conn.recv())
        .await
        .map_err(|_| ClientError::Timeout(REQUEST_TIMEOUT))??
    {
        ServerFrame::SessionOpened { session, .. } => session,
        ServerFrame::Error { message } => return Err(ClientError::OpenFailed(message)),
        _ => return Err(ClientError::Handshake),
    };

    Ok((
        spawn_pumps(conn, session, client_id, &cwd, socket_path.to_path_buf()),
        attached,
    ))
}

enum Outbound {
    Op(Op),
    ListThreads,
}

struct Shared {
    current: Mutex<SessionId>,
    current_thread: Mutex<Option<i64>>,
    idmap: Mutex<IdMap>,
    cwd: String,
}

fn spawn_pumps(
    conn: ClientConn<Stream>,
    session: SessionId,
    client_id: u64,
    cwd: &Path,
    socket_path: PathBuf,
) -> Attachment {
    let (ops_tx, mut ops_rx) = mpsc::channel::<Op>(OPS_CAPACITY);
    let (events_tx, events_rx) = mpsc::channel::<Event>(EVENTS_CAPACITY);
    let (presence_tx, presence_rx) = mpsc::channel::<usize>(PRESENCE_CAPACITY);
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<Outbound>(OPS_CAPACITY + 8);

    let shared = Arc::new(Shared {
        current: Mutex::new(session),
        current_thread: Mutex::new(None),
        idmap: Mutex::new(IdMap::new()),
        cwd: cwd.display().to_string(),
    });

    let cmd_for_ops = cmd_tx.clone();
    tokio::spawn(async move {
        while let Some(op) = ops_rx.recv().await {
            let cmd = match op {
                Op::ListThreads {} => Outbound::ListThreads,
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
                None => match reconnect(&socket_path, &shared).await {
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
        pump,
    }
}

const RECONNECT_BUDGET: Duration = Duration::from_secs(20);
const RECONNECT_POLL: Duration = Duration::from_millis(200);

async fn reconnect(socket_path: &Path, shared: &Arc<Shared>) -> Option<ClientConn<Stream>> {
    let ours = mine();
    let deadline = tokio::time::Instant::now() + RECONNECT_BUDGET;
    while tokio::time::Instant::now() < deadline {
        match reconnect_once(socket_path, shared, &ours).await {
            Ok(conn) => return Some(conn),
            Err(true) => return None,
            Err(false) => tokio::time::sleep(RECONNECT_POLL).await,
        }
    }
    None
}

async fn reconnect_once(
    socket_path: &Path,
    shared: &Arc<Shared>,
    ours: &Identity,
) -> Result<ClientConn<Stream>, bool> {
    let Ok((mut conn, identity, _)) = open(socket_path).await else {
        return Err(false);
    };
    if identity.wire != ours.wire {
        return Err(true);
    }
    let resume = match *shared.current_thread.lock().await {
        Some(thread_id) => ResumeMode::Thread { thread_id },
        None => ResumeMode::New {},
    };
    if conn
        .send(&ClientFrame::OpenSession {
            cwd: shared.cwd.clone(),
            resume,
        })
        .await
        .is_err()
    {
        return Err(false);
    }
    match tokio::time::timeout(REQUEST_TIMEOUT, conn.recv()).await {
        Ok(Ok(ServerFrame::SessionOpened { session })) => {
            *shared.current.lock().await = session;
            shared.idmap.lock().await.reset();
            Ok(conn)
        }
        _ => Err(false),
    }
}

async fn run_connection(
    conn: ClientConn<Stream>,
    shared: &Arc<Shared>,
    cmd_rx: &mut mpsc::Receiver<Outbound>,
    events_tx: &mpsc::Sender<Event>,
    presence_tx: &mpsc::Sender<usize>,
) -> bool {
    use futures::{SinkExt, StreamExt};
    let (sink, mut source) = conn.split();
    let mut sink = Box::pin(sink);
    let mut expected_seq: Option<u64> = None;
    let mut replaying = false;

    loop {
        tokio::select! {
            biased;
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { return false };
                let frame = match cmd {
                    Outbound::ListThreads => ClientFrame::ListThreads {
                        cwd: shared.cwd.clone(),
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
                    ServerFrame::SessionOpened { session: new } => {
                        *shared.current.lock().await = *new;
                        *shared.current_thread.lock().await = None;
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
                    _ => {}
                }
                match sequenced_delivery(&mut expected_seq, &mut replaying, &frame) {
                    Delivery::RequestResync => {
                        let session = *shared.current.lock().await;
                        if sink.send(ClientFrame::Attach { session }).await.is_err() {
                            return true;
                        }
                        continue;
                    }
                    Delivery::Skip => continue,
                    Delivery::Forward => {}
                }
                if let ServerFrame::Event {
                    event: Event::ThreadBound { thread_id },
                    ..
                } = &frame
                {
                    *shared.current_thread.lock().await = Some(*thread_id);
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
    RequestResync,
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
                Delivery::RequestResync
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
            context_tokens,
            compaction_threshold,
            skills,
            accounts,
            model_list,
            selected,
            rate_limits,
            ..
        } => {
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
            events.push(Event::ModelListChanged {
                entries: model_list,
            });
            if let Some(target) = selected {
                events.push(Event::ModelSelected { target });
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
        ServerFrame::Threads { threads } => vec![Event::ThreadsListed {
            threads: threads
                .into_iter()
                .map(|t| goat_protocol::ThreadSummary {
                    id: t.thread_id,
                    title: t.title.unwrap_or_default(),
                    model: t.model,
                    updated_at: t.updated_at,
                    live: t.live.is_some(),
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

pub async fn status(socket_path: &Path) -> Result<Vec<goat_wire::SessionInfo>, ClientError> {
    let (mut conn, _identity, _client_id) = open(socket_path).await?;
    conn.send(&ClientFrame::ListSessions {}).await?;
    match request(&mut conn).await? {
        ServerFrame::Sessions { sessions } => Ok(sessions),
        _ => Err(ClientError::Handshake),
    }
}

pub async fn list_threads(
    socket_path: &Path,
    cwd: &Path,
) -> Result<Vec<goat_wire::ThreadInfo>, ClientError> {
    let (mut conn, _identity, _client_id) = open(socket_path).await?;
    conn.send(&ClientFrame::ListThreads {
        cwd: cwd.display().to_string(),
    })
    .await?;
    match request(&mut conn).await? {
        ServerFrame::Threads { threads } => Ok(threads),
        _ => Err(ClientError::Handshake),
    }
}

pub async fn stop(socket_path: &Path) -> Result<Identity, ClientError> {
    let (mut conn, identity, _client_id) = open(socket_path).await?;
    conn.send(&ClientFrame::StopDaemon {}).await?;
    tokio::time::timeout(STOP_TIMEOUT, async { while conn.recv().await.is_ok() {} })
        .await
        .map_err(|_| ClientError::Timeout(STOP_TIMEOUT))?;
    Ok(identity)
}

pub async fn reload(
    socket_path: &Path,
    agent: Option<String>,
) -> Result<goat_wire::ReloadReport, ClientError> {
    let (mut conn, _identity, _client_id) = open(socket_path).await?;
    conn.send(&ClientFrame::ReloadAgents { agent }).await?;
    match request(&mut conn).await? {
        ServerFrame::Reloaded { report } => Ok(report),
        ServerFrame::Error { message } => Err(ClientError::Refused(message)),
        _ => Err(ClientError::Handshake),
    }
}

pub async fn kill_session(socket_path: &Path, session: u64) -> Result<(), ClientError> {
    let (mut conn, _identity, _client_id) = open(socket_path).await?;
    conn.send(&ClientFrame::KillSession {
        session: SessionId(session),
    })
    .await?;
    Ok(())
}

pub struct PairingInfo {
    pub code: String,
    pub server_fingerprint: String,
    pub advertised: Vec<String>,
}

pub async fn pair_device(socket_path: &Path, label: String) -> Result<PairingInfo, ClientError> {
    let (mut conn, _identity, _client_id) = open(socket_path).await?;
    conn.send(&ClientFrame::PairDevice { label }).await?;
    match request(&mut conn).await? {
        ServerFrame::PairingCode {
            code,
            server_fingerprint,
            advertised,
        } => Ok(PairingInfo {
            code,
            server_fingerprint,
            advertised,
        }),
        ServerFrame::Error { message } => Err(ClientError::OpenFailed(message)),
        _ => Err(ClientError::Handshake),
    }
}

pub async fn list_devices(socket_path: &Path) -> Result<Vec<goat_wire::DeviceInfo>, ClientError> {
    let (mut conn, _identity, _client_id) = open(socket_path).await?;
    conn.send(&ClientFrame::ListDevices {}).await?;
    match request(&mut conn).await? {
        ServerFrame::Devices { devices } => Ok(devices),
        ServerFrame::Error { message } => Err(ClientError::OpenFailed(message)),
        _ => Err(ClientError::Handshake),
    }
}

pub async fn revoke_device(socket_path: &Path, device: String) -> Result<bool, ClientError> {
    let (mut conn, _identity, _client_id) = open(socket_path).await?;
    conn.send(&ClientFrame::RevokeDevice { device }).await?;
    match request(&mut conn).await? {
        ServerFrame::DeviceRevoked { ok } => Ok(ok),
        ServerFrame::Error { message } => Err(ClientError::OpenFailed(message)),
        _ => Err(ClientError::Handshake),
    }
}

fn spawn_daemon(daemon_exe: &Path) -> Result<(), ClientError> {
    use std::process::{Command, Stdio};
    Command::new(daemon_exe)
        .arg("daemon")
        .arg("serve")
        .arg("--detached")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| ClientError::SpawnFailed(e.to_string()))?;
    Ok(())
}

pub async fn start(socket_path: &Path, daemon_exe: &Path) -> Result<Attached, ClientError> {
    let (_conn, _identity, _client_id, attached) = ensure(socket_path, daemon_exe).await?;
    Ok(attached)
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
    fn decision_table() {
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
    fn an_unknown_build_is_never_stale() {
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
    fn gap_requests_resync_and_suppresses_until_snapshot() {
        let mut expected = Some(2);
        let mut replaying = false;
        assert_eq!(
            sequenced_delivery(&mut expected, &mut replaying, &text(4)),
            Delivery::RequestResync
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
            target: None,
            transcript: Vec::new(),
            context_tokens: None,
            compaction_threshold: None,
            skills: Vec::new(),
            accounts: Vec::new(),
            model_list: Vec::new(),
            selected: None,
            rate_limits: Vec::new(),
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
            target: Some(ModelTarget {
                provider: "p".to_owned(),
                model: "m".to_owned(),
                account: "a".to_owned(),
                effort: None,
            }),
            transcript: Vec::new(),
            context_tokens: None,
            compaction_threshold: None,
            skills: vec![SkillInfo {
                name: "deploy".to_owned(),
                description: "ship".to_owned(),
                command: None,
            }],
            accounts: Vec::new(),
            model_list: Vec::new(),
            selected: None,
            rate_limits: Vec::new(),
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
            target: None,
            transcript: Vec::new(),
            context_tokens: None,
            compaction_threshold: None,
            skills: vec![SkillInfo {
                name: "deploy".to_owned(),
                description: "ship".to_owned(),
                command: None,
            }],
            accounts: Vec::new(),
            model_list: Vec::new(),
            selected: None,
            rate_limits: Vec::new(),
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
            target: None,
            transcript: Vec::new(),
            context_tokens: None,
            compaction_threshold: None,
            skills: Vec::new(),
            accounts: Vec::new(),
            model_list: Vec::new(),
            selected: None,
            rate_limits: Vec::new(),
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
                &ServerFrame::Threads {
                    threads: Vec::new()
                },
            ),
            Delivery::Forward
        );
    }
}
