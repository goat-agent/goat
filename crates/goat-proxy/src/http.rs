use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Path, Query, Request as AxumRequest, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::routing::{delete, get, post};
use goat_auth::{Credential, CredentialKey, CredentialService, CredentialStore};
use goat_provider::RateLimitSnapshot;
use goat_store::{ProxyStore, ProxyStoreError, RateLimitRow, RequestRow, Totals, UsageBucket};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::ProxyError;

const DEFAULT_USAGE_DAYS: i64 = 30;
const DEFAULT_REQUESTS_LIMIT: i64 = 100;
const MAX_REQUESTS_LIMIT: i64 = 500;
const DAY_MS: i64 = 86_400_000;
const MAX_LOGIN_TASKS: usize = 16;

#[derive(Debug, Clone, Serialize)]
pub struct ProviderMeta {
    pub id: String,
    pub auth: String,
    pub oauth_note: Option<String>,
    pub setup: Vec<String>,
    pub env_var: Option<String>,
    pub endpoint_default: Option<String>,
    pub endpoint_env_var: Option<String>,
}

#[async_trait::async_trait]
pub trait AccountOps: Send + Sync {
    fn providers(&self) -> Vec<ProviderMeta>;
    async fn store_api_key(
        &self,
        provider: &str,
        account: &str,
        secret: &str,
        endpoint: Option<&str>,
    ) -> Result<(), String>;
    async fn remove(&self, provider: &str, account: &str) -> Result<bool, String>;
    async fn verify(&self, provider: &str, account: &str) -> Result<usize, String>;
    fn oauth_login(
        &self,
        provider: &str,
        status: mpsc::Sender<String>,
    ) -> JoinHandle<Result<goat_auth::TokenSet, String>>;
}

#[derive(Clone)]
struct HttpState {
    store: ProxyStore,
    creds: CredentialStore,
    ops: Arc<dyn AccountOps>,
    logins: Arc<LoginManager>,
}

pub async fn serve(
    bind: SocketAddr,
    store: ProxyStore,
    creds: CredentialStore,
    ops: Arc<dyn AccountOps>,
    shutdown: CancellationToken,
) -> Result<(), ProxyError> {
    let app = router(HttpState {
        store,
        creds,
        ops,
        logins: Arc::new(LoginManager::default()),
    });
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, "proxy dashboard listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown.cancelled().await;
        })
        .await?;
    Ok(())
}

fn router(state: HttpState) -> Router {
    let mutations = Router::new()
        .route("/api/accounts", post(create_account))
        .route("/api/accounts/{provider}/{account}", delete(remove_account))
        .route("/api/oauth", post(oauth_start))
        .route_layer(middleware::from_fn(csrf_guard));
    Router::new()
        .route("/", get(index))
        .route("/api/overview", get(overview))
        .route("/api/usage", get(usage))
        .route("/api/requests", get(requests))
        .route("/api/rate-limits", get(rate_limits))
        .route("/api/providers", get(provider_metas))
        .route("/api/accounts", get(accounts))
        .route("/api/oauth/{id}", get(oauth_status))
        .merge(mutations)
        .with_state(state)
}

async fn csrf_guard(req: AxumRequest, next: Next) -> Response {
    let allowed = req
        .headers()
        .get("x-goat-proxy")
        .and_then(|value| value.to_str().ok())
        == Some("1");
    if allowed {
        next.run(req).await
    } else {
        ApiError::new(StatusCode::FORBIDDEN, "missing x-goat-proxy header").into_response()
    }
}

async fn index() -> Html<&'static str> {
    Html(crate::web::INDEX)
}

#[derive(Debug, Serialize)]
struct ProviderView {
    provider: String,
    account: String,
    kind: &'static str,
    expires_at: Option<i64>,
    expired: bool,
    refreshable: bool,
}

#[derive(Debug, Serialize)]
struct OverviewResponse {
    providers: Vec<ProviderView>,
    today: TotalsView,
    rate_limits: Vec<RateLimitView>,
    last24h: TotalsView,
    providers_24h: Vec<BucketView>,
    hourly_24h: Vec<BucketView>,
}

async fn overview(State(state): State<HttpState>) -> Result<Json<OverviewResponse>, ApiError> {
    let providers = provider_views(&state.creds);
    let today = state.store.totals_since(day_start_ms()).await?;
    let day_ago = now_ms() - DAY_MS;
    let last24h = state.store.totals_since(day_ago).await?;
    let providers_24h = state.store.usage_by_provider(day_ago).await?;
    let hourly_24h = state.store.usage_by_hour(day_ago).await?;
    let rate_limits = rate_limit_views(state.store.latest_rate_limits().await?);
    Ok(Json(OverviewResponse {
        providers,
        today: TotalsView::from(today),
        rate_limits,
        last24h: TotalsView::from(last24h),
        providers_24h: providers_24h.into_iter().map(BucketView::from).collect(),
        hourly_24h: hourly_24h.into_iter().map(BucketView::from).collect(),
    }))
}

#[derive(Debug, Serialize)]
struct AccountsResponse {
    providers: Vec<ProviderMeta>,
    accounts: Vec<ProviderView>,
}

async fn accounts(State(state): State<HttpState>) -> Json<AccountsResponse> {
    Json(AccountsResponse {
        providers: state.ops.providers(),
        accounts: provider_views(&state.creds),
    })
}

#[derive(Debug, Serialize)]
struct ProvidersResponse {
    providers: Vec<ProviderMeta>,
}

async fn provider_metas(State(state): State<HttpState>) -> Json<ProvidersResponse> {
    Json(ProvidersResponse {
        providers: state.ops.providers(),
    })
}

#[derive(Debug, Deserialize)]
struct CreateAccount {
    provider: String,
    account: String,
    secret: String,
    endpoint: Option<String>,
}

#[derive(Debug, Serialize)]
struct CreateAccountResponse {
    ok: bool,
    verified: bool,
    models: usize,
}

async fn create_account(
    State(state): State<HttpState>,
    Json(body): Json<CreateAccount>,
) -> Result<Json<CreateAccountResponse>, ApiError> {
    let provider = body.provider.trim();
    let account = body.account.trim();
    if provider.is_empty() || account.is_empty() {
        return Err(ApiError::bad_request("provider and account are required"));
    }
    if body.secret.trim().is_empty() {
        return Err(ApiError::bad_request("secret is required"));
    }
    let meta = find_meta(&state, provider)?;
    match meta.auth.as_str() {
        "none" => return Err(ApiError::bad_request("this provider needs no credential")),
        "oauth" => {
            return Err(ApiError::bad_request(
                "this provider only supports OAuth; use the OAuth flow",
            ));
        }
        _ => {}
    }
    state
        .ops
        .store_api_key(
            provider,
            account,
            body.secret.trim(),
            body.endpoint.as_deref().map(str::trim),
        )
        .await
        .map_err(ApiError::bad_request)?;
    let models = state.ops.verify(provider, account).await.unwrap_or(0);
    Ok(Json(CreateAccountResponse {
        ok: true,
        verified: models > 0,
        models,
    }))
}

#[derive(Debug, Serialize)]
struct RemoveAccountResponse {
    removed: bool,
}

async fn remove_account(
    State(state): State<HttpState>,
    Path((provider, account)): Path<(String, String)>,
) -> Result<Json<RemoveAccountResponse>, ApiError> {
    let removed = state
        .ops
        .remove(&provider, &account)
        .await
        .map_err(ApiError::internal)?;
    if !removed {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no such provider account",
        ));
    }
    Ok(Json(RemoveAccountResponse { removed }))
}

#[derive(Debug, Deserialize)]
struct OAuthStart {
    provider: String,
    account: String,
}

#[derive(Debug, Serialize)]
struct OAuthStartResponse {
    id: u64,
}

async fn oauth_start(
    State(state): State<HttpState>,
    Json(body): Json<OAuthStart>,
) -> Result<Json<OAuthStartResponse>, ApiError> {
    let provider = body.provider.trim();
    let account = body.account.trim();
    if provider.is_empty() || account.is_empty() {
        return Err(ApiError::bad_request("provider and account are required"));
    }
    let meta = find_meta(&state, provider)?;
    if !matches!(meta.auth.as_str(), "oauth" | "api_key_or_oauth") {
        return Err(ApiError::bad_request(
            "this provider does not support OAuth",
        ));
    }
    let id = state.logins.register(provider, account)?;
    let Some(task) = state.logins.get(id) else {
        return Err(ApiError::internal("login task missing"));
    };
    let (status, mut lines) = mpsc::channel::<String>(32);
    let handle = state.ops.oauth_login(provider, status);
    let creds = state.creds.clone();
    tokio::spawn(async move {
        let collect = tokio::spawn({
            let task = task.clone();
            async move {
                while let Some(line) = lines.recv().await {
                    task.push_message(line);
                }
            }
        });
        let result = handle.await;
        let _ = collect.await;
        match result {
            Ok(Ok(tokens)) => {
                let key = CredentialKey::model(&task.provider, &task.account);
                match creds.store(&key, Credential::OAuth(tokens)) {
                    Ok(()) => task.finish(None),
                    Err(err) => task.finish(Some(err.to_string())),
                }
            }
            Ok(Err(err)) => task.finish(Some(err)),
            Err(err) => task.finish(Some(err.to_string())),
        }
    });
    Ok(Json(OAuthStartResponse { id }))
}

#[derive(Debug, Serialize)]
struct OAuthStatusResponse {
    messages: Vec<String>,
    state: &'static str,
    error: Option<String>,
}

async fn oauth_status(
    State(state): State<HttpState>,
    Path(id): Path<u64>,
) -> Result<Json<OAuthStatusResponse>, ApiError> {
    let Some(task) = state.logins.get(id) else {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "unknown login id"));
    };
    let (state_label, error) = task.status();
    Ok(Json(OAuthStatusResponse {
        messages: task.messages(),
        state: state_label,
        error,
    }))
}

fn find_meta(state: &HttpState, provider: &str) -> Result<ProviderMeta, ApiError> {
    state
        .ops
        .providers()
        .into_iter()
        .find(|meta| meta.id == provider)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "unknown provider"))
}

#[derive(Debug, Deserialize)]
struct UsageParams {
    days: Option<i64>,
    group: Option<String>,
}

#[derive(Debug, Serialize)]
struct UsageResponse {
    group: String,
    days: i64,
    buckets: Vec<BucketView>,
}

async fn usage(
    State(state): State<HttpState>,
    Query(params): Query<UsageParams>,
) -> Result<Json<UsageResponse>, ApiError> {
    let days = params.days.unwrap_or(DEFAULT_USAGE_DAYS).clamp(1, 365);
    let group = params.group.as_deref().unwrap_or("day").to_owned();
    let since = now_ms() - days * DAY_MS;
    let buckets = match group.as_str() {
        "provider" => state.store.usage_by_provider(since).await?,
        "model" => state.store.usage_by_model(since).await?,
        "hour" => state.store.usage_by_hour(since).await?,
        _ => state.store.usage_by_day(since).await?,
    };
    Ok(Json(UsageResponse {
        group,
        days,
        buckets: buckets.into_iter().map(BucketView::from).collect(),
    }))
}

#[derive(Debug, Deserialize)]
struct RequestsParams {
    limit: Option<i64>,
    offset: Option<i64>,
    provider: Option<String>,
    status: Option<String>,
    source: Option<String>,
}

#[derive(Debug, Serialize)]
struct RequestsResponse {
    requests: Vec<RequestView>,
}

async fn requests(
    State(state): State<HttpState>,
    Query(params): Query<RequestsParams>,
) -> Result<Json<RequestsResponse>, ApiError> {
    let limit = params
        .limit
        .unwrap_or(DEFAULT_REQUESTS_LIMIT)
        .clamp(1, MAX_REQUESTS_LIMIT);
    let offset = params.offset.unwrap_or(0).max(0);
    let rows = state
        .store
        .recent_requests(
            limit,
            offset,
            params.provider.as_deref(),
            params.status.as_deref(),
            params.source.as_deref(),
        )
        .await?;
    Ok(Json(RequestsResponse {
        requests: rows.into_iter().map(RequestView::from).collect(),
    }))
}

#[derive(Debug, Serialize)]
struct RateLimitsResponse {
    entries: Vec<RateLimitView>,
}

async fn rate_limits(State(state): State<HttpState>) -> Result<Json<RateLimitsResponse>, ApiError> {
    let rows = state.store.latest_rate_limits().await?;
    Ok(Json(RateLimitsResponse {
        entries: rate_limit_views(rows),
    }))
}

fn provider_views(creds: &CredentialStore) -> Vec<ProviderView> {
    let mut views: Vec<ProviderView> = creds
        .entries()
        .into_iter()
        .filter(|(key, _)| key.service == CredentialService::Model)
        .map(|(key, kind)| oauth_details(creds, &key, kind))
        .collect();
    views.sort_by(|a, b| (&a.provider, &a.account).cmp(&(&b.provider, &b.account)));
    views
}

fn oauth_details(
    creds: &CredentialStore,
    key: &CredentialKey,
    kind: goat_auth::CredentialKind,
) -> ProviderView {
    let (kind_label, expires_at, refreshable) = match kind {
        goat_auth::CredentialKind::ApiKey => ("api_key", None, false),
        goat_auth::CredentialKind::OAuth => {
            let (expires_at, refreshable) = match creds.get(key) {
                Some(Credential::OAuth(tokens)) => {
                    (tokens.expires_at, tokens.refresh_token.is_some())
                }
                _ => (None, false),
            };
            ("oauth", expires_at, refreshable)
        }
    };
    let expired = expires_at.is_some_and(|exp| exp <= now_secs() + 60);
    ProviderView {
        provider: key.provider.clone(),
        account: key.account.clone(),
        kind: kind_label,
        expires_at,
        expired,
        refreshable,
    }
}

fn rate_limit_views(rows: Vec<RateLimitRow>) -> Vec<RateLimitView> {
    let now = now_secs();
    rows.into_iter()
        .filter_map(|row| {
            let snapshot: RateLimitSnapshot = match serde_json::from_str(&row.snapshot) {
                Ok(snapshot) => snapshot,
                Err(err) => {
                    tracing::warn!(%err, provider = %row.provider, "bad rate limit snapshot");
                    return None;
                }
            };
            Some(RateLimitView {
                provider: row.provider,
                account: row.account,
                updated_at: row.updated_at,
                age_secs: (now - row.updated_at).max(0),
                representative: snapshot.representative,
                windows: snapshot
                    .windows
                    .into_iter()
                    .map(|window| RateWindowView {
                        label: window.label,
                        used_percent: window.used_percent,
                        resets_at: window.resets_at,
                        reset_in_secs: window.resets_at.map(|reset| (reset - now).max(0)),
                    })
                    .collect(),
            })
        })
        .collect()
}

#[derive(Default)]
struct LoginManager {
    tasks: Mutex<HashMap<u64, Arc<LoginTask>>>,
    next: AtomicU64,
}

impl LoginManager {
    fn register(&self, provider: &str, account: &str) -> Result<u64, ApiError> {
        let mut tasks = self
            .tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        tasks.retain(|_, task| task.is_pending());
        if tasks.len() >= MAX_LOGIN_TASKS {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "too many pending logins",
            ));
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        tasks.insert(id, Arc::new(LoginTask::new(provider, account)));
        Ok(id)
    }

    fn get(&self, id: u64) -> Option<Arc<LoginTask>> {
        self.tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&id)
            .cloned()
    }
}

struct LoginTask {
    provider: String,
    account: String,
    messages: Mutex<Vec<String>>,
    outcome: Mutex<Option<Result<(), String>>>,
}

impl LoginTask {
    fn new(provider: &str, account: &str) -> Self {
        Self {
            provider: provider.to_owned(),
            account: account.to_owned(),
            messages: Mutex::new(Vec::new()),
            outcome: Mutex::new(None),
        }
    }

    fn push_message(&self, line: String) {
        self.messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(line);
    }

    fn finish(&self, error: Option<String>) {
        *self
            .outcome
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(match error {
            Some(err) => Err(err),
            None => Ok(()),
        });
    }

    fn is_pending(&self) -> bool {
        self.outcome
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_none()
    }

    fn messages(&self) -> Vec<String> {
        self.messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn status(&self) -> (&'static str, Option<String>) {
        match &*self
            .outcome
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            None => ("pending", None),
            Some(Ok(())) => ("done", None),
            Some(Err(err)) => ("failed", Some(err.clone())),
        }
    }
}

#[derive(Debug, Serialize)]
struct TotalsView {
    requests: i64,
    errors: i64,
    cancelled: i64,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    avg_duration_ms: f64,
}

impl From<Totals> for TotalsView {
    fn from(totals: Totals) -> Self {
        Self {
            requests: totals.requests,
            errors: totals.errors,
            cancelled: totals.cancelled,
            input_tokens: totals.input_tokens,
            output_tokens: totals.output_tokens,
            cache_read_tokens: totals.cache_read_tokens,
            cache_write_tokens: totals.cache_write_tokens,
            avg_duration_ms: totals.avg_duration_ms,
        }
    }
}

#[derive(Debug, Serialize)]
struct BucketView {
    key: String,
    requests: i64,
    errors: i64,
    cancelled: i64,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    avg_duration_ms: f64,
}

impl From<UsageBucket> for BucketView {
    fn from(bucket: UsageBucket) -> Self {
        Self {
            key: bucket.key,
            requests: bucket.requests,
            errors: bucket.errors,
            cancelled: bucket.cancelled,
            input_tokens: bucket.input_tokens,
            output_tokens: bucket.output_tokens,
            cache_read_tokens: bucket.cache_read_tokens,
            cache_write_tokens: bucket.cache_write_tokens,
            avg_duration_ms: bucket.avg_duration_ms,
        }
    }
}

#[derive(Debug, Serialize)]
struct RequestView {
    id: i64,
    ts: i64,
    source: String,
    provider: String,
    account: String,
    model: String,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    duration_ms: i64,
    status: String,
    error_kind: Option<String>,
}

impl From<RequestRow> for RequestView {
    fn from(row: RequestRow) -> Self {
        Self {
            id: row.id,
            ts: row.ts,
            source: row.source,
            provider: row.provider,
            account: row.account,
            model: row.model,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cache_read_tokens: row.cache_read_tokens,
            cache_write_tokens: row.cache_write_tokens,
            duration_ms: row.duration_ms,
            status: row.status,
            error_kind: row.error_kind,
        }
    }
}

#[derive(Debug, Serialize)]
struct RateLimitView {
    provider: String,
    account: String,
    updated_at: i64,
    age_secs: i64,
    representative: Option<String>,
    windows: Vec<RateWindowView>,
}

#[derive(Debug, Serialize)]
struct RateWindowView {
    label: String,
    used_percent: f32,
    resets_at: Option<i64>,
    reset_in_secs: Option<i64>,
}

struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }
}

impl From<ProxyStoreError> for ApiError {
    fn from(err: ProxyStoreError) -> Self {
        Self::internal(err.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "error": self.message });
        (self.status, Json(body)).into_response()
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

fn day_start_ms() -> i64 {
    now_ms() - now_ms().rem_euclid(DAY_MS)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use goat_auth::{Credential, CredentialKey, CredentialStore, SecretString, TokenSet};
    use goat_store::NewRequest;
    use tokio::sync::mpsc;
    use tokio::task::JoinHandle;
    use tower::ServiceExt;

    use super::{AccountOps, HttpState, LoginManager, ProviderMeta, router};

    struct StubOps {
        creds: CredentialStore,
    }

    #[async_trait::async_trait]
    impl AccountOps for StubOps {
        fn providers(&self) -> Vec<ProviderMeta> {
            vec![
                ProviderMeta {
                    id: "openai".into(),
                    auth: "api_key".into(),
                    oauth_note: None,
                    setup: vec![],
                    env_var: Some("OPENAI_API_KEY".into()),
                    endpoint_default: None,
                    endpoint_env_var: None,
                },
                ProviderMeta {
                    id: "anthropic".into(),
                    auth: "api_key_or_oauth".into(),
                    oauth_note: Some("browser (Claude Pro/Max)".into()),
                    setup: vec!["run login and approve in browser".into()],
                    env_var: None,
                    endpoint_default: None,
                    endpoint_env_var: None,
                },
                ProviderMeta {
                    id: "ollama".into(),
                    auth: "none".into(),
                    oauth_note: None,
                    setup: vec![],
                    env_var: None,
                    endpoint_default: None,
                    endpoint_env_var: None,
                },
            ]
        }

        async fn store_api_key(
            &self,
            provider: &str,
            account: &str,
            secret: &str,
            _endpoint: Option<&str>,
        ) -> Result<(), String> {
            self.creds
                .store(
                    &CredentialKey::model(provider, account),
                    Credential::ApiKey(SecretString::from(secret)),
                )
                .map_err(|err| err.to_string())
        }

        async fn remove(&self, provider: &str, account: &str) -> Result<bool, String> {
            self.creds
                .remove(&CredentialKey::model(provider, account))
                .map_err(|err| err.to_string())
        }

        async fn verify(&self, _provider: &str, _account: &str) -> Result<usize, String> {
            Ok(3)
        }

        fn oauth_login(
            &self,
            provider: &str,
            status: mpsc::Sender<String>,
        ) -> JoinHandle<Result<TokenSet, String>> {
            let provider = provider.to_owned();
            tokio::spawn(async move {
                let _ = status
                    .send(format!("open https://example.com/activate for {provider}"))
                    .await;
                let _ = status.send("waiting for approval…".into()).await;
                Ok(TokenSet::from_parts(
                    "oauth-access-secret".into(),
                    Some("oauth-refresh-secret".into()),
                    Some(3600),
                    None,
                ))
            })
        }
    }

    fn temp_creds() -> CredentialStore {
        CredentialStore::new(tempfile::tempdir().unwrap().keep().join("creds.json"))
    }

    async fn state() -> HttpState {
        let store = goat_store::ProxyStore::open_in_memory().await.unwrap();
        let creds = temp_creds();
        HttpState {
            store,
            ops: Arc::new(StubOps {
                creds: creds.clone(),
            }),
            creds,
            logins: Arc::new(LoginManager::default()),
        }
    }

    async fn body_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn get(uri: &str) -> Request<Body> {
        Request::get(uri).body(Body::empty()).unwrap()
    }

    fn post_json(uri: &str, body: &str, with_header: bool) -> Request<Body> {
        let mut builder = Request::post(uri).header("content-type", "application/json");
        if with_header {
            builder = builder.header("x-goat-proxy", "1");
        }
        builder.body(Body::from(body.to_owned())).unwrap()
    }

    fn delete(uri: &str, with_header: bool) -> Request<Body> {
        let mut builder = Request::delete(uri);
        if with_header {
            builder = builder.header("x-goat-proxy", "1");
        }
        builder.body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn serves_index_page() {
        let app = router(state().await);
        let response = app.oneshot(get("/")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(html.contains("goat proxy"));
    }

    #[tokio::test]
    async fn usage_endpoint_aggregates() {
        let state = state().await;
        state
            .store
            .insert_request(NewRequest {
                ts: super::now_ms(),
                source: "code".into(),
                provider: "openai".into(),
                account: "default".into(),
                model: "gpt-5".into(),
                input_tokens: 40,
                output_tokens: 10,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                duration_ms: 500,
                status: "ok".into(),
                error_kind: None,
            })
            .await
            .unwrap();

        let app = router(state);
        let response = app.oneshot(get("/api/usage?group=provider")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        assert_eq!(json["group"], "provider");
        assert_eq!(json["buckets"][0]["key"], "openai");
        assert_eq!(json["buckets"][0]["input_tokens"], 40);
    }

    #[tokio::test]
    async fn requests_endpoint_filters_source_and_status() {
        let state = state().await;
        for (source, status) in [("agent", "cancelled"), ("code", "ok")] {
            state
                .store
                .insert_request(NewRequest {
                    ts: super::now_ms(),
                    source: source.into(),
                    provider: "kimi".into(),
                    account: "default".into(),
                    model: "k2".into(),
                    input_tokens: 5,
                    output_tokens: 2,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                    duration_ms: 100,
                    status: status.into(),
                    error_kind: None,
                })
                .await
                .unwrap();
        }

        let app = router(state);
        let response = app
            .oneshot(get("/api/requests?status=cancelled&source=agent"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        assert_eq!(json["requests"].as_array().unwrap().len(), 1);
        assert_eq!(json["requests"][0]["source"], "agent");
        assert_eq!(json["requests"][0]["status"], "cancelled");
    }

    #[tokio::test]
    async fn rate_limits_endpoint_computes_age_and_reset() {
        let state = state().await;
        let reset_future = super::now_secs() + 3600;
        let snapshot = serde_json::json!({
            "windows": [{ "label": "5h", "used_percent": 42.0, "resets_at": reset_future }],
            "representative": null,
        });
        state
            .store
            .upsert_rate_limits(
                "openai",
                "default",
                &snapshot.to_string(),
                super::now_secs(),
            )
            .await
            .unwrap();

        let app = router(state);
        let response = app.oneshot(get("/api/rate-limits")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        let entry = &json["entries"][0];
        assert_eq!(entry["provider"], "openai");
        assert!(entry["age_secs"].as_i64().unwrap() < 5);
        let window = &entry["windows"][0];
        assert_eq!(window["label"], "5h");
        assert!(window["reset_in_secs"].as_i64().unwrap() > 3500);
    }

    #[tokio::test]
    async fn overview_lists_accounts_without_secrets() {
        let state = state().await;
        let creds = temp_creds();
        creds
            .store(
                &CredentialKey::model("openai", "default"),
                Credential::OAuth(TokenSet::from_parts(
                    "sk-live-secret".into(),
                    Some("refresh-secret".into()),
                    Some(3600),
                    None,
                )),
            )
            .unwrap();
        let state = HttpState { creds, ..state };

        let app = router(state);
        let response = app.oneshot(get("/api/overview")).await.unwrap();
        let status = response.status();
        let json = body_json(response).await;
        assert_eq!(status, StatusCode::OK, "body: {json}");
        let raw = json.to_string();
        assert!(!raw.contains("sk-live-secret"));
        assert!(!raw.contains("refresh-secret"));
        assert_eq!(json["providers"][0]["provider"], "openai");
        assert_eq!(json["providers"][0]["kind"], "oauth");
        assert!(json["providers"][0]["expires_at"].is_number());
        assert_eq!(json["providers"][0]["refreshable"], true);
        assert_eq!(json["providers"][0]["expired"], false);
        assert_eq!(json["today"]["requests"], 0);
    }

    #[tokio::test]
    async fn mutations_require_proxy_header() {
        let app = router(state().await);
        let response = app
            .oneshot(post_json(
                "/api/accounts",
                r#"{"provider":"openai","account":"default","secret":"sk-x"}"#,
                false,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn create_account_stores_key_and_hides_secret() {
        let state = state().await;
        let creds = state.creds.clone();
        let app = router(state);
        let response = app
            .oneshot(post_json(
                "/api/accounts",
                r#"{"provider":"openai","account":"work","secret":"sk-created-secret"}"#,
                true,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        assert_eq!(json["ok"], true);
        assert_eq!(json["verified"], true);
        assert!(!json.to_string().contains("sk-created-secret"));

        let Some(Credential::ApiKey(_)) = creds.get(&CredentialKey::model("openai", "work")) else {
            panic!("expected stored api key");
        };
    }

    #[tokio::test]
    async fn create_account_rejects_local_provider() {
        let app = router(state().await);
        let response = app
            .oneshot(post_json(
                "/api/accounts",
                r#"{"provider":"ollama","account":"default","secret":"sk-x"}"#,
                true,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn remove_account_deletes_then_404s() {
        let seeded = state().await;
        seeded
            .ops
            .store_api_key("openai", "gone", "sk-temp", None)
            .await
            .unwrap();
        let app = router(seeded);
        let response = app
            .oneshot(delete("/api/accounts/openai/gone", true))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let fresh = state().await;
        let app = router(fresh);
        let response = app
            .oneshot(delete("/api/accounts/openai/missing", true))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn oauth_flow_completes_and_stores_tokens() {
        let state = state().await;
        let creds = state.creds.clone();
        let app = router(state);
        let response = app
            .clone()
            .oneshot(post_json(
                "/api/oauth",
                r#"{"provider":"anthropic","account":"work"}"#,
                true,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        let id = json["id"].as_u64().unwrap();

        let mut status_json = serde_json::Value::Null;
        for _ in 0..100 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let response = app
                .clone()
                .oneshot(get(&format!("/api/oauth/{id}")))
                .await
                .unwrap();
            status_json = body_json(response).await;
            if status_json["state"] != "pending" {
                break;
            }
        }
        assert_eq!(status_json["state"], "done");
        assert!(
            status_json["messages"]
                .to_string()
                .contains("https://example.com/activate")
        );

        let Some(Credential::OAuth(tokens)) = creds.get(&CredentialKey::model("anthropic", "work"))
        else {
            panic!("expected stored oauth tokens");
        };
        assert!(tokens.refresh_token.is_some());
    }
}
