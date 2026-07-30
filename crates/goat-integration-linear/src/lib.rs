mod parse;
mod watch;

use std::sync::Arc;

use goat_integration::query::{KeySpec, LimitSpec, Residue, TermPolicy, WatchVocabulary};
use goat_integration::{IntegrationError, IntegrationFactory, IntegrationResult};
use goat_integration_mcp::{McpService, ServiceUrl, ToolPolicy};
use goat_types::IntegrationId;
use serde::Deserialize;
use serde_json::Value;

pub const ID: IntegrationId = IntegrationId::from_static("linear");
pub const PREFIX: &str = "linear_";

const MCP_URL: &str = "https://mcp.linear.app/mcp";
const ENV_VAR: &str = "LINEAR_API_KEY";

const SETUP: &str = "connects to Linear's hosted MCP server; a browser window will ask you to approve access.\n\
     by default the watcher briefs you on open issues assigned to you (`assignee:@me is:open`).\n\
     declare workflows in the agent's `watch` section to change that, e.g.\n\
     { \"source\": \"linear\", \"query\": \"assignee:@me is:open label:bug priority:urgent limit:25\" } —\n\
     known keys: assignee, team, project, label, state, cycle, priority, is:open/closed, limit; free text searches title and body.\n\
     to run headless, set LINEAR_API_KEY to a Linear personal API key.";

pub const VOCABULARY: WatchVocabulary = WatchVocabulary {
    integration: "linear",
    residue: Residue::Reject,
    terms: TermPolicy::Collect,
    limit: Some(LimitSpec {
        default: 50,
        max: 250,
    }),
    keys: &[
        KeySpec::new("assignee").selfref(),
        KeySpec::new("team"),
        KeySpec::new("project"),
        KeySpec::new("label"),
        KeySpec::new("state"),
        KeySpec::new("cycle"),
        KeySpec::new("priority").one_of(&["urgent", "high", "medium", "low", "none"]),
        KeySpec::new("is")
            .many()
            .negatable()
            .one_of(&["open", "closed"]),
    ],
};

pub fn service() -> McpService {
    McpService::new("linear", "Linear", ServiceUrl::Fixed(MCP_URL), SETUP)
        .env_var(ENV_VAR)
        .tools(ToolPolicy::all(PREFIX))
        .truncation_hint("narrow the filter, request fewer issues, or fetch a single issue instead")
        .defaults(watch::defaults)
        .compile(watch::compile)
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LinearBinding {}

const MOVED_KEYS: &[&str] = &["assignee", "team", "project", "include_closed"];

fn validate_config(config: &Value) -> IntegrationResult<()> {
    if let Some(object) = config.as_object() {
        for key in MOVED_KEYS {
            if object.contains_key(*key) {
                return Err(IntegrationError::Config(format!(
                    "linear binding: `{key}` moved to the agent-level `watch` section; \
                     write {{ \"source\": \"linear\", \"query\": \"...\" }} there instead"
                )));
            }
        }
    }
    goat_integration_mcp::validate_binding::<LinearBinding>("linear", config)
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
        assert!(validate_config(&json!({ "unknown": true })).is_err());
    }

    #[test]
    fn an_old_policy_key_points_at_the_watch_section() {
        let err = validate_config(&json!({ "assignee": "jmo" })).unwrap_err();
        assert!(err.to_string().contains("agent-level `watch` section"));
        let err = validate_config(&json!({ "include_closed": true })).unwrap_err();
        assert!(err.to_string().contains("watch"));
    }

    #[test]
    fn factory_is_registered_in_inventory() {
        let registry = goat_integration::registry_from_inventory();
        assert!(registry.contains_key("linear"));
        assert!(goat_integration::factory_for("linear").is_some());
    }

    #[test]
    fn metadata_advertises_oauth_with_a_headless_escape_hatch() {
        let meta = service().build().metadata();
        assert_eq!(meta.id, "linear");
        assert_eq!(meta.display, "Linear");
        assert_eq!(meta.auth, IntegrationAuth::OAuth);
        assert_eq!(meta.env_var, Some("LINEAR_API_KEY"));
        assert!(service().compile.is_some());
        assert!(service().defaults.is_some());
        assert!(meta.setup.contains("LINEAR_API_KEY"));
        assert!(meta.setup.contains("assignee:@me"));
    }

    #[tokio::test]
    async fn the_watcher_honours_the_shared_contract() {
        use goat_integration::diff::REBUILD;
        use goat_integration::test_support::{WatchContract, assert_watch_contract};
        use goat_types::IntegrationUpdateKind;

        assert_watch_contract(&WatchContract {
            integration: ID,
            stream: watch::STREAM.to_owned(),
            kind: IntegrationUpdateKind::Assigned,
            entity: "issue",
            diff: REBUILD,
        })
        .await;
    }
}
