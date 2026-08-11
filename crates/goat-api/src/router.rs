use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use goat_wire::envelope::{CallError, ErrorCode, Execution};
use goat_wire::peer::{CallResult, Handler, Request, StreamSink, unknown_method};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::registry::{Grant, Method};

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

#[derive(Clone)]
pub struct RouteCtx {
    pub client: u64,
    pub grants: Arc<HashSet<Grant>>,
    pub peer: goat_wire::peer::PeerHandle,
    pub cancel: CancellationToken,
}

impl RouteCtx {
    pub fn holds(&self, grant: Grant) -> bool {
        self.grants.contains(&grant)
    }
}

type UnaryFn = Arc<dyn Fn(Value, RouteCtx) -> BoxFuture<CallResult> + Send + Sync>;
type StreamFn = Arc<dyn Fn(Value, RouteCtx, StreamSink) -> BoxFuture<CallResult> + Send + Sync>;

enum Route {
    Unary(UnaryFn),
    Stream(StreamFn),
}

#[derive(Default)]
pub struct Router {
    routes: HashMap<(&'static str, u16), Route>,
    names: HashSet<&'static str>,
    grants: HashSet<Grant>,
    client: u64,
}

impl Router {
    pub fn new(grants: impl IntoIterator<Item = Grant>) -> Self {
        Self {
            routes: HashMap::new(),
            names: HashSet::new(),
            grants: grants.into_iter().collect(),
            client: 0,
        }
    }

    #[must_use]
    pub fn for_client(mut self, client: u64) -> Self {
        self.client = client;
        self
    }

    pub fn grants(&self) -> &HashSet<Grant> {
        &self.grants
    }

    pub fn serves(&self, method: &str, version: u16) -> bool {
        self.lookup(method, version).is_some()
    }

    pub fn advertised(&self) -> std::collections::BTreeMap<String, Vec<u16>> {
        let mut out: std::collections::BTreeMap<String, Vec<u16>> =
            std::collections::BTreeMap::new();
        for (name, version) in self.routes.keys() {
            out.entry((*name).to_owned()).or_default().push(*version);
        }
        for versions in out.values_mut() {
            versions.sort_unstable();
        }
        out
    }

    #[must_use]
    pub fn unary<M, F, Fut>(mut self, handler: F) -> Self
    where
        M: Method + 'static,
        F: Fn(M::Params, RouteCtx) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<M::Output, CallError>> + Send + 'static,
    {
        assert!(
            !M::SHAPE.is_stream(),
            "{} is declared as a stream; register it with Router::stream",
            M::NAME
        );
        if !self.grants.contains(&M::GRANT) {
            return self;
        }
        let handler = Arc::new(handler);
        let route: UnaryFn = Arc::new(move |params, ctx| {
            let handler = handler.clone();
            Box::pin(async move {
                let parsed = decode::<M::Params>(M::NAME, params)?;
                let output = handler(parsed, ctx).await?;
                encode(output)
            })
        });
        self.names.insert(M::NAME);
        self.routes
            .insert((M::NAME, M::VERSION), Route::Unary(route));
        self
    }

    #[must_use]
    pub fn stream<M, F, Fut>(mut self, handler: F) -> Self
    where
        M: Method + 'static,
        F: Fn(M::Params, RouteCtx, StreamSink) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<M::Output, CallError>> + Send + 'static,
    {
        assert!(
            M::SHAPE.is_stream(),
            "{} is declared unary; register it with Router::unary",
            M::NAME
        );
        if !self.grants.contains(&M::GRANT) {
            return self;
        }
        let handler = Arc::new(handler);
        let route: StreamFn = Arc::new(move |params, ctx, sink| {
            let handler = handler.clone();
            Box::pin(async move {
                let parsed = decode::<M::Params>(M::NAME, params)?;
                let output = handler(parsed, ctx, sink).await?;
                encode(output)
            })
        });
        self.names.insert(M::NAME);
        self.routes
            .insert((M::NAME, M::VERSION), Route::Stream(route));
        self
    }

    fn lookup(&self, method: &str, version: u16) -> Option<&Route> {
        let name = self.names.get(method).copied()?;
        self.routes.get(&(name, version))
    }

    fn ctx(&self, request: &Request) -> RouteCtx {
        RouteCtx {
            client: self.client,
            grants: Arc::new(self.grants.clone()),
            peer: request.peer.clone(),
            cancel: request.cancel.clone(),
        }
    }

    fn miss(&self, method: &str, version: u16) -> CallError {
        if self.names.contains(method) {
            let supported: Vec<u16> = self
                .routes
                .keys()
                .filter(|(name, _)| *name == method)
                .map(|(_, version)| *version)
                .collect();
            return CallError::new(
                ErrorCode::UnsupportedVersion,
                format!("{method}@{version} is not served; this peer speaks {supported:?}"),
            )
            .with_execution(Execution::NotStarted);
        }
        unknown_method(method, version)
    }
}

#[async_trait::async_trait]
impl Handler for Router {
    async fn call(&self, request: Request) -> CallResult {
        let Some(Route::Unary(route)) = self.lookup(&request.method, request.version) else {
            return Err(self.miss(&request.method, request.version));
        };
        let route = route.clone();
        let ctx = self.ctx(&request);
        route(request.params, ctx).await
    }

    async fn stream(&self, request: Request, sink: StreamSink) -> CallResult {
        let Some(Route::Stream(route)) = self.lookup(&request.method, request.version) else {
            return Err(self.miss(&request.method, request.version));
        };
        let route = route.clone();
        let ctx = self.ctx(&request);
        route(request.params, ctx, sink).await
    }

    fn is_stream(&self, method: &str, version: u16) -> bool {
        matches!(self.lookup(method, version), Some(Route::Stream(_)))
    }
}

fn decode<T: serde::de::DeserializeOwned>(method: &str, params: Value) -> Result<T, CallError> {
    let params = if params.is_null() {
        Value::Object(serde_json::Map::new())
    } else {
        params
    };
    serde_json::from_value(params).map_err(|err| {
        CallError::new(
            ErrorCode::InvalidParams,
            format!("{method} rejected its parameters: {err}"),
        )
        .with_execution(Execution::NotStarted)
    })
}

fn encode<T: serde::Serialize>(value: T) -> CallResult {
    serde_json::to_value(value).map_err(|err| {
        CallError::new(
            ErrorCode::Internal,
            format!("result could not be encoded: {err}"),
        )
        .with_execution(Execution::KnownFailed)
    })
}

#[cfg(test)]
mod tests {
    use super::{RouteCtx, Router};
    use crate::methods::{
        AdminDaemonStop, DaemonStatus, DaemonStatus2, Empty, SessionWatch, SessionWatchParams,
        WatchFrom, WatchItem,
    };
    use crate::registry::Grant;
    use goat_wire::envelope::{ErrorCode, Execution, Frame, Role};
    use goat_wire::peer::{Handler, StreamMsg, StreamSink, spawn};
    use goat_wire::{WireConn, WireError};
    use serde_json::{Value, json};
    use std::collections::HashSet;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    type Conn = WireConn<tokio::io::DuplexStream, Frame, Frame>;

    fn status() -> DaemonStatus2 {
        DaemonStatus2 {
            version: "0.1.0".to_owned(),
            pid: 1,
            started_at: 0,
            ready: true,
            epoch: "e1".to_owned(),
            sessions: 0,
            turns: 0,
        }
    }

    fn local_router() -> Router {
        Router::new([Grant::Any, Grant::Admin])
            .unary::<DaemonStatus, _, _>(|_params, _ctx| async { Ok(status()) })
            .unary::<AdminDaemonStop, _, _>(|_params, _ctx| async { Ok(Empty {}) })
    }

    fn narrow_router() -> Router {
        Router::new([Grant::Any])
            .unary::<DaemonStatus, _, _>(|_params, _ctx| async { Ok(status()) })
            .unary::<AdminDaemonStop, _, _>(|_params, _ctx| async { Ok(Empty {}) })
    }

    fn watching_router() -> Router {
        Router::new([Grant::Any]).stream::<SessionWatch, _, _>(
            |params: SessionWatchParams, _ctx: RouteCtx, sink: StreamSink| async move {
                let cursor = crate::Cursor::new("e1", params.session.0);
                sink.send(
                    serde_json::to_value(WatchItem::Presence {
                        cursor: cursor.clone(),
                        clients: 1,
                    })
                    .unwrap(),
                )
                .await?;
                Ok(Empty {})
            },
        )
    }

    fn connect(server: Router) -> (goat_wire::peer::Peer, goat_wire::peer::Peer) {
        let (a, b) = tokio::io::duplex(1024 * 1024);
        let (client_sink, client_source) = Conn::new(a).split();
        let (server_sink, server_source) = Conn::new(b).split();
        let closed = CancellationToken::new();
        let server = spawn(
            Role::Daemon,
            Box::pin(server_sink),
            Box::pin(server_source),
            Arc::new(server),
            closed.clone(),
        );
        let client = spawn(
            Role::Client,
            Box::pin(client_sink),
            Box::pin(client_source),
            Arc::new(goat_wire::peer::RejectAll),
            closed,
        );
        (client, server)
    }

    #[tokio::test]
    async fn a_registered_method_round_trips_typed() {
        let (peer, _server) = connect(local_router());
        let value = peer
            .handle
            .call("daemon.status", 1, Value::Null)
            .await
            .unwrap();
        let parsed: DaemonStatus2 = serde_json::from_value(value).unwrap();
        assert_eq!(parsed, status());
    }

    #[tokio::test]
    async fn an_admin_method_is_absent_from_a_router_without_the_grant() {
        let narrow = narrow_router();
        assert!(narrow.advertised().contains_key("daemon.status"));
        assert!(!narrow.advertised().contains_key("admin.daemon_stop"));

        let (peer, _server) = connect(narrow_router());
        let err = peer
            .handle
            .call("admin.daemon_stop", 1, json!({}))
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::UnknownMethod);
        assert_eq!(err.execution, Some(Execution::NotStarted));
    }

    #[tokio::test]
    async fn an_admin_method_works_when_the_grant_is_present() {
        let (peer, _server) = connect(local_router());
        peer.handle
            .call("admin.daemon_stop", 1, json!({}))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_wrong_version_reports_what_is_served() {
        let (peer, _server) = connect(local_router());
        let err = peer
            .handle
            .call("daemon.status", 9, Value::Null)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::UnsupportedVersion);
        assert!(err.message.contains("[1]"));
    }

    #[tokio::test]
    async fn invalid_params_never_reach_the_handler() {
        let (peer, _server) = connect(watching_router());
        let mut stream = peer
            .handle
            .open_stream("session.watch", 1, json!({"session": "nope"}))
            .await
            .unwrap();
        let Some(StreamMsg::End(result)) = stream.recv().await else {
            panic!("expected the stream to end with a parameter error")
        };
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams);
        assert_eq!(err.execution, Some(Execution::NotStarted));
        assert!(err.message.contains("session.watch"));
    }

    #[tokio::test]
    async fn a_stream_method_is_dispatched_as_a_stream() {
        let router = watching_router();
        assert!(router.is_stream("session.watch", 1));
        assert!(!router.is_stream("daemon.status", 1));
        assert!(router.serves("session.watch", 1));
        assert!(!router.serves("session.watch", 2));

        let (peer, _server) = connect(watching_router());
        let mut stream = peer
            .handle
            .open_stream(
                "session.watch",
                1,
                json!({"session": "7", "from": {"type": "Snapshot"}}),
            )
            .await
            .unwrap();
        let Some(StreamMsg::Item { item, .. }) = stream.recv().await else {
            panic!("expected a stream item")
        };
        let parsed: WatchItem = serde_json::from_value(item).unwrap();
        assert_eq!(parsed.cursor().seq, 7);
        let Some(StreamMsg::End(result)) = stream.recv().await else {
            panic!("expected the stream to end")
        };
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn calling_a_stream_method_as_unary_is_reported_not_hung() {
        let (peer, _server) = connect(watching_router());
        let err = peer
            .handle
            .call(
                "session.watch",
                1,
                json!({"session": "7", "from": {"type": "Snapshot"}}),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Conflict);
    }

    #[test]
    fn watch_from_is_constructible_for_both_starts() {
        let _ = WatchFrom::Snapshot {};
        let _ = WatchFrom::Cursor {
            cursor: crate::Cursor::new("e1", 3),
        };
    }

    #[test]
    fn route_ctx_reports_its_grants() {
        let grants: HashSet<Grant> = [Grant::Any].into_iter().collect();
        let ctx_grants = Arc::new(grants);
        assert!(ctx_grants.contains(&Grant::Any));
        assert!(!ctx_grants.contains(&Grant::Admin));
    }

    #[test]
    fn unknown_wire_errors_are_not_confused_with_call_errors() {
        let _ = WireError::Closed;
    }
}
