pub mod native;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use goat_api::{CapabilityAdvertise, CapabilityAdvertiseParams, CapabilityOffer, Empty};
use goat_wire::envelope::{CallError, ErrorCode, Execution};
use goat_wire::peer::{CallResult, Handler, Request, unknown_method};
use serde_json::{Value, json};
use tokio::sync::{Mutex, oneshot};

pub const CAPABILITY: &str = "host.browser";
pub const CAPABILITY_VERSION: u16 = 1;
pub const MAX_IN_FLIGHT: usize = 8;

#[async_trait::async_trait]
pub trait NativePort: Send + Sync {
    async fn emit(&self, body: &Value) -> Result<(), String>;
}

pub struct BrowserHost {
    port: Arc<dyn NativePort>,
    next: AtomicU64,
    waiting: Mutex<std::collections::HashMap<u64, oneshot::Sender<Result<Value, CallError>>>>,
    deadline: Duration,
}

impl BrowserHost {
    #[must_use]
    pub fn new(port: Arc<dyn NativePort>) -> Self {
        Self::with_deadline(port, Duration::from_secs(90))
    }

    #[must_use]
    pub fn with_deadline(port: Arc<dyn NativePort>, deadline: Duration) -> Self {
        Self {
            port,
            next: AtomicU64::new(0),
            waiting: Mutex::new(std::collections::HashMap::new()),
            deadline,
        }
    }

    pub async fn settle(&self, request_id: u64, result: Result<Value, CallError>) -> bool {
        let sender = self.waiting.lock().await.remove(&request_id);
        match sender {
            Some(sender) => sender.send(result).is_ok(),
            None => false,
        }
    }

    pub async fn fail_all(&self, reason: &str) {
        let drained: Vec<oneshot::Sender<Result<Value, CallError>>> = {
            let mut waiting = self.waiting.lock().await;
            waiting.drain().map(|(_, sender)| sender).collect()
        };
        for sender in drained {
            let _ = sender.send(Err(CallError::new(ErrorCode::HostGone, reason.to_owned())
                .with_execution(Execution::OutcomeUnknown)));
        }
    }

    pub async fn in_flight(&self) -> usize {
        self.waiting.lock().await.len()
    }

    async fn drive(
        &self,
        params: Value,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> CallResult {
        let request_id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = oneshot::channel();
        self.waiting.lock().await.insert(request_id, tx);

        let body = json!({
            "type": "browser.request",
            "request_id": request_id.to_string(),
            "params": params,
        });
        if let Err(reason) = self.port.emit(&body).await {
            self.waiting.lock().await.remove(&request_id);
            return Err(
                CallError::new(ErrorCode::HostGone, format!("browser port: {reason}"))
                    .with_execution(Execution::NotStarted),
            );
        }

        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                self.waiting.lock().await.remove(&request_id);
                Err(CallError::new(ErrorCode::Canceled, "the turn was interrupted")
                    .with_execution(Execution::OutcomeUnknown))
            }
            settled = tokio::time::timeout(self.deadline, rx) => match settled {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => Err(CallError::new(ErrorCode::HostGone, "the browser port closed")
                    .with_execution(Execution::OutcomeUnknown)),
                Err(_) => {
                    self.waiting.lock().await.remove(&request_id);
                    Err(CallError::new(
                        ErrorCode::Timeout,
                        "the browser did not answer in time; its action may have completed",
                    )
                    .with_execution(Execution::OutcomeUnknown))
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl Handler for BrowserHost {
    async fn call(&self, request: Request) -> CallResult {
        if request.method != CAPABILITY {
            return Err(unknown_method(&request.method, request.version));
        }
        self.drive(request.params, &request.cancel).await
    }
}

#[must_use]
pub fn advertisement(
    instance: String,
    label: String,
    boot_epoch: u64,
) -> CapabilityAdvertiseParams {
    CapabilityAdvertiseParams {
        instance,
        label,
        boot_epoch,
        offers: vec![CapabilityOffer {
            id: CAPABILITY.to_owned(),
            version: CAPABILITY_VERSION,
            max_in_flight: MAX_IN_FLIGHT,
        }],
    }
}

#[must_use]
pub fn withdrawal(instance: String, boot_epoch: u64) -> CapabilityAdvertiseParams {
    CapabilityAdvertiseParams {
        instance,
        label: String::new(),
        boot_epoch,
        offers: Vec::new(),
    }
}

pub async fn advertise(
    api: &goat_api::Api,
    params: CapabilityAdvertiseParams,
) -> Result<Empty, CallError> {
    api.call::<CapabilityAdvertise>(params).await
}

#[cfg(test)]
mod tests {
    use super::{BrowserHost, CAPABILITY, NativePort, advertisement, withdrawal};
    use goat_wire::envelope::{CallError, ErrorCode, Execution};
    use goat_wire::peer::{Handler, RejectAll, Request, spawn};
    use goat_wire::{WireConn, envelope::Frame, envelope::Role};
    use serde_json::{Value, json};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    struct Recorder {
        sent: mpsc::UnboundedSender<(u64, Value)>,
        fail: bool,
    }

    impl Recorder {
        fn new(fail: bool) -> (Arc<Self>, mpsc::UnboundedReceiver<(u64, Value)>) {
            let (sent, rx) = mpsc::unbounded_channel();
            (Arc::new(Self { sent, fail }), rx)
        }
    }

    #[async_trait::async_trait]
    impl NativePort for Recorder {
        async fn emit(&self, body: &Value) -> Result<(), String> {
            if self.fail {
                return Err("port closed".to_owned());
            }
            let request_id = body["request_id"]
                .as_str()
                .and_then(|text| text.parse::<u64>().ok())
                .ok_or("a request carries a numeric id")?;
            let _ = self.sent.send((request_id, body["params"].clone()));
            Ok(())
        }
    }

    type Conn = WireConn<tokio::io::DuplexStream, Frame, Frame>;

    fn idle_peer() -> goat_wire::peer::Peer {
        let (a, _b) = tokio::io::duplex(4096);
        let (sink, source) = Conn::new(a).split();
        spawn(
            Role::Client,
            Box::pin(sink),
            Box::pin(source),
            Arc::new(RejectAll),
            CancellationToken::new(),
        )
    }

    fn request(peer: &goat_wire::peer::Peer, method: &str) -> Request {
        Request {
            method: method.to_owned(),
            version: 1,
            params: json!({"action": "navigate"}),
            peer: peer.handle.clone(),
            cancel: CancellationToken::new(),
        }
    }

    #[tokio::test]
    async fn a_browser_call_reaches_the_port_and_returns_its_answer() {
        let (port, mut sent) = Recorder::new(false);
        let host = Arc::new(BrowserHost::new(port));
        let peer = idle_peer();

        let call = {
            let host = host.clone();
            let req = request(&peer, CAPABILITY);
            tokio::spawn(async move { host.call(req).await })
        };

        let (request_id, params) = sent.recv().await.expect("the port receives the call");
        assert_eq!(params["action"], "navigate");
        assert!(host.settle(request_id, Ok(json!({"summary": "ok"}))).await);

        let value = call.await.unwrap().unwrap();
        assert_eq!(value["summary"], "ok");
        assert_eq!(host.in_flight().await, 0);
    }

    #[tokio::test]
    async fn an_unknown_method_is_refused_without_touching_the_port() {
        let (port, mut sent) = Recorder::new(false);
        let host = Arc::new(BrowserHost::new(port));
        let peer = idle_peer();
        let req = request(&peer, "host.simulator");
        let err = host.call(req).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::UnknownMethod);
        assert!(sent.try_recv().is_err());
    }

    #[tokio::test]
    async fn a_dead_port_reports_that_nothing_was_sent() {
        let (port, _sent) = Recorder::new(true);
        let host = Arc::new(BrowserHost::new(port));
        let peer = idle_peer();
        let req = request(&peer, CAPABILITY);
        let err = host.call(req).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::HostGone);
        assert_eq!(err.execution, Some(Execution::NotStarted));
        assert!(err.retry_is_safe());
        assert_eq!(host.in_flight().await, 0);
    }

    #[tokio::test]
    async fn a_browser_error_is_carried_back_verbatim() {
        let (port, mut sent) = Recorder::new(false);
        let host = Arc::new(BrowserHost::new(port));
        let peer = idle_peer();
        let call = {
            let host = host.clone();
            let req = request(&peer, CAPABILITY);
            tokio::spawn(async move { host.call(req).await })
        };
        let (request_id, _params) = sent.recv().await.unwrap();
        host.settle(
            request_id,
            Err(
                CallError::new(ErrorCode::Denied, "the user refused the action")
                    .with_execution(Execution::NotStarted),
            ),
        )
        .await;
        let err = call.await.unwrap().unwrap_err();
        assert_eq!(err.code, ErrorCode::Denied);
        assert!(err.retry_is_safe());
    }

    #[tokio::test(start_paused = true)]
    async fn a_silent_browser_times_out_as_an_unknown_outcome() {
        let (port, mut sent) = Recorder::new(false);
        let host = Arc::new(BrowserHost::with_deadline(port, Duration::from_secs(30)));
        let peer = idle_peer();
        let call = {
            let host = host.clone();
            let req = request(&peer, CAPABILITY);
            tokio::spawn(async move { host.call(req).await })
        };
        let _ = sent.recv().await.unwrap();
        tokio::time::advance(Duration::from_secs(31)).await;
        let err = call.await.unwrap().unwrap_err();
        assert_eq!(err.code, ErrorCode::Timeout);
        assert_eq!(err.execution, Some(Execution::OutcomeUnknown));
        assert!(!err.retry_is_safe());
        assert_eq!(host.in_flight().await, 0);
    }

    #[tokio::test]
    async fn the_browser_closing_fails_every_call_as_an_unknown_outcome() {
        let (port, mut sent) = Recorder::new(false);
        let host = Arc::new(BrowserHost::new(port));
        let peer = idle_peer();
        let call = {
            let host = host.clone();
            let req = request(&peer, CAPABILITY);
            tokio::spawn(async move { host.call(req).await })
        };
        let _ = sent.recv().await.unwrap();
        host.fail_all("the browser closed the port").await;
        let err = call.await.unwrap().unwrap_err();
        assert_eq!(err.code, ErrorCode::HostGone);
        assert_eq!(err.execution, Some(Execution::OutcomeUnknown));
        assert!(!err.retry_is_safe());
    }

    #[tokio::test]
    async fn an_interrupted_turn_cancels_the_pending_browser_call() {
        let (port, mut sent) = Recorder::new(false);
        let host = Arc::new(BrowserHost::new(port));
        let peer = idle_peer();
        let cancel = CancellationToken::new();
        let mut req = request(&peer, CAPABILITY);
        req.cancel = cancel.clone();

        let call = {
            let host = host.clone();
            tokio::spawn(async move { host.call(req).await })
        };
        let _ = sent.recv().await.unwrap();
        cancel.cancel();
        let err = call.await.unwrap().unwrap_err();
        assert_eq!(err.code, ErrorCode::Canceled);
        assert_eq!(err.execution, Some(Execution::OutcomeUnknown));
    }

    #[tokio::test]
    async fn settling_an_unknown_request_is_ignored() {
        let (port, _sent) = Recorder::new(false);
        let host = Arc::new(BrowserHost::new(port));
        assert!(!host.settle(999, Ok(Value::Null)).await);
    }

    #[test]
    fn the_advertisement_leaves_room_for_a_dialog_while_an_evaluate_is_blocked() {
        let params = advertisement("chrome-a".to_owned(), "Work Chrome".to_owned(), 7);
        assert_eq!(params.offers.len(), 1);
        assert_eq!(params.offers[0].id, CAPABILITY);
        assert!(params.offers[0].max_in_flight > 1);
        assert_eq!(params.boot_epoch, 7);
    }

    #[test]
    fn the_withdrawal_offers_nothing() {
        let params = withdrawal("chrome-a".to_owned(), 7);
        assert!(params.offers.is_empty());
        assert_eq!(params.instance, "chrome-a");
    }
}
