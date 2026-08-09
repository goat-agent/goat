mod watch;

use std::sync::Arc;

use goat_integration::query::{KeySpec, LimitSpec, Residue, TermPolicy, WatchVocabulary};
use goat_integration::{IntegrationFactory, IntegrationResult};
use goat_integration_mcp::{McpService, NameRule, ServiceUrl, ToolPolicy};
use goat_types::IntegrationId;
use serde::Deserialize;
use serde_json::Value;

pub const ID: IntegrationId = IntegrationId::from_static("atlassian");
pub const PREFIX: &str = "atlassian_";

const MCP_URL: &str = "https://mcp.atlassian.com/v1/sse";
const ENV_VAR: &str = "GOAT_ATLASSIAN_TOKEN";

const SETUP: &str = "connects to Atlassian's hosted Rovo MCP server (Jira and Confluence); a browser window will ask you to approve access.\n\
     add `\"cloud_id\": \"<id>\"` to the agent's atlassian binding in ~/.goat/agents/<slug>/config.json — the `atlassian_getAccessibleAtlassianResources` tool prints it. without it the tools still work but the watch stays off.\n\
     to run headless, or to recover if the browser flow fails, set GOAT_ATLASSIAN_TOKEN.\n\
     deletion tools are refused; tighten further with `deny_prefixes` or `deny_suffixes` in the agent's binding";

pub const DENY: &[NameRule] = &[
    NameRule::Prefix("delete"),
    NameRule::Prefix("delete_"),
    NameRule::Prefix("delete-"),
    NameRule::Suffix("_delete"),
    NameRule::Suffix("-delete"),
];

pub const VOCABULARY: WatchVocabulary = WatchVocabulary {
    integration: "atlassian",
    residue: Residue::Keep,
    terms: TermPolicy::Collect,
    limit: Some(LimitSpec {
        default: 50,
        max: 100,
    }),
    keys: &[
        KeySpec::new("assignee").many().negatable().selfref(),
        KeySpec::new("project").many().negatable(),
        KeySpec::new("status").many().negatable(),
        KeySpec::new("type").many().negatable(),
        KeySpec::new("priority").many().negatable(),
        KeySpec::new("label").many().negatable(),
        KeySpec::new("is")
            .many()
            .negatable()
            .one_of(&["open", "closed"]),
    ],
};

pub fn service() -> McpService {
    McpService::new("atlassian", "Atlassian", ServiceUrl::Fixed(MCP_URL), SETUP)
        .env_var(ENV_VAR)
        .tools(ToolPolicy::all(PREFIX).deny(DENY))
        .truncation_hint("narrow the JQL, request fewer fields, or fetch a single issue instead")
        .defaults(watch::defaults)
        .watch(&VOCABULARY, watch::compile)
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AtlassianBinding {
    #[serde(default, deserialize_with = "meaningful")]
    pub cloud_id: Option<String>,
}

impl AtlassianBinding {
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
    goat_integration_mcp::validate_binding::<AtlassianBinding>("atlassian", config)
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
    use goat_integration::test_support::{WatchContract, assert_watch_contract};
    use goat_integration::{Integration, IntegrationAuth};
    use goat_integration_mcp::Enable;
    use goat_types::IntegrationUpdateKind;
    use serde_json::json;

    #[test]
    fn the_binding_keeps_only_connection_keys() {
        assert!(validate_config(&json!({})).is_ok());
        assert!(validate_config(&json!({ "account": "work", "client_id": "cid" })).is_ok());
        assert!(validate_config(&json!({ "cloud_id": "abc" })).is_ok());
        assert!(validate_config(&json!("nope")).is_err());
        assert!(validate_config(&json!({ "assignee": "@me" })).is_err());
    }

    #[test]
    fn factory_is_registered_in_inventory() {
        let registry = goat_integration::registry_from_inventory();
        assert!(registry.contains_key("atlassian"));
        assert!(goat_integration::factory_for("atlassian").is_some());
    }

    #[test]
    fn metadata_names_the_service_and_its_watch() {
        let meta = service().build().metadata();
        assert_eq!(meta.id, "atlassian");
        assert_eq!(meta.display, "Atlassian");
        assert_eq!(meta.auth, IntegrationAuth::OAuth);
        assert_eq!(meta.env_var, Some(ENV_VAR));
        assert!(service().compile.is_some());
        assert!(service().defaults.is_some());
        assert!(meta.setup.contains("cloud_id"));
    }

    #[test]
    fn the_vocabulary_holds_its_invariants() {
        goat_integration::query::assert_vocabulary(&VOCABULARY);
    }

    #[test]
    fn the_tool_policy_exposes_everything_but_deletion() {
        let policy = service().tools;
        assert_eq!(policy.prefix, PREFIX);
        assert!(matches!(policy.enable, Enable::All));
        assert!(policy.deny.contains(&NameRule::Prefix("delete")));
    }

    #[tokio::test]
    async fn the_watcher_honours_the_shared_contract() {
        assert_watch_contract(&WatchContract {
            integration: ID,
            stream: watch::STREAM.to_owned(),
            kind: IntegrationUpdateKind::Assigned,
            entity: "issue",
            diff: goat_integration::diff::REBUILD,
        })
        .await;
    }
}
