mod watch;

use std::sync::Arc;

use goat_integration::query::{KeySpec, LimitSpec, Residue, TermPolicy, WatchVocabulary};

use goat_integration::{IntegrationFactory, IntegrationResult};
use goat_integration_mcp::{McpService, NameRule, ServiceUrl, ToolPolicy};
use goat_types::IntegrationId;
use serde::Deserialize;
use serde_json::Value;

pub const ID: IntegrationId = IntegrationId::from_static("datadog");
pub const PREFIX: &str = "datadog_";

const DEFAULT_HOST: &str = "https://mcp.datadoghq.com";
const MCP_PATH: &str = "/v1/mcp";
const ENV_VAR: &str = "GOAT_DATADOG_TOKEN";

const SETUP: &str = "connects to Datadog's hosted MCP server; a browser window will ask you to approve access.\n\
     the default watch briefs monitors that are alerting.\n\
     outside the US1 site, add `\"host\": \"https://mcp.datadoghq.eu\"` (or your site's host) to the agent's datadog binding in ~/.goat/agents/<slug>/config.json.\n\
     to run headless, or to recover if the browser flow fails, set GOAT_DATADOG_TOKEN.\n\
     deletion tools are refused; tighten further with `deny_prefixes` or `deny_suffixes` in the agent's binding";

pub const DENY: &[NameRule] = &[
    NameRule::Prefix("delete_"),
    NameRule::Prefix("delete-"),
    NameRule::Suffix("_delete"),
    NameRule::Suffix("-delete"),
];

pub const VOCABULARY: WatchVocabulary = WatchVocabulary {
    integration: "datadog",
    residue: Residue::Reject,
    terms: TermPolicy::Reject,
    limit: Some(LimitSpec {
        default: 50,
        max: 200,
    }),
    keys: &[
        KeySpec::new("state")
            .many()
            .negatable()
            .one_of(&["alert", "warn", "no_data", "ok"]),
        KeySpec::new("tag").many(),
        KeySpec::new("name"),
    ],
};

pub fn service() -> McpService {
    McpService::new(
        "datadog",
        "Datadog",
        ServiceUrl::FromHost {
            default: DEFAULT_HOST,
            path: MCP_PATH,
        },
        SETUP,
    )
    .env_var(ENV_VAR)
    .tools(ToolPolicy::all(PREFIX).deny(DENY))
    .truncation_hint("narrow the time window, request fewer series, or add a filter and call again")
    .defaults(watch::defaults)
    .watch(&VOCABULARY, watch::compile)
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatadogBinding {
    #[serde(default, deserialize_with = "meaningful")]
    pub host: Option<String>,
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
    goat_integration_mcp::validate_binding::<DatadogBinding>("datadog", config)
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
        assert!(validate_config(&json!({ "host": "https://mcp.datadoghq.eu" })).is_ok());
        assert!(validate_config(&json!("nope")).is_err());
        assert!(validate_config(&json!({ "site": "eu" })).is_err());
    }

    #[test]
    fn factory_is_registered_in_inventory() {
        let registry = goat_integration::registry_from_inventory();
        assert!(registry.contains_key("datadog"));
        assert!(goat_integration::factory_for("datadog").is_some());
    }

    #[test]
    fn metadata_says_this_integration_only_brings_tools() {
        let meta = service().build().metadata();
        assert_eq!(meta.id, "datadog");
        assert_eq!(meta.display, "Datadog");
        assert_eq!(meta.auth, IntegrationAuth::OAuth);
        assert_eq!(meta.env_var, Some(ENV_VAR));
        assert!(service().compile.is_some());
        assert!(service().defaults.is_some());
    }

    #[test]
    fn the_site_comes_from_the_binding_and_falls_back_to_us1() {
        let url = service().url;
        assert_eq!(
            url.resolve(&json!({})).unwrap(),
            "https://mcp.datadoghq.com/v1/mcp"
        );
        assert_eq!(
            url.resolve(&json!({ "host": "https://mcp.datadoghq.eu" }))
                .unwrap(),
            "https://mcp.datadoghq.eu/v1/mcp"
        );
        assert!(url.resolve(&json!({ "host": "mcp.datadoghq.eu" })).is_err());
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
            entity: "monitor",
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
    }
}
