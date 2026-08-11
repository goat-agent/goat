use std::sync::Arc;

use goat_api::Api;
use goat_wire::envelope::{Hello, Role};
use goat_wire::peer::{Handler, Peer, RejectAll};
use tokio_util::sync::CancellationToken;

use crate::ClientError;
use crate::link::Link;

pub struct ApiSession {
    pub api: Api,
    pub daemon: Hello,
    pub closed: CancellationToken,
    peer: Peer,
}

impl ApiSession {
    #[must_use]
    pub fn peer(&self) -> &Peer {
        &self.peer
    }

    pub fn shutdown(&self) {
        self.closed.cancel();
    }
}

pub async fn open(link: &Link, agent: &str) -> Result<ApiSession, ClientError> {
    open_serving(
        link,
        agent,
        Arc::new(RejectAll),
        Hello::new(Role::Client, agent),
    )
    .await
}

pub async fn open_serving(
    link: &Link,
    agent: &str,
    handler: Arc<dyn Handler>,
    hello: Hello,
) -> Result<ApiSession, ClientError> {
    let conn = link.dial_envelope().await?;
    let (sink, source) = conn.split();
    let closed = CancellationToken::new();
    let mut peer = goat_wire::peer::spawn(Role::Client, sink, source, handler, closed.clone());

    peer.handle
        .send_hello(hello)
        .await
        .map_err(|err| ClientError::Refused(err.to_string()))?;

    let daemon = tokio::time::timeout(crate::GREET_TIMEOUT, peer.hello.recv())
        .await
        .map_err(|_| ClientError::Timeout(crate::GREET_TIMEOUT))?
        .ok_or(ClientError::Handshake)?;

    if !daemon.compatible() {
        closed.cancel();
        return Err(ClientError::RemoteIncompatible);
    }

    let api = Api::negotiated(peer.handle.clone(), &daemon);
    let _ = agent;
    Ok(ApiSession {
        api,
        daemon,
        closed,
        peer,
    })
}

#[cfg(test)]
mod tests {
    use goat_api::{Api, DaemonStatus, DaemonStatus2, Empty, Grant, Router};
    use goat_wire::WireConn;
    use goat_wire::envelope::{ErrorCode, Frame, Hello, Role};
    use goat_wire::peer::{RejectAll, spawn};
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    type Conn = WireConn<tokio::io::DuplexStream, Frame, Frame>;

    fn status() -> DaemonStatus2 {
        DaemonStatus2 {
            version: "0.1.0".to_owned(),
            pid: 5,
            started_at: 1,
            ready: true,
            epoch: "e2".to_owned(),
            sessions: 0,
            turns: 0,
        }
    }

    fn daemon_router() -> Router {
        Router::new([Grant::Any]).unary::<DaemonStatus, _, _>(|_p, _c| async { Ok(status()) })
    }

    async fn handshake(daemon_hello: Hello) -> (Api, CancellationToken) {
        let (a, b) = tokio::io::duplex(1024 * 1024);
        let (client_sink, client_source) = Conn::new(a).split();
        let (daemon_sink, daemon_source) = Conn::new(b).split();
        let closed = CancellationToken::new();

        let mut daemon = spawn(
            Role::Daemon,
            Box::pin(daemon_sink),
            Box::pin(daemon_source),
            Arc::new(daemon_router()),
            closed.clone(),
        );
        let mut client = spawn(
            Role::Client,
            Box::pin(client_sink),
            Box::pin(client_source),
            Arc::new(RejectAll),
            closed.clone(),
        );

        client
            .handle
            .send_hello(Hello::new(Role::Client, "test/1.0"))
            .await
            .unwrap();
        let seen = daemon.hello.recv().await.expect("daemon sees the greeting");
        assert_eq!(seen.role, Role::Client);

        daemon.handle.send_hello(daemon_hello).await.unwrap();
        let theirs = client.hello.recv().await.expect("client sees the greeting");
        let api = Api::negotiated(client.handle.clone(), &theirs);
        std::mem::forget(daemon);
        std::mem::forget(client);
        (api, closed)
    }

    #[tokio::test]
    async fn a_greeting_that_advertises_the_method_lets_the_call_through() {
        let hello =
            Hello::new(Role::Daemon, "goat-daemon/test").with_method("daemon.status", vec![1]);
        let (api, _closed) = handshake(hello).await;
        assert_eq!(api.call::<DaemonStatus>(Empty {}).await.unwrap(), status());
    }

    #[tokio::test]
    async fn a_greeting_that_omits_the_method_refuses_before_the_wire() {
        let hello = Hello::new(Role::Daemon, "goat-daemon/test").with_method("fs.list", vec![1]);
        let (api, _closed) = handshake(hello).await;
        let err = api.call::<DaemonStatus>(Empty {}).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::UnsupportedVersion);
        assert!(err.message.contains("does not offer daemon.status"));
    }

    #[tokio::test]
    async fn an_incompatible_envelope_is_detected_from_the_greeting() {
        let mut hello = Hello::new(Role::Daemon, "goat-daemon/test");
        hello.envelope = "env0:deadbeef".to_owned();
        assert!(!hello.compatible());
        let good = Hello::new(Role::Daemon, "goat-daemon/test");
        assert!(good.compatible());
    }

    #[tokio::test]
    async fn one_connection_carries_many_concurrent_calls() {
        let hello =
            Hello::new(Role::Daemon, "goat-daemon/test").with_method("daemon.status", vec![1]);
        let (api, _closed) = handshake(hello).await;
        let calls = (0..8).map(|_| {
            let api = api.clone();
            tokio::spawn(async move { api.call::<DaemonStatus>(Empty {}).await })
        });
        for call in calls {
            assert_eq!(call.await.unwrap().unwrap(), status());
        }
    }
}
