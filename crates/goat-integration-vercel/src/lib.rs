mod watch;

use std::sync::Arc;

use goat_integration::query::{KeySpec, LimitSpec, Residue, TermPolicy, WatchVocabulary};

use goat_integration::{IntegrationFactory, IntegrationResult};
use goat_integration_mcp::{McpService, NameRule, ServiceUrl, ToolPolicy};
use goat_types::IntegrationId;
use serde::Deserialize;
use serde_json::Value;

pub const ID: IntegrationId = IntegrationId::from_static("vercel");
pub const PREFIX: &str = "vercel_";

const MCP_URL: &str = "https://mcp.vercel.com/";
const ENV_VAR: &str = "GOAT_VERCEL_TOKEN";

const SETUP: &str = "connects to Vercel's hosted MCP server; a browser window will ask you to approve access.\n\
     the watch is off until you ask for it: add \"team\": \"team_…\" to the agent's vercel binding, plus a workflow whose query names a project, as in `project:goat-web state:error`. the deployment list tool takes no wildcard, so there is no zero-config default.\n\
     to run headless, or to recover if the browser flow fails, set GOAT_VERCEL_TOKEN.\n\
     deletion tools are refused; tighten further with `deny_prefixes` or `deny_suffixes` in the agent's binding";

pub const DENY: &[NameRule] = &[
    NameRule::Prefix("delete_"),
    NameRule::Prefix("delete-"),
    NameRule::Suffix("_delete"),
    NameRule::Suffix("-delete"),
];

pub const VOCABULARY: WatchVocabulary = WatchVocabulary {
    integration: "vercel",
    residue: Residue::Reject,
    terms: TermPolicy::Reject,
    limit: Some(LimitSpec {
        default: 20,
        max: 100,
    }),
    keys: &[
        KeySpec::new("project"),
        KeySpec::new("state").many().negatable().one_of(&[
            "error",
            "ready",
            "building",
            "queued",
            "canceled",
            "initializing",
        ]),
    ],
};

pub fn service() -> McpService {
    McpService::new("vercel", "Vercel", ServiceUrl::Fixed(MCP_URL), SETUP)
        .env_var(ENV_VAR)
        .tools(ToolPolicy::all(PREFIX).deny(DENY))
        .truncation_hint("narrow the project or time window, or fetch a single deployment instead")
        .defaults(watch::defaults)
        .watch(&VOCABULARY, watch::compile)
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VercelBinding {
    #[serde(default, deserialize_with = "meaningful")]
    pub team: Option<String>,
}

impl VercelBinding {
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
    goat_integration_mcp::validate_binding::<VercelBinding>("vercel", config)
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
    use goat_integration::{Integration, IntegrationAuth};
    use goat_integration_mcp::Enable;
    use serde_json::json;

    #[test]
    fn the_binding_keeps_only_connection_keys() {
        assert!(validate_config(&json!({})).is_ok());
        assert!(validate_config(&json!({ "account": "work", "client_id": "cid" })).is_ok());
        assert!(validate_config(&json!({ "deny_suffixes": ["-delete"] })).is_ok());
        assert!(validate_config(&json!({ "team": "team_1" })).is_ok());
        assert!(validate_config(&json!("nope")).is_err());
        assert!(validate_config(&json!({ "assignee": "@me" })).is_err());
    }

    #[test]
    fn factory_is_registered_in_inventory() {
        let registry = goat_integration::registry_from_inventory();
        assert!(registry.contains_key("vercel"));
        assert!(goat_integration::factory_for("vercel").is_some());
    }

    #[test]
    fn metadata_says_this_integration_only_brings_tools() {
        let meta = service().build().metadata();
        assert_eq!(meta.id, "vercel");
        assert_eq!(meta.display, "Vercel");
        assert_eq!(meta.auth, IntegrationAuth::OAuth);
        assert_eq!(meta.env_var, Some(ENV_VAR));
        assert!(service().compile.is_some());
        assert!(meta.setup.contains("project:goat-web"));
    }

    #[test]
    fn the_vocabulary_holds_its_invariants() {
        goat_integration::query::assert_vocabulary(&VOCABULARY);
    }

    #[tokio::test]
    async fn the_watcher_honours_the_shared_contract() {
        use goat_integration::test_support::{WatchContract, assert_watch_contract};
        assert_watch_contract(&WatchContract {
            integration: ID,
            stream: watch::STREAM.to_owned(),
            kind: goat_types::IntegrationUpdateKind::Updated,
            entity: "deployment",
            diff: goat_integration::diff::SETTLE,
        })
        .await;
    }

    #[test]
    fn the_tool_policy_exposes_everything_but_deletion() {
        let policy = service().tools;
        assert_eq!(policy.prefix, PREFIX);
        assert!(matches!(policy.enable, Enable::All));
        assert!(policy.deny.contains(&NameRule::Prefix("delete_")));
        assert!(policy.deny.contains(&NameRule::Suffix("-delete")));
    }
}
