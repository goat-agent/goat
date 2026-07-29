mod mcp;
mod tool;

use std::sync::Arc;

use async_trait::async_trait;
use goat_agent_tool::{ToolName, ToolRegistry};
use goat_auth::CredentialStore;
use goat_integration::{
    BindingMap, Integration, IntegrationAuth, IntegrationError, IntegrationFactory,
    IntegrationMetadata, IntegrationResult, IntegrationRuntime,
};
use goat_types::IntegrationId;
use serde_json::Value;

pub const ID: IntegrationId = IntegrationId::from_static("posthog");

const SETUP: &str = "connects to PostHog's hosted MCP server; a browser window will ask you to approve access.\n\
     this integration adds `posthog_*` tools — it does not watch PostHog or brief you on its own.\n\
     to run headless, or to recover if the browser flow fails, set GOAT_POSTHOG_API_KEY to a PostHog personal API key (phx-…).\n\
     with more than one project, add `\"project_id\": \"<id>\"` to the agent's posthog binding in ~/.goat/agents/<slug>/config.json";

pub struct PosthogIntegration;

#[async_trait]
impl Integration for PosthogIntegration {
    fn id(&self) -> IntegrationId {
        ID
    }

    fn metadata(&self) -> IntegrationMetadata {
        IntegrationMetadata {
            id: "posthog",
            display: "PostHog",
            auth: IntegrationAuth::OAuth,
            secret_label: "PostHog personal API key (phx-…)",
            env_var: Some(mcp::ENV_VAR),
            setup: SETUP,
            has_watcher: false,
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
        let scope = mcp::ProjectScope::from_config(config);
        let session = mcp::connect(&auth, &scope).await?;
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
            .start_authorization(
                AuthorizationRequest::new(&redirect)
                    .with_client_name("goat")
                    .with_scopes(mcp::SCOPES.iter().copied()),
            )
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
                &goat_auth::CredentialKey::integration("posthog", account),
                goat_auth::Credential::OAuth(mcp::token_set_from_response(&tokens)?),
            )
            .map_err(|e| IntegrationError::Auth(e.to_string()))?;
        Ok(serde_json::json!({ "client_id": client_id }))
    }
}

fn validate_config(config: &Value) -> IntegrationResult<()> {
    let obj = config
        .as_object()
        .ok_or_else(|| IntegrationError::Config("posthog binding must be an object".into()))?;
    for key in ["account", "project_id", "organization_id", "client_id"] {
        if let Some(value) = obj.get(key)
            && !value.is_string()
        {
            return Err(IntegrationError::Config(format!(
                "`{key}` must be a string"
            )));
        }
    }
    if let Some(value) = obj.get("deny_suffixes") {
        let all_strings = value
            .as_array()
            .is_some_and(|values| values.iter().all(Value::is_string));
        if !all_strings {
            return Err(IntegrationError::Config(
                "`deny_suffixes` must be an array of strings".into(),
            ));
        }
    }
    Ok(())
}

inventory::submit! {
    IntegrationFactory {
        id: ID,
        ctor: || Arc::new(PosthogIntegration),
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
        assert!(validate_config(&json!({ "project_id": "12345" })).is_ok());
        assert!(validate_config(&json!({ "deny_suffixes": ["-delete"] })).is_ok());
        assert!(validate_config(&json!({ "organization_id": "o", "client_id": "c" })).is_ok());
        assert!(validate_config(&json!("nope")).is_err());
        assert!(validate_config(&json!({ "project_id": 3 })).is_err());
        assert!(validate_config(&json!({ "deny_suffixes": "-delete" })).is_err());
        assert!(validate_config(&json!({ "deny_suffixes": [1] })).is_err());
    }

    #[test]
    fn factory_is_registered_in_inventory() {
        let registry = goat_integration::registry_from_inventory();
        assert!(registry.contains_key("posthog"));
        assert!(goat_integration::factory_for("posthog").is_some());
    }

    #[test]
    fn metadata_declares_oauth_and_no_watcher() {
        let meta = PosthogIntegration.metadata();
        assert!(matches!(meta.auth, IntegrationAuth::OAuth));
        assert_eq!(meta.env_var, Some("GOAT_POSTHOG_API_KEY"));
        assert!(!meta.has_watcher);
    }

    #[test]
    fn setup_states_the_scope_limit_and_both_auth_paths() {
        let meta = PosthogIntegration.metadata();
        assert!(meta.setup.contains("does not watch"));
        assert!(meta.setup.contains("GOAT_POSTHOG_API_KEY"));
        assert!(meta.setup.contains("project_id"));
    }
}
