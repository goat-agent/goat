use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::time::Duration;

use goat_wire::envelope::{CallError, ErrorCode, Execution, Hello};
use goat_wire::peer::{PeerHandle, StreamHandle, StreamMsg};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::registry::Method;

#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent<T> {
    Item { item: T, dropped: u64 },
    End(Result<(), CallError>),
}

pub struct Stream<T> {
    inner: StreamHandle,
    marker: PhantomData<fn() -> T>,
}

impl<T: DeserializeOwned> Stream<T> {
    pub async fn recv(&mut self) -> Option<StreamEvent<T>> {
        match self.inner.recv().await? {
            StreamMsg::Item { item, dropped } => match serde_json::from_value(item) {
                Ok(item) => Some(StreamEvent::Item { item, dropped }),
                Err(err) => Some(StreamEvent::End(Err(decode_failed(&err.to_string())))),
            },
            StreamMsg::End(Ok(_)) => Some(StreamEvent::End(Ok(()))),
            StreamMsg::End(Err(error)) => Some(StreamEvent::End(Err(error))),
        }
    }

    pub async fn cancel(&self, reason: impl Into<String>) {
        self.inner.cancel(reason).await;
    }
}

#[derive(Clone)]
pub struct Api {
    peer: PeerHandle,
    methods: BTreeMap<String, Vec<u16>>,
}

impl Api {
    #[must_use]
    pub fn new(peer: PeerHandle) -> Self {
        Self {
            peer,
            methods: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn negotiated(peer: PeerHandle, hello: &Hello) -> Self {
        Self {
            peer,
            methods: hello.methods.clone(),
        }
    }

    #[must_use]
    pub fn peer(&self) -> &PeerHandle {
        &self.peer
    }

    pub fn speaks<M: Method>(&self) -> bool {
        if self.methods.is_empty() {
            return true;
        }
        self.methods
            .get(M::NAME)
            .is_some_and(|versions| versions.contains(&M::VERSION))
    }

    pub async fn call<M: Method>(&self, params: M::Params) -> Result<M::Output, CallError> {
        self.call_with_deadline::<M>(params, goat_wire::peer::DEFAULT_DEADLINE)
            .await
    }

    pub async fn call_with_deadline<M: Method>(
        &self,
        params: M::Params,
        deadline: Duration,
    ) -> Result<M::Output, CallError> {
        if !self.speaks::<M>() {
            return Err(unsupported::<M>(&self.methods));
        }
        let encoded = encode(params)?;
        let value = self
            .peer
            .call_with_deadline(M::NAME, M::VERSION, encoded, deadline)
            .await?;
        serde_json::from_value(value).map_err(|err| decode_failed(&err.to_string()))
    }

    pub async fn open<M: Method>(&self, params: M::Params) -> Result<Stream<M::Item>, CallError> {
        if !self.speaks::<M>() {
            return Err(unsupported::<M>(&self.methods));
        }
        let encoded = encode(params)?;
        let inner = self.peer.open_stream(M::NAME, M::VERSION, encoded).await?;
        Ok(Stream {
            inner,
            marker: PhantomData,
        })
    }
}

fn encode<T: serde::Serialize>(params: T) -> Result<Value, CallError> {
    serde_json::to_value(params).map_err(|err| {
        CallError::new(
            ErrorCode::InvalidParams,
            format!("parameters could not be encoded: {err}"),
        )
        .with_execution(Execution::NotStarted)
    })
}

fn decode_failed(reason: &str) -> CallError {
    CallError::new(
        ErrorCode::Internal,
        format!("the peer answered with a payload this build cannot read: {reason}"),
    )
    .with_execution(Execution::KnownFailed)
}

fn unsupported<M: Method>(methods: &BTreeMap<String, Vec<u16>>) -> CallError {
    let known = methods.get(M::NAME).cloned().unwrap_or_default();
    let message = if known.is_empty() {
        format!("the peer does not offer {}", M::NAME)
    } else {
        format!(
            "the peer offers {} at {:?}, not version {}",
            M::NAME,
            known,
            M::VERSION
        )
    };
    CallError::new(ErrorCode::UnsupportedVersion, message).with_execution(Execution::NotStarted)
}

#[cfg(test)]
mod tests {
    use super::{Api, StreamEvent};
    use crate::methods::{
        DaemonStatus, DaemonStatus2, Empty, SessionList, SessionListOutput, SessionWatch,
        SessionWatchParams, WatchFrom, WatchItem,
    };
    use crate::registry::Grant;
    use crate::router::Router;
    use crate::{Cursor, SessionId};
    use goat_wire::envelope::{CallError, ErrorCode, Execution, Frame, Hello, Role};
    use goat_wire::peer::{Peer, RejectAll, StreamSink, spawn};
    use goat_wire::{WireConn, WireError};
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    type Conn = WireConn<tokio::io::DuplexStream, Frame, Frame>;

    fn status() -> DaemonStatus2 {
        DaemonStatus2 {
            version: "0.1.0".to_owned(),
            pid: 7,
            started_at: 3,
            ready: true,
            epoch: "e4".to_owned(),
            sessions: 1,
            turns: 0,
        }
    }

    fn server() -> Router {
        Router::new([Grant::Any])
            .unary::<DaemonStatus, _, _>(|_p, _c| async { Ok(status()) })
            .unary::<SessionList, _, _>(|_p, _c| async {
                Err::<SessionListOutput, CallError>(
                    CallError::new(ErrorCode::Denied, "not now")
                        .with_execution(Execution::NotStarted),
                )
            })
            .stream::<SessionWatch, _, _>(
                |params: SessionWatchParams, _ctx, sink: StreamSink| async move {
                    for seq in 0..3u64 {
                        sink.send_dropping(
                            serde_json::to_value(WatchItem::Presence {
                                cursor: Cursor::new("e4", seq),
                                clients: 1,
                            })
                            .unwrap(),
                            u64::from(seq == 1),
                        )
                        .await?;
                    }
                    let _ = params;
                    Ok(Empty {})
                },
            )
    }

    fn rig() -> (Peer, Peer, CancellationToken) {
        let (a, b) = tokio::io::duplex(1024 * 1024);
        let (client_sink, client_source) = Conn::new(a).split();
        let (daemon_sink, daemon_source) = Conn::new(b).split();
        let closed = CancellationToken::new();
        let daemon = spawn(
            Role::Daemon,
            Box::pin(daemon_sink),
            Box::pin(daemon_source),
            Arc::new(server()),
            closed.clone(),
        );
        let client = spawn(
            Role::Client,
            Box::pin(client_sink),
            Box::pin(client_source),
            Arc::new(RejectAll),
            closed.clone(),
        );
        (client, daemon, closed)
    }

    #[tokio::test]
    async fn a_typed_call_decodes_the_result() {
        let (client, _daemon, _closed) = rig();
        let api = Api::new(client.handle.clone());
        let got = api.call::<DaemonStatus>(Empty {}).await.unwrap();
        assert_eq!(got, status());
    }

    #[tokio::test]
    async fn a_typed_error_keeps_its_code_and_disposition() {
        let (client, _daemon, _closed) = rig();
        let api = Api::new(client.handle.clone());
        let err = api.call::<SessionList>(Empty {}).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Denied);
        assert_eq!(err.execution, Some(Execution::NotStarted));
    }

    #[tokio::test]
    async fn a_typed_stream_decodes_items_and_reports_drops() {
        let (client, _daemon, _closed) = rig();
        let api = Api::new(client.handle.clone());
        let mut stream = api
            .open::<SessionWatch>(SessionWatchParams {
                session: SessionId(1),
                from: WatchFrom::Snapshot {},
            })
            .await
            .unwrap();

        let mut seen = Vec::new();
        while let Some(event) = stream.recv().await {
            match event {
                StreamEvent::Item { item, dropped } => seen.push((item.cursor().seq, dropped)),
                StreamEvent::End(result) => {
                    assert!(result.is_ok());
                    break;
                }
            }
        }
        assert_eq!(seen, vec![(0, 0), (1, 1), (2, 0)]);
    }

    #[tokio::test]
    async fn negotiation_refuses_a_method_the_peer_never_offered() {
        let (client, _daemon, _closed) = rig();
        let hello =
            Hello::new(Role::Daemon, "goat-daemon/test").with_method("session.list", vec![1]);
        let api = Api::negotiated(client.handle.clone(), &hello);

        assert!(api.speaks::<SessionList>());
        assert!(!api.speaks::<DaemonStatus>());

        let err = api.call::<DaemonStatus>(Empty {}).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::UnsupportedVersion);
        assert_eq!(err.execution, Some(Execution::NotStarted));
        assert!(err.message.contains("does not offer daemon.status"));
    }

    #[tokio::test]
    async fn negotiation_refuses_a_version_the_peer_does_not_speak() {
        let (client, _daemon, _closed) = rig();
        let hello =
            Hello::new(Role::Daemon, "goat-daemon/test").with_method("daemon.status", vec![2, 3]);
        let api = Api::negotiated(client.handle.clone(), &hello);
        assert!(!api.speaks::<DaemonStatus>());
        let err = api.call::<DaemonStatus>(Empty {}).await.unwrap_err();
        assert!(err.message.contains("not version 1"));
    }

    #[tokio::test]
    async fn an_empty_negotiation_table_assumes_everything_is_offered() {
        let (client, _daemon, _closed) = rig();
        let api = Api::new(client.handle.clone());
        assert!(api.speaks::<DaemonStatus>());
        assert!(api.call::<DaemonStatus>(Empty {}).await.is_ok());
    }

    #[tokio::test]
    async fn a_dropped_connection_surfaces_an_unknown_outcome() {
        let (client, _daemon, closed) = rig();
        let api = Api::new(client.handle.clone());
        closed.cancel();
        let err = api.call::<DaemonStatus>(Empty {}).await.unwrap_err();
        assert!(matches!(
            err.code,
            ErrorCode::HostGone | ErrorCode::Timeout | ErrorCode::UnknownMethod
        ));
        assert!(!matches!(err.execution, Some(Execution::KnownFailed)));
    }

    #[tokio::test]
    async fn cancelling_a_typed_stream_ends_it() {
        let (client, _daemon, _closed) = rig();
        let api = Api::new(client.handle.clone());
        let mut stream = api
            .open::<SessionWatch>(SessionWatchParams {
                session: SessionId(1),
                from: WatchFrom::Snapshot {},
            })
            .await
            .unwrap();
        stream.cancel("done").await;
        let mut terminated = false;
        while let Some(event) = stream.recv().await {
            if let StreamEvent::End(_) = event {
                terminated = true;
                break;
            }
        }
        assert!(terminated);
    }

    #[test]
    fn wire_error_type_is_distinct_from_call_error() {
        let _ = WireError::Closed;
    }
}
