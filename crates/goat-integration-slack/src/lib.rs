mod diff;
mod mcp;
mod parse;
mod tool;
mod watcher;

use std::sync::{Arc, OnceLock};

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

pub const ID: IntegrationId = IntegrationId::from_static("slack");

const SETUP: &str = "1. create the goat app in your workspace: https://api.slack.com/apps?new_app=1&manifest_yaml=display_information%3A%0A%20%20name%3A%20goat%0A%20%20description%3A%20Personal%20AI%20agent%20access%20to%20Slack%0Aoauth_config%3A%0A%20%20scopes%3A%0A%20%20%20%20user%3A%0A%20%20%20%20%20%20-%20search%3Aread.public%0A%20%20%20%20%20%20-%20search%3Aread.private%0A%20%20%20%20%20%20-%20search%3Aread.im%0A%20%20%20%20%20%20-%20search%3Aread.mpim%0A%20%20%20%20%20%20-%20search%3Aread.users%0A%20%20%20%20%20%20-%20channels%3Ahistory%0A%20%20%20%20%20%20-%20groups%3Ahistory%0A%20%20%20%20%20%20-%20im%3Ahistory%0A%20%20%20%20%20%20-%20mpim%3Ahistory%0A%20%20%20%20%20%20-%20users%3Aread%0A%20%20%20%20%20%20-%20emoji%3Aread%0A%20%20%20%20%20%20-%20chat%3Awrite%0A%20%20%20%20%20%20-%20reactions%3Awrite%0A%20%20%20%20%20%20-%20canvases%3Aread%0A%20%20%20%20%20%20-%20canvases%3Awrite%0Asettings%3A%0A%20%20org_deploy_enabled%3A%20false%0A%20%20socket_mode_enabled%3A%20false%0A%20%20token_rotation_enabled%3A%20false%0A\n2. in that app, open Agents & AI Apps and turn on the Slack MCP Server — scopes alone are not enough\n3. Install to Workspace, then copy the User OAuth Token (xoxp-…) from OAuth & Permissions";

pub struct SlackIntegration;

#[async_trait]
impl Integration for SlackIntegration {
    fn id(&self) -> IntegrationId {
        ID
    }

    fn metadata(&self) -> IntegrationMetadata {
        IntegrationMetadata {
            id: "slack",
            display: "Slack",
            auth: IntegrationAuth::Secret,
            secret_label: "Slack user OAuth token (xoxp-…)",
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
        let Some(user_id) = string_setting(&binding.config, "user_id") else {
            warn!(
                profile = %persona,
                "slack watcher disabled; set `user_id` to your Slack member ID in the agent's slack binding",
            );
            return None;
        };
        let fetch = watcher::McpFetch {
            credentials: runtime.credentials.clone(),
            account: binding.account.clone(),
            query: string_setting(&binding.config, "query")
                .unwrap_or_else(|| format!("<@{user_id}>")),
            search_tool: string_setting(&binding.config, "search_tool"),
            resolved_tool: OnceLock::new(),
        };
        Some(tokio::spawn(watcher::run(
            persona,
            runtime,
            binding.account,
            user_id,
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
        let token = mcp::resolve_auth(credentials, account)?;
        let session = mcp::connect(&token).await?;
        let name = session.server_name();
        session.close().await;
        Ok(name)
    }
}

fn string_setting(config: &Value, key: &str) -> Option<String> {
    config
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn validate_config(config: &Value) -> IntegrationResult<()> {
    let obj = config
        .as_object()
        .ok_or_else(|| IntegrationError::Config("slack binding must be an object".into()))?;
    for key in ["account", "user_id", "query", "search_tool"] {
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
        ctor: || Arc::new(SlackIntegration),
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
        assert!(validate_config(&json!({ "account": "work", "user_id": "U1" })).is_ok());
        assert!(validate_config(&json!({ "query": "in:#eng", "search_tool": "x" })).is_ok());
        assert!(validate_config(&json!("nope")).is_err());
        assert!(validate_config(&json!({ "user_id": 3 })).is_err());
        assert!(validate_config(&json!({ "search_tool": false })).is_err());
    }

    #[test]
    fn factory_is_registered_in_inventory() {
        let registry = goat_integration::registry_from_inventory();
        assert!(registry.contains_key("slack"));
        assert!(goat_integration::factory_for("slack").is_some());
    }

    #[test]
    fn metadata_uses_a_pasted_secret_not_an_oauth_round_trip() {
        let meta = SlackIntegration.metadata();
        assert!(matches!(meta.auth, IntegrationAuth::Secret));
        assert_eq!(meta.env_var, Some("SLACK_USER_TOKEN"));
        assert!(meta.setup.contains("Agents & AI Apps"));
        assert!(meta.setup.contains("api.slack.com/apps?new_app=1"));
    }

    #[test]
    fn watcher_stays_off_until_the_owner_supplies_a_member_id() {
        assert_eq!(string_setting(&json!({ "user_id": "" }), "user_id"), None);
        assert_eq!(
            string_setting(&json!({ "user_id": "U1" }), "user_id"),
            Some("U1".to_string())
        );
    }
}
