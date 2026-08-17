use std::sync::Arc;

use goat_api::{
    AdminAgentReload, AdminAgentReloadOutput, AdminAgentReloadParams, AdminDaemonStop,
    AdminDaemonStopOutput, AdminDaemonStopParams, AdminDeviceList, AdminDeviceListOutput,
    AdminDevicePair, AdminDevicePairOutput, AdminDevicePairParams, AdminDeviceRevoke,
    AdminDeviceRevokeOutput, AdminDeviceRevokeParams, AgentWatch, AgentWatchParams, AnswerOutcome,
    AskAnswer, AskAnswerOutput, AskAnswerParams, ConversationInfo, ConversationList,
    ConversationListOutput, ConversationListParams, DaemonStatus, DaemonStatus2, DeviceInfo,
    DirEntry, DirEntryKind, Empty, FsList, FsListOutput, FsListParams, FsRead, FsReadParams,
    FsWrite, FsWriteOutput, FsWriteParams, GitDiff, GitDiffParams, Grant, PtyOpen, PtyOpenParams,
    PtyResize, PtyResizeParams, PtyWrite, PtyWriteParams, ReloadFailure, Router, SessionControl,
    SessionControlParams, SessionId, SessionInfo, SessionKill, SessionKillParams, SessionList,
    SessionListOutput, SessionLiveState, SessionOpen, SessionOpenOutput, SessionOpenParams,
    SessionSubmit, SessionSubmitOutput, SessionSubmitParams, SessionWatch, SessionWatchParams,
};
use goat_capability::Broker;
use goat_store::Store as _;
use goat_wire::envelope::{CallError, ErrorCode, Execution};

use crate::manager::CodeSessionHub;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

pub const LOCAL_GRANTS: [Grant; 2] = [Grant::Any, Grant::Admin];
pub const REMOTE_GRANTS: [Grant; 1] = [Grant::Any];

const WATCH_QUEUE: usize = 1024;
const ACTIVITY_PAGE: i64 = 256;
const ACTIVITY_POLL: std::time::Duration = std::time::Duration::from_millis(500);

fn parse_agents(slugs: &[String]) -> Vec<goat_types::AgentId> {
    slugs
        .iter()
        .map(|slug| goat_types::AgentId::from_slug(slug))
        .collect()
}

async fn activity_start(
    store: &goat_store::SqliteStore,
    epoch: &str,
    from: &goat_api::WatchFrom,
) -> Result<i64, CallError> {
    match from {
        goat_api::WatchFrom::Snapshot {} => Ok(0),
        goat_api::WatchFrom::Cursor { cursor } => {
            if cursor.epoch == epoch {
                Ok(i64::try_from(cursor.seq).unwrap_or(0))
            } else {
                store.activity_watermark().await.map_err(|err| {
                    CallError::new(ErrorCode::Internal, err.to_string())
                        .with_execution(Execution::KnownFailed)
                })
            }
        }
    }
}

fn activity_item(record: &goat_store::ActivityRecord, epoch: &str) -> goat_api::AgentActivity {
    let cursor = goat_api::cursor_for(epoch, u64::try_from(record.id).unwrap_or(0));
    let agent = record.agent.to_string();
    match record.kind {
        goat_store::ActivityKind::TurnStarted => goat_api::AgentActivity::TurnStarted {
            cursor,
            agent,
            run: record.run_id,
            trigger: record.detail.clone().unwrap_or_default(),
        },
        goat_store::ActivityKind::TurnFinished => goat_api::AgentActivity::TurnFinished {
            cursor,
            agent,
            run: record.run_id,
            ok: record.ok.unwrap_or(false),
        },
        goat_store::ActivityKind::ToolStarted => goat_api::AgentActivity::ToolStarted {
            cursor,
            agent,
            run: record.run_id,
            tool: record.detail.clone().unwrap_or_default(),
        },
        goat_store::ActivityKind::ScheduleFired => goat_api::AgentActivity::ScheduleFired {
            cursor,
            agent,
            run: record.run_id,
            schedule: record
                .detail
                .as_deref()
                .and_then(|d| d.parse().ok())
                .unwrap_or(0),
        },
    }
}

fn encode_activity(item: &goat_api::AgentActivity) -> Result<serde_json::Value, CallError> {
    serde_json::to_value(item).map_err(|err| {
        CallError::new(
            ErrorCode::Internal,
            format!("activity item could not be encoded: {err}"),
        )
        .with_execution(Execution::KnownFailed)
    })
}

fn refused(message: String) -> CallError {
    CallError::new(ErrorCode::Denied, message).with_execution(Execution::NotStarted)
}

fn encode_pty(item: &goat_api::PtyItem) -> Result<serde_json::Value, CallError> {
    serde_json::to_value(item).map_err(|err| {
        CallError::new(
            ErrorCode::Internal,
            format!("terminal item could not be encoded: {err}"),
        )
        .with_execution(Execution::KnownFailed)
    })
}

fn encode_item(item: &goat_api::WatchItem) -> Result<serde_json::Value, CallError> {
    serde_json::to_value(item).map_err(|err| {
        CallError::new(
            ErrorCode::Internal,
            format!("watch item could not be encoded: {err}"),
        )
        .with_execution(Execution::KnownFailed)
    })
}

fn resume_mode(mode: goat_api::ResumeMode) -> goat_wire::ResumeMode {
    match mode {
        goat_api::ResumeMode::New {} => goat_wire::ResumeMode::New {},
        goat_api::ResumeMode::Latest {} => goat_wire::ResumeMode::Latest {},
        goat_api::ResumeMode::Conversation { conversation_id } => {
            goat_wire::ResumeMode::Conversation { conversation_id }
        }
    }
}

#[must_use]
pub struct DaemonApi {
    pub manager: CodeSessionHub,
    pub broker: Arc<Broker>,
    pub device: String,
    pub epoch: String,
    pub shutdown: CancellationToken,
    pub terminals: Arc<crate::pty::Terminals>,
    pub db_path: std::path::PathBuf,
}

pub fn build(api: DaemonApi, grants: &[Grant]) -> Router {
    let DaemonApi {
        manager,
        broker,
        device,
        epoch,
        shutdown,
        terminals,
        db_path,
    } = api;
    let epoch = epoch.as_str();
    let status_manager = manager.clone();
    let status_epoch = epoch.to_owned();
    let sessions_manager = manager.clone();
    let conversations_manager = manager.clone();
    let open_manager = manager.clone();
    let open_epoch = epoch.to_owned();
    let control_manager = manager.clone();
    let kill_manager = manager.clone();
    let stop_manager = manager.clone();
    let submit_manager = manager.clone();
    let ask_manager = manager.clone();
    let activity_db = db_path.clone();
    let activity_epoch = epoch.to_owned();
    let pty_terminals = terminals.clone();
    let write_terminals = terminals.clone();
    let resize_terminals = terminals;
    let watch_manager = manager.clone();
    let watch_epoch = epoch.to_owned();
    let reload_manager = manager.clone();
    let pair_manager = manager.clone();
    let devices_manager = manager.clone();
    let revoke_manager = manager;

    let router =
        Router::new(grants.iter().copied())
            .unary::<DaemonStatus, _, _>(move |_params, _ctx| {
                let manager = status_manager.clone();
                let epoch = status_epoch.clone();
                async move {
                    let busy = manager.busy().await;
                    Ok(DaemonStatus2 {
                        version: env!("CARGO_PKG_VERSION").to_owned(),
                        pid: std::process::id(),
                        started_at: manager.started_at(),
                        ready: manager.is_ready(),
                        epoch,
                        sessions: busy.sessions,
                        turns: busy.turns,
                    })
                }
            })
            .unary::<SessionList, _, _>(move |_params, _ctx| {
                let manager = sessions_manager.clone();
                async move {
                    Ok(SessionListOutput {
                        sessions: manager
                            .list_sessions()
                            .await
                            .into_iter()
                            .map(session_info)
                            .collect(),
                    })
                }
            })
            .unary::<ConversationList, _, _>(move |params: ConversationListParams, _ctx| {
                let manager = conversations_manager.clone();
                async move {
                    Ok(ConversationListOutput {
                        conversations: manager
                            .list_conversations(&params.cwd)
                            .await
                            .into_iter()
                            .map(conversation_info)
                            .collect(),
                    })
                }
            })
            .unary::<SessionOpen, _, _>(move |params: SessionOpenParams, _ctx| {
                let manager = open_manager.clone();
                let epoch = open_epoch.clone();
                async move {
                    let (session, cwd) = manager
                        .open_or_attach(PathBuf::from(&params.cwd), resume_mode(params.resume))
                        .await
                        .map_err(refused)?;
                    Ok(SessionOpenOutput {
                        session: SessionId(session.0),
                        cwd,
                        epoch,
                    })
                }
            })
            .unary::<SessionSubmit, _, _>(move |params: SessionSubmitParams, _ctx| {
                let manager = submit_manager.clone();
                async move {
                    let task = manager
                        .submit_task(goat_wire::SessionId(params.session.0), params.op)
                        .await
                        .map_err(refused)?;
                    Ok(SessionSubmitOutput { task })
                }
            })
            .unary::<AskAnswer, _, _>(move |params: AskAnswerParams, _ctx| {
                let manager = ask_manager.clone();
                async move {
                    let call =
                        goat_protocol::ToolCallId(u64::try_from(params.prompt).map_err(|_| {
                            CallError::new(ErrorCode::InvalidParams, "prompt id must be positive")
                                .with_execution(Execution::NotStarted)
                        })?);
                    let settled = manager
                        .settle_ask(
                            goat_wire::SessionId(params.session.0),
                            call,
                            params.revision,
                            params.answers,
                        )
                        .await
                        .map_err(refused)?;
                    match settled {
                    crate::session::AskSettlement::Accepted => Ok(AskAnswerOutput {
                        outcome: AnswerOutcome::Accepted,
                    }),
                    crate::session::AskSettlement::AlreadyAnswered => Ok(AskAnswerOutput {
                        outcome: AnswerOutcome::AlreadyAnswered,
                    }),
                    crate::session::AskSettlement::StaleRevision { current } => Err(CallError::new(
                        ErrorCode::Conflict,
                        format!(
                            "this prompt is at revision {current}; re-read it before answering"
                        ),
                    )
                    .with_execution(Execution::NotStarted)),
                }
                }
            })
            .unary::<SessionControl, _, _>(move |params: SessionControlParams, _ctx| {
                let manager = control_manager.clone();
                async move {
                    manager
                        .control(goat_wire::SessionId(params.session.0), params.op)
                        .await
                        .map_err(refused)?;
                    Ok(Empty {})
                }
            })
            .unary::<SessionKill, _, _>(move |params: SessionKillParams, _ctx| {
                let manager = kill_manager.clone();
                async move {
                    manager
                        .kill_session(goat_wire::SessionId(params.session.0))
                        .await
                        .map_err(refused)?;
                    Ok(Empty {})
                }
            })
            .unary::<AdminAgentReload, _, _>(move |params: AdminAgentReloadParams, _ctx| {
                let manager = reload_manager.clone();
                async move {
                    let report = manager.reload_agents(params.agent).await.map_err(refused)?;
                    Ok(AdminAgentReloadOutput {
                        reloaded: report.reloaded,
                        unchanged: report.unchanged,
                        failed: report
                            .failed
                            .into_iter()
                            .map(|failure| ReloadFailure {
                                agent: failure.agent,
                                reason: failure.reason,
                            })
                            .collect(),
                        warnings: report.warnings,
                    })
                }
            })
            .unary::<AdminDaemonStop, _, _>(move |params: AdminDaemonStopParams, _ctx| {
                let shutdown = shutdown.clone();
                let manager = stop_manager.clone();
                async move {
                    if params.if_idle {
                        let busy = manager.busy().await;
                        if !busy.is_idle() {
                            return Ok(AdminDaemonStopOutput::Busy {
                                sessions: busy.sessions,
                                turns: busy.turns,
                            });
                        }
                    }
                    shutdown.cancel();
                    Ok(AdminDaemonStopOutput::Stopping)
                }
            })
            .unary::<AdminDevicePair, _, _>(move |params: AdminDevicePairParams, _ctx| {
                let manager = pair_manager.clone();
                async move {
                    let (code, server_fingerprint, advertised) =
                        manager.pair_device(params.label).await.map_err(refused)?;
                    Ok(AdminDevicePairOutput {
                        code,
                        server_fingerprint,
                        advertised,
                    })
                }
            })
            .unary::<AdminDeviceList, _, _>(move |_params, _ctx| {
                let manager = devices_manager.clone();
                async move {
                    let devices = manager.list_devices().await.map_err(refused)?;
                    Ok(AdminDeviceListOutput {
                        devices: devices
                            .into_iter()
                            .map(|device| DeviceInfo {
                                id: device.id,
                                label: device.label,
                                paired_at: device.paired_at,
                            })
                            .collect(),
                    })
                }
            })
            .unary::<AdminDeviceRevoke, _, _>(move |params: AdminDeviceRevokeParams, _ctx| {
                let manager = revoke_manager.clone();
                async move {
                    let ok = manager
                        .revoke_device(&params.device)
                        .await
                        .map_err(refused)?;
                    Ok(AdminDeviceRevokeOutput { ok })
                }
            })
            .stream::<SessionWatch, _, _>(move |params: SessionWatchParams, ctx, sink| {
                let manager = watch_manager.clone();
                let epoch = watch_epoch.clone();
                async move {
                    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(WATCH_QUEUE);
                    let lagged = tokio_util::sync::CancellationToken::new();
                    let (backlog, _cwd) = manager
                        .watch_open(
                            goat_wire::SessionId(params.session.0),
                            goat_wire::ClientId(ctx.client),
                            &epoch,
                            &params.from,
                            out_tx,
                            lagged.clone(),
                        )
                        .await
                        .map_err(|message| {
                            CallError::new(ErrorCode::NotFound, message)
                                .with_execution(Execution::NotStarted)
                        })?;
                    for item in backlog {
                        sink.send(encode_item(&item)?).await?;
                    }
                    loop {
                        tokio::select! {
                            biased;
                            () = ctx.cancel.cancelled() => break,
                            () = lagged.cancelled() => {
                                return Err(CallError::new(
                                    ErrorCode::Lagged,
                                    "this watcher fell behind; reopen with the last cursor you saw",
                                )
                                .with_execution(Execution::KnownFailed));
                            }
                            frame = out_rx.recv() => {
                                let Some(frame) = frame else { break };
                                if let Some(item) = live_item(frame, &epoch) {
                                    sink.send(encode_item(&item)?).await?;
                                }
                            }
                        }
                    }
                    manager
                        .unsubscribe(
                            goat_wire::SessionId(params.session.0),
                            goat_wire::ClientId(ctx.client),
                        )
                        .await;
                    Ok(Empty {})
                }
            })
            .stream::<AgentWatch, _, _>(move |params: AgentWatchParams, ctx, sink| {
                let db_path = activity_db.clone();
                let epoch = activity_epoch.clone();
                async move {
                    let store = goat_store::SqliteStore::open(&db_path)
                        .await
                        .map_err(|err| {
                            CallError::new(
                                ErrorCode::Internal,
                                format!("the agent store is unavailable: {err}"),
                            )
                            .with_execution(Execution::KnownFailed)
                        })?;
                    let agents = parse_agents(&params.agents);
                    let mut cursor = activity_start(&store, &epoch, &params.from).await?;
                    loop {
                        let batch = store
                            .activity_since(&agents, cursor, ACTIVITY_PAGE)
                            .await
                            .map_err(|err| {
                                CallError::new(ErrorCode::Internal, err.to_string())
                                    .with_execution(Execution::KnownFailed)
                            })?;
                        for record in &batch {
                            cursor = record.id;
                            sink.send(encode_activity(&activity_item(record, &epoch))?)
                                .await?;
                        }
                        if batch.is_empty() {
                            tokio::select! {
                                biased;
                                () = ctx.cancel.cancelled() => return Ok(Empty {}),
                                () = tokio::time::sleep(ACTIVITY_POLL) => {}
                            }
                        } else if ctx.cancel.is_cancelled() {
                            return Ok(Empty {});
                        }
                    }
                }
            })
            .stream::<PtyOpen, _, _>(move |params: PtyOpenParams, ctx, sink| {
                let terminals = pty_terminals.clone();
                async move {
                    let id = terminals.mint_id();
                    let spawned = crate::pty_spawn::spawn(
                        &params.cwd,
                        params.cols,
                        params.rows,
                        params.command.as_deref(),
                        id.clone(),
                    )?;
                    terminals.insert(id.clone(), spawned.terminal).await;
                    let mut output = spawned.output;
                    let result = loop {
                        tokio::select! {
                            biased;
                            () = ctx.cancel.cancelled() => break Ok(Empty {}),
                            item = output.recv() => {
                                let Some(item) = item else { break Ok(Empty {}) };
                                let exited = matches!(item, goat_api::PtyItem::Exited { .. });
                                sink.send(encode_pty(&item)?).await?;
                                if exited {
                                    break Ok(Empty {});
                                }
                            }
                        }
                    };
                    terminals.close(&id).await;
                    result
                }
            })
            .unary::<PtyWrite, _, _>(move |params: PtyWriteParams, _ctx| {
                let terminals = write_terminals.clone();
                async move {
                    terminals.write(&params.pty, &params.data).await?;
                    Ok(Empty {})
                }
            })
            .unary::<PtyResize, _, _>(move |params: PtyResizeParams, _ctx| {
                let terminals = resize_terminals.clone();
                async move {
                    terminals
                        .resize(&params.pty, params.cols, params.rows)
                        .await?;
                    Ok(Empty {})
                }
            })
            .unary::<FsRead, _, _>(|params: FsReadParams, _ctx| async move {
                crate::files::read(&params)
            })
            .unary::<FsWrite, _, _>(|params: FsWriteParams, _ctx| async move {
                let len = crate::files::write(&params)?;
                Ok(FsWriteOutput {
                    path: params.path,
                    len,
                })
            })
            .unary::<GitDiff, _, _>(|params: GitDiffParams, _ctx| async move {
                crate::files::diff(&params)
            })
            .unary::<FsList, _, _>(|params: FsListParams, _ctx| async move {
                let children = CodeSessionHub::list_directory(&params.path, params.recursive)
                    .map_err(|message| {
                        CallError::new(ErrorCode::NotFound, message)
                            .with_execution(Execution::NotStarted)
                    })?;
                Ok(FsListOutput {
                    path: params.path,
                    entries: children.into_iter().map(dir_entry).collect(),
                    truncated: false,
                })
            });

    goat_capability::routes(router, broker, device)
}

pub(crate) fn snapshot_item(
    frame: goat_wire::ServerFrame,
    cwd: String,
    epoch: &str,
    watermark: u64,
    reset: bool,
) -> goat_api::WatchItem {
    let goat_wire::ServerFrame::Snapshot {
        session,
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
    } = frame
    else {
        return goat_api::WatchItem::Presence {
            cursor: goat_api::cursor_for(epoch, watermark),
            clients: 0,
        };
    };
    goat_api::WatchItem::Snapshot {
        cursor: goat_api::cursor_for(epoch, watermark),
        reset,
        state: Box::new(goat_api::SessionSnapshot {
            session: SessionId(session.0),
            cwd,
            target: *target,
            transcript,
            pending,
            context_tokens,
            compaction_threshold,
            skills,
            accounts,
            models: model_list,
            selected: *selected,
            mode: mode.mode,
            plan_path: mode.plan_path,
            processes,
            usage: usage
                .into_iter()
                .map(|entry| goat_api::UsageEntry {
                    provider: entry.provider,
                    account: entry.account,
                    usage: entry.usage,
                    context_window: entry.context_window,
                    compaction_threshold: entry.compaction_threshold,
                })
                .collect(),
            retry: retry.map(|entry| goat_api::RetryEntry {
                id: entry.id,
                attempt: entry.attempt,
                max_attempts: entry.max_attempts,
                delay_ms: entry.delay_ms,
                reason: entry.reason,
                resets_at: entry.resets_at,
            }),
            rate_limits: rate_limits
                .into_iter()
                .map(|entry| goat_api::RateLimitEntry {
                    provider: entry.provider,
                    account: entry.account,
                    snapshot: entry.snapshot,
                    cached_at: entry.cached_at,
                })
                .collect(),
            active,
        }),
    }
}

pub(crate) fn live_item(frame: goat_wire::ServerFrame, epoch: &str) -> Option<goat_api::WatchItem> {
    match frame {
        goat_wire::ServerFrame::Event { seq, event, .. } => Some(goat_api::WatchItem::Event {
            cursor: goat_api::cursor_for(epoch, seq),
            event: Box::new(event),
        }),
        goat_wire::ServerFrame::Presence { clients, .. } => Some(goat_api::WatchItem::Presence {
            cursor: goat_api::cursor_for(epoch, 0),
            clients: clients.len(),
        }),
        _ => None,
    }
}

fn session_info(info: goat_wire::SessionInfo) -> SessionInfo {
    SessionInfo {
        session: SessionId(info.session.0),
        cwd: info.cwd,
        state: live_state(info.state),
        windows: info.windows,
        age_ms: info.age_ms,
        tokens: info.tokens,
    }
}

fn conversation_info(info: goat_wire::ConversationInfo) -> ConversationInfo {
    ConversationInfo {
        conversation_id: info.conversation_id,
        cwd: info.cwd,
        title: info.title,
        model: info.model,
        updated_at: info.updated_at,
        live: info.live.map(|session| SessionId(session.0)),
        state: info.state.map(live_state),
    }
}

fn live_state(state: goat_wire::SessionLiveState) -> SessionLiveState {
    match state {
        goat_wire::SessionLiveState::Idle {} => SessionLiveState::Idle {},
        goat_wire::SessionLiveState::Active {} => SessionLiveState::Active {},
        goat_wire::SessionLiveState::WaitingOnAsk {} => SessionLiveState::WaitingOnAsk {},
    }
}

fn dir_entry(entry: goat_wire::DirEntry) -> DirEntry {
    DirEntry {
        name: entry.name,
        kind: match entry.kind {
            goat_wire::DirEntryKind::Directory {} => DirEntryKind::Directory {},
            goat_wire::DirEntryKind::File {} => DirEntryKind::File {},
            goat_wire::DirEntryKind::Symlink {} => DirEntryKind::Symlink {},
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{LOCAL_GRANTS, REMOTE_GRANTS, build, conversation_info, dir_entry, session_info};
    use goat_api::{
        DaemonStatus2, FsListOutput, Grant, Router, SessionListOutput, SessionLiveState,
    };
    use goat_capability::Broker;
    use goat_wire::envelope::{ErrorCode, Frame, Role};
    use goat_wire::peer::{Handler, Peer, RejectAll, spawn};
    use goat_wire::{SessionId as WireSessionId, WireConn};
    use serde_json::{Value, json};
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    type Conn = WireConn<tokio::io::DuplexStream, Frame, Frame>;

    fn manager() -> crate::manager::CodeSessionHub {
        let dir = std::env::temp_dir().join(format!("goat-api-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        crate::manager::CodeSessionHub::new(
            dir.join("credentials.json"),
            goat_config::UserProviders::at(dir.join("config.json")),
            dir.join("goat.db"),
        )
    }

    fn daemon_api(shutdown: CancellationToken) -> super::DaemonApi {
        super::DaemonApi {
            manager: manager(),
            broker: Arc::new(Broker::new()),
            device: "device".to_owned(),
            epoch: "e1".to_owned(),
            shutdown,
            terminals: Arc::new(crate::pty::Terminals::new()),
            db_path: std::env::temp_dir().join("goat-api-test.db"),
        }
    }

    fn router_for(grants: &[Grant]) -> Router {
        build(daemon_api(CancellationToken::new()), grants)
    }

    fn connect(grants: &[Grant]) -> Peer {
        let router = router_for(grants);
        let (a, b) = tokio::io::duplex(1024 * 1024);
        let (client_sink, client_source) = Conn::new(a).split();
        let (daemon_sink, daemon_source) = Conn::new(b).split();
        let closed = CancellationToken::new();
        let daemon = spawn(
            Role::Daemon,
            Box::pin(daemon_sink),
            Box::pin(daemon_source),
            Arc::new(router),
            closed.clone(),
        );
        std::mem::forget(daemon);
        spawn(
            Role::Client,
            Box::pin(client_sink),
            Box::pin(client_source),
            Arc::new(RejectAll),
            closed,
        )
    }

    #[tokio::test]
    async fn daemon_status_answers_with_this_process() {
        let peer = connect(&LOCAL_GRANTS);
        let value = peer
            .handle
            .call("daemon.status", 1, Value::Null)
            .await
            .unwrap();
        let status: DaemonStatus2 = serde_json::from_value(value).unwrap();
        assert_eq!(status.pid, std::process::id());
        assert_eq!(status.epoch, "e1");
        assert!(!status.ready);
        assert_eq!(status.sessions, 0);
    }

    #[tokio::test]
    async fn session_list_starts_empty() {
        let peer = connect(&LOCAL_GRANTS);
        let value = peer
            .handle
            .call("session.list", 1, Value::Null)
            .await
            .unwrap();
        let listed: SessionListOutput = serde_json::from_value(value).unwrap();
        assert!(listed.sessions.is_empty());
    }

    #[tokio::test]
    async fn fs_list_reads_a_real_directory() {
        let peer = connect(&LOCAL_GRANTS);
        let dir = std::env::temp_dir().join(format!("goat-fs-list-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        std::fs::write(dir.join("a.txt"), "hi").unwrap();

        let value = peer
            .handle
            .call(
                "fs.list",
                1,
                json!({"path": dir.display().to_string(), "recursive": false}),
            )
            .await
            .unwrap();
        let listed: FsListOutput = serde_json::from_value(value).unwrap();
        let names: Vec<&str> = listed.entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"a.txt"), "got {names:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn fs_list_reports_a_missing_path_without_panicking() {
        let peer = connect(&LOCAL_GRANTS);
        let err = peer
            .handle
            .call("fs.list", 1, json!({"path": "/definitely/not/here"}))
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound);
        assert!(err.retry_is_safe());
    }

    #[tokio::test]
    async fn a_remote_router_does_not_contain_the_admin_routes() {
        let local = router_for(&LOCAL_GRANTS);
        let remote = router_for(&REMOTE_GRANTS);
        assert!(local.grants().contains(&Grant::Admin));
        assert!(!remote.grants().contains(&Grant::Admin));

        let admin = [
            "admin.agent_reload",
            "admin.daemon_stop",
            "admin.device_pair",
            "admin.device_list",
            "admin.device_revoke",
        ];
        for method in admin {
            assert!(local.serves(method, 1), "local should serve {method}");
            assert!(
                !remote.serves(method, 1),
                "remote must not even contain {method}"
            );
        }
        assert!(remote.serves("session.open", 1));
        assert!(remote.serves("fs.list", 1));
        assert_eq!(local.advertised().len(), remote.advertised().len() + 5);
    }

    #[tokio::test]
    async fn a_remote_peer_calling_an_admin_method_gets_unknown_method() {
        let peer = connect(&REMOTE_GRANTS);
        let err = peer
            .handle
            .call("admin.daemon_stop", 1, json!({}))
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::UnknownMethod);
        assert!(err.retry_is_safe());
    }

    #[tokio::test]
    async fn admin_daemon_stop_cancels_the_shutdown_token() {
        let shutdown = CancellationToken::new();
        let router = build(daemon_api(shutdown.clone()), &LOCAL_GRANTS);
        let (a, b) = tokio::io::duplex(1024 * 1024);
        let (client_sink, client_source) = Conn::new(a).split();
        let (daemon_sink, daemon_source) = Conn::new(b).split();
        let closed = CancellationToken::new();
        let daemon = spawn(
            Role::Daemon,
            Box::pin(daemon_sink),
            Box::pin(daemon_source),
            Arc::new(router),
            closed.clone(),
        );
        std::mem::forget(daemon);
        let peer = spawn(
            Role::Client,
            Box::pin(client_sink),
            Box::pin(client_source),
            Arc::new(RejectAll),
            closed,
        );
        assert!(!shutdown.is_cancelled());
        peer.handle
            .call("admin.daemon_stop", 1, json!({}))
            .await
            .unwrap();
        assert!(shutdown.is_cancelled());
    }

    #[tokio::test]
    async fn session_open_reports_the_normalized_cwd_and_epoch() {
        let peer = connect(&LOCAL_GRANTS);
        let dir = std::env::temp_dir();
        let value = peer
            .handle
            .call(
                "session.open",
                1,
                json!({"cwd": dir.display().to_string(), "resume": {"type": "New"}}),
            )
            .await;
        match value {
            Ok(value) => {
                let opened: goat_api::SessionOpenOutput = serde_json::from_value(value).unwrap();
                assert_eq!(opened.epoch, "e1");
                assert!(!opened.cwd.is_empty());
            }
            Err(err) => assert_eq!(err.code, ErrorCode::Denied),
        }
    }

    #[tokio::test]
    async fn answering_a_prompt_on_an_unknown_session_is_refused_not_accepted() {
        let peer = connect(&LOCAL_GRANTS);
        let err = peer
            .handle
            .call(
                "ask.answer",
                1,
                json!({"session": "999", "prompt": 1, "revision": 1, "answers": ["main"]}),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Denied);
        assert!(err.retry_is_safe());
    }

    #[tokio::test]
    async fn a_negative_prompt_id_is_rejected_as_invalid_params() {
        let peer = connect(&LOCAL_GRANTS);
        let err = peer
            .handle
            .call(
                "ask.answer",
                1,
                json!({"session": "1", "prompt": -3, "revision": 1, "answers": []}),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams);
        assert_eq!(
            err.execution,
            Some(goat_wire::envelope::Execution::NotStarted)
        );
    }

    #[tokio::test]
    async fn submit_and_ask_are_served_on_both_routers() {
        let local = router_for(&LOCAL_GRANTS);
        let remote = router_for(&REMOTE_GRANTS);
        for router in [&local, &remote] {
            assert!(router.serves("session.submit", 1));
            assert!(router.serves("ask.answer", 1));
            assert!(!router.is_stream("session.submit", 1));
            assert!(!router.is_stream("ask.answer", 1));
        }
    }

    #[tokio::test]
    async fn submitting_to_an_unknown_session_is_refused_before_any_task_is_allocated() {
        let peer = connect(&LOCAL_GRANTS);
        let err = peer
            .handle
            .call(
                "session.submit",
                1,
                json!({"session": "999", "op": {"type": "SubmitMessage", "id": "0", "text": "hi"}}),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Denied);
        assert_eq!(
            err.execution,
            Some(goat_wire::envelope::Execution::NotStarted)
        );
    }

    #[tokio::test]
    async fn watching_an_unknown_session_ends_the_stream_instead_of_hanging() {
        let peer = connect(&LOCAL_GRANTS);
        let mut stream = peer
            .handle
            .open_stream(
                "session.watch",
                1,
                json!({"session": "999", "from": {"type": "Snapshot"}}),
            )
            .await
            .unwrap();
        let Some(goat_wire::peer::StreamMsg::End(result)) = stream.recv().await else {
            panic!("expected the watch stream to terminate")
        };
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound);
        assert_eq!(
            err.execution,
            Some(goat_wire::envelope::Execution::NotStarted)
        );
    }

    #[tokio::test]
    async fn agent_watch_is_a_reliable_stream_on_both_routers() {
        let local = router_for(&LOCAL_GRANTS);
        let remote = router_for(&REMOTE_GRANTS);
        for router in [&local, &remote] {
            assert!(router.serves("agent.watch", 1));
            assert!(router.is_stream("agent.watch", 1));
        }
        let declared = goat_api::registry()
            .into_iter()
            .find(|schema| schema.name == "agent.watch")
            .expect("agent.watch is declared");
        assert_eq!(
            declared.shape,
            goat_api::Shape::Stream(goat_wire::envelope::StreamClass::Reliable)
        );
    }

    #[test]
    fn a_cursor_from_another_epoch_starts_at_the_watermark_not_at_zero() {
        assert!(matches!(
            goat_api::WatchFrom::Cursor {
                cursor: goat_api::Cursor::new("e1", 5)
            },
            goat_api::WatchFrom::Cursor { .. }
        ));
    }

    #[test]
    fn activity_records_translate_to_their_wire_variants() {
        let agent = goat_types::AgentId::from_slug("scout");
        let base = goat_store::ActivityRecord {
            id: 12,
            agent,
            kind: goat_store::ActivityKind::TurnStarted,
            run_id: 41,
            detail: Some("discord".to_owned()),
            ok: None,
            at: chrono::Utc::now(),
        };

        let started = super::activity_item(&base, "e5");
        let goat_api::AgentActivity::TurnStarted {
            cursor,
            trigger,
            run,
            ..
        } = &started
        else {
            panic!("expected a turn_started item")
        };
        assert_eq!(cursor.to_string(), "e5:12");
        assert_eq!(trigger, "discord");
        assert_eq!(*run, 41);

        let finished = super::activity_item(
            &goat_store::ActivityRecord {
                kind: goat_store::ActivityKind::TurnFinished,
                ok: Some(true),
                detail: None,
                ..base.clone()
            },
            "e5",
        );
        assert!(matches!(
            finished,
            goat_api::AgentActivity::TurnFinished { ok: true, .. }
        ));

        let fired = super::activity_item(
            &goat_store::ActivityRecord {
                kind: goat_store::ActivityKind::ScheduleFired,
                detail: Some("7".to_owned()),
                ..base.clone()
            },
            "e5",
        );
        assert!(matches!(
            fired,
            goat_api::AgentActivity::ScheduleFired { schedule: 7, .. }
        ));

        let tool = super::activity_item(
            &goat_store::ActivityRecord {
                kind: goat_store::ActivityKind::ToolStarted,
                detail: Some("shell".to_owned()),
                ..base
            },
            "e5",
        );
        let goat_api::AgentActivity::ToolStarted { tool: name, .. } = &tool else {
            panic!("expected a tool_started item")
        };
        assert_eq!(name, "shell");
    }

    #[test]
    fn agent_slugs_resolve_deterministically() {
        let first = super::parse_agents(&["scout".to_owned(), "scribe".to_owned()]);
        let again = super::parse_agents(&["scout".to_owned(), "scribe".to_owned()]);
        assert_eq!(first, again);
        assert_eq!(first.len(), 2);
        assert_ne!(first[0], first[1]);
        assert!(super::parse_agents(&[]).is_empty());
    }

    #[tokio::test]
    async fn watch_is_registered_as_a_reliable_stream_on_both_routers() {
        let local = router_for(&LOCAL_GRANTS);
        let remote = router_for(&REMOTE_GRANTS);
        for router in [&local, &remote] {
            assert!(router.serves("session.watch", 1));
            assert!(router.is_stream("session.watch", 1));
        }
        let declared = goat_api::registry()
            .into_iter()
            .find(|schema| schema.name == "session.watch")
            .expect("session.watch is declared");
        assert_eq!(
            declared.shape,
            goat_api::Shape::Stream(goat_wire::envelope::StreamClass::Reliable)
        );
    }

    #[test]
    fn a_snapshot_frame_becomes_a_watch_item_carrying_its_cursor() {
        let frame = goat_wire::ServerFrame::Snapshot {
            session: WireSessionId(4),
            watermark: 12,
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
        let item = super::snapshot_item(frame, "/w".to_owned(), "e3", 12, true);
        let goat_api::WatchItem::Snapshot {
            cursor,
            reset,
            state,
        } = item
        else {
            panic!("expected a snapshot item")
        };
        assert_eq!(cursor.to_string(), "e3:12");
        assert!(reset);
        assert_eq!(state.session.0, 4);
        assert_eq!(state.cwd, "/w");
    }

    #[test]
    fn live_frames_translate_only_where_a_cursor_exists() {
        let event = super::live_item(
            goat_wire::ServerFrame::Event {
                session: WireSessionId(1),
                seq: 41,
                event: goat_protocol::Event::TaskDone {
                    id: goat_protocol::TaskId(2),
                    interrupted: false,
                },
            },
            "e3",
        )
        .expect("events translate");
        assert_eq!(event.cursor().to_string(), "e3:41");

        let presence = super::live_item(
            goat_wire::ServerFrame::Presence {
                session: WireSessionId(1),
                clients: vec![goat_wire::ClientId(1), goat_wire::ClientId(2)],
            },
            "e3",
        )
        .expect("presence translates");
        assert!(matches!(
            presence,
            goat_api::WatchItem::Presence { clients: 2, .. }
        ));

        assert!(
            super::live_item(
                goat_wire::ServerFrame::Error {
                    message: "x".to_owned()
                },
                "e3"
            )
            .is_none()
        );
    }

    #[tokio::test]
    async fn capability_routes_are_reachable_through_the_daemon_router() {
        let peer = connect(&LOCAL_GRANTS);
        let value = peer
            .handle
            .call("capability.list", 1, json!({"capability": "host.browser"}))
            .await
            .unwrap();
        assert_eq!(value["providers"], json!([]));
    }

    #[test]
    fn wire_types_convert_without_losing_state() {
        let info = session_info(goat_wire::SessionInfo {
            session: WireSessionId(4),
            cwd: "/w".to_owned(),
            state: goat_wire::SessionLiveState::WaitingOnAsk {},
            windows: 2,
            age_ms: 10,
            tokens: 99,
        });
        assert_eq!(info.session.0, 4);
        assert_eq!(info.state, SessionLiveState::WaitingOnAsk {});

        let conversation = conversation_info(goat_wire::ConversationInfo {
            conversation_id: 7,
            cwd: "/w".to_owned(),
            title: Some("t".to_owned()),
            model: "m".to_owned(),
            updated_at: 1,
            live: Some(WireSessionId(4)),
            state: Some(goat_wire::SessionLiveState::Active {}),
        });
        assert_eq!(conversation.live.map(|s| s.0), Some(4));

        let entry = dir_entry(goat_wire::DirEntry {
            name: "src".to_owned(),
            kind: goat_wire::DirEntryKind::Directory {},
        });
        assert_eq!(entry.name, "src");
    }

    #[test]
    fn every_served_method_is_a_frozen_contract() {
        let router = build(daemon_api(CancellationToken::new()), &LOCAL_GRANTS);
        let served: std::collections::BTreeSet<String> = router.advertised().into_keys().collect();
        let frozen: std::collections::BTreeSet<String> = goat_api::registry()
            .into_iter()
            .filter(|schema| schema.direction == goat_api::Direction::ToDaemon)
            .map(|schema| schema.name.to_owned())
            .collect();
        assert_eq!(
            served, frozen,
            "the daemon router and goat-api's registry disagree; a method served but not registered \
             is invisible to methods_fingerprint.txt, and one registered but not served is advertised \
             to nobody"
        );
    }
}
