use std::sync::Arc;

use futures::{Sink, Stream};
use goat_api::Grant;
use goat_wire::envelope::{Frame, Hello, Role};
use tokio_util::sync::CancellationToken;

use crate::api;
use crate::manager::CodeSessionHub;

#[derive(Debug, Clone)]
pub enum ClientOrigin {
    Local,
    Remote { device: String },
}

#[must_use]
pub fn grants_for(origin: &ClientOrigin) -> &'static [Grant] {
    match origin {
        ClientOrigin::Local => &api::LOCAL_GRANTS,
        ClientOrigin::Remote { .. } => &api::REMOTE_GRANTS,
    }
}

#[must_use]
pub fn device_for(origin: &ClientOrigin) -> String {
    match origin {
        ClientOrigin::Local => "local".to_owned(),
        ClientOrigin::Remote { device } => device.clone(),
    }
}

#[derive(Clone)]
pub struct EnvelopeHost {
    pub manager: CodeSessionHub,
    pub shutdown: CancellationToken,
    pub epoch: String,
    pub(crate) terminals: Arc<crate::pty::Terminals>,
    pub db_path: std::path::PathBuf,
}

impl EnvelopeHost {
    #[must_use]
    pub fn new(
        manager: CodeSessionHub,
        shutdown: CancellationToken,
        epoch: String,
        db_path: std::path::PathBuf,
    ) -> Self {
        Self {
            manager,
            shutdown,
            epoch,
            terminals: Arc::new(crate::pty::Terminals::new()),
            db_path,
        }
    }
}

pub async fn serve_envelope<Si, St>(
    host: EnvelopeHost,
    origin: ClientOrigin,
    sink: Si,
    source: St,
    disconnect: CancellationToken,
) where
    Si: Sink<Frame> + Unpin + Send + 'static,
    St: Stream<Item = Result<Frame, goat_wire::WireError>> + Unpin + Send + 'static,
{
    let EnvelopeHost {
        manager,
        shutdown,
        epoch,
        terminals,
        db_path,
    } = host;
    let epoch = epoch.as_str();
    let broker = manager.broker();
    let browser_events = manager.browser_events();
    let client_id = manager.next_client_id();
    let build = manager.build();
    let grants = grants_for(&origin);
    let device = device_for(&origin);
    let router = api::build(
        api::DaemonApi {
            manager,
            broker,
            browser_events,
            device,
            epoch: epoch.to_owned(),
            shutdown: shutdown.clone(),
            terminals,
            db_path,
        },
        grants,
    )
    .for_client(client_id.0);
    let advertised = router.advertised();

    let mut peer = goat_wire::peer::spawn(
        Role::Daemon,
        sink,
        source,
        Arc::new(router),
        disconnect.clone(),
    );

    let mut hello = Hello::new(
        Role::Daemon,
        concat!("goat-daemon/", env!("CARGO_PKG_VERSION")),
    );
    for (name, versions) in advertised {
        hello = hello.with_method(name, versions);
    }
    hello = hello
        .with_grants(
            grants
                .iter()
                .map(|grant| grant.as_str().to_owned())
                .collect(),
        )
        .with_info(serde_json::json!({
            "client_id": client_id.0.to_string(),
            "epoch": epoch,
            "pid": std::process::id(),
            "build": build,
        }));

    if peer.handle.send_hello(hello).await.is_err() {
        return;
    }

    tokio::select! {
        () = shutdown.cancelled() => {}
        () = disconnect.cancelled() => {}
        _ = peer.hello.recv() => {
            tokio::select! {
                () = shutdown.cancelled() => {}
                () = disconnect.cancelled() => {}
            }
        }
    }
    disconnect.cancel();
}

#[cfg(test)]
mod tests {
    use super::ClientOrigin;
    use super::{device_for, grants_for, serve_envelope};
    use goat_api::{Api, DaemonStatus, DaemonStatus2, Empty, Grant};
    use goat_wire::WireConn;
    use goat_wire::envelope::{ErrorCode, Frame, Hello, Role};
    use goat_wire::peer::{RejectAll, spawn};
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    type Conn = WireConn<tokio::io::DuplexStream, Frame, Frame>;

    fn manager() -> crate::manager::CodeSessionHub {
        let dir = std::env::temp_dir().join(format!("goat-envelope-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        crate::manager::CodeSessionHub::new(
            dir.join("credentials.json"),
            goat_config::UserProviders::at(dir.join("config.json")),
            dir.join("goat.db"),
        )
    }

    async fn dial(origin: ClientOrigin) -> (Api, Hello, CancellationToken) {
        let (a, b) = tokio::io::duplex(1024 * 1024);
        let (client_sink, client_source) = Conn::new(a).split();
        let (daemon_sink, daemon_source) = Conn::new(b).split();
        let closed = CancellationToken::new();

        tokio::spawn(serve_envelope(
            super::EnvelopeHost {
                manager: manager(),
                shutdown: CancellationToken::new(),
                epoch: "e9".to_owned(),
                terminals: Arc::new(crate::pty::Terminals::new()),
                db_path: std::env::temp_dir().join("goat-envelope-test.db"),
            },
            origin,
            Box::pin(daemon_sink),
            Box::pin(daemon_source),
            closed.clone(),
        ));

        let mut client = spawn(
            Role::Client,
            Box::pin(client_sink),
            Box::pin(client_source),
            Arc::new(RejectAll),
            closed.clone(),
        );
        let hello = client.hello.recv().await.expect("the daemon greets first");
        let api = Api::negotiated(client.handle.clone(), &hello);
        std::mem::forget(client);
        (api, hello, closed)
    }

    #[test]
    fn origin_decides_the_grant_set_and_the_device_name() {
        assert!(grants_for(&ClientOrigin::Local).contains(&Grant::Admin));
        assert!(
            !grants_for(&ClientOrigin::Remote {
                device: "phone".to_owned()
            })
            .contains(&Grant::Admin)
        );
        assert_eq!(device_for(&ClientOrigin::Local), "local");
        assert_eq!(
            device_for(&ClientOrigin::Remote {
                device: "phone".to_owned()
            }),
            "phone"
        );
    }

    #[tokio::test]
    async fn the_daemon_greets_first_with_its_method_table() {
        let (_api, hello, _closed) = dial(ClientOrigin::Local).await;
        assert_eq!(hello.role, Role::Daemon);
        assert!(hello.compatible());
        assert!(hello.speaks("daemon.status", 1));
        assert!(hello.speaks("session.watch", 1));
        assert_eq!(hello.info["epoch"], "e9");
    }

    #[tokio::test]
    async fn a_local_greeting_advertises_admin_and_a_remote_one_does_not() {
        let (_api, local, _a) = dial(ClientOrigin::Local).await;
        let (_api2, remote, _b) = dial(ClientOrigin::Remote {
            device: "phone".to_owned(),
        })
        .await;

        assert!(local.speaks("admin.daemon_stop", 1));
        assert!(local.speaks("admin.credential_set", 1));
        assert!(local.speaks("admin.credential_remove", 1));
        assert!(local.grants.iter().any(|grant| grant == "admin"));

        assert!(!remote.speaks("admin.daemon_stop", 1));
        assert!(!remote.speaks("admin.credential_set", 1));
        assert!(!remote.speaks("admin.credential_remove", 1));
        assert!(!remote.grants.iter().any(|grant| grant == "admin"));
        assert!(remote.speaks("session.open", 1));
    }

    #[tokio::test]
    async fn a_negotiated_call_round_trips_over_the_served_connection() {
        let (api, _hello, _closed) = dial(ClientOrigin::Local).await;
        let status: DaemonStatus2 = api.call::<DaemonStatus>(Empty {}).await.unwrap();
        assert_eq!(status.epoch, "e9");
        assert_eq!(status.pid, std::process::id());
    }

    #[tokio::test]
    async fn a_remote_client_is_refused_admin_before_the_wire() {
        let (api, _hello, _closed) = dial(ClientOrigin::Remote {
            device: "phone".to_owned(),
        })
        .await;
        let err = api
            .call::<goat_api::AdminDaemonStop>(goat_api::AdminDaemonStopParams::default())
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::UnsupportedVersion);
        assert!(err.retry_is_safe());
    }
}
