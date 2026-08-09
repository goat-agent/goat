use std::{
    collections::HashMap,
    fmt, fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

pub const BASE64URL: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::URL_SAFE_NO_PAD;

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
        .unwrap_or(0)
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretString(String);

impl SecretString {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString(***)")
    }
}

impl From<String> for SecretString {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for SecretString {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CredentialService {
    #[default]
    Model,
    Search,
    Integration,
    Channel,
    Remote,
    Mcp,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CredentialKey {
    #[serde(default)]
    pub service: CredentialService,
    pub provider: String,
    pub account: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<String>,
}

impl CredentialKey {
    pub fn model(provider: impl Into<String>, account: impl Into<String>) -> Self {
        Self {
            service: CredentialService::Model,
            provider: provider.into(),
            account: account.into(),
            slot: None,
        }
    }

    pub fn search(provider: impl Into<String>, account: impl Into<String>) -> Self {
        Self {
            service: CredentialService::Search,
            provider: provider.into(),
            account: account.into(),
            slot: None,
        }
    }

    pub fn integration(provider: impl Into<String>, account: impl Into<String>) -> Self {
        Self {
            service: CredentialService::Integration,
            provider: provider.into(),
            account: account.into(),
            slot: None,
        }
    }

    pub fn integration_slot(
        provider: impl Into<String>,
        account: impl Into<String>,
        slot: impl Into<String>,
    ) -> Self {
        Self {
            service: CredentialService::Integration,
            provider: provider.into(),
            account: account.into(),
            slot: Some(slot.into()),
        }
    }

    pub fn channel(
        provider: impl Into<String>,
        account: impl Into<String>,
        slot: impl Into<String>,
    ) -> Self {
        Self {
            service: CredentialService::Channel,
            provider: provider.into(),
            account: account.into(),
            slot: Some(slot.into()),
        }
    }

    pub fn remote(remote: impl Into<String>, slot: impl Into<String>) -> Self {
        Self {
            service: CredentialService::Remote,
            provider: remote.into(),
            account: "device".to_owned(),
            slot: Some(slot.into()),
        }
    }

    pub fn mcp(
        provider: impl Into<String>,
        account: impl Into<String>,
        slot: impl Into<String>,
    ) -> Self {
        Self {
            service: CredentialService::Mcp,
            provider: provider.into(),
            account: account.into(),
            slot: Some(slot.into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKind {
    ApiKey,
    #[serde(rename = "oauth", alias = "o_auth")]
    OAuth,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenSet {
    #[serde(deserialize_with = "deserialize_access_token")]
    access_token: SecretString,
    pub refresh_token: Option<SecretString>,
    pub expires_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TokenSetError {
    #[error("access token must not be empty")]
    EmptyAccessToken,
}

fn deserialize_access_token<'de, D>(deserializer: D) -> Result<SecretString, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let access = SecretString::deserialize(deserializer)?;
    if access.expose().is_empty() {
        return Err(serde::de::Error::custom(TokenSetError::EmptyAccessToken));
    }
    Ok(access)
}

impl TokenSet {
    pub fn from_parts(
        access: String,
        refresh: Option<String>,
        expires_in: Option<i64>,
        fallback_refresh: Option<&str>,
    ) -> Result<Self, TokenSetError> {
        if access.is_empty() {
            return Err(TokenSetError::EmptyAccessToken);
        }
        let expires_at = expires_in.map(|secs| now_secs() + secs);
        Ok(Self {
            access_token: SecretString::from(access),
            refresh_token: refresh
                .map(SecretString::from)
                .or_else(|| fallback_refresh.map(SecretString::from)),
            expires_at,
            client_id: None,
            scopes: Vec::new(),
            issuer: None,
        })
    }

    pub fn access_token(&self) -> &SecretString {
        &self.access_token
    }

    #[must_use]
    pub fn with_client(mut self, client_id: impl Into<String>) -> Self {
        self.client_id = Some(client_id.into());
        self
    }

    #[must_use]
    pub fn with_scopes(mut self, scopes: Vec<String>) -> Self {
        self.scopes = scopes;
        self
    }

    #[must_use]
    pub fn with_issuer(mut self, issuer: Option<String>) -> Self {
        self.issuer = issuer;
        self
    }

    pub fn is_expired(&self) -> bool {
        self.expires_at.is_some_and(|exp| exp <= now_secs() + 60)
    }
}

fn refresh_locks() -> &'static std::sync::Mutex<HashMap<CredentialKey, Arc<tokio::sync::Mutex<()>>>>
{
    static LOCKS: std::sync::OnceLock<
        std::sync::Mutex<HashMap<CredentialKey, Arc<tokio::sync::Mutex<()>>>>,
    > = std::sync::OnceLock::new();
    LOCKS.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

fn refresh_lock_for(key: &CredentialKey) -> Arc<tokio::sync::Mutex<()>> {
    let mut map = refresh_locks()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    map.entry(key.clone())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

const REFRESH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

pub async fn ensure_valid<F, Fut>(
    tokens: TokenSet,
    store: &CredentialStore,
    key: &CredentialKey,
    refresh: F,
) -> Option<TokenSet>
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = Result<TokenSet, String>>,
{
    if !tokens.is_expired() {
        return Some(tokens);
    }
    let lock = refresh_lock_for(key);
    let _guard = lock.lock().await;
    if let Some(Credential::OAuth(current)) = store.file_get(key) {
        let changed = current.access_token().expose() != tokens.access_token().expose();
        if changed && !current.is_expired() {
            return Some(current);
        }
    }
    let refresh_token = tokens.refresh_token.as_ref()?.expose().to_owned();
    match tokio::time::timeout(REFRESH_TIMEOUT, refresh(refresh_token)).await {
        Ok(Ok(fresh)) => {
            if let Err(err) = store.store(key, Credential::OAuth(fresh.clone())) {
                tracing::warn!(%err, "failed to persist refreshed oauth tokens");
            }
            Some(fresh)
        }
        Ok(Err(err)) => {
            tracing::warn!(%err, "token refresh failed; treating as logged out");
            None
        }
        Err(_) => {
            tracing::warn!("token refresh timed out; treating as logged out");
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credential {
    ApiKey(SecretString),
    ApiKeyWithEndpoint {
        secret: SecretString,
        endpoint: String,
    },
    OAuth(TokenSet),
}

impl Credential {
    pub fn kind(&self) -> CredentialKind {
        match self {
            Credential::ApiKey(_) | Credential::ApiKeyWithEndpoint { .. } => CredentialKind::ApiKey,
            Credential::OAuth(_) => CredentialKind::OAuth,
        }
    }

    pub fn bearer(&self) -> &str {
        match self {
            Credential::ApiKey(secret) | Credential::ApiKeyWithEndpoint { secret, .. } => {
                secret.expose()
            }
            Credential::OAuth(tokens) => tokens.access_token().expose(),
        }
    }

    pub fn endpoint(&self) -> Option<&str> {
        match self {
            Credential::ApiKeyWithEndpoint { endpoint, .. } => Some(endpoint),
            Credential::ApiKey(_) | Credential::OAuth(_) => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredValue {
    ApiKey {
        secret: SecretString,
    },
    ApiKeyWithEndpoint {
        secret: SecretString,
        endpoint: String,
    },
    #[serde(rename = "oauth", alias = "o_auth")]
    OAuth {
        tokens: TokenSet,
    },
}

impl From<Credential> for StoredValue {
    fn from(value: Credential) -> Self {
        match value {
            Credential::ApiKey(secret) => StoredValue::ApiKey { secret },
            Credential::ApiKeyWithEndpoint { secret, endpoint } => {
                StoredValue::ApiKeyWithEndpoint { secret, endpoint }
            }
            Credential::OAuth(tokens) => StoredValue::OAuth { tokens },
        }
    }
}

impl From<StoredValue> for Credential {
    fn from(value: StoredValue) -> Self {
        match value {
            StoredValue::ApiKey { secret } => Credential::ApiKey(secret),
            StoredValue::ApiKeyWithEndpoint { secret, endpoint } => {
                Credential::ApiKeyWithEndpoint { secret, endpoint }
            }
            StoredValue::OAuth { tokens } => Credential::OAuth(tokens),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredEntry {
    key: CredentialKey,
    value: StoredValue,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct AuthFile {
    credentials: Vec<StoredEntry>,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("credential store at {path} is corrupt: {source}")]
    Corrupt {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("oauth error: {0}")]
    OAuth(String),
}

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl Pkce {
    pub fn generate() -> Self {
        let bytes: [u8; 32] = std::array::from_fn(|_| rand::random::<u8>());
        let verifier = BASE64URL.encode(bytes);
        let challenge = BASE64URL.encode(Sha256::digest(verifier.as_bytes()));
        Self {
            verifier,
            challenge,
        }
    }
}

pub fn random_state() -> String {
    let bytes: [u8; 32] = std::array::from_fn(|_| rand::random::<u8>());
    BASE64URL.encode(bytes)
}

fn form_urldecode(raw: &str) -> String {
    let plus_decoded = raw.replace('+', " ");
    percent_encoding::percent_decode_str(&plus_decoded)
        .decode_utf8_lossy()
        .into_owned()
}

pub async fn bind_loopback() -> Result<(TcpListener, u16), AuthError> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    Ok((listener, port))
}

pub struct AuthorizationResponse {
    pub code: String,
    pub issuer: Option<String>,
}

pub async fn capture_loopback(
    port: u16,
    expected_state: &str,
) -> Result<AuthorizationResponse, AuthError> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    capture_on(listener, expected_state).await
}

const LOGIN_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(5);

pub async fn capture_on(
    listener: TcpListener,
    expected_state: &str,
) -> Result<AuthorizationResponse, AuthError> {
    tokio::time::timeout(LOGIN_TIMEOUT, capture_loop(listener, expected_state))
        .await
        .map_err(|_| AuthError::OAuth("login timed out".to_owned()))?
}

async fn capture_loop(
    listener: TcpListener,
    expected_state: &str,
) -> Result<AuthorizationResponse, AuthError> {
    loop {
        let (mut stream, _) = listener.accept().await?;
        let mut buf = vec![0u8; 8192];
        let read = stream.read(&mut buf).await?;
        let request = String::from_utf8_lossy(&buf[..read]);
        let target = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1));
        let Some(query) = target.and_then(|path| path.split_once('?')).map(|(_, q)| q) else {
            let _ = stream
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
            continue;
        };
        let outcome = read_response(query, expected_state);
        respond(&mut stream, outcome.is_ok()).await;
        return outcome;
    }
}

fn read_response(query: &str, expected_state: &str) -> Result<AuthorizationResponse, AuthError> {
    let mut code = None;
    let mut state = None;
    let mut issuer = None;
    let mut error = None;
    let mut description = None;
    for pair in query.split('&') {
        if let Some((key, value)) = pair.split_once('=') {
            match form_urldecode(key).as_str() {
                "code" => code = Some(form_urldecode(value)),
                "state" => state = Some(form_urldecode(value)),
                "iss" => issuer = Some(form_urldecode(value)),
                "error" => error = Some(form_urldecode(value)),
                "error_description" => description = Some(form_urldecode(value)),
                _ => {}
            }
        }
    }
    tracing::debug!(
        has_code = code.is_some(),
        issuer = ?issuer,
        error = ?error,
        "captured oauth authorization response"
    );
    if state.as_deref() != Some(expected_state) {
        return Err(AuthError::OAuth("state mismatch".to_owned()));
    }
    if let Some(error) = error {
        return Err(AuthError::OAuth(match description {
            Some(description) => format!("authorization denied: {error} ({description})"),
            None => format!("authorization denied: {error}"),
        }));
    }
    let code = code.ok_or_else(|| AuthError::OAuth("missing authorization code".to_owned()))?;
    Ok(AuthorizationResponse { code, issuer })
}

async fn respond(stream: &mut tokio::net::TcpStream, granted: bool) {
    let body = if granted {
        "<html><body>goat login complete. You can close this tab.</body></html>"
    } else {
        "<html><body>goat login failed. Check the terminal for details.</body></html>"
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.flush().await;
}

#[derive(Clone)]
pub struct CredentialStore {
    path: PathBuf,
}

struct FileLock {
    file: fs::File,
}

impl FileLock {
    fn acquire(target: &Path) -> Result<Self, AuthError> {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let lock_path = lock_path_for(target);
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&lock_path)?;
        fs2::FileExt::lock_exclusive(&file)?;
        Ok(Self { file })
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

fn lock_path_for(target: &Path) -> PathBuf {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let file_name = target.file_name().map_or_else(
        || "credentials.json".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    parent.join(format!("{file_name}.lock"))
}

struct TempCleanup {
    path: Option<PathBuf>,
}

impl TempCleanup {
    fn disarm(mut self) {
        self.path = None;
    }
}

impl Drop for TempCleanup {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = fs::remove_file(path);
        }
    }
}

impl CredentialStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn resolve(&self, key: &CredentialKey, env_var: Option<&str>) -> Option<Credential> {
        if let Some(var) = env_var
            && let Ok(value) = std::env::var(var)
            && !value.is_empty()
        {
            return Some(Credential::ApiKey(SecretString::from(value)));
        }
        self.file_get(key)
    }

    pub fn store(&self, key: &CredentialKey, value: Credential) -> Result<(), AuthError> {
        self.file_set(key, value)
    }

    pub fn get(&self, key: &CredentialKey) -> Option<Credential> {
        self.file_get(key)
    }

    pub fn entries(&self) -> Vec<(CredentialKey, CredentialKind)> {
        self.read_file()
            .credentials
            .into_iter()
            .map(|entry| {
                let resolved: Credential = entry.value.into();
                (entry.key, resolved.kind())
            })
            .collect()
    }

    pub fn remove(&self, key: &CredentialKey) -> Result<bool, AuthError> {
        let _lock = FileLock::acquire(&self.path)?;
        let mut file = self.load_file()?;
        let before = file.credentials.len();
        file.credentials.retain(|entry| &entry.key != key);
        let removed = file.credentials.len() != before;
        if removed {
            self.save_file(&file)?;
        }
        Ok(removed)
    }

    fn load_file(&self) -> Result<AuthFile, AuthError> {
        let raw = match fs::read_to_string(&self.path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(AuthFile::default());
            }
            Err(err) => return Err(err.into()),
        };
        serde_json::from_str(&raw).map_err(|source| AuthError::Corrupt {
            path: self.path.clone(),
            source,
        })
    }

    fn read_file(&self) -> AuthFile {
        match self.load_file() {
            Ok(file) => file,
            Err(err) => {
                tracing::error!(error = %err, "failed to read credential store; treating as empty");
                AuthFile::default()
            }
        }
    }

    fn save_file(&self, file: &AuthFile) -> Result<(), AuthError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let contents = serde_json::to_string_pretty(file)?;
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        let file_name = self.path.file_name().map_or_else(
            || "auth.json".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
        let tmp_path = parent.join(format!("{file_name}.tmp-{}", std::process::id()));
        let cleanup = TempCleanup {
            path: Some(tmp_path.clone()),
        };
        {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut handle = match options.open(&tmp_path) {
                Ok(handle) => handle,
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    let _ = fs::remove_file(&tmp_path);
                    options.open(&tmp_path)?
                }
                Err(err) => return Err(err.into()),
            };
            std::io::Write::write_all(&mut handle, contents.as_bytes())?;
            handle.sync_all()?;
        }
        fs::rename(&tmp_path, &self.path)?;
        cleanup.disarm();
        #[cfg(unix)]
        if let Ok(dir) = fs::File::open(parent) {
            let _ = dir.sync_all();
        }
        Ok(())
    }

    fn file_get(&self, key: &CredentialKey) -> Option<Credential> {
        self.read_file()
            .credentials
            .into_iter()
            .find(|entry| &entry.key == key)
            .map(|entry| entry.value.into())
    }

    fn file_set(&self, key: &CredentialKey, value: Credential) -> Result<(), AuthError> {
        let _lock = FileLock::acquire(&self.path)?;
        let mut file = self.load_file()?;
        let stored = StoredValue::from(value);
        if let Some(entry) = file.credentials.iter_mut().find(|entry| &entry.key == key) {
            entry.value = stored;
        } else {
            file.credentials.push(StoredEntry {
                key: key.clone(),
                value: stored,
            });
        }
        self.save_file(&file)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Credential, CredentialKey, CredentialKind, CredentialService, CredentialStore, Pkce,
        SecretString, StoredValue, TokenSet, ensure_valid, now_secs,
    };

    fn token_set(access: &str, refresh: Option<&str>, expires_at: Option<i64>) -> TokenSet {
        let mut tokens =
            TokenSet::from_parts(access.to_owned(), refresh.map(str::to_owned), None, None)
                .unwrap();
        tokens.expires_at = expires_at;
        tokens
    }

    #[tokio::test]
    async fn ensure_valid_single_flights_concurrent_refresh() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let path = std::env::temp_dir().join("goat-auth-singleflight-test.json");
        let _ = std::fs::remove_file(&path);
        let store = CredentialStore::new(path.clone());
        let key = CredentialKey::model("goat-singleflight", "a");
        let expired = token_set("old", Some("refresh"), Some(now_secs() - 100));
        let calls = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let store = store.clone();
            let key = key.clone();
            let tokens = expired.clone();
            let calls = calls.clone();
            handles.push(tokio::spawn(async move {
                ensure_valid(tokens, &store, &key, |_| {
                    let calls = calls.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        Ok(token_set("new", Some("refresh2"), Some(now_secs() + 3600)))
                    }
                })
                .await
            }));
        }
        for handle in handles {
            let result = handle.await.unwrap();
            assert!(matches!(result, Some(t) if t.access_token().expose() == "new"));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn legacy_key_defaults_to_model_service() {
        let key: CredentialKey =
            serde_json::from_str(r#"{"provider":"openai","account":"default"}"#).unwrap();
        assert_eq!(key.service, CredentialService::Model);
        assert_eq!(key.provider, "openai");
        assert_eq!(key.account, "default");
    }

    #[test]
    fn integration_key_serde_round_trip() {
        let key = CredentialKey::integration("linear", "default");
        let raw = serde_json::to_string(&key).unwrap();
        assert!(raw.contains(r#""service":"integration""#));
        let parsed: CredentialKey = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed, key);
        assert_eq!(parsed.service, CredentialService::Integration);
    }

    #[test]
    fn channel_key_carries_a_slot_and_stays_distinct_per_slot() {
        let bot = CredentialKey::channel("slack", "personal", "bot_token");
        let app = CredentialKey::channel("slack", "personal", "app_token");
        assert_ne!(bot, app);
        assert_eq!(bot.service, CredentialService::Channel);
        assert_eq!(bot.account, "personal");
        assert_eq!(bot.slot.as_deref(), Some("bot_token"));

        let raw = serde_json::to_string(&bot).unwrap();
        assert!(raw.contains(r#""service":"channel""#));
        assert!(raw.contains(r#""slot":"bot_token""#));
        let parsed: CredentialKey = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed, bot);
    }

    #[test]
    fn non_channel_keys_omit_the_slot_field_entirely() {
        let raw = serde_json::to_string(&CredentialKey::model("anthropic", "default")).unwrap();
        assert!(!raw.contains("slot"));
    }

    #[test]
    fn channel_slots_round_trip_through_the_store() {
        let dir = std::env::temp_dir().join(format!("goat-auth-slots-{}", std::process::id()));
        let path = dir.join("credentials.json");
        let store = CredentialStore::new(path.clone());
        let bot = CredentialKey::channel("slack", "personal", "bot_token");
        let app = CredentialKey::channel("slack", "personal", "app_token");
        store
            .store(&bot, Credential::ApiKey(SecretString::from("xoxb-1")))
            .unwrap();
        store
            .store(&app, Credential::ApiKey(SecretString::from("xapp-1")))
            .unwrap();

        assert_eq!(store.get(&bot).unwrap().bearer(), "xoxb-1");
        assert_eq!(store.get(&app).unwrap().bearer(), "xapp-1");
        assert_eq!(store.entries().len(), 2);

        assert!(store.remove(&bot).unwrap());
        assert!(store.get(&bot).is_none());
        assert_eq!(store.get(&app).unwrap().bearer(), "xapp-1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pkce_generates_s256_challenge() {
        use base64::Engine;
        use sha2::{Digest, Sha256};
        let pkce = Pkce::generate();
        assert_eq!(pkce.verifier.len(), 43);
        assert_eq!(
            pkce.challenge,
            super::BASE64URL.encode(Sha256::digest(pkce.verifier.as_bytes()))
        );
    }

    #[test]
    fn secret_string_debug_is_redacted() {
        let secret = SecretString::from("topsecret");
        assert_eq!(format!("{secret:?}"), "SecretString(***)");
        assert_eq!(secret.expose(), "topsecret");
    }

    #[test]
    fn secret_string_serializes_transparently() {
        let secret = SecretString::from("abc");
        assert_eq!(serde_json::to_string(&secret).unwrap(), "\"abc\"");
    }

    #[test]
    fn resolved_credential_kind() {
        let cred = Credential::ApiKey(SecretString::from("k"));
        assert_eq!(cred.kind(), CredentialKind::ApiKey);
    }

    #[cfg(unix)]
    #[test]
    fn an_oauth_credential_is_stored_under_the_plain_spelling() {
        let stored = StoredValue::from(Credential::OAuth(token_set("at", None, None)));
        assert_eq!(serde_json::to_value(&stored).unwrap()["kind"], "oauth");
    }

    #[test]
    fn credentials_written_before_the_rename_still_load() {
        let legacy = serde_json::json!({
            "kind": "o_auth",
            "tokens": { "access_token": "at", "refresh_token": null, "expires_at": null }
        });
        let stored: StoredValue = serde_json::from_value(legacy).unwrap();
        let credential = Credential::from(stored);
        assert!(matches!(credential, Credential::OAuth(t) if t.access_token().expose() == "at"));
    }

    #[test]
    fn the_credential_kind_uses_the_same_spelling_both_ways() {
        assert_eq!(
            serde_json::to_value(CredentialKind::OAuth).unwrap(),
            serde_json::Value::String("oauth".to_owned())
        );
        let from_new: CredentialKind = serde_json::from_str("\"oauth\"").unwrap();
        let from_old: CredentialKind = serde_json::from_str("\"o_auth\"").unwrap();
        assert_eq!(from_new, CredentialKind::OAuth);
        assert_eq!(from_old, CredentialKind::OAuth);
    }

    #[test]
    fn the_oauth_identity_is_omitted_when_empty_and_kept_when_set() {
        let bare = token_set("at", None, None);
        let raw = serde_json::to_value(&bare).unwrap();
        assert!(raw.get("client_id").is_none());
        assert!(raw.get("scopes").is_none());
        assert!(raw.get("issuer").is_none());

        let full = bare
            .with_client("cid")
            .with_scopes(vec!["read".to_owned()])
            .with_issuer(Some("https://as.test".to_owned()));
        let raw = serde_json::to_value(&full).unwrap();
        assert_eq!(raw["client_id"], "cid");
        assert_eq!(raw["scopes"], serde_json::json!(["read"]));
        assert_eq!(raw["issuer"], "https://as.test");

        let back: TokenSet = serde_json::from_value(raw).unwrap();
        assert_eq!(back, full);
    }

    #[test]
    fn tokens_written_before_the_identity_fields_still_load() {
        let legacy = serde_json::json!({ "access_token": "at" });
        let tokens: TokenSet = serde_json::from_value(legacy).unwrap();
        assert_eq!(tokens.access_token().expose(), "at");
        assert!(tokens.client_id.is_none());
        assert!(tokens.scopes.is_empty());
        assert!(tokens.issuer.is_none());
    }

    #[test]
    fn saved_file_is_owner_only_and_atomic() {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join("goat-auth-perms-test.json");
        let _ = std::fs::remove_file(&path);
        let store = CredentialStore::new(path.clone());
        let key = CredentialKey::model("p", "a");
        store
            .file_set(&key, Credential::ApiKey(SecretString::from("secret")))
            .unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let got = store.file_get(&key).unwrap();
        assert!(matches!(got, Credential::ApiKey(secret) if secret.expose() == "secret"));
        let leftover = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .contains("goat-auth-perms-test.json.tmp-")
            });
        assert!(!leftover, "temp file should be cleaned up");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn file_store_roundtrip() {
        let path = std::env::temp_dir().join("goat-auth-file-roundtrip-test.json");
        let _ = std::fs::remove_file(&path);
        let store = CredentialStore::new(path.clone());
        let key = CredentialKey::model("p", "a");
        store
            .file_set(&key, Credential::ApiKey(SecretString::from("k")))
            .unwrap();
        let got = store.file_get(&key).unwrap();
        assert!(matches!(got, Credential::ApiKey(secret) if secret.expose() == "k"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn resolve_prefers_env() {
        let path = std::env::temp_dir().join("goat-auth-env-pref-test.json");
        let _ = std::fs::remove_file(&path);
        let store = CredentialStore::new(path);
        let key = CredentialKey::model("goat-test-noexist", "x");
        let cred = store.resolve(&key, Some("PATH")).unwrap();
        assert!(matches!(cred, Credential::ApiKey(_)));
    }

    #[test]
    fn resolve_absent_is_none() {
        let path = std::env::temp_dir().join("goat-auth-absent-test.json");
        let _ = std::fs::remove_file(&path);
        let store = CredentialStore::new(path);
        let key = CredentialKey::model("goat-test-absent-xyz", "none");
        assert!(
            store
                .resolve(&key, Some("GOAT_DEFINITELY_NOT_SET_VAR_42"))
                .is_none()
        );
    }

    #[test]
    fn corrupt_file_is_not_overwritten_on_store() {
        let path = std::env::temp_dir().join("goat-auth-corrupt-test.json");
        std::fs::write(&path, "{ not valid json").unwrap();
        let store = CredentialStore::new(path.clone());
        let key = CredentialKey::model("p", "a");
        let result = store.store(&key, Credential::ApiKey(SecretString::from("k")));
        assert!(matches!(result, Err(super::AuthError::Corrupt { .. })));
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert_eq!(on_disk, "{ not valid json");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_file_loads_as_empty() {
        let path = std::env::temp_dir().join("goat-auth-missing-test.json");
        let _ = std::fs::remove_file(&path);
        let store = CredentialStore::new(path.clone());
        assert!(store.entries().is_empty());
        let key = CredentialKey::model("p", "a");
        store
            .store(&key, Credential::ApiKey(SecretString::from("k")))
            .unwrap();
        assert_eq!(store.entries().len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn token_set_is_expired() {
        assert!(token_set("a", None, Some(0)).is_expired());
        assert!(!token_set("a", None, Some(i64::MAX)).is_expired());
        assert!(!token_set("a", None, None).is_expired());
    }

    #[test]
    fn token_set_from_parts() {
        let ts = TokenSet::from_parts(
            "access".to_owned(),
            Some("refresh".to_owned()),
            Some(3600),
            None,
        )
        .unwrap();
        assert_eq!(ts.access_token().expose(), "access");
        assert_eq!(ts.refresh_token.as_ref().unwrap().expose(), "refresh");
        assert!(ts.expires_at.is_some());
    }

    #[test]
    fn token_set_from_parts_fallback_refresh() {
        let ts = TokenSet::from_parts("access".to_owned(), None, None, Some("fallback")).unwrap();
        assert_eq!(ts.refresh_token.as_ref().unwrap().expose(), "fallback");
    }

    #[test]
    fn empty_access_tokens_are_rejected_by_construction_and_deserialization() {
        assert!(TokenSet::from_parts(String::new(), None, None, None).is_err());
        assert!(
            serde_json::from_value::<TokenSet>(serde_json::json!({
                "access_token": "",
                "refresh_token": null,
                "expires_at": null
            }))
            .is_err()
        );
    }

    #[test]
    fn form_urldecode_handles_percent_and_plus() {
        assert_eq!(super::form_urldecode("a%2Fb%3Dc"), "a/b=c");
        assert_eq!(super::form_urldecode("one+two"), "one two");
        assert_eq!(super::form_urldecode("plain"), "plain");
    }

    #[test]
    fn concurrent_writers_do_not_lose_updates() {
        let path = std::env::temp_dir().join(format!(
            "goat-auth-concurrent-test-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(super::lock_path_for(&path));

        let threads = 16;
        let handles: Vec<_> = (0..threads)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let store = CredentialStore::new(path);
                    let key = CredentialKey::model("provider", format!("account-{i}"));
                    store
                        .store(
                            &key,
                            Credential::ApiKey(SecretString::from(format!("key-{i}"))),
                        )
                        .expect("store");
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("thread");
        }

        let store = CredentialStore::new(path.clone());
        let entries = store.entries();
        assert_eq!(
            entries.len(),
            threads,
            "every concurrent write must survive"
        );
        for i in 0..threads {
            let key = CredentialKey::model("provider", format!("account-{i}"));
            assert!(
                entries.iter().any(|(k, _)| k == &key),
                "missing account-{i}"
            );
        }

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(super::lock_path_for(&path));
    }

    fn granted(query: &str) -> super::AuthorizationResponse {
        match super::read_response(query, "s") {
            Ok(response) => response,
            Err(error) => panic!("expected a success for {query}, got {error}"),
        }
    }

    fn denial(query: &str) -> String {
        match super::read_response(query, "s") {
            Err(super::AuthError::OAuth(message)) => message,
            Err(other) => panic!("expected an oauth error, got {other}"),
            Ok(_) => panic!("expected a failure for {query}"),
        }
    }

    #[test]
    fn the_rfc_9207_issuer_is_kept_and_percent_decoded() {
        let response = granted("code=abc&state=s&iss=https%3A%2F%2Fmcp.sentry.dev");
        assert_eq!(response.code, "abc");
        assert_eq!(response.issuer.as_deref(), Some("https://mcp.sentry.dev"));
    }

    #[test]
    fn a_callback_without_an_issuer_still_succeeds() {
        let response = granted("code=abc&state=s");
        assert_eq!(response.code, "abc");
        assert_eq!(response.issuer, None);
    }

    #[test]
    fn a_denial_reports_the_reason_the_server_gave() {
        let message = denial("error=access_denied&error_description=User+denied&state=s");
        assert!(message.contains("access_denied"), "{message}");
        assert!(message.contains("User denied"), "{message}");

        let bare = denial("error=access_denied&state=s");
        assert!(bare.contains("access_denied"), "{bare}");
    }

    #[test]
    fn a_response_carrying_neither_code_nor_error_is_named_as_such() {
        assert_eq!(denial("state=s"), "missing authorization code");
    }

    #[test]
    fn the_state_is_checked_before_anything_the_server_said() {
        assert_eq!(
            denial("code=abc&state=other&iss=https%3A%2F%2Fevil.test"),
            "state mismatch"
        );
        assert_eq!(
            denial("error=access_denied&error_description=User+denied&state=other"),
            "state mismatch"
        );
    }

    #[tokio::test]
    async fn the_loopback_server_reads_the_query_off_the_request_line() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let (listener, port) = super::bind_loopback().await.unwrap();
        let captured = tokio::spawn(async move { super::capture_on(listener, "s").await });

        let mut client = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        client
            .write_all(
                b"GET /callback?code=abc&state=s&iss=https%3A%2F%2Fmcp.sentry.dev HTTP/1.1\r\n\
                  Host: 127.0.0.1\r\n\r\n",
            )
            .await
            .unwrap();

        let mut page = String::new();
        client.read_to_string(&mut page).await.unwrap();
        assert!(page.contains("login complete"), "{page}");

        let response = match captured.await.unwrap() {
            Ok(response) => response,
            Err(error) => panic!("capture failed: {error}"),
        };
        assert_eq!(response.code, "abc");
        assert_eq!(response.issuer.as_deref(), Some("https://mcp.sentry.dev"));
    }

    #[tokio::test]
    async fn a_rejected_login_tells_the_browser_it_failed() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let (listener, port) = super::bind_loopback().await.unwrap();
        let captured = tokio::spawn(async move { super::capture_on(listener, "s").await });

        let mut client = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        client
            .write_all(b"GET /callback?error=access_denied&state=s HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();

        let mut page = String::new();
        client.read_to_string(&mut page).await.unwrap();
        assert!(page.contains("login failed"), "{page}");
        assert!(captured.await.unwrap().is_err());
    }
}
