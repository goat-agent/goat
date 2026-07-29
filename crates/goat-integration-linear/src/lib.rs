mod diff;
mod mcp;
mod parse;
mod tool;
mod watcher;

use std::sync::Arc;

use async_trait::async_trait;
use goat_agent_tool::{ToolName, ToolRegistry};
use goat_auth::CredentialStore;
use goat_integration::{
    BindingMap, Integration, IntegrationAuth, IntegrationBinding, IntegrationError,
    IntegrationFactory, IntegrationMetadata, IntegrationResult, IntegrationRuntime,
};
use goat_types::{IntegrationId, ProfileId};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

pub const ID: IntegrationId = IntegrationId::from_static("linear");

pub struct LinearIntegration;

#[async_trait]
impl Integration for LinearIntegration {
    fn id(&self) -> IntegrationId {
        ID
    }

    fn metadata(&self) -> IntegrationMetadata {
        IntegrationMetadata {
            id: "linear",
            display: "Linear",
            auth: IntegrationAuth::OAuth,
            secret_label: "Linear personal API key",
            env_var: Some(mcp::ENV_VAR),
            setup: "connects to Linear's hosted MCP server; a browser window will ask you to approve access",
            has_watcher: true,
        }
    }

    async fn register_tools(
        &self,
        registry: &mut ToolRegistry,
        runtime: &IntegrationRuntime,
        bindings: Arc<BindingMap>,
    ) -> Vec<ToolName> {
        tool::register(registry, runtime, bindings).await
    }

    fn spawn_watcher(
        &self,
        persona: ProfileId,
        binding: IntegrationBinding,
        runtime: IntegrationRuntime,
        cancel: CancellationToken,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let fetch = watcher::McpFetch {
            credentials: runtime.credentials.clone(),
            account: binding.account.clone(),
            client_id: binding
                .config
                .get("client_id")
                .and_then(Value::as_str)
                .map(str::to_string),
        };
        Some(tokio::spawn(watcher::run(
            persona,
            runtime,
            binding.account,
            fetch,
            cancel,
        )))
    }

    async fn verify(
        &self,
        config: &Value,
        credentials: &CredentialStore,
    ) -> IntegrationResult<String> {
        let account = config
            .get("account")
            .and_then(Value::as_str)
            .unwrap_or("default");
        let client_id = config.get("client_id").and_then(Value::as_str);
        let auth = mcp::resolve_auth(credentials, account, client_id)?;
        let session = mcp::connect(&auth).await?;
        let name = session.server_name();
        mcp::persist_tokens(credentials, account, &session).await;
        session.close().await;
        Ok(name)
    }

    async fn oauth_login(
        &self,
        credentials: &CredentialStore,
        account: &str,
        present_url: &(dyn for<'a> Fn(&'a str) + Send + Sync),
    ) -> IntegrationResult<serde_json::Value> {
        use rmcp::transport::auth::{AuthorizationRequest, OAuthState};

        let (listener, port) = goat_auth::bind_loopback()
            .await
            .map_err(|e| IntegrationError::Auth(e.to_string()))?;
        let redirect = format!("http://127.0.0.1:{port}/callback");

        let mut oauth = OAuthState::new(mcp::MCP_URL, None)
            .await
            .map_err(|e| IntegrationError::Auth(e.to_string()))?;
        oauth
            .start_authorization(AuthorizationRequest::new(&redirect).with_client_name("goat"))
            .await
            .map_err(|e| IntegrationError::Auth(format!("authorization start failed: {e}")))?;
        let auth_url = oauth
            .get_authorization_url()
            .await
            .map_err(|e| IntegrationError::Auth(e.to_string()))?;
        let state = url::Url::parse(&auth_url)
            .ok()
            .and_then(|u| {
                u.query_pairs()
                    .find(|(k, _)| k == "state")
                    .map(|(_, v)| v.to_string())
            })
            .ok_or_else(|| IntegrationError::Auth("authorization url missing state".into()))?;

        present_url(&auth_url);
        let code = goat_auth::capture_on(listener, &state)
            .await
            .map_err(|e| IntegrationError::Auth(e.to_string()))?;
        oauth
            .handle_callback(&code, &state)
            .await
            .map_err(|e| IntegrationError::Auth(format!("token exchange failed: {e}")))?;

        let (client_id, tokens) = oauth
            .get_credentials()
            .await
            .map_err(|e| IntegrationError::Auth(e.to_string()))?;
        let tokens =
            tokens.ok_or_else(|| IntegrationError::Auth("no tokens after authorization".into()))?;
        credentials
            .store(
                &goat_auth::CredentialKey::integration("linear", account),
                goat_auth::Credential::OAuth(mcp::token_set_from_response(&tokens)?),
            )
            .map_err(|e| IntegrationError::Auth(e.to_string()))?;
        Ok(serde_json::json!({ "client_id": client_id }))
    }
}

fn validate_config(config: &Value) -> IntegrationResult<()> {
    let obj = config
        .as_object()
        .ok_or_else(|| IntegrationError::Config("linear binding must be an object".into()))?;
    if let Some(v) = obj.get("account")
        && !v.is_string()
    {
        return Err(IntegrationError::Config(
            "`account` must be a string".into(),
        ));
    }
    Ok(())
}

inventory::submit! {
    IntegrationFactory {
        id: ID,
        ctor: || Arc::new(LinearIntegration),
        validate_config,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validate_config_accepts_valid_and_rejects_invalid() {
        assert!(validate_config(&json!({})).is_ok());
        assert!(validate_config(&json!({ "account": "work" })).is_ok());
        assert!(validate_config(&json!("nope")).is_err());
        assert!(validate_config(&json!({ "account": 3 })).is_err());
    }

    #[test]
    fn factory_is_registered_in_inventory() {
        let registry = goat_integration::registry_from_inventory();
        assert!(registry.contains_key("linear"));
        assert!(goat_integration::factory_for("linear").is_some());
    }
}
