mod watch;

use std::sync::Arc;

use goat_integration::query::{KeySpec, LimitSpec, Residue, TermPolicy, WatchVocabulary};
use goat_integration::{IntegrationFactory, IntegrationResult};
use goat_integration_mcp::{McpService, NameRule, ServiceUrl, ToolPolicy};
use goat_types::IntegrationId;
use serde::Deserialize;
use serde_json::Value;

pub const ID: IntegrationId = IntegrationId::from_static("pagerduty");
pub const PREFIX: &str = "pagerduty_";

const DEFAULT_HOST: &str = "https://mcp.pagerduty.com";
const MCP_PATH: &str = "/mcp";
const ENV_VAR: &str = "GOAT_PAGERDUTY_TOKEN";

const SETUP: &str = "connects to PagerDuty's hosted MCP server; a browser window will ask you to approve access.\n\
     the default watch briefs triggered incidents. to use `assignee:@me`, add `\"user_id\": \"P…\"` to the agent's pagerduty binding in ~/.goat/agents/<slug>/config.json.\n\
     on the EU service region, add `\"host\": \"https://mcp.eu.pagerduty.com\"` to the same binding.\n\
     to run headless, or to recover if the browser flow fails, set GOAT_PAGERDUTY_TOKEN.\n\
     deletion tools are refused; tighten further with `deny_prefixes` or `deny_suffixes` in the agent's binding";

pub const DENY: &[NameRule] = &[
    NameRule::Prefix("delete_"),
    NameRule::Prefix("delete-"),
    NameRule::Suffix("_delete"),
    NameRule::Suffix("-delete"),
];

pub const VOCABULARY: WatchVocabulary = WatchVocabulary {
    integration: "pagerduty",
    residue: Residue::Reject,
    terms: TermPolicy::Reject,
    limit: Some(LimitSpec {
        default: 50,
        max: 100,
    }),
    keys: &[
        KeySpec::new("assignee").many().selfref(),
        KeySpec::new("service").many(),
        KeySpec::new("urgency").many().one_of(&["high", "low"]),
        KeySpec::new("status")
            .many()
            .one_of(&["triggered", "acknowledged", "resolved"]),
        KeySpec::new("is")
            .many()
            .negatable()
            .one_of(&["triggered", "open", "resolved"]),
    ],
};

pub fn service() -> McpService {
    McpService::new(
        "pagerduty",
        "PagerDuty",
        ServiceUrl::FromHost {
            default: DEFAULT_HOST,
            path: MCP_PATH,
        },
        SETUP,
    )
    .env_var(ENV_VAR)
    .tools(ToolPolicy::all(PREFIX).deny(DENY))
    .truncation_hint("narrow the time window or service, or fetch a single incident instead")
    .defaults(watch::defaults)
    .watch(&VOCABULARY, watch::compile)
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PagerdutyBinding {
    #[serde(default, deserialize_with = "meaningful")]
    pub user_id: Option<String>,
    #[serde(default, deserialize_with = "meaningful")]
    pub host: Option<String>,
}

impl PagerdutyBinding {
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
    goat_integration_mcp::validate_binding::<PagerdutyBinding>("pagerduty", config)
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
    use goat_types::IntegrationUpdateKind;
    use serde_json::json;

    #[test]
    fn the_binding_keeps_only_connection_keys() {
        assert!(validate_config(&json!({})).is_ok());
        assert!(
            validate_config(&json!({ "user_id": "P1", "host": "https://mcp.eu.pagerduty.com" }))
                .is_ok()
        );
        assert!(validate_config(&json!("nope")).is_err());
        assert!(validate_config(&json!({ "assignee": "@me" })).is_err());
    }

    #[test]
    fn factory_is_registered_in_inventory() {
        let registry = goat_integration::registry_from_inventory();
        assert!(registry.contains_key("pagerduty"));
        assert!(goat_integration::factory_for("pagerduty").is_some());
    }

    #[test]
    fn metadata_names_the_service_and_its_watch() {
        let meta = service().build().metadata();
        assert_eq!(meta.id, "pagerduty");
        assert_eq!(meta.display, "PagerDuty");
        assert_eq!(meta.auth, IntegrationAuth::OAuth);
        assert!(service().compile.is_some());
        assert!(service().defaults.is_some());
    }

    #[test]
    fn the_region_comes_from_the_binding_and_falls_back_to_us() {
        let url = service().url;
        assert_eq!(
            url.resolve(&json!({})).unwrap(),
            "https://mcp.pagerduty.com/mcp"
        );
        assert_eq!(
            url.resolve(&json!({ "host": "https://mcp.eu.pagerduty.com" }))
                .unwrap(),
            "https://mcp.eu.pagerduty.com/mcp"
        );
    }

    #[test]
    fn the_vocabulary_holds_its_invariants() {
        goat_integration::query::assert_vocabulary(&VOCABULARY);
    }

    #[tokio::test]
    async fn the_watcher_honours_the_shared_contract() {
        assert_watch_contract(&WatchContract {
            integration: ID,
            stream: watch::STREAM.to_owned(),
            kind: IntegrationUpdateKind::Assigned,
            entity: "incident",
            diff: goat_integration::diff::RETAIN,
        })
        .await;
    }
}
