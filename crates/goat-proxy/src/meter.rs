use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use goat_provider::{
    Capabilities, ChunkStream, Effort, Model, ModelListSource, Provider, ProviderId,
    ProviderMetadata, RateLimitSnapshot, Request, StreamChunk, StreamError, TokenSet, Usage,
    ValidateError, Validated, WebSearchOutput,
};
use goat_proxy_store::{NewRequest, ProxyStore};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub const SOURCE_CODE: &str = "code";
pub const SOURCE_AGENT: &str = "agent";

const STATUS_OK: &str = "ok";
const STATUS_ERROR: &str = "error";
const STATUS_CANCELLED: &str = "cancelled";

const QUEUE_CAPACITY: usize = 1024;

pub enum ProxyEvent {
    Request(NewRequest),
    RateLimits {
        provider: String,
        account: String,
        snapshot: RateLimitSnapshot,
        updated_at: i64,
    },
}

#[derive(Clone)]
pub struct Recorder {
    tx: mpsc::Sender<ProxyEvent>,
}

impl Recorder {
    pub fn spawn(store: ProxyStore) -> (Self, JoinHandle<()>) {
        let (tx, mut rx) = mpsc::channel::<ProxyEvent>(QUEUE_CAPACITY);
        let task = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                match event {
                    ProxyEvent::Request(request) => {
                        if let Err(err) = store.insert_request(request).await {
                            tracing::warn!(%err, "proxy usage insert failed");
                        }
                    }
                    ProxyEvent::RateLimits {
                        provider,
                        account,
                        snapshot,
                        updated_at,
                    } => match serde_json::to_string(&snapshot) {
                        Ok(json) => {
                            if let Err(err) = store
                                .upsert_rate_limits(&provider, &account, &json, updated_at)
                                .await
                            {
                                tracing::warn!(%err, "proxy rate limits upsert failed");
                            }
                        }
                        Err(err) => {
                            tracing::warn!(%err, "proxy rate limits serialize failed");
                        }
                    },
                }
            }
        });
        (Self { tx }, task)
    }

    fn record(&self, event: ProxyEvent) {
        if let Err(err) = self.tx.try_send(event) {
            tracing::warn!(%err, "proxy recorder queue unavailable; dropping event");
        }
    }
}

#[derive(Clone)]
pub struct Meter {
    source: &'static str,
    recorder: Recorder,
}

impl Meter {
    pub fn new(source: &'static str, recorder: Recorder) -> Self {
        Self { source, recorder }
    }

    pub fn wrap(
        &self,
        provider: Arc<dyn Provider>,
        account: impl Into<String>,
    ) -> Arc<dyn Provider> {
        Arc::new(MeteredProvider {
            inner: provider,
            account: account.into(),
            source: self.source,
            recorder: self.recorder.clone(),
        })
    }
}

pub struct MeteredProvider {
    inner: Arc<dyn Provider>,
    account: String,
    source: &'static str,
    recorder: Recorder,
}

#[async_trait]
impl Provider for MeteredProvider {
    fn id(&self) -> ProviderId {
        self.inner.id()
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }

    fn metadata(&self) -> ProviderMetadata {
        self.inner.metadata()
    }

    async fn stream(&self, req: Request) -> Result<ChunkStream, StreamError> {
        let model = req.model.clone();
        let started = Instant::now();
        match self.inner.stream(req).await {
            Ok(stream) => Ok(Box::pin(MeteredStream {
                inner: stream,
                provider: self.inner.id().0,
                account: self.account.clone(),
                model,
                source: self.source,
                recorder: self.recorder.clone(),
                started,
                usage: None,
                recorded: false,
            })),
            Err(err) => {
                self.recorder.record(ProxyEvent::Request(NewRequest {
                    ts: now_ms(),
                    source: self.source.to_owned(),
                    provider: self.inner.id().0,
                    account: self.account.clone(),
                    model,
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                    duration_ms: elapsed_ms(started),
                    status: STATUS_ERROR.to_owned(),
                    error_kind: Some(error_kind(&err).to_owned()),
                }));
                Err(err)
            }
        }
    }

    fn discover(&self, out: mpsc::Sender<Model>) -> JoinHandle<()> {
        self.inner.discover(out)
    }

    fn model_list_source(&self) -> ModelListSource {
        self.inner.model_list_source()
    }

    fn list_models(&self) -> Vec<String> {
        self.inner.list_models()
    }

    fn efforts(&self, model: &str) -> Vec<Effort> {
        self.inner.efforts(model)
    }

    fn authenticated(&self) -> bool {
        self.inner.authenticated()
    }

    fn validate(&self) -> JoinHandle<Result<Validated, ValidateError>> {
        self.inner.validate()
    }

    fn verifies_credentials(&self) -> bool {
        self.inner.verifies_credentials()
    }

    fn context_window(&self, model: &str) -> Option<u32> {
        self.inner.context_window(model)
    }

    fn supports_images(&self, model: &str) -> bool {
        self.inner.supports_images(model)
    }

    fn supports_web_search(&self) -> bool {
        self.inner.supports_web_search()
    }

    fn web_search(&self, query: String) -> JoinHandle<Result<WebSearchOutput, StreamError>> {
        self.inner.web_search(query)
    }

    fn login(&self, status: mpsc::Sender<String>) -> JoinHandle<Result<TokenSet, String>> {
        self.inner.login(status)
    }
}

struct MeteredStream {
    inner: ChunkStream,
    provider: String,
    account: String,
    model: String,
    source: &'static str,
    recorder: Recorder,
    started: Instant,
    usage: Option<Usage>,
    recorded: bool,
}

impl MeteredStream {
    fn finish(&mut self, status: &str, error_kind: Option<&str>) {
        if self.recorded {
            return;
        }
        self.recorded = true;
        let usage = self.usage.clone().unwrap_or_default();
        self.recorder.record(ProxyEvent::Request(NewRequest {
            ts: now_ms(),
            source: self.source.to_owned(),
            provider: self.provider.clone(),
            account: self.account.clone(),
            model: self.model.clone(),
            input_tokens: i64::from(usage.input_tokens),
            output_tokens: i64::from(usage.output_tokens),
            cache_read_tokens: i64::from(usage.cache_read_tokens),
            cache_write_tokens: i64::from(usage.cache_write_tokens),
            duration_ms: elapsed_ms(self.started),
            status: status.to_owned(),
            error_kind: error_kind.map(str::to_owned),
        }));
    }
}

impl Stream for MeteredStream {
    type Item = Result<StreamChunk, StreamError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.inner.poll_next_unpin(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                match &chunk {
                    StreamChunk::Usage { usage } => {
                        self.usage = Some(usage.clone());
                    }
                    StreamChunk::RateLimits { snapshot } => {
                        self.recorder.record(ProxyEvent::RateLimits {
                            provider: self.provider.clone(),
                            account: self.account.clone(),
                            snapshot: snapshot.clone(),
                            updated_at: now_secs(),
                        });
                    }
                    _ => {}
                }
                Poll::Ready(Some(Ok(chunk)))
            }
            Poll::Ready(Some(Err(err))) => {
                let kind = error_kind(&err).to_owned();
                self.finish(STATUS_ERROR, Some(&kind));
                Poll::Ready(Some(Err(err)))
            }
            Poll::Ready(None) => {
                self.finish(STATUS_OK, None);
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for MeteredStream {
    fn drop(&mut self) {
        self.finish(STATUS_CANCELLED, None);
    }
}

fn error_kind(err: &StreamError) -> &'static str {
    match err {
        StreamError::RateLimited { .. } => "rate_limited",
        StreamError::Overloaded { .. } => "overloaded",
        StreamError::ContextOverflow { .. } => "context_overflow",
        StreamError::Auth { .. } => "auth",
        StreamError::InvalidRequest { .. } => "invalid_request",
        StreamError::Transport { .. } => "transport",
        StreamError::Other { .. } => "other",
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

fn now_secs() -> i64 {
    now_ms() / 1000
}

fn elapsed_ms(started: Instant) -> i64 {
    i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use futures::StreamExt;
    use goat_provider::{
        AuthMethod, Capabilities, Message, MessageRole, Request, StreamChunk, StreamError,
        ToolChoice,
    };

    use super::{Meter, Recorder, STATUS_CANCELLED};
    use goat_proxy_store as goat_store;

    struct MockProvider;

    #[async_trait::async_trait]
    impl goat_provider::Provider for MockProvider {
        fn id(&self) -> goat_provider::ProviderId {
            goat_provider::ProviderId::from("mock")
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities {
                tools: true,
                auth: AuthMethod::ApiKey,
                images: false,
            }
        }

        async fn stream(&self, _req: Request) -> Result<goat_provider::ChunkStream, StreamError> {
            Ok(Box::pin(async_stream::try_stream! {
                yield StreamChunk::TextDelta { text: "hi".into() };
                yield StreamChunk::Usage {
                    usage: goat_provider::Usage {
                        input_tokens: 11,
                        output_tokens: 7,
                        cache_read_tokens: 3,
                        cache_write_tokens: 2,
                    },
                };
                yield StreamChunk::RateLimits {
                    snapshot: goat_provider::RateLimitSnapshot {
                        windows: vec![goat_provider::RateWindow {
                            label: "5h".into(),
                            used_percent: 12.0,
                            resets_at: None,
                        }],
                        representative: None,
                    },
                };
            }))
        }

        fn discover(
            &self,
            _out: tokio::sync::mpsc::Sender<goat_provider::Model>,
        ) -> tokio::task::JoinHandle<()> {
            tokio::spawn(async {})
        }

        fn context_window(&self, _model: &str) -> Option<u32> {
            Some(999_000)
        }
    }

    struct FailingProvider;

    #[async_trait::async_trait]
    impl goat_provider::Provider for FailingProvider {
        fn id(&self) -> goat_provider::ProviderId {
            goat_provider::ProviderId::from("failing")
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities {
                tools: false,
                auth: AuthMethod::ApiKey,
                images: false,
            }
        }

        async fn stream(&self, _req: Request) -> Result<goat_provider::ChunkStream, StreamError> {
            Err(StreamError::rate_limited("slow down", None))
        }

        fn discover(
            &self,
            _out: tokio::sync::mpsc::Sender<goat_provider::Model>,
        ) -> tokio::task::JoinHandle<()> {
            tokio::spawn(async {})
        }
    }

    struct MidStreamFailProvider;

    #[async_trait::async_trait]
    impl goat_provider::Provider for MidStreamFailProvider {
        fn id(&self) -> goat_provider::ProviderId {
            goat_provider::ProviderId::from("midfail")
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities {
                tools: false,
                auth: AuthMethod::ApiKey,
                images: false,
            }
        }

        async fn stream(&self, _req: Request) -> Result<goat_provider::ChunkStream, StreamError> {
            Ok(Box::pin(async_stream::try_stream! {
                yield StreamChunk::TextDelta { text: "partial".into() };
                Err(StreamError::transport("connection reset"))?;
                yield StreamChunk::TextDelta { text: "never".into() };
            }))
        }

        fn discover(
            &self,
            _out: tokio::sync::mpsc::Sender<goat_provider::Model>,
        ) -> tokio::task::JoinHandle<()> {
            tokio::spawn(async {})
        }
    }

    fn request() -> Request {
        Request {
            model: "mock-1".into(),
            messages: vec![Message::text(MessageRole::User, "hi")],
            tools: vec![],
            effort: None,
            tool_choice: ToolChoice::Auto,
            temperature: None,
            max_tokens: None,
            system: None,
        }
    }

    #[tokio::test]
    async fn records_usage_and_rate_limits_on_success() {
        let store = goat_store::ProxyStore::open_in_memory().await.unwrap();
        let (recorder, task) = Recorder::spawn(store.clone());
        let meter = Meter::new("code", recorder);
        let provider = meter.wrap(Arc::new(MockProvider), "default");

        let mut stream = provider.stream(request()).await.unwrap();
        let mut text = String::new();
        while let Some(chunk) = stream.next().await {
            if let Ok(StreamChunk::TextDelta { text: t }) = chunk {
                text.push_str(&t);
            }
        }
        assert_eq!(text, "hi");
        drop(stream);
        drop(meter);
        drop(provider);
        task.await.unwrap();

        let rows = store
            .recent_requests(10, 0, None, None, None)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].provider, "mock");
        assert_eq!(rows[0].account, "default");
        assert_eq!(rows[0].model, "mock-1");
        assert_eq!(rows[0].status, "ok");
        assert_eq!(rows[0].input_tokens, 11);
        assert_eq!(rows[0].output_tokens, 7);
        assert_eq!(rows[0].cache_read_tokens, 3);
        assert_eq!(rows[0].cache_write_tokens, 2);

        let limits = store.latest_rate_limits().await.unwrap();
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].provider, "mock");
        let snapshot: goat_provider::RateLimitSnapshot =
            serde_json::from_str(&limits[0].snapshot).unwrap();
        assert_eq!(snapshot.windows[0].label, "5h");
    }

    #[tokio::test]
    async fn records_open_failure() {
        let store = goat_store::ProxyStore::open_in_memory().await.unwrap();
        let (recorder, task) = Recorder::spawn(store.clone());
        let meter = Meter::new("agent", recorder);
        let provider = meter.wrap(Arc::new(FailingProvider), "work");

        let result = provider.stream(request()).await;
        let Err(err) = result else {
            panic!("expected stream open to fail");
        };
        assert!(matches!(err, StreamError::RateLimited { .. }));
        drop(meter);
        drop(provider);
        task.await.unwrap();

        let rows = store
            .recent_requests(10, 0, None, None, None)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, "error");
        assert_eq!(rows[0].error_kind.as_deref(), Some("rate_limited"));
        assert_eq!(rows[0].source, "agent");
    }

    #[tokio::test]
    async fn records_mid_stream_error() {
        let store = goat_store::ProxyStore::open_in_memory().await.unwrap();
        let (recorder, task) = Recorder::spawn(store.clone());
        let meter = Meter::new("code", recorder);
        let provider = meter.wrap(Arc::new(MidStreamFailProvider), "default");

        let mut stream = provider.stream(request()).await.unwrap();
        while stream.next().await.is_some() {}
        drop(stream);
        drop(meter);
        drop(provider);
        task.await.unwrap();

        let rows = store
            .recent_requests(10, 0, None, None, None)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, "error");
        assert_eq!(rows[0].error_kind.as_deref(), Some("transport"));
    }

    #[tokio::test]
    async fn records_cancelled_on_drop() {
        let store = goat_store::ProxyStore::open_in_memory().await.unwrap();
        let (recorder, task) = Recorder::spawn(store.clone());
        let meter = Meter::new("code", recorder);
        let provider = meter.wrap(Arc::new(MockProvider), "default");

        let mut stream = provider.stream(request()).await.unwrap();
        let first = stream.next().await;
        assert!(matches!(first, Some(Ok(StreamChunk::TextDelta { .. }))));
        drop(stream);
        drop(meter);
        drop(provider);
        task.await.unwrap();

        let rows = store
            .recent_requests(10, 0, None, None, None)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, STATUS_CANCELLED);
    }

    #[tokio::test]
    async fn delegates_trait_surface() {
        let provider: Arc<dyn goat_provider::Provider> = Arc::new(MockProvider);
        let store = goat_store::ProxyStore::open_in_memory().await.unwrap();
        let (recorder, _task) = Recorder::spawn(store);
        let meter = Meter::new("code", recorder);
        let wrapped = meter.wrap(provider, "default");

        assert_eq!(wrapped.id(), goat_provider::ProviderId::from("mock"));
        assert!(wrapped.capabilities().tools);
        assert_eq!(wrapped.context_window("mock-1"), Some(999_000));
        assert!(wrapped.authenticated());
    }
}
