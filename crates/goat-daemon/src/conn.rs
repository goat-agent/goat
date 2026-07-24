use std::path::PathBuf;

use futures::{Sink, SinkExt, Stream, StreamExt};
use goat_wire::transport::Stream as LocalStream;
use goat_wire::{ClientFrame, PROTOCOL_VERSION, ServerConn, ServerFrame};
use tokio::sync::mpsc;

use crate::manager::Manager;

const CLIENT_QUEUE: usize = 1024;

#[derive(Debug, Clone)]
pub(crate) enum ClientOrigin {
    Local,
    Remote { device: String },
}

impl ClientOrigin {
    fn is_local(&self) -> bool {
        matches!(self, ClientOrigin::Local)
    }
}

pub(crate) async fn handle_connection(
    stream: LocalStream,
    manager: Manager,
    shutdown: tokio_util::sync::CancellationToken,
) {
    let conn: ServerConn<LocalStream> = ServerConn::new(stream);
    let (sink, source) = conn.split();
    serve_connection(
        sink,
        source,
        manager,
        shutdown,
        ClientOrigin::Local,
        tokio_util::sync::CancellationToken::new(),
    )
    .await;
}

pub(crate) async fn serve_connection<Si, St>(
    sink: Si,
    mut source: St,
    manager: Manager,
    shutdown: tokio_util::sync::CancellationToken,
    origin: ClientOrigin,
    disconnect: tokio_util::sync::CancellationToken,
) where
    Si: Sink<ServerFrame> + Send + 'static,
    St: Stream<Item = Result<ClientFrame, goat_wire::WireError>> + Unpin,
{
    let mut sink = Box::pin(sink);

    match source.next().await {
        Some(Ok(ClientFrame::Hello { version })) if version == PROTOCOL_VERSION => {}
        Some(Ok(ClientFrame::Hello { .. })) => {
            let _ = sink
                .send(ServerFrame::VersionMismatch {
                    daemon_version: PROTOCOL_VERSION,
                })
                .await;
            return;
        }
        _ => {
            let _ = sink
                .send(ServerFrame::Error {
                    message: "expected Hello".to_owned(),
                })
                .await;
            return;
        }
    }

    let client_id = manager.next_client_id();
    if let ClientOrigin::Remote { device } = &origin {
        tracing::info!(client = client_id.0, device = %device, "remote client connected");
    }
    if sink
        .send(ServerFrame::Welcome {
            version: PROTOCOL_VERSION,
            client_id,
        })
        .await
        .is_err()
    {
        return;
    }

    let (out_tx, mut out_rx) = mpsc::channel::<ServerFrame>(CLIENT_QUEUE);

    let writer = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            if sink.send(frame).await.is_err() {
                break;
            }
        }
    });

    let mut graceful = false;
    loop {
        tokio::select! {
            () = disconnect.cancelled() => break,
            next = source.next() => {
                let Some(Ok(frame)) = next else { break };
                match dispatch(&manager, client_id, &out_tx, &shutdown, &origin, frame).await {
                    Disposition::Continue => {}
                    Disposition::Closed => {
                        graceful = true;
                        break;
                    }
                }
            }
        }
    }

    if graceful {
        tracing::debug!(client = client_id.0, "client disconnected gracefully");
    } else {
        tracing::warn!(client = client_id.0, "client disconnected unexpectedly");
    }
    manager.drop_client(client_id).await;
    writer.abort();
}

enum Disposition {
    Continue,
    Closed,
}

async fn dispatch(
    manager: &Manager,
    client_id: goat_wire::ClientId,
    out_tx: &mpsc::Sender<ServerFrame>,
    shutdown: &tokio_util::sync::CancellationToken,
    origin: &ClientOrigin,
    frame: ClientFrame,
) -> Disposition {
    match frame {
        ClientFrame::Hello { .. } => Disposition::Continue,
        ClientFrame::OpenSession { cwd, resume } => {
            let cwd_path = PathBuf::from(&cwd);
            match manager.open_or_attach(cwd_path, resume).await {
                Ok(session) => {
                    let _ = out_tx.send(ServerFrame::SessionOpened { session }).await;
                    let _ = manager.subscribe(session, client_id, out_tx.clone()).await;
                }
                Err(message) => {
                    let _ = out_tx.send(ServerFrame::Error { message }).await;
                }
            }
            Disposition::Continue
        }
        ClientFrame::Attach { session } => {
            if let Err(message) = manager.subscribe(session, client_id, out_tx.clone()).await {
                let _ = out_tx.send(ServerFrame::Error { message }).await;
            }
            Disposition::Continue
        }
        ClientFrame::Submit {
            session,
            correlation,
            op,
        } => {
            let result = match op {
                goat_protocol::Op::Clear {} => {
                    manager
                        .rebind(client_id, session, out_tx, goat_wire::ResumeMode::New {})
                        .await
                }
                goat_protocol::Op::ResumeLatest {} => {
                    manager
                        .rebind(client_id, session, out_tx, goat_wire::ResumeMode::Latest {})
                        .await
                }
                goat_protocol::Op::Resume { thread_id } => {
                    manager
                        .rebind(
                            client_id,
                            session,
                            out_tx,
                            goat_wire::ResumeMode::Thread { thread_id },
                        )
                        .await
                }
                other => manager.submit(session, out_tx, correlation, other).await,
            };
            if let Err(message) = result {
                let _ = out_tx.send(ServerFrame::Error { message }).await;
            }
            Disposition::Continue
        }
        ClientFrame::Control { session, op } => {
            if let Err(message) = manager.control(session, op).await {
                let _ = out_tx.send(ServerFrame::Error { message }).await;
            }
            Disposition::Continue
        }
        ClientFrame::ListSessions {} => {
            let sessions = manager.list_sessions().await;
            let _ = out_tx.send(ServerFrame::Sessions { sessions }).await;
            Disposition::Continue
        }
        ClientFrame::ListThreads { cwd } => {
            let threads = manager.list_threads(&cwd).await;
            let _ = out_tx.send(ServerFrame::Threads { threads }).await;
            Disposition::Continue
        }
        ClientFrame::ListDirectory { path } => {
            match Manager::list_directory(&path) {
                Ok(children) => {
                    let _ = out_tx.send(ServerFrame::Directory { path, children }).await;
                }
                Err(message) => {
                    let _ = out_tx.send(ServerFrame::Error { message }).await;
                }
            }
            Disposition::Continue
        }
        ClientFrame::KillSession { session } => {
            if let Err(message) = manager.kill_session(session).await {
                let _ = out_tx.send(ServerFrame::Error { message }).await;
            }
            Disposition::Continue
        }
        ClientFrame::PairDevice { label } => {
            if origin.is_local() {
                match manager.pair_device(label).await {
                    Ok((code, server_fingerprint, advertised)) => {
                        let _ = out_tx
                            .send(ServerFrame::PairingCode {
                                code,
                                server_fingerprint,
                                advertised,
                            })
                            .await;
                    }
                    Err(message) => {
                        let _ = out_tx.send(ServerFrame::Error { message }).await;
                    }
                }
            } else {
                let _ = out_tx
                    .send(ServerFrame::Error {
                        message: "pairing is local-only".to_owned(),
                    })
                    .await;
            }
            Disposition::Continue
        }
        ClientFrame::ListDevices {} => {
            match manager.list_devices().await {
                Ok(devices) => {
                    let _ = out_tx.send(ServerFrame::Devices { devices }).await;
                }
                Err(message) => {
                    let _ = out_tx.send(ServerFrame::Error { message }).await;
                }
            }
            Disposition::Continue
        }
        ClientFrame::RevokeDevice { device } => {
            match manager.revoke_device(&device).await {
                Ok(ok) => {
                    let _ = out_tx.send(ServerFrame::DeviceRevoked { ok }).await;
                }
                Err(message) => {
                    let _ = out_tx.send(ServerFrame::Error { message }).await;
                }
            }
            Disposition::Continue
        }
        ClientFrame::StopDaemon {} => {
            if origin.is_local() {
                shutdown.cancel();
                Disposition::Closed
            } else {
                let _ = out_tx
                    .send(ServerFrame::Error {
                        message: "StopDaemon is local-only".to_owned(),
                    })
                    .await;
                Disposition::Continue
            }
        }
        ClientFrame::Goodbye {} => Disposition::Closed,
    }
}

#[cfg(test)]
mod tests {
    use super::ClientOrigin;

    #[test]
    fn local_origin_is_local() {
        assert!(ClientOrigin::Local.is_local());
    }

    #[test]
    fn remote_origin_is_not_local() {
        let origin = ClientOrigin::Remote {
            device: "abc".to_owned(),
        };
        assert!(!origin.is_local());
    }
}
