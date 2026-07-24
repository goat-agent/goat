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

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CredentialKey {
    #[serde(default)]
    pub service: CredentialService,
    pub provider: String,
    pub account: String,
}

impl CredentialKey {
    pub fn model(provider: impl Into<String>, account: impl Into<String>) -> Self {
        Self {
            service: CredentialService::Model,
            provider: provider.into(),
            account: account.into(),
        }
    }

    pub fn search(provider: impl Into<String>, account: impl Into<String>) -> Self {
        Self {
            service: CredentialService::Search,
            provider: provider.into(),
            account: account.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKind {
    ApiKey,
    OAuth,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenSet {
    pub access_token: SecretString,
    pub refresh_token: Option<SecretString>,
    pub expires_at: Option<i64>,
}

impl TokenSet {
    pub fn from_parts(
        access: String,
        refresh: Option<String>,
        expires_in: Option<i64>,
        fallback_refresh: Option<&str>,
    ) -> Self {
        let expires_at = expires_in.map(|secs| now_secs() + secs);
        Self {
            access_token: SecretString::from(access),
            refresh_token: refresh
                .map(SecretString::from)
                .or_else(|| fallback_refresh.map(SecretString::from)),
            expires_at,
        }
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
        let changed = current.access_token.expose() != tokens.access_token.expose();
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
            Credential::OAuth(tokens) => tokens.access_token.expose(),
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

pub async fn capture_loopback_code(port: u16, expected_state: &str) -> Result<String, AuthError> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    capture_on(listener, expected_state).await
}

const LOGIN_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(5);

pub async fn capture_on(listener: TcpListener, expected_state: &str) -> Result<String, AuthError> {
    tokio::time::timeout(LOGIN_TIMEOUT, capture_loop(listener, expected_state))
        .await
        .map_err(|_| AuthError::OAuth("login timed out".to_owned()))?
}

async fn capture_loop(listener: TcpListener, expected_state: &str) -> Result<String, AuthError> {
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
        let mut code = None;
        let mut state = None;
        for pair in query.split('&') {
            if let Some((key, value)) = pair.split_once('=') {
                match form_urldecode(key).as_str() {
                    "code" => code = Some(form_urldecode(value)),
                    "state" => state = Some(form_urldecode(value)),
                    _ => {}
                }
            }
        }
        let body = "<html><body>goat-code login complete. You can close this tab.</body></html>";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.flush().await;
        if state.as_deref() != Some(expected_state) {
            return Err(AuthError::OAuth("state mismatch".to_owned()));
        }
        return code.ok_or_else(|| AuthError::OAuth("missing authorization code".to_owned()));
    }
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
        SecretString, TokenSet, ensure_valid, now_secs,
    };

    #[tokio::test]
    async fn ensure_valid_single_flights_concurrent_refresh() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let path = std::env::temp_dir().join("goat-auth-singleflight-test.json");
        let _ = std::fs::remove_file(&path);
        let store = CredentialStore::new(path.clone());
        let key = CredentialKey::model("goat-singleflight", "a");
        let expired = TokenSet {
            access_token: SecretString::from("old"),
            refresh_token: Some(SecretString::from("refresh")),
            expires_at: Some(now_secs() - 100),
        };
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
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        Ok(TokenSet {
                            access_token: SecretString::from("new"),
                            refresh_token: Some(SecretString::from("refresh2")),
                            expires_at: Some(now_secs() + 3600),
                        })
                    }
                })
                .await
            }));
        }
        for handle in handles {
            let result = handle.await.unwrap();
            assert!(matches!(result, Some(t) if t.access_token.expose() == "new"));
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
        let expired = TokenSet {
            access_token: SecretString::from("a"),
            refresh_token: None,
            expires_at: Some(0),
        };
        assert!(expired.is_expired());
        let fresh = TokenSet {
            access_token: SecretString::from("a"),
            refresh_token: None,
            expires_at: Some(i64::MAX),
        };
        assert!(!fresh.is_expired());
        let no_expiry = TokenSet {
            access_token: SecretString::from("a"),
            refresh_token: None,
            expires_at: None,
        };
        assert!(!no_expiry.is_expired());
    }

    #[test]
    fn token_set_from_parts() {
        let ts = TokenSet::from_parts(
            "access".to_owned(),
            Some("refresh".to_owned()),
            Some(3600),
            None,
        );
        assert_eq!(ts.access_token.expose(), "access");
        assert_eq!(ts.refresh_token.as_ref().unwrap().expose(), "refresh");
        assert!(ts.expires_at.is_some());
    }

    #[test]
    fn token_set_from_parts_fallback_refresh() {
        let ts = TokenSet::from_parts("access".to_owned(), None, None, Some("fallback"));
        assert_eq!(ts.refresh_token.as_ref().unwrap().expose(), "fallback");
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
}
