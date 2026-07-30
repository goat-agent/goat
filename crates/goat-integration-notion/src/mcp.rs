use std::time::Duration;

use goat_auth::{Credential, CredentialKey, CredentialStore, SecretString, TokenSet};
use goat_integration::{IntegrationError, IntegrationResult};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ClientRequest, ContentBlock, ServerResult,
};
use rmcp::service::{PeerRequestOptions, RunningService};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::auth::{AuthClient, OAuthState, OAuthTokenResponse};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use tracing::warn;

pub const MCP_URL: &str = "https://mcp.notion.com/mcp";

pub const VIEW_TOOL_CANDIDATES: &[&str] = &["query-data-sources", "query-database-view"];

const START_TIMEOUT: Duration = Duration::from_secs(15);
const CALL_TIMEOUT: Duration = Duration::from_mins(1);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

pub fn normalize(raw: &str) -> String {
    let trimmed = raw
        .strip_prefix("notion-")
        .or_else(|| raw.strip_prefix("notion_"))
        .unwrap_or(raw);
    trimmed.replace('-', "_")
}

pub fn pick_tool<'a, I>(available: I, candidates: &[&str]) -> Option<String>
where
    I: IntoIterator<Item = &'a str>,
{
    let available: Vec<&str> = available.into_iter().collect();
    candidates.iter().find_map(|candidate| {
        let want = normalize(candidate);
        available
            .iter()
            .find(|name| normalize(name) == want)
            .map(|name| (*name).to_string())
    })
}

pub enum NotionAuth {
    Bearer(String),
    OAuth { client_id: String, tokens: TokenSet },
}

pub fn resolve_auth(
    credentials: &CredentialStore,
    account: &str,
    client_id: Option<&str>,
) -> IntegrationResult<NotionAuth> {
    let key = CredentialKey::integration("notion", account);
    match credentials.resolve(&key, None) {
        Some(Credential::ApiKey(secret) | Credential::ApiKeyWithEndpoint { secret, .. }) => {
            Ok(NotionAuth::Bearer(secret.expose().to_string()))
        }
        Some(Credential::OAuth(tokens)) => {
            let client_id = client_id.ok_or_else(|| {
                IntegrationError::Config(
                    "notion binding missing `client_id`; run `goat integration add notion`".into(),
                )
            })?;
            Ok(NotionAuth::OAuth {
                client_id: client_id.to_string(),
                tokens,
            })
        }
        None => Err(IntegrationError::Auth(format!(
            "no notion credential for account `{account}`; run `goat integration add notion`"
        ))),
    }
}

pub struct NotionSession {
    service: RunningService<RoleClient, ()>,
    auth: Option<AuthClient<rmcp_reqwest::Client>>,
}

pub async fn connect(auth: &NotionAuth) -> IntegrationResult<NotionSession> {
    match auth {
        NotionAuth::Bearer(token) => {
            let config =
                StreamableHttpClientTransportConfig::with_uri(MCP_URL).auth_header(token.clone());
            let transport =
                StreamableHttpClientTransport::with_client(rmcp_reqwest::Client::default(), config);
            let service = start(().serve_with_ct(transport, CancellationToken::new())).await?;
            Ok(NotionSession {
                service,
                auth: None,
            })
        }
        NotionAuth::OAuth { client_id, tokens } => {
            let mut oauth = OAuthState::new(MCP_URL, None)
                .await
                .map_err(|e| auth_err(&e))?;
            oauth
                .set_credentials(client_id, response_from_token_set(tokens)?)
                .await
                .map_err(|e| auth_err(&e))?;
            let OAuthState::Authorized(manager) = oauth else {
                return Err(IntegrationError::Auth(
                    "notion oauth credentials did not authorize".into(),
                ));
            };
            let client = AuthClient::new(rmcp_reqwest::Client::default(), manager);
            let config = StreamableHttpClientTransportConfig::with_uri(MCP_URL);
            let transport = StreamableHttpClientTransport::with_client(client.clone(), config);
            let service = start(().serve_with_ct(transport, CancellationToken::new())).await?;
            Ok(NotionSession {
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
                "notion mcp initialize timed out after {}s",
                START_TIMEOUT.as_secs()
            ))
        })?
        .map_err(|e| IntegrationError::Service(format!("notion mcp initialize failed: {e}")))
}

impl NotionSession {
    pub fn server_name(&self) -> String {
        self.service
            .peer_info()
            .and_then(|info| info.server_info.clone())
            .map_or_else(|| "notion mcp".to_string(), |server| server.name)
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
            .map_err(|e| IntegrationError::Service(format!("notion mcp request failed: {e}")))?;
        let result = handle
            .await_response()
            .await
            .map_err(|e| IntegrationError::Service(format!("notion mcp request failed: {e}")))?;
        let ServerResult::CallToolResult(result) = result else {
            return Err(IntegrationError::Service(
                "unexpected notion mcp response".into(),
            ));
        };
        if result.is_error.unwrap_or(false) {
            return Err(IntegrationError::Service(format!(
                "notion mcp tool `{tool}` failed: {}",
                result_text(&result)
            )));
        }
        Ok(result_value(result))
    }

    pub async fn list_tools(&self) -> IntegrationResult<Vec<rmcp::model::Tool>> {
        tokio::time::timeout(CALL_TIMEOUT, self.service.list_all_tools())
            .await
            .map_err(|_| IntegrationError::Service("notion mcp list_tools timed out".into()))?
            .map_err(|e| IntegrationError::Service(format!("notion mcp list_tools failed: {e}")))
    }

    pub async fn close(mut self) {
        if let Err(e) = self.service.close_with_timeout(CLOSE_TIMEOUT).await {
            warn!(error = %e, "failed to close notion mcp session");
        }
    }
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
        .filter_map(|content| match content {
            ContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub async fn persist_tokens(credentials: &CredentialStore, account: &str, session: &NotionSession) {
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
    let key = CredentialKey::integration("notion", account);
    let changed = match credentials.get(&key) {
        Some(Credential::OAuth(old)) => old.access_token.expose() != fresh.access_token.expose(),
        _ => true,
    };
    if changed && let Err(e) = credentials.store(&key, Credential::OAuth(fresh)) {
        warn!(error = %e, "failed to persist refreshed notion tokens");
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
        ..TokenSet::default()
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

fn auth_err(e: &rmcp::transport::auth::AuthError) -> IntegrationError {
    IntegrationError::Auth(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_collapses_vendor_prefix_and_hyphens() {
        assert_eq!(normalize("notion-search"), "search");
        assert_eq!(normalize("notion_search"), "search");
        assert_eq!(normalize("search"), "search");
        assert_eq!(normalize("notion-create-pages"), "create_pages");
        assert_eq!(normalize("query-data-sources"), "query_data_sources");
    }

    #[test]
    fn pick_tool_matches_across_naming_styles() {
        let available = ["notion-fetch", "notion-query-data-sources"];
        assert_eq!(
            pick_tool(available, VIEW_TOOL_CANDIDATES).as_deref(),
            Some("notion-query-data-sources")
        );
        assert_eq!(
            pick_tool(["query_data_sources"], VIEW_TOOL_CANDIDATES).as_deref(),
            Some("query_data_sources")
        );
        assert_eq!(pick_tool(["notion-fetch"], VIEW_TOOL_CANDIDATES), None);
    }

    #[test]
    fn pick_tool_honours_candidate_order() {
        let available = ["notion-query-database-view", "notion-query-data-sources"];
        assert_eq!(
            pick_tool(available, VIEW_TOOL_CANDIDATES).as_deref(),
            Some("notion-query-data-sources")
        );
    }

    #[test]
    fn token_set_round_trips_through_oauth_response() {
        let tokens = TokenSet {
            access_token: SecretString::from("access-1"),
            refresh_token: Some(SecretString::from("refresh-1")),
            expires_at: Some(chrono::Utc::now().timestamp() + 3600),
            ..TokenSet::default()
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
    fn resolve_auth_requires_client_id_for_oauth() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path().join("credentials.json"));
        assert!(matches!(
            resolve_auth(&store, "default", None),
            Err(IntegrationError::Auth(_))
        ));

        store
            .store(
                &CredentialKey::integration("notion", "default"),
                Credential::OAuth(TokenSet {
                    access_token: SecretString::from("tok"),
                    refresh_token: None,
                    expires_at: None,
                    ..TokenSet::default()
                }),
            )
            .unwrap();
        assert!(matches!(
            resolve_auth(&store, "default", None),
            Err(IntegrationError::Config(_))
        ));
        assert!(matches!(
            resolve_auth(&store, "default", Some("client-1")),
            Ok(NotionAuth::OAuth { client_id, .. }) if client_id == "client-1"
        ));
    }
}
