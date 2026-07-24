use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use goat_protocol::{Event, Op};
use goat_wire::transport::{self, Stream};
use goat_wire::{
    ClientConn, ClientFrame, PROTOCOL_VERSION, ResumeMode, ServerFrame, SessionId, WireError,
};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;

use crate::idmap::IdMap;

mod idmap;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("wire error: {0}")]
    Wire(#[from] WireError),
    #[error("daemon protocol version {0} does not match client {PROTOCOL_VERSION}")]
    VersionMismatch(u32),
    #[error("unexpected daemon response during handshake")]
    Handshake,
    #[error("daemon did not open a session: {0}")]
    OpenFailed(String),
    #[error("could not start daemon: {0}")]
    SpawnFailed(String),
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

pub async fn connect(
    socket_path: &Path,
    daemon_exe: &Path,
    cwd: PathBuf,
    resume: ResumeMode,
) -> Result<Attachment, ClientError> {
    let stream = connect_or_spawn(socket_path, daemon_exe).await?;
    let mut conn: ClientConn<Stream> = ClientConn::new(stream);

    conn.send(&ClientFrame::Hello {
        version: PROTOCOL_VERSION,
    })
    .await?;
    let client_id = match conn.recv().await? {
        ServerFrame::Welcome { version, client_id } => {
            if version != PROTOCOL_VERSION {
                return Err(ClientError::VersionMismatch(version));
            }
            client_id.0
        }
        ServerFrame::VersionMismatch { daemon_version } => {
            return Err(ClientError::VersionMismatch(daemon_version));
        }
        _ => return Err(ClientError::Handshake),
    };

    conn.send(&ClientFrame::OpenSession {
        cwd: cwd.display().to_string(),
        resume,
    })
    .await?;
    let session = match conn.recv().await? {
        ServerFrame::SessionOpened { session, .. } => session,
        ServerFrame::Error { message } => return Err(ClientError::OpenFailed(message)),
        _ => return Err(ClientError::Handshake),
    };

    Ok(spawn_pumps(
        conn,
        session,
        client_id,
        &cwd,
        socket_path.to_path_buf(),
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

async fn reconnect(socket_path: &Path, shared: &Arc<Shared>) -> Option<ClientConn<Stream>> {
    for _ in 0..100 {
        let Ok(stream) = transport::connect(socket_path).await else {
            tokio::time::sleep(Duration::from_millis(200)).await;
            continue;
        };
        let mut conn: ClientConn<Stream> = ClientConn::new(stream);
        if conn
            .send(&ClientFrame::Hello {
                version: PROTOCOL_VERSION,
            })
            .await
            .is_err()
        {
            continue;
        }
        match conn.recv().await {
            Ok(ServerFrame::Welcome { version, .. }) if version == PROTOCOL_VERSION => {}
            _ => continue,
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
            continue;
        }
        if let Ok(ServerFrame::SessionOpened { session }) = conn.recv().await {
            *shared.current.lock().await = session;
            shared.idmap.lock().await.reset();
            return Some(conn);
        }
    }
    None
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
    let stream = transport::connect(socket_path).await?;
    let mut conn: ClientConn<Stream> = ClientConn::new(stream);
    conn.send(&ClientFrame::Hello {
        version: PROTOCOL_VERSION,
    })
    .await?;
    expect_welcome(&mut conn).await?;
    conn.send(&ClientFrame::ListSessions {}).await?;
    match conn.recv().await? {
        ServerFrame::Sessions { sessions } => Ok(sessions),
        _ => Err(ClientError::Handshake),
    }
}

pub async fn list_threads(
    socket_path: &Path,
    cwd: &Path,
) -> Result<Vec<goat_wire::ThreadInfo>, ClientError> {
    let stream = transport::connect(socket_path).await?;
    let mut conn: ClientConn<Stream> = ClientConn::new(stream);
    conn.send(&ClientFrame::Hello {
        version: PROTOCOL_VERSION,
    })
    .await?;
    expect_welcome(&mut conn).await?;
    conn.send(&ClientFrame::ListThreads {
        cwd: cwd.display().to_string(),
    })
    .await?;
    match conn.recv().await? {
        ServerFrame::Threads { threads } => Ok(threads),
        _ => Err(ClientError::Handshake),
    }
}

pub async fn stop(socket_path: &Path) -> Result<(), ClientError> {
    let stream = transport::connect(socket_path).await?;
    let mut conn: ClientConn<Stream> = ClientConn::new(stream);
    conn.send(&ClientFrame::Hello {
        version: PROTOCOL_VERSION,
    })
    .await?;
    expect_welcome(&mut conn).await?;
    conn.send(&ClientFrame::StopDaemon {}).await?;
    Ok(())
}

pub async fn kill_session(socket_path: &Path, session: u64) -> Result<(), ClientError> {
    let stream = transport::connect(socket_path).await?;
    let mut conn: ClientConn<Stream> = ClientConn::new(stream);
    conn.send(&ClientFrame::Hello {
        version: PROTOCOL_VERSION,
    })
    .await?;
    expect_welcome(&mut conn).await?;
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
    let stream = transport::connect(socket_path).await?;
    let mut conn: ClientConn<Stream> = ClientConn::new(stream);
    conn.send(&ClientFrame::Hello {
        version: PROTOCOL_VERSION,
    })
    .await?;
    expect_welcome(&mut conn).await?;
    conn.send(&ClientFrame::PairDevice { label }).await?;
    match conn.recv().await? {
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
    let stream = transport::connect(socket_path).await?;
    let mut conn: ClientConn<Stream> = ClientConn::new(stream);
    conn.send(&ClientFrame::Hello {
        version: PROTOCOL_VERSION,
    })
    .await?;
    expect_welcome(&mut conn).await?;
    conn.send(&ClientFrame::ListDevices {}).await?;
    match conn.recv().await? {
        ServerFrame::Devices { devices } => Ok(devices),
        ServerFrame::Error { message } => Err(ClientError::OpenFailed(message)),
        _ => Err(ClientError::Handshake),
    }
}

pub async fn revoke_device(socket_path: &Path, device: String) -> Result<bool, ClientError> {
    let stream = transport::connect(socket_path).await?;
    let mut conn: ClientConn<Stream> = ClientConn::new(stream);
    conn.send(&ClientFrame::Hello {
        version: PROTOCOL_VERSION,
    })
    .await?;
    expect_welcome(&mut conn).await?;
    conn.send(&ClientFrame::RevokeDevice { device }).await?;
    match conn.recv().await? {
        ServerFrame::DeviceRevoked { ok } => Ok(ok),
        ServerFrame::Error { message } => Err(ClientError::OpenFailed(message)),
        _ => Err(ClientError::Handshake),
    }
}

async fn expect_welcome(conn: &mut ClientConn<Stream>) -> Result<(), ClientError> {
    match conn.recv().await? {
        ServerFrame::Welcome { version, .. } => {
            if version != PROTOCOL_VERSION {
                return Err(ClientError::VersionMismatch(version));
            }
            Ok(())
        }
        ServerFrame::VersionMismatch { daemon_version } => {
            Err(ClientError::VersionMismatch(daemon_version))
        }
        _ => Err(ClientError::Handshake),
    }
}

async fn connect_or_spawn(socket_path: &Path, daemon_exe: &Path) -> Result<Stream, ClientError> {
    if let Ok(stream) = transport::connect(socket_path).await {
        return Ok(stream);
    }
    spawn_daemon(daemon_exe, socket_path)?;
    for _ in 0..50 {
        if let Ok(stream) = transport::connect(socket_path).await {
            return Ok(stream);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(ClientError::SpawnFailed(
        "daemon did not become reachable".to_owned(),
    ))
}

fn spawn_daemon(daemon_exe: &Path, socket_path: &Path) -> Result<(), ClientError> {
    use std::process::{Command, Stdio};
    let stderr = daemon_stderr(socket_path);
    Command::new(daemon_exe)
        .arg("daemon")
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr)
        .spawn()
        .map_err(|e| ClientError::SpawnFailed(e.to_string()))?;
    Ok(())
}

fn daemon_stderr(socket_path: &Path) -> std::process::Stdio {
    use std::process::Stdio;
    let Some(home) = socket_path.parent() else {
        return Stdio::null();
    };
    let log_dir = home.join("logs");
    if std::fs::create_dir_all(&log_dir).is_err() {
        return Stdio::null();
    }
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("daemon-stderr.log"))
    {
        Ok(file) => Stdio::from(file),
        Err(_) => Stdio::null(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Delivery, frame_to_events, sequenced_delivery};
    use goat_protocol::{Event, ModelTarget, SkillInfo, TaskId};
    use goat_wire::{ServerFrame, SessionId};

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
