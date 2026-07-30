mod parse;
mod watch;

use std::sync::Arc;

use goat_integration::query::{KeySpec, LimitSpec, Residue, TermPolicy, WatchVocabulary};
use goat_integration::{IntegrationError, IntegrationFactory, IntegrationResult};
use goat_integration_mcp::{AuthScheme, IdentityProbe, McpService, ServiceUrl, ToolPolicy};
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
     the watcher stays off until you declare a workflow in the agent's `watch` section, e.g.\n\
     { \"source\": \"tiro\", \"query\": \"workspace:<name>\" } or { \"source\": \"tiro\", \"query\": \"folder:<id>\" } —\n\
     known keys: workspace, folder, limit; at least one of workspace/folder is required;\n\
     find values with `tiro_list_workspaces` and `tiro_search_private_folders`.\n\
     to run headless, or to recover if the browser flow fails, set GOAT_TIRO_API_KEY to a Tiro api key.";

pub const VOCABULARY: WatchVocabulary = WatchVocabulary {
    integration: "tiro",
    residue: Residue::Reject,
    terms: TermPolicy::Reject,
    limit: Some(LimitSpec {
        default: 50,
        max: 250,
    }),
    keys: &[KeySpec::new("workspace"), KeySpec::new("folder")],
};

pub fn service() -> McpService {
    McpService::new("tiro", "Tiro", ServiceUrl::Fixed(MCP_URL), SETUP)
        .env_var(ENV_VAR)
        .token_scheme(AuthScheme::Bearer)
        .tools(ToolPolicy::all(PREFIX))
        .truncation_hint(
            "narrow the date range, request a smaller page, or fetch one note at a time",
        )
        .identity(IdentityProbe {
            tool: TOOL_AUTH_STATUS,
            describe: parse::describe_identity,
        })
        .defaults(watch::defaults)
        .compile(watch::compile)
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TiroBinding {}

const MOVED_KEYS: &[(&str, &str)] = &[
    ("workspace", "workspace:<name>"),
    ("folder_id", "folder:<id>"),
];

fn validate_config(config: &Value) -> IntegrationResult<()> {
    if let Some(object) = config.as_object() {
        for (key, query) in MOVED_KEYS {
            if object.contains_key(*key) {
                return Err(IntegrationError::Config(format!(
                    "tiro binding: `{key}` moved to the agent-level `watch` section; \
                     write {{ \"source\": \"tiro\", \"query\": \"{query}\" }} there instead"
                )));
            }
        }
    }
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
    use goat_integration::query::assert_vocabulary;
    use goat_integration::{Integration, IntegrationAuth};
    use serde_json::json;

    #[test]
    fn the_vocabulary_holds_its_invariants() {
        assert_vocabulary(&VOCABULARY);
    }

    #[test]
    fn the_binding_keeps_only_connection_keys() {
        assert!(validate_config(&json!({})).is_ok());
        assert!(validate_config(&json!({ "account": "work", "client_id": "cid" })).is_ok());
        assert!(validate_config(&json!("nope")).is_err());
        assert!(validate_config(&json!({ "folderid": "F2" })).is_err());
    }

    #[test]
    fn an_old_workspace_key_points_at_the_watch_section() {
        let err = validate_config(&json!({ "workspace": "W1" })).unwrap_err();
        assert_eq!(
            err.to_string(),
            "config: tiro binding: `workspace` moved to the agent-level `watch` section; \
             write { \"source\": \"tiro\", \"query\": \"workspace:<name>\" } there instead"
        );
    }

    #[test]
    fn an_old_folder_id_key_points_at_the_watch_section() {
        let err = validate_config(&json!({ "folder_id": "F2" })).unwrap_err();
        assert_eq!(
            err.to_string(),
            "config: tiro binding: `folder_id` moved to the agent-level `watch` section; \
             write { \"source\": \"tiro\", \"query\": \"folder:<id>\" } there instead"
        );
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
        assert!(service().compile.is_some());
        assert!(service().defaults.is_some());
        assert!(meta.setup.contains("GOAT_TIRO_API_KEY"));
        assert!(meta.setup.contains("workspace:<name>"));
        assert!(meta.setup.contains("folder:<id>"));
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

    #[tokio::test]
    async fn the_watcher_honours_the_shared_contract() {
        use goat_integration::diff::SETTLE;
        use goat_integration::test_support::{WatchContract, assert_watch_contract};
        use goat_types::IntegrationUpdateKind;

        assert_watch_contract(&WatchContract {
            integration: ID,
            stream: "notes".to_owned(),
            kind: IntegrationUpdateKind::Updated,
            entity: "note",
            diff: SETTLE,
        })
        .await;
    }
}
