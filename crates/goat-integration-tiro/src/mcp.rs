use std::time::Duration;

use goat_auth::{Credential, CredentialKey, CredentialStore, SecretString, TokenSet};
use goat_integration::{IntegrationError, IntegrationResult};
use rmcp::model::{CallToolRequestParams, CallToolResult, ClientRequest, RawContent, ServerResult};
use rmcp::service::{PeerRequestOptions, RunningService};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::auth::{AuthClient, OAuthState, OAuthTokenResponse};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

pub const MCP_URL: &str = "https://mcp.tiro.ooo/mcp";
pub const ENV_VAR: &str = "GOAT_TIRO_API_KEY";
pub const TOOL_LIST_NOTES: &str = "list_notes";
pub const TOOL_AUTH_STATUS: &str = "auth_status";
pub const MAX_RESULT_BYTES: usize = 96 * 1024;

const AUTH_SCHEME: &str = "Bearer";
const START_TIMEOUT: Duration = Duration::from_secs(15);
const CALL_TIMEOUT: Duration = Duration::from_mins(2);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

pub enum TiroAuth {
    ApiKey(String),
    OAuth { client_id: String, tokens: TokenSet },
}

pub fn resolve_auth(
    credentials: &CredentialStore,
    account: &str,
    client_id: Option<&str>,
) -> IntegrationResult<TiroAuth> {
    let key = CredentialKey::integration("tiro", account);
    if env_overrides_stored_oauth(credentials, &key) {
        info!(
            env_var = ENV_VAR,
            "tiro key from the environment overrides the stored oauth credential"
        );
    }
    match credentials.resolve(&key, Some(ENV_VAR)) {
        Some(Credential::ApiKey(secret) | Credential::ApiKeyWithEndpoint { secret, .. }) => {
            Ok(TiroAuth::ApiKey(secret.expose().to_string()))
        }
        Some(Credential::OAuth(tokens)) => {
            let client_id = client_id.ok_or_else(|| {
                IntegrationError::Config(
                    "tiro connection missing `client_id`; run `goat integration add tiro`".into(),
                )
            })?;
            Ok(TiroAuth::OAuth {
                client_id: client_id.to_string(),
                tokens,
            })
        }
        None => Err(IntegrationError::Auth(format!(
            "no tiro credential for account `{account}`; run `goat integration add tiro` or set {ENV_VAR}"
        ))),
    }
}

fn env_overrides_stored_oauth(credentials: &CredentialStore, key: &CredentialKey) -> bool {
    std::env::var(ENV_VAR).is_ok_and(|value| !value.is_empty())
        && matches!(credentials.get(key), Some(Credential::OAuth(_)))
}

pub fn auth_header_value(token: &str) -> String {
    let trimmed = token.trim();
    if trimmed.contains(char::is_whitespace) {
        trimmed.to_string()
    } else {
        format!("{AUTH_SCHEME} {trimmed}")
    }
}

pub struct TiroSession {
    service: RunningService<RoleClient, ()>,
    auth: Option<AuthClient<rmcp_reqwest::Client>>,
}

pub async fn connect(auth: &TiroAuth) -> IntegrationResult<TiroSession> {
    match auth {
        TiroAuth::ApiKey(key) => {
            let config = StreamableHttpClientTransportConfig::with_uri(MCP_URL)
                .auth_header(auth_header_value(key));
            let transport =
                StreamableHttpClientTransport::with_client(rmcp_reqwest::Client::default(), config);
            let service = start(().serve_with_ct(transport, CancellationToken::new())).await?;
            Ok(TiroSession {
                service,
                auth: None,
            })
        }
        TiroAuth::OAuth { client_id, tokens } => {
            let mut oauth = OAuthState::new(MCP_URL, None)
                .await
                .map_err(|e| IntegrationError::Auth(e.to_string()))?;
            oauth
                .set_credentials(client_id, response_from_token_set(tokens)?)
                .await
                .map_err(|e| IntegrationError::Auth(e.to_string()))?;
            let OAuthState::Authorized(manager) = oauth else {
                return Err(IntegrationError::Auth(
                    "tiro oauth credentials did not authorize".into(),
                ));
            };
            let client = AuthClient::new(rmcp_reqwest::Client::default(), manager);
            let config = StreamableHttpClientTransportConfig::with_uri(MCP_URL);
            let transport = StreamableHttpClientTransport::with_client(client.clone(), config);
            let service = start(().serve_with_ct(transport, CancellationToken::new())).await?;
            Ok(TiroSession {
                service,
                auth: Some(client),
            })
        }
    }
}

async fn start<F, E>(fut: F) -> IntegrationResult<RunningService<RoleClient, ()>>
where
    F: Future<Output = Result<RunningService<RoleClient, ()>, E>>,
    E: std::fmt::Display,
{
    tokio::time::timeout(START_TIMEOUT, fut)
        .await
        .map_err(|_| {
            IntegrationError::Service(format!(
                "tiro mcp initialize timed out after {}s",
                START_TIMEOUT.as_secs()
            ))
        })?
        .map_err(|e| classify_transport_error("initialize", &e.to_string()))
}

pub fn classify_transport_error(stage: &str, rendered: &str) -> IntegrationError {
    if looks_like_auth_failure(rendered) {
        return IntegrationError::Auth(format!(
            "tiro rejected the credential; run `goat integration add tiro` or set {ENV_VAR} ({rendered})"
        ));
    }
    if let Some(seconds) = retry_after_seconds(rendered) {
        return IntegrationError::Service(format!(
            "tiro rate limit reached; wait {seconds}s before retrying ({rendered})"
        ));
    }
    if looks_like_rate_limit(rendered) {
        return IntegrationError::Service(format!(
            "tiro rate limit reached; wait before retrying ({rendered})"
        ));
    }
    IntegrationError::Service(format!("tiro mcp {stage} failed: {rendered}"))
}

fn looks_like_auth_failure(rendered: &str) -> bool {
    let lowered = rendered.to_ascii_lowercase();
    lowered.contains("401")
        || lowered.contains("403")
        || lowered.contains("unauthorized")
        || lowered.contains("invalid_token")
        || lowered.contains("invalid_grant")
        || lowered.contains("authorization required")
}

fn looks_like_rate_limit(rendered: &str) -> bool {
    let lowered = rendered.to_ascii_lowercase();
    lowered.contains("429") || lowered.contains("too many requests")
}

fn retry_after_seconds(rendered: &str) -> Option<u64> {
    let lowered = rendered.to_ascii_lowercase();
    let start = lowered.find("retry-after")? + "retry-after".len();
    lowered[start..]
        .split(|c: char| !c.is_ascii_digit())
        .find(|part| !part.is_empty())
        .and_then(|digits| digits.parse().ok())
}

impl TiroSession {
    pub fn server_name(&self) -> String {
        self.service.peer_info().map_or_else(
            || "tiro mcp".to_string(),
            |info| info.server_info.name.clone(),
        )
    }

    pub async fn identity(&self) -> IntegrationResult<String> {
        let status = self.call(TOOL_AUTH_STATUS, json!({})).await?;
        if status.get("authenticated") == Some(&Value::Bool(false)) {
            return Err(IntegrationError::Auth(
                "tiro reports the credential is not authenticated".into(),
            ));
        }
        Ok(describe_identity(&status).unwrap_or_else(|| self.server_name()))
    }

    pub async fn call(&self, tool: &str, arguments: Value) -> IntegrationResult<Value> {
        let params = match arguments {
            Value::Object(map) => CallToolRequestParams::new(tool.to_owned()).with_arguments(map),
            Value::Null => CallToolRequestParams::new(tool.to_owned()),
            other => {
                return Err(IntegrationError::Config(format!(
                    "tool arguments must be an object, got {other}"
                )));
            }
        };
        let request = ClientRequest::CallToolRequest(rmcp::model::CallToolRequest::new(params));
        let mut options = PeerRequestOptions::no_options();
        options.timeout = Some(CALL_TIMEOUT);
        let handle = self
            .service
            .send_cancellable_request(request, options)
            .await
            .map_err(|e| classify_transport_error("request", &e.to_string()))?;
        let result = handle
            .await_response()
            .await
            .map_err(|e| classify_transport_error("request", &e.to_string()))?;
        let ServerResult::CallToolResult(result) = result else {
            return Err(IntegrationError::Service(
                "unexpected tiro mcp response".into(),
            ));
        };
        if result.is_error.unwrap_or(false) {
            return Err(classify_transport_error(
                &format!("tool `{tool}`"),
                &result_text(&result),
            ));
        }
        Ok(cap_result(result_value(result)))
    }

    pub async fn list_tools(&self) -> IntegrationResult<Vec<rmcp::model::Tool>> {
        tokio::time::timeout(CALL_TIMEOUT, self.service.list_all_tools())
            .await
            .map_err(|_| IntegrationError::Service("tiro mcp list_tools timed out".into()))?
            .map_err(|e| classify_transport_error("list_tools", &e.to_string()))
    }

    pub async fn close(mut self) {
        if let Err(e) = self.service.close_with_timeout(CLOSE_TIMEOUT).await {
            warn!(error = %e, "failed to close tiro mcp session");
        }
    }
}

pub fn describe_identity(status: &Value) -> Option<String> {
    let user = scalar(status.get("userId")?)?;
    let method = status
        .get("authMethod")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let scopes = status
        .get("scopes")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|joined| !joined.is_empty());
    let head = format!("tiro user {user} ({method})");
    Some(match scopes {
        Some(scopes) => format!("{head} · {scopes}"),
        None => head,
    })
}

fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

pub fn cap_result(value: Value) -> Value {
    let rendered = value.to_string();
    if rendered.len() <= MAX_RESULT_BYTES {
        return value;
    }
    let mut end = MAX_RESULT_BYTES;
    while end > 0 && !rendered.is_char_boundary(end) {
        end -= 1;
    }
    json!({
        "truncated": true,
        "bytes_returned": rendered.len(),
        "bytes_kept": end,
        "note": format!(
            "result truncated at {MAX_RESULT_BYTES} bytes; narrow the date range, request a smaller page, or fetch one note at a time"
        ),
        "partial": rendered[..end],
    })
}

fn result_value(result: CallToolResult) -> Value {
    if let Some(value) = result.structured_content {
        return value;
    }
    let joined = collect_text(&result);
    serde_json::from_str(&joined).unwrap_or(Value::String(joined))
}

fn result_text(result: &CallToolResult) -> String {
    let text = collect_text(result);
    if text.is_empty() {
        "no error detail".to_string()
    } else {
        text
    }
}

fn collect_text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|content| match &content.raw {
            RawContent::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub async fn persist_tokens(credentials: &CredentialStore, account: &str, session: &TiroSession) {
    let Some(auth) = &session.auth else {
        return;
    };
    let fetched = { auth.auth_manager.lock().await.get_credentials().await };
    let Ok((_, Some(response))) = fetched else {
        return;
    };
    let Ok(fresh) = token_set_from_response(&response) else {
        return;
    };
    let key = CredentialKey::integration("tiro", account);
    let changed = match credentials.get(&key) {
        Some(Credential::OAuth(old)) => old.access_token.expose() != fresh.access_token.expose(),
        _ => true,
    };
    if changed && let Err(e) = credentials.store(&key, Credential::OAuth(fresh)) {
        warn!(error = %e, "failed to persist refreshed tiro tokens");
    }
}

pub fn token_set_from_response(response: &OAuthTokenResponse) -> IntegrationResult<TokenSet> {
    let raw = serde_json::to_value(response)
        .map_err(|e| IntegrationError::Auth(format!("token response serialization: {e}")))?;
    let access = raw
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| IntegrationError::Auth("token response missing access_token".into()))?;
    Ok(TokenSet {
        access_token: SecretString::from(access),
        refresh_token: raw
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(SecretString::from),
        expires_at: raw
            .get("expires_in")
            .and_then(Value::as_i64)
            .map(|secs| chrono::Utc::now().timestamp() + secs),
    })
}

pub fn response_from_token_set(tokens: &TokenSet) -> IntegrationResult<OAuthTokenResponse> {
    let mut raw = json!({
        "access_token": tokens.access_token.expose(),
        "token_type": "bearer",
    });
    if let Some(refresh) = &tokens.refresh_token {
        raw["refresh_token"] = json!(refresh.expose());
    }
    if let Some(expires_at) = tokens.expires_at {
        raw["expires_in"] = json!((expires_at - chrono::Utc::now().timestamp()).max(0));
    }
    serde_json::from_value(raw)
        .map_err(|e| IntegrationError::Auth(format!("token response reconstruction: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_set_round_trips_through_oauth_response() {
        let tokens = TokenSet {
            access_token: SecretString::from("access-1"),
            refresh_token: Some(SecretString::from("refresh-1")),
            expires_at: Some(chrono::Utc::now().timestamp() + 3600),
        };
        let response = response_from_token_set(&tokens).unwrap();
        let back = token_set_from_response(&response).unwrap();
        assert_eq!(back.access_token.expose(), "access-1");
        assert_eq!(
            back.refresh_token.as_ref().map(SecretString::expose),
            Some("refresh-1")
        );
        let drift = (back.expires_at.unwrap() - tokens.expires_at.unwrap()).abs();
        assert!(drift <= 2, "expiry drifted by {drift}s");
    }

    #[test]
    fn resolve_auth_reports_missing_and_incomplete_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path().join("credentials.json"));
        assert!(matches!(
            resolve_auth(&store, "default", None),
            Err(IntegrationError::Auth(_))
        ));

        store
            .store(
                &CredentialKey::integration("tiro", "default"),
                Credential::OAuth(TokenSet {
                    access_token: SecretString::from("tok"),
                    refresh_token: None,
                    expires_at: None,
                }),
            )
            .unwrap();
        assert!(matches!(
            resolve_auth(&store, "default", None),
            Err(IntegrationError::Config(_))
        ));
        assert!(matches!(
            resolve_auth(&store, "default", Some("client-1")),
            Ok(TiroAuth::OAuth { client_id, .. }) if client_id == "client-1"
        ));
    }

    #[test]
    fn resolve_auth_reads_a_stored_api_key() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path().join("credentials.json"));
        store
            .store(
                &CredentialKey::integration("tiro", "default"),
                Credential::ApiKey(SecretString::from("tiro-key-1")),
            )
            .unwrap();
        assert!(matches!(
            resolve_auth(&store, "default", None),
            Ok(TiroAuth::ApiKey(key)) if key == "tiro-key-1"
        ));
    }

    #[test]
    fn direct_keys_get_the_bearer_scheme_only_when_missing() {
        assert_eq!(auth_header_value("tiro_abc"), "Bearer tiro_abc");
        assert_eq!(auth_header_value("  tiro_abc  "), "Bearer tiro_abc");
        assert_eq!(auth_header_value("Bearer tiro_abc"), "Bearer tiro_abc");
    }

    #[test]
    fn transport_errors_are_classified_for_the_watcher() {
        assert!(matches!(
            classify_transport_error("request", "HTTP status 401 Unauthorized"),
            IntegrationError::Auth(_)
        ));
        assert!(matches!(
            classify_transport_error("request", "invalid_token"),
            IntegrationError::Auth(_)
        ));
        let rate = classify_transport_error("request", "429 retry-after: 30");
        assert!(matches!(&rate, IntegrationError::Service(msg) if msg.contains("wait 30s")));
        assert!(matches!(
            classify_transport_error("request", "connection reset"),
            IntegrationError::Service(_)
        ));
    }

    #[test]
    fn identity_reports_the_account_and_the_scopes_it_actually_got() {
        let status = json!({
            "authenticated": true,
            "authMethod": "api_key",
            "userId": 12345,
            "workspaceGuid": null,
            "scopes": ["mcp:notes:read", "mcp:folders:read"]
        });
        assert_eq!(
            describe_identity(&status).unwrap(),
            "tiro user 12345 (api_key) · mcp:notes:read, mcp:folders:read"
        );
    }

    #[test]
    fn identity_degrades_when_the_status_is_sparse() {
        assert_eq!(
            describe_identity(&json!({ "userId": "u-1" })).unwrap(),
            "tiro user u-1 (unknown)"
        );
        assert!(describe_identity(&json!({ "authMethod": "oauth" })).is_none());
        assert!(describe_identity(&json!({ "userId": null })).is_none());
    }

    #[test]
    fn oversized_results_are_truncated_with_a_hint() {
        let small = json!({ "content": [] });
        assert_eq!(cap_result(small.clone()), small);

        let big = json!({ "content": "x".repeat(MAX_RESULT_BYTES + 1024) });
        let capped = cap_result(big);
        assert_eq!(capped["truncated"], true);
        assert!(capped["partial"].as_str().unwrap().len() <= MAX_RESULT_BYTES);
        assert!(capped["note"].as_str().unwrap().contains("truncated"));
    }
}
