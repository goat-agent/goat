use std::time::Duration;

use goat_auth::{Credential, CredentialKey, CredentialStore};
use goat_integration::{IntegrationError, IntegrationResult};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ClientRequest, ContentBlock, ServerResult,
};
use rmcp::service::{PeerRequestOptions, RunningService};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{RoleClient, ServiceExt};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::warn;

pub const MCP_URL: &str = "https://mcp.slack.com/mcp";
pub const ENV_VAR: &str = "SLACK_USER_TOKEN";

pub const SEARCH_TOOL_CANDIDATES: &[&str] = &[
    "search_public_and_private",
    "search_messages",
    "search_public",
];

const START_TIMEOUT: Duration = Duration::from_secs(15);
const CALL_TIMEOUT: Duration = Duration::from_mins(1);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

pub fn resolve_auth(credentials: &CredentialStore, account: &str) -> IntegrationResult<String> {
    let key = CredentialKey::integration("slack", account);
    match credentials.resolve(&key, Some(ENV_VAR)) {
        Some(Credential::ApiKey(secret) | Credential::ApiKeyWithEndpoint { secret, .. }) => {
            Ok(secret.expose().to_string())
        }
        Some(Credential::OAuth(tokens)) => Ok(tokens.access_token.expose().to_string()),
        None => Err(IntegrationError::Auth(format!(
            "no slack credential for account `{account}`; run `goat integration add slack` or set {ENV_VAR}"
        ))),
    }
}

pub struct SlackSession {
    service: RunningService<RoleClient, ()>,
}

pub async fn connect(token: &str) -> IntegrationResult<SlackSession> {
    let config =
        StreamableHttpClientTransportConfig::with_uri(MCP_URL).auth_header(token.to_owned());
    let transport =
        StreamableHttpClientTransport::with_client(rmcp_reqwest::Client::default(), config);
    let service = tokio::time::timeout(
        START_TIMEOUT,
        ().serve_with_ct(transport, CancellationToken::new()),
    )
    .await
    .map_err(|_| {
        IntegrationError::Service(format!(
            "slack mcp initialize timed out after {}s",
            START_TIMEOUT.as_secs()
        ))
    })?
    .map_err(|e| IntegrationError::Service(format!("slack mcp initialize failed: {e}")))?;
    Ok(SlackSession { service })
}

impl SlackSession {
    pub fn server_name(&self) -> String {
        self.service
            .peer_info()
            .and_then(|info| info.server_info.clone())
            .map_or_else(|| "slack mcp".to_string(), |server| server.name)
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
            .map_err(|e| IntegrationError::Service(format!("slack mcp request failed: {e}")))?;
        let result = handle
            .await_response()
            .await
            .map_err(|e| IntegrationError::Service(format!("slack mcp request failed: {e}")))?;
        let ServerResult::CallToolResult(result) = result else {
            return Err(IntegrationError::Service(
                "unexpected slack mcp response".into(),
            ));
        };
        if result.is_error.unwrap_or(false) {
            return Err(IntegrationError::Service(format!(
                "slack mcp tool `{tool}` failed: {}",
                result_text(&result)
            )));
        }
        Ok(result_value(result))
    }

    pub async fn list_tools(&self) -> IntegrationResult<Vec<rmcp::model::Tool>> {
        tokio::time::timeout(CALL_TIMEOUT, self.service.list_all_tools())
            .await
            .map_err(|_| IntegrationError::Service("slack mcp list_tools timed out".into()))?
            .map_err(|e| IntegrationError::Service(format!("slack mcp list_tools failed: {e}")))
    }

    pub async fn close(mut self) {
        if let Err(e) = self.service.close_with_timeout(CLOSE_TIMEOUT).await {
            warn!(error = %e, "failed to close slack mcp session");
        }
    }
}

pub fn pick_search_tool<'a, I: IntoIterator<Item = &'a str>>(available: I) -> Option<String> {
    let available: Vec<&str> = available.into_iter().collect();
    SEARCH_TOOL_CANDIDATES.iter().find_map(|candidate| {
        available
            .iter()
            .find(|name| name.trim_start_matches("slack_") == *candidate)
            .map(|name| (*name).to_string())
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
        .filter_map(|content| match content {
            ContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use goat_auth::{SecretString, TokenSet};

    #[test]
    fn resolve_auth_reports_missing_credential() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path().join("credentials.json"));
        assert!(matches!(
            resolve_auth(&store, "default"),
            Err(IntegrationError::Auth(_))
        ));
    }

    #[test]
    fn resolve_auth_reads_stored_api_key_and_oauth_access_token() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path().join("credentials.json"));
        store
            .store(
                &CredentialKey::integration("slack", "default"),
                Credential::ApiKey(SecretString::from("xoxp-1")),
            )
            .unwrap();
        assert_eq!(resolve_auth(&store, "default").unwrap(), "xoxp-1");

        store
            .store(
                &CredentialKey::integration("slack", "other"),
                Credential::OAuth(TokenSet {
                    access_token: SecretString::from("xoxp-2"),
                    refresh_token: None,
                    expires_at: None,
                }),
            )
            .unwrap();
        assert_eq!(resolve_auth(&store, "other").unwrap(), "xoxp-2");
    }

    #[test]
    fn search_tool_is_picked_with_or_without_vendor_prefix() {
        assert_eq!(
            pick_search_tool(["slack_read_channel", "slack_search_public_and_private"]),
            Some("slack_search_public_and_private".to_string())
        );
        assert_eq!(
            pick_search_tool(["search_messages"]),
            Some("search_messages".to_string())
        );
        assert_eq!(pick_search_tool(["read_channel"]), None);
    }

    #[test]
    fn search_tool_prefers_the_widest_scope() {
        assert_eq!(
            pick_search_tool(["search_public", "search_public_and_private"]),
            Some("search_public_and_private".to_string())
        );
    }
}
