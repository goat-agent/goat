use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::{Sink, SinkExt, Stream, StreamExt};
use serde_json::Value;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::envelope::{CallError, ErrorCode, Execution, Frame, Id, IdAllocator, Outcome, Role};

pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(90);

const CONTROL_CAPACITY: usize = 256;
const DATA_CAPACITY: usize = 256;
const REQUEST_CAPACITY: usize = 64;
const STREAM_CAPACITY: usize = 256;

pub type CallResult = Result<Value, CallError>;

#[derive(Debug, Clone, PartialEq)]
pub enum StreamMsg {
    Item { item: Value, dropped: u64 },
    End(Result<Value, CallError>),
}

pub struct Request {
    pub method: String,
    pub version: u16,
    pub params: Value,
    pub peer: PeerHandle,
    pub cancel: CancellationToken,
}

pub struct StreamSink {
    id: Id,
    data: mpsc::Sender<Frame>,
}

impl StreamSink {
    pub async fn send(&self, item: Value) -> Result<(), CallError> {
        self.send_dropping(item, 0).await
    }

    pub async fn send_dropping(&self, item: Value, dropped: u64) -> Result<(), CallError> {
        self.data
            .send(Frame::data_after_drop(self.id, item, dropped))
            .await
            .map_err(|_| {
                CallError::new(ErrorCode::HostGone, "peer disconnected")
                    .with_execution(Execution::OutcomeUnknown)
            })
    }
}

#[async_trait::async_trait]
pub trait Handler: Send + Sync + 'static {
    async fn call(&self, request: Request) -> CallResult;

    async fn stream(&self, request: Request, sink: StreamSink) -> CallResult {
        let _ = sink;
        Err(unknown_method(&request.method, request.version))
    }

    fn is_stream(&self, method: &str, version: u16) -> bool {
        let _ = (method, version);
        false
    }
}

pub struct RejectAll;

#[async_trait::async_trait]
impl Handler for RejectAll {
    async fn call(&self, request: Request) -> CallResult {
        Err(unknown_method(&request.method, request.version))
    }
}

pub fn unknown_method(method: &str, version: u16) -> CallError {
    CallError::new(
        ErrorCode::UnknownMethod,
        format!("this peer does not serve {method}@{version}"),
    )
    .with_execution(Execution::NotStarted)
}

fn shape_mismatch() -> CallError {
    CallError::new(
        ErrorCode::Conflict,
        "this method answers with a stream; open it as a stream instead of a unary call",
    )
    .with_execution(Execution::OutcomeUnknown)
}

enum Pending {
    Unary(oneshot::Sender<Outcome>),
    Stream(mpsc::Sender<StreamMsg>),
}

enum DataTarget {
    Stream(mpsc::Sender<StreamMsg>),
    Mismatch(oneshot::Sender<Outcome>),
    Absent,
}

struct Shared {
    control: mpsc::Sender<Frame>,
    data: mpsc::Sender<Frame>,
    requests: mpsc::Sender<Frame>,
    pending: Mutex<HashMap<Id, Pending>>,
    alloc: Mutex<IdAllocator>,
    inbound: Mutex<HashMap<Id, CancellationToken>>,
}

#[derive(Clone)]
pub struct PeerHandle {
    shared: Arc<Shared>,
}

pub struct StreamHandle {
    id: Id,
    rx: mpsc::Receiver<StreamMsg>,
    shared: Arc<Shared>,
    finished: bool,
}

impl StreamHandle {
    pub async fn recv(&mut self) -> Option<StreamMsg> {
        let msg = self.rx.recv().await;
        match &msg {
            Some(StreamMsg::End(_)) | None => self.finished = true,
            Some(StreamMsg::Item { .. }) => {}
        }
        msg
    }

    pub async fn cancel(&self, reason: impl Into<String>) {
        let _ = self
            .shared
            .control
            .send(Frame::cancel(self.id, Some(reason.into())))
            .await;
    }

    pub fn id(&self) -> Id {
        self.id
    }
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self
                .shared
                .control
                .try_send(Frame::cancel(self.id, Some("dropped".to_owned())));
        }
        let shared = self.shared.clone();
        let id = self.id;
        tokio::spawn(async move {
            shared.pending.lock().await.remove(&id);
        });
    }
}

impl PeerHandle {
    pub async fn call(&self, method: &str, version: u16, params: Value) -> CallResult {
        self.call_with_deadline(method, version, params, DEFAULT_DEADLINE)
            .await
    }

    pub async fn call_with_deadline(
        &self,
        method: &str,
        version: u16,
        params: Value,
        deadline: Duration,
    ) -> CallResult {
        let (id, rx) = self.register_unary().await;
        if self
            .shared
            .requests
            .send(Frame::req(id, method, version, params))
            .await
            .is_err()
        {
            self.shared.pending.lock().await.remove(&id);
            return Err(CallError::new(ErrorCode::HostGone, "peer disconnected")
                .with_execution(Execution::NotStarted));
        }
        match tokio::time::timeout(deadline, rx).await {
            Ok(Ok(outcome)) => outcome.into_result(),
            Ok(Err(_)) => Err(CallError::new(ErrorCode::HostGone, "peer disconnected")
                .with_execution(Execution::OutcomeUnknown)),
            Err(_) => {
                self.shared.pending.lock().await.remove(&id);
                let _ = self
                    .shared
                    .control
                    .send(Frame::cancel(id, Some("deadline exceeded".to_owned())))
                    .await;
                Err(CallError::new(
                    ErrorCode::Timeout,
                    format!("{method}@{version} exceeded {deadline:?}"),
                )
                .with_execution(Execution::OutcomeUnknown))
            }
        }
    }

    pub async fn open_stream(
        &self,
        method: &str,
        version: u16,
        params: Value,
    ) -> Result<StreamHandle, CallError> {
        let (id, rx) = self.register_stream().await;
        if self
            .shared
            .requests
            .send(Frame::req(id, method, version, params))
            .await
            .is_err()
        {
            self.shared.pending.lock().await.remove(&id);
            return Err(CallError::new(ErrorCode::HostGone, "peer disconnected")
                .with_execution(Execution::NotStarted));
        }
        Ok(StreamHandle {
            id,
            rx,
            shared: self.shared.clone(),
            finished: false,
        })
    }

    async fn register_unary(&self) -> (Id, oneshot::Receiver<Outcome>) {
        let id = self.shared.alloc.lock().await.allocate();
        let (tx, rx) = oneshot::channel();
        self.shared
            .pending
            .lock()
            .await
            .insert(id, Pending::Unary(tx));
        (id, rx)
    }

    async fn register_stream(&self) -> (Id, mpsc::Receiver<StreamMsg>) {
        let id = self.shared.alloc.lock().await.allocate();
        let (tx, rx) = mpsc::channel(STREAM_CAPACITY);
        self.shared
            .pending
            .lock()
            .await
            .insert(id, Pending::Stream(tx));
        (id, rx)
    }

    pub async fn send_hello(&self, hello: crate::envelope::Hello) -> Result<(), CallError> {
        self.shared
            .control
            .send(Frame::Hello(hello))
            .await
            .map_err(|_| {
                CallError::new(ErrorCode::HostGone, "peer disconnected")
                    .with_execution(Execution::NotStarted)
            })
    }
}

pub struct Peer {
    pub handle: PeerHandle,
    pub reader: JoinHandle<()>,
    pub writer: JoinHandle<()>,
    pub hello: mpsc::Receiver<crate::envelope::Hello>,
}

pub fn spawn<Tx, Rx>(
    role: Role,
    sink: Tx,
    source: Rx,
    handler: Arc<dyn Handler>,
    closed: CancellationToken,
) -> Peer
where
    Tx: Sink<Frame> + Unpin + Send + 'static,
    Rx: Stream<Item = Result<Frame, crate::WireError>> + Unpin + Send + 'static,
{
    let (control_tx, control_rx) = mpsc::channel::<Frame>(CONTROL_CAPACITY);
    let (data_tx, data_rx) = mpsc::channel::<Frame>(DATA_CAPACITY);
    let (request_tx, request_rx) = mpsc::channel::<Frame>(REQUEST_CAPACITY);
    let (hello_tx, hello_rx) = mpsc::channel::<crate::envelope::Hello>(4);

    let shared = Arc::new(Shared {
        control: control_tx,
        data: data_tx,
        requests: request_tx,
        pending: Mutex::new(HashMap::new()),
        alloc: Mutex::new(IdAllocator::for_role(role)),
        inbound: Mutex::new(HashMap::new()),
    });

    let writer = tokio::spawn(write_loop(
        sink,
        control_rx,
        data_rx,
        request_rx,
        closed.clone(),
    ));
    let handle = PeerHandle {
        shared: shared.clone(),
    };
    let reader = tokio::spawn(read_loop(
        source,
        shared,
        handler,
        handle.clone(),
        hello_tx,
        closed,
    ));

    Peer {
        handle,
        reader,
        writer,
        hello: hello_rx,
    }
}

async fn write_loop<Tx>(
    mut sink: Tx,
    mut control: mpsc::Receiver<Frame>,
    mut data: mpsc::Receiver<Frame>,
    mut requests: mpsc::Receiver<Frame>,
    closed: CancellationToken,
) where
    Tx: Sink<Frame> + Unpin + Send + 'static,
{
    loop {
        let frame = tokio::select! {
            biased;
            () = closed.cancelled() => break,
            Some(frame) = control.recv() => frame,
            Some(frame) = data.recv() => frame,
            Some(frame) = requests.recv() => frame,
            else => break,
        };
        if sink.send(frame).await.is_err() {
            break;
        }
    }
    closed.cancel();
    let _ = sink.close().await;
}

async fn read_loop<Rx>(
    mut source: Rx,
    shared: Arc<Shared>,
    handler: Arc<dyn Handler>,
    handle: PeerHandle,
    hello_tx: mpsc::Sender<crate::envelope::Hello>,
    closed: CancellationToken,
) where
    Rx: Stream<Item = Result<Frame, crate::WireError>> + Unpin + Send + 'static,
{
    loop {
        let next = tokio::select! {
            biased;
            () = closed.cancelled() => None,
            item = source.next() => item,
        };
        let Some(Ok(frame)) = next else { break };
        match frame {
            Frame::Hello(hello) => {
                let _ = hello_tx.try_send(hello);
            }
            Frame::Req {
                id,
                method,
                version,
                params,
            } => {
                serve_inbound(&shared, &handler, &handle, id, method, version, params);
            }
            Frame::Res { id, outcome } | Frame::End { id, outcome } => {
                let pending = shared.pending.lock().await.remove(&id);
                match pending {
                    Some(Pending::Unary(tx)) => {
                        let _ = tx.send(outcome);
                    }
                    Some(Pending::Stream(tx)) => {
                        let _ = tx.send(StreamMsg::End(outcome.into_result())).await;
                    }
                    None => {}
                }
            }
            Frame::Data { id, item, dropped } => {
                let target = {
                    let mut pending = shared.pending.lock().await;
                    match pending.get(&id) {
                        Some(Pending::Stream(tx)) => DataTarget::Stream(tx.clone()),
                        Some(Pending::Unary(_)) => match pending.remove(&id) {
                            Some(Pending::Unary(tx)) => DataTarget::Mismatch(tx),
                            _ => DataTarget::Absent,
                        },
                        None => DataTarget::Absent,
                    }
                };
                match target {
                    DataTarget::Stream(tx) => {
                        let _ = tx.send(StreamMsg::Item { item, dropped }).await;
                    }
                    DataTarget::Mismatch(tx) => {
                        let _ = tx.send(Outcome::error(shape_mismatch()));
                    }
                    DataTarget::Absent => {}
                }
            }
            Frame::Cancel { id, .. } => {
                let token = shared.inbound.lock().await.remove(&id);
                if let Some(token) = token {
                    token.cancel();
                }
            }
        }
    }
    closed.cancel();
    fail_all_pending(&shared).await;
}

fn serve_inbound(
    shared: &Arc<Shared>,
    handler: &Arc<dyn Handler>,
    handle: &PeerHandle,
    id: Id,
    method: String,
    version: u16,
    params: Value,
) {
    let shared = shared.clone();
    let handler = handler.clone();
    let handle = handle.clone();
    tokio::spawn(async move {
        let token = CancellationToken::new();
        shared.inbound.lock().await.insert(id, token.clone());
        let streaming = handler.is_stream(&method, version);
        let request = Request {
            method,
            version,
            params,
            peer: handle,
            cancel: token,
        };
        let (frame, lane) = if streaming {
            let sink = StreamSink {
                id,
                data: shared.data.clone(),
            };
            let frame = match handler.stream(request, sink).await {
                Ok(value) => Frame::end(id, Outcome::ok(value)),
                Err(error) => Frame::end(id, Outcome::error(error)),
            };
            (frame, &shared.data)
        } else {
            let frame = match handler.call(request).await {
                Ok(value) => Frame::res(id, Outcome::ok(value)),
                Err(error) => Frame::res(id, Outcome::error(error)),
            };
            (frame, &shared.control)
        };
        shared.inbound.lock().await.remove(&id);
        let _ = lane.send(frame).await;
    });
}

async fn fail_all_pending(shared: &Arc<Shared>) {
    let drained: Vec<(Id, Pending)> = shared.pending.lock().await.drain().collect();
    for (_, pending) in drained {
        let error = CallError::new(ErrorCode::HostGone, "peer disconnected")
            .with_execution(Execution::OutcomeUnknown);
        match pending {
            Pending::Unary(tx) => {
                let _ = tx.send(Outcome::error(error));
            }
            Pending::Stream(tx) => {
                let _ = tx.send(StreamMsg::End(Err(error))).await;
            }
        }
    }
    let tokens: Vec<CancellationToken> = shared
        .inbound
        .lock()
        .await
        .drain()
        .map(|(_, t)| t)
        .collect();
    for token in tokens {
        token.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CallResult, Handler, Peer, PeerHandle, Request, StreamMsg, StreamSink, spawn,
        unknown_method,
    };
    use crate::envelope::{CallError, ErrorCode, Execution, Frame, Role};
    use crate::{WireConn, WireError};
    use serde_json::{Value, json};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::sync::{Mutex, mpsc, oneshot};
    use tokio_util::sync::CancellationToken;

    type Conn = WireConn<tokio::io::DuplexStream, Frame, Frame>;

    fn split(
        conn: Conn,
    ) -> (
        impl futures::Sink<Frame, Error = WireError>,
        impl futures::Stream<Item = Result<Frame, WireError>>,
    ) {
        conn.split()
    }

    fn pair(
        client_handler: Arc<dyn Handler>,
        daemon_handler: Arc<dyn Handler>,
    ) -> (Peer, Peer, CancellationToken, CancellationToken) {
        let (a, b) = tokio::io::duplex(1024 * 1024);
        let (client_sink, client_source) = split(Conn::new(a));
        let (daemon_sink, daemon_source) = split(Conn::new(b));
        let client_closed = CancellationToken::new();
        let daemon_closed = CancellationToken::new();
        let client = spawn(
            Role::Client,
            Box::pin(client_sink),
            Box::pin(client_source),
            client_handler,
            client_closed.clone(),
        );
        let daemon = spawn(
            Role::Daemon,
            Box::pin(daemon_sink),
            Box::pin(daemon_source),
            daemon_handler,
            daemon_closed.clone(),
        );
        (client, daemon, client_closed, daemon_closed)
    }

    struct Echo;

    #[async_trait::async_trait]
    impl Handler for Echo {
        async fn call(&self, request: Request) -> CallResult {
            match request.method.as_str() {
                "echo" => Ok(request.params),
                "fail" => {
                    Err(CallError::new(ErrorCode::Denied, "nope")
                        .with_execution(Execution::NotStarted))
                }
                other => Err(unknown_method(other, request.version)),
            }
        }
    }

    struct Never {
        entered: mpsc::UnboundedSender<String>,
        cancelled: mpsc::UnboundedSender<String>,
    }

    impl Never {
        fn new() -> (
            Arc<Self>,
            mpsc::UnboundedReceiver<String>,
            mpsc::UnboundedReceiver<String>,
        ) {
            let (entered, entered_rx) = mpsc::unbounded_channel();
            let (cancelled, cancelled_rx) = mpsc::unbounded_channel();
            (
                Arc::new(Self { entered, cancelled }),
                entered_rx,
                cancelled_rx,
            )
        }

        async fn hold(&self, request: &Request) {
            let _ = self.entered.send(request.method.clone());
            request.cancel.cancelled().await;
            let _ = self.cancelled.send(request.method.clone());
        }
    }

    #[async_trait::async_trait]
    impl Handler for Never {
        async fn call(&self, request: Request) -> CallResult {
            self.hold(&request).await;
            Err(CallError::new(ErrorCode::Canceled, "cancelled")
                .with_execution(Execution::OutcomeUnknown))
        }

        async fn stream(&self, request: Request, _sink: StreamSink) -> CallResult {
            self.hold(&request).await;
            Err(CallError::new(ErrorCode::Canceled, "cancelled")
                .with_execution(Execution::KnownFailed))
        }

        fn is_stream(&self, method: &str, _version: u16) -> bool {
            method == "hangstream"
        }
    }

    struct Gated {
        release: Mutex<Option<oneshot::Receiver<()>>>,
        started: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl Handler for Gated {
        async fn call(&self, request: Request) -> CallResult {
            self.started.fetch_add(1, Ordering::SeqCst);
            if request.method == "slow"
                && let Some(rx) = self.release.lock().await.take()
            {
                let _ = rx.await;
            }
            Ok(json!({"method": request.method}))
        }
    }

    struct Nested;

    #[async_trait::async_trait]
    impl Handler for Nested {
        async fn call(&self, request: Request) -> CallResult {
            if request.method == "outer" {
                let inner = request.peer.call("inner", 1, json!({"n": 1})).await?;
                return Ok(json!({"from_peer": inner}));
            }
            Ok(json!({"inner": true}))
        }
    }

    struct Flood;

    #[async_trait::async_trait]
    impl Handler for Flood {
        async fn call(&self, _request: Request) -> CallResult {
            Ok(json!("pong"))
        }

        async fn stream(&self, _request: Request, sink: StreamSink) -> CallResult {
            for i in 0..10_000u64 {
                sink.send(json!({ "i": i })).await?;
            }
            Ok(Value::Null)
        }

        fn is_stream(&self, method: &str, _version: u16) -> bool {
            method == "flood"
        }
    }

    struct Ticker;

    #[async_trait::async_trait]
    impl Handler for Ticker {
        async fn call(&self, request: Request) -> CallResult {
            Err(unknown_method(&request.method, request.version))
        }

        async fn stream(&self, request: Request, sink: StreamSink) -> CallResult {
            let count = request
                .params
                .get("count")
                .and_then(Value::as_u64)
                .unwrap_or(3);
            for i in 0..count {
                if request.cancel.is_cancelled() {
                    return Err(CallError::new(ErrorCode::Canceled, "cancelled")
                        .with_execution(Execution::KnownFailed));
                }
                let dropped = u64::from(i == 1);
                sink.send_dropping(json!({"i": i}), dropped).await?;
            }
            Ok(Value::Null)
        }

        fn is_stream(&self, method: &str, _version: u16) -> bool {
            method == "tick"
        }
    }

    fn drain_hello(peer: &mut Peer) {
        while peer.hello.try_recv().is_ok() {}
    }

    #[tokio::test]
    async fn unary_call_round_trips() {
        let (client, _daemon, _c, _d) = pair(Arc::new(Echo), Arc::new(Echo));
        let got = client
            .handle
            .call("echo", 1, json!({"hi": true}))
            .await
            .unwrap();
        assert_eq!(got, json!({"hi": true}));
    }

    #[tokio::test]
    async fn typed_error_survives_the_wire() {
        let (client, _daemon, _c, _d) = pair(Arc::new(Echo), Arc::new(Echo));
        let err = client
            .handle
            .call("fail", 1, Value::Null)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Denied);
        assert!(err.retry_is_safe());
    }

    #[tokio::test]
    async fn unknown_method_is_reported_not_hung() {
        let (client, _daemon, _c, _d) = pair(Arc::new(Echo), Arc::new(Echo));
        let err = client
            .handle
            .call("nope", 7, Value::Null)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::UnknownMethod);
        assert!(err.message.contains("nope@7"));
    }

    #[tokio::test]
    async fn concurrent_calls_resolve_out_of_order() {
        let (release_tx, release_rx) = oneshot::channel();
        let started = Arc::new(AtomicUsize::new(0));
        let handler = Arc::new(Gated {
            release: Mutex::new(Some(release_rx)),
            started: started.clone(),
        });
        let (client, _daemon, _c, _d) = pair(Arc::new(Echo), handler);

        let slow = {
            let handle = client.handle.clone();
            tokio::spawn(async move { handle.call("slow", 1, Value::Null).await })
        };
        let fast = client.handle.call("fast", 1, Value::Null).await.unwrap();
        assert_eq!(fast, json!({"method": "fast"}));
        assert_eq!(started.load(Ordering::SeqCst), 2);

        release_tx.send(()).unwrap();
        let slow = slow.await.unwrap().unwrap();
        assert_eq!(slow, json!({"method": "slow"}));
    }

    #[tokio::test]
    async fn nested_reverse_call_does_not_deadlock() {
        let (client, _daemon, _c, _d) = pair(Arc::new(Nested), Arc::new(Nested));
        let got = client.handle.call("outer", 1, Value::Null).await.unwrap();
        assert_eq!(got, json!({"from_peer": {"inner": true}}));
    }

    #[tokio::test]
    async fn daemon_can_originate_a_call_to_the_client() {
        let (_client, daemon, _c, _d) = pair(Arc::new(Echo), Arc::new(Echo));
        let got = daemon
            .handle
            .call("echo", 1, json!({"reverse": true}))
            .await
            .unwrap();
        assert_eq!(got, json!({"reverse": true}));
    }

    #[tokio::test(start_paused = true)]
    async fn deadline_reports_outcome_unknown_and_cancels_the_responder() {
        let (never, mut entered, mut cancelled) = Never::new();
        let (client, _daemon, _c, _d) = pair(Arc::new(Echo), never);
        let call = {
            let handle = client.handle.clone();
            tokio::spawn(async move {
                handle
                    .call_with_deadline("hang", 1, Value::Null, Duration::from_secs(5))
                    .await
            })
        };
        assert_eq!(entered.recv().await.as_deref(), Some("hang"));
        tokio::time::advance(Duration::from_secs(6)).await;
        let err = call.await.unwrap().unwrap_err();
        assert_eq!(err.code, ErrorCode::Timeout);
        assert_eq!(err.execution, Some(Execution::OutcomeUnknown));
        assert!(!err.retry_is_safe());
        assert_eq!(cancelled.recv().await.as_deref(), Some("hang"));
    }

    #[tokio::test]
    async fn disconnect_fails_every_pending_call_as_outcome_unknown() {
        let (never, mut entered, _cancelled) = Never::new();
        let (client, _daemon, _c, daemon_closed) = pair(Arc::new(Echo), never);
        let call = {
            let handle = client.handle.clone();
            tokio::spawn(async move { handle.call("hang", 1, Value::Null).await })
        };
        assert_eq!(entered.recv().await.as_deref(), Some("hang"));
        daemon_closed.cancel();
        let err = call.await.unwrap().unwrap_err();
        assert_eq!(err.code, ErrorCode::HostGone);
        assert_eq!(err.execution, Some(Execution::OutcomeUnknown));
    }

    #[tokio::test]
    async fn stream_delivers_items_then_end_with_dropped_counts() {
        let (client, _daemon, _c, _d) = pair(Arc::new(Echo), Arc::new(Ticker));
        let mut stream = client
            .handle
            .open_stream("tick", 1, json!({"count": 3}))
            .await
            .unwrap();
        let mut items = Vec::new();
        let mut ended = false;
        while let Some(msg) = stream.recv().await {
            match msg {
                StreamMsg::Item { item, dropped } => items.push((item, dropped)),
                StreamMsg::End(result) => {
                    assert!(result.is_ok());
                    ended = true;
                    break;
                }
            }
        }
        assert!(ended);
        assert_eq!(
            items,
            vec![
                (json!({"i": 0}), 0),
                (json!({"i": 1}), 1),
                (json!({"i": 2}), 0),
            ]
        );
    }

    #[tokio::test]
    async fn cancelling_a_stream_reaches_the_responder_and_ends_it() {
        let (never, mut entered, mut cancelled) = Never::new();
        let (client, _daemon, _c, _d) = pair(Arc::new(Echo), never);
        let mut stream = client
            .handle
            .open_stream("hangstream", 1, Value::Null)
            .await
            .unwrap();
        assert!(stream.id().is_client_originated());
        assert_eq!(entered.recv().await.as_deref(), Some("hangstream"));
        stream.cancel("test").await;
        assert_eq!(cancelled.recv().await.as_deref(), Some("hangstream"));
        let Some(StreamMsg::End(result)) = stream.recv().await else {
            panic!("expected a terminal end frame after cancel")
        };
        assert_eq!(result.unwrap_err().code, ErrorCode::Canceled);
    }

    #[tokio::test]
    async fn dropping_a_live_stream_cancels_the_responder() {
        let (never, mut entered, mut cancelled) = Never::new();
        let (client, _daemon, _c, _d) = pair(Arc::new(Echo), never);
        let stream = client
            .handle
            .open_stream("hangstream", 1, Value::Null)
            .await
            .unwrap();
        assert_eq!(entered.recv().await.as_deref(), Some("hangstream"));
        drop(stream);
        assert_eq!(cancelled.recv().await.as_deref(), Some("hangstream"));
    }

    #[tokio::test]
    async fn calling_a_stream_method_as_unary_fails_instead_of_hanging() {
        let (client, _daemon, _c, _d) = pair(Arc::new(Echo), Arc::new(Ticker));
        let err = client
            .handle
            .call("tick", 1, json!({"count": 2}))
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Conflict);
        assert!(!err.retry_is_safe());
    }

    #[tokio::test]
    async fn opening_a_unary_method_as_a_stream_ends_immediately() {
        let (client, _daemon, _c, _d) = pair(Arc::new(Echo), Arc::new(Echo));
        let mut stream = client
            .handle
            .open_stream("echo", 1, json!({"hi": true}))
            .await
            .unwrap();
        let Some(StreamMsg::End(result)) = stream.recv().await else {
            panic!("expected the unary response to terminate the stream")
        };
        assert_eq!(result.unwrap(), json!({"hi": true}));
    }

    #[tokio::test]
    async fn a_flooding_stream_never_starves_a_concurrent_call() {
        let (client, _daemon, _c, _d) = pair(Arc::new(Echo), Arc::new(Flood));
        let mut stream = client
            .handle
            .open_stream("flood", 1, Value::Null)
            .await
            .unwrap();
        let Some(StreamMsg::Item { .. }) = stream.recv().await else {
            panic!("expected the flood to start")
        };
        let pong = tokio::time::timeout(
            Duration::from_secs(10),
            client.handle.call("ping", 1, Value::Null),
        )
        .await
        .expect("a saturated stream must not starve an unrelated call")
        .unwrap();
        assert_eq!(pong, json!("pong"));
    }

    #[tokio::test]
    async fn hello_is_delivered_out_of_band() {
        let (client, mut daemon, _c, _d) = pair(Arc::new(Echo), Arc::new(Echo));
        drain_hello(&mut daemon);
        let hello = crate::envelope::Hello::new(Role::Client, "test/1.0");
        client.handle.send_hello(hello.clone()).await.unwrap();
        let got = daemon.hello.recv().await.unwrap();
        assert_eq!(got, hello);
    }

    fn _assert_handle_is_send_sync() {
        fn require<T: Send + Sync>() {}
        require::<PeerHandle>();
    }
}
