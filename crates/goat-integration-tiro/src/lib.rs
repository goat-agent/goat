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
use tracing::warn;

pub const ID: IntegrationId = IntegrationId::from_static("tiro");

const SETUP: &str = "connects to Tiro's hosted MCP server; a browser window will ask you to approve access.\n\
     the scopes you were actually granted are printed on connect — an oauth session can be read-only, and folder or share-link writes then need an api key instead.\n\
     the watcher stays off until you set `workspace` or `folder_id` in the agent's tiro binding; find them with `tiro_list_workspaces` and `tiro_search_private_folders`.\n\
     to run headless, or to recover if the browser flow fails, set GOAT_TIRO_API_KEY to a Tiro api key.";

const STRING_SETTINGS: [&str; 4] = ["account", "client_id", "workspace", "folder_id"];

pub struct TiroIntegration;

#[async_trait]
impl Integration for TiroIntegration {
    fn id(&self) -> IntegrationId {
        ID
    }

    fn metadata(&self) -> IntegrationMetadata {
        IntegrationMetadata {
            id: "tiro",
            display: "Tiro",
            auth: IntegrationAuth::OAuth,
            secret_label: "Tiro api key",
            env_var: Some(mcp::ENV_VAR),
            setup: SETUP,
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
        let workspace = string_setting(&binding.config, "workspace");
        let folder_id = string_setting(&binding.config, "folder_id");
        if workspace.is_none() && folder_id.is_none() {
            warn!(
                profile = %persona,
                "tiro watcher disabled; set `workspace` or `folder_id` in the agent's tiro binding",
            );
            return None;
        }
        let fetch = watcher::McpFetch {
            credentials: runtime.credentials.clone(),
            account: binding.account.clone(),
            client_id: tool::client_id_of(&binding),
            workspace,
            folder_id,
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
        let identity = session.identity().await;
        mcp::persist_tokens(credentials, account, &session).await;
        session.close().await;
        identity
    }

    async fn oauth_login(
        &self,
        credentials: &CredentialStore,
        account: &str,
        present_url: &(dyn for<'a> Fn(&'a str) + Send + Sync),
    ) -> IntegrationResult<serde_json::Value> {
        use rmcp::transport::auth::OAuthState;

        let (listener, port) = goat_auth::bind_loopback()
            .await
            .map_err(|e| IntegrationError::Auth(e.to_string()))?;
        let redirect = format!("http://127.0.0.1:{port}/callback");

        let mut oauth = OAuthState::new(mcp::MCP_URL, None)
            .await
            .map_err(|e| IntegrationError::Auth(e.to_string()))?;
        oauth
            .start_authorization(&[], &redirect, Some("goat"))
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
                &goat_auth::CredentialKey::integration("tiro", account),
                goat_auth::Credential::OAuth(mcp::token_set_from_response(&tokens)?),
            )
            .map_err(|e| IntegrationError::Auth(e.to_string()))?;
        Ok(serde_json::json!({ "client_id": client_id }))
    }
}

fn string_setting(config: &Value, key: &str) -> Option<String> {
    config
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_string())
}

fn validate_config(config: &Value) -> IntegrationResult<()> {
    let obj = config
        .as_object()
        .ok_or_else(|| IntegrationError::Config("tiro binding must be an object".into()))?;
    for key in STRING_SETTINGS {
        if let Some(value) = obj.get(key)
            && !value.is_string()
        {
            return Err(IntegrationError::Config(format!(
                "`{key}` must be a string"
            )));
        }
    }
    Ok(())
}

inventory::submit! {
    IntegrationFactory {
        id: ID,
        ctor: || Arc::new(TiroIntegration),
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
        assert!(
            validate_config(&json!({
                "account": "work",
                "client_id": "client-1",
                "workspace": "ws-guid",
                "folder_id": "455765"
            }))
            .is_ok()
        );
        assert!(validate_config(&json!("nope")).is_err());
        assert!(validate_config(&json!({ "workspace": 3 })).is_err());
        assert!(validate_config(&json!({ "folder_id": ["455765"] })).is_err());
    }

    #[test]
    fn factory_is_registered_in_inventory() {
        let registry = goat_integration::registry_from_inventory();
        assert!(registry.contains_key("tiro"));
        assert!(goat_integration::factory_for("tiro").is_some());
    }

    #[test]
    fn metadata_advertises_the_prefixed_environment_override() {
        let meta = TiroIntegration.metadata();
        assert_eq!(meta.auth, IntegrationAuth::OAuth);
        assert_eq!(meta.env_var, Some("GOAT_TIRO_API_KEY"));
        assert!(meta.has_watcher);
        assert!(meta.setup.contains("GOAT_TIRO_API_KEY"));
        assert!(meta.setup.contains("workspace"));
    }

    #[test]
    fn watcher_settings_are_trimmed_and_blank_values_are_ignored() {
        let config = json!({ "workspace": "  ws-guid  ", "folder_id": "   " });
        assert_eq!(
            string_setting(&config, "workspace"),
            Some("ws-guid".to_string())
        );
        assert_eq!(string_setting(&config, "folder_id"), None);
        assert_eq!(string_setting(&config, "account"), None);
    }

    #[test]
    fn watcher_stays_off_until_a_scope_is_declared() {
        let bare = IntegrationBinding::from_config(json!({}));
        assert!(string_setting(&bare.config, "workspace").is_none());
        assert!(string_setting(&bare.config, "folder_id").is_none());

        let scoped = IntegrationBinding::from_config(json!({ "folder_id": "455765" }));
        assert!(string_setting(&scoped.config, "folder_id").is_some());
    }
}
