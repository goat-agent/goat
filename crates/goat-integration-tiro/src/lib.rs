mod parse;
mod watch;

use std::sync::Arc;

use goat_integration::{IntegrationFactory, IntegrationResult};
use goat_integration_mcp::{IdentityProbe, McpService};
use goat_types::IntegrationId;
use serde::Deserialize;
use serde_json::Value;

pub const ID: IntegrationId = IntegrationId::from_static("tiro");
pub const PREFIX: &str = "tiro_";

const MCP_URL: &str = "https://mcp.tiro.ooo/mcp";
const ENV_VAR: &str = "GOAT_TIRO_API_KEY";
const TOOL_AUTH_STATUS: &str = "auth_status";

const SETUP: &str = "connects to Tiro's hosted MCP server; a browser window will ask you to approve access.\n\
     the scopes you were actually granted are printed on connect — an oauth session can be read-only, and folder or share-link writes then need an api key instead.\n\
     the watcher stays off until you set `workspace` or `folder_id` in the agent's tiro binding; find them with `tiro_list_workspaces` and `tiro_search_private_folders`.\n\
     to run headless, or to recover if the browser flow fails, set GOAT_TIRO_API_KEY to a Tiro api key.";

pub fn service() -> McpService {
    McpService::new("tiro", "Tiro", MCP_URL, SETUP)
        .with_env_var(ENV_VAR)
        .with_auth_scheme("Bearer")
        .with_tool_prefix(PREFIX)
        .with_truncation_hint(
            "narrow the date range, request a smaller page, or fetch one note at a time",
        )
        .with_identity(IdentityProbe {
            tool: TOOL_AUTH_STATUS,
            describe: parse::describe_identity,
        })
        .with_watch(watch::spawn)
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TiroBinding {
    #[serde(default, deserialize_with = "meaningful")]
    pub workspace: Option<String>,
    #[serde(default, deserialize_with = "meaningful")]
    pub folder_id: Option<String>,
}

impl TiroBinding {
    pub(crate) fn read(config: &Value) -> Self {
        goat_integration_mcp::read_binding(config)
    }
}

fn meaningful<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Option<String> = Option::deserialize(deserializer)?;
    Ok(raw
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty()))
}

fn validate_config(config: &Value) -> IntegrationResult<()> {
    goat_integration_mcp::validate_binding::<TiroBinding>("tiro", config)
}

inventory::submit! {
    IntegrationFactory {
        id: ID,
        ctor: || Arc::new(service().build()),
        validate_config,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use goat_integration::{Integration, IntegrationAuth, IntegrationError};
    use serde_json::json;

    #[test]
    fn the_binding_is_typo_checked() {
        assert!(validate_config(&json!({})).is_ok());
        assert!(validate_config(&json!({ "account": "work", "client_id": "cid" })).is_ok());
        assert!(validate_config(&json!({ "workspace": "W1", "folder_id": "F2" })).is_ok());
        assert!(validate_config(&json!("nope")).is_err());
        assert!(validate_config(&json!({ "workspace": 3 })).is_err());
        assert!(validate_config(&json!({ "folderid": "F2" })).is_err());
    }

    #[test]
    fn factory_is_registered_in_inventory() {
        let registry = goat_integration::registry_from_inventory();
        assert!(registry.contains_key("tiro"));
        assert!(goat_integration::factory_for("tiro").is_some());
    }

    #[test]
    fn metadata_advertises_oauth_with_an_api_key_escape_hatch() {
        let meta = service().build().metadata();
        assert_eq!(meta.id, "tiro");
        assert_eq!(meta.display, "Tiro");
        assert_eq!(meta.auth, IntegrationAuth::OAuth);
        assert_eq!(meta.env_var, Some("GOAT_TIRO_API_KEY"));
        assert!(meta.has_watcher);
        assert!(meta.setup.contains("GOAT_TIRO_API_KEY"));
    }

    #[test]
    fn verify_probes_the_credential_rather_than_the_server_name() {
        let probe = service().identity.expect("an identity probe");
        assert_eq!(probe.tool, "auth_status");

        let rendered = (probe.describe)(&json!({
            "userId": 42,
            "authMethod": "oauth",
            "scopes": ["notes:read", "folders:read"]
        }))
        .unwrap();
        assert_eq!(rendered, "tiro user 42 (oauth) · notes:read, folders:read");
    }

    #[test]
    fn an_unauthenticated_credential_is_an_auth_error_not_a_name() {
        let probe = service().identity.expect("an identity probe");
        let err = (probe.describe)(&json!({ "authenticated": false })).unwrap_err();
        assert!(matches!(err, IntegrationError::Auth(_)));
    }

    #[test]
    fn the_watcher_stays_off_until_a_workspace_or_folder_is_named() {
        let empty = TiroBinding::read(&json!({}));
        assert!(empty.workspace.is_none() && empty.folder_id.is_none());
        let blank = TiroBinding::read(&json!({ "workspace": "  " }));
        assert!(blank.workspace.is_none());
        let set = TiroBinding::read(&json!({ "folder_id": " F2 " }));
        assert_eq!(set.folder_id, Some("F2".to_owned()));
    }

    #[tokio::test]
    async fn the_watcher_honours_the_shared_contract() {
        use goat_integration::diff::SETTLE;
        use goat_integration::test_support::{WatchContract, assert_watch_contract};
        use goat_types::IntegrationUpdateKind;

        assert_watch_contract(&WatchContract {
            integration: ID,
            stream: watch::STREAM.to_owned(),
            kind: IntegrationUpdateKind::Updated,
            entity: "note",
            overflow_tail: "notes waiting",
            diff: SETTLE,
        })
        .await;
    }
}
