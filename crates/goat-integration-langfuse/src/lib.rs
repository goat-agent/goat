mod parse;
mod watch;

use std::collections::BTreeSet;
use std::sync::Arc;

use goat_integration::{IntegrationError, IntegrationFactory, IntegrationResult};
use goat_integration_mcp::{
    AuthScheme, IdentityProbe, McpService, NameRule, ServiceUrl, ToolPolicy,
};
use goat_types::IntegrationId;
use serde::Deserialize;
use serde_json::Value;

pub const ID: IntegrationId = IntegrationId::from_static("langfuse");
pub const PREFIX: &str = "langfuse_";

const DEFAULT_HOST: &str = "https://cloud.langfuse.com";
const MCP_PATH: &str = "/api/public/mcp";
const ENV_VAR: &str = "GOAT_LANGFUSE_API_KEY";
const TOOL_HEALTH: &str = "getHealth";

const SETUP: &str = "connects to your Langfuse deployment's MCP server.\n\
     paste the project's public and secret key joined by a colon: pk-lf-…:sk-lf-…\n\
     for the us/jp/hipaa cloud regions or a self-hosted instance, add `\"host\": \"https://us.cloud.langfuse.com\"` to the langfuse entry in ~/.goat/config.json — a self-hosted instance must be recent enough to serve the full MCP tool catalogue\n\
     the watcher stays off until the agent's langfuse binding declares `watch` streams; each entry's `filter` is passed to listObservations as-is\n\
     to run headless, set GOAT_LANGFUSE_API_KEY to the same colon-joined pair";

pub const ENABLED_TOOLS: &[&str] = &[
    "listObservations",
    "getObservation",
    "getObservationFieldSchema",
    "getObservationFilterSchema",
    "getObservationFilterValues",
    "getObservationFilterMetadataKeys",
    "queryMetrics",
    "getMetricsSchema",
    "listScores",
    "getScore",
    "createScore",
    "listComments",
    "getComment",
    "createComment",
    "getMedia",
    "getHealth",
    "searchLangfuseDocs",
    "getLangfuseDocsPage",
];

pub const DENY: &[NameRule] = &[NameRule::Prefix("delete")];

pub fn service() -> McpService {
    McpService::new(
        "langfuse",
        "Langfuse",
        ServiceUrl::FromHost {
            default: DEFAULT_HOST,
            path: MCP_PATH,
        },
        SETUP,
    )
    .secret(
        "Langfuse public and secret key, colon-joined (pk-lf-…:sk-lf-…)",
        AuthScheme::Basic,
    )
    .env_var(ENV_VAR)
    .tools(ToolPolicy::only(PREFIX, ENABLED_TOOLS).deny(DENY))
    .truncation_hint(
        "narrow the time range, filter on fewer columns, or page with the cursor and call again",
    )
    .identity(IdentityProbe {
        tool: TOOL_HEALTH,
        describe: parse::version,
    })
    .watch(watch::spawn)
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LangfuseBinding {
    #[serde(default, deserialize_with = "meaningful")]
    pub host: Option<String>,
    #[serde(default)]
    pub watch: Vec<WatchEntry>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WatchEntry {
    pub stream: String,
    #[serde(default)]
    pub filter: Vec<Value>,
}

impl LangfuseBinding {
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
    goat_integration_mcp::validate_binding::<LangfuseBinding>("langfuse", config)?;
    let binding = LangfuseBinding::read(config);
    if let Some(host) = &binding.host
        && !host.starts_with("https://")
        && !host.starts_with("http://")
    {
        return Err(IntegrationError::Config(
            "langfuse binding: `host` must start with http:// or https://".into(),
        ));
    }
    if binding.limit == Some(0) {
        return Err(IntegrationError::Config(
            "langfuse binding: `limit` must be a positive integer".into(),
        ));
    }
    let mut streams = BTreeSet::new();
    for entry in &binding.watch {
        let stream = entry.stream.trim();
        if stream.is_empty() {
            return Err(IntegrationError::Config(
                "langfuse binding: each `watch` entry needs a non-empty `stream`".into(),
            ));
        }
        if !streams.insert(stream.to_owned()) {
            return Err(IntegrationError::Config(format!(
                "langfuse binding: `watch` reuses the stream name `{stream}`; stream names key stored state and must be unique"
            )));
        }
    }
    Ok(())
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
    fn the_binding_is_typo_checked() {
        assert!(validate_config(&json!({})).is_ok());
        assert!(validate_config(&json!({ "account": "work", "client_id": "cid" })).is_ok());
        assert!(validate_config(&json!({ "host": "https://us.cloud.langfuse.com" })).is_ok());
        assert!(validate_config(&json!({ "deny_prefixes": ["delete"] })).is_ok());
        assert!(
            validate_config(&json!({
                "watch": [
                    { "stream": "errors",
                      "filter": [{ "column": "level", "operator": "=", "value": "ERROR" }] }
                ],
                "limit": 25
            }))
            .is_ok()
        );
        assert!(validate_config(&json!("nope")).is_err());
        assert!(validate_config(&json!({ "host": 3 })).is_err());
        assert!(validate_config(&json!({ "host ": "x" })).is_err());
        assert!(validate_config(&json!({ "wach": [] })).is_err());
    }

    #[test]
    fn a_host_without_a_scheme_is_rejected() {
        assert!(validate_config(&json!({ "host": "us.cloud.langfuse.com" })).is_err());
        assert!(validate_config(&json!({ "host": "http://langfuse.internal" })).is_ok());
    }

    #[test]
    fn watch_streams_must_be_unique_and_named() {
        assert!(validate_config(&json!({ "watch": [{ "stream": "  " }] })).is_err());
        assert!(
            validate_config(&json!({ "watch": [
                { "stream": "errors" },
                { "stream": "errors" }
            ] }))
            .is_err()
        );
        assert!(validate_config(&json!({ "watch": [{ "filter": [] }] })).is_err());
        assert!(validate_config(&json!({ "limit": 0 })).is_err());
    }

    #[test]
    fn factory_is_registered_in_inventory() {
        let registry = goat_integration::registry_from_inventory();
        assert!(registry.contains_key("langfuse"));
        assert!(goat_integration::factory_for("langfuse").is_some());
    }

    #[test]
    fn metadata_takes_a_pasted_colon_joined_key_pair() {
        let meta = service().build().metadata();
        assert_eq!(meta.id, "langfuse");
        assert_eq!(meta.display, "Langfuse");
        assert_eq!(meta.auth, IntegrationAuth::Secret);
        assert!(meta.secret_label.contains("pk-lf-…:sk-lf-…"));
        assert_eq!(meta.env_var, Some("GOAT_LANGFUSE_API_KEY"));
        assert!(service().watch.is_some());
        assert!(meta.setup.contains("host"));
        assert_eq!(service().credential.scheme, AuthScheme::Basic);
    }

    #[test]
    fn the_url_follows_the_binding_host_and_defaults_to_the_cloud() {
        let url = service().url;
        assert_eq!(
            url.resolve(&json!({})).unwrap(),
            "https://cloud.langfuse.com/api/public/mcp"
        );
        assert_eq!(
            url.resolve(&json!({ "host": "https://langfuse.acme.dev/" }))
                .unwrap(),
            "https://langfuse.acme.dev/api/public/mcp"
        );
    }

    #[test]
    fn the_tool_policy_enables_investigation_and_denies_deletion() {
        let policy = service().tools;
        assert_eq!(policy.prefix, PREFIX);
        let Enable::Only(wanted) = policy.enable else {
            panic!("langfuse should enable a curated list");
        };
        assert_eq!(wanted.len(), 18);
        assert!(wanted.contains(&"listObservations"));
        assert!(wanted.contains(&"getObservationFilterSchema"));
        assert!(wanted.contains(&"createScore"));
        assert!(!wanted.iter().any(|name| name.starts_with("delete")));
        assert_eq!(policy.deny, &[NameRule::Prefix("delete")]);
    }

    #[test]
    fn the_watcher_stays_off_until_streams_are_declared() {
        assert!(LangfuseBinding::read(&json!({})).watch.is_empty());
        let read = LangfuseBinding::read(&json!({
            "watch": [{ "stream": "errors", "filter": [] }]
        }));
        assert_eq!(read.watch.len(), 1);
        assert_eq!(read.watch[0].stream, "errors");
    }

    #[tokio::test]
    async fn the_watcher_honours_the_shared_contract() {
        use goat_integration::diff::RETAIN;
        use goat_integration::test_support::{WatchContract, assert_watch_contract};
        use goat_types::IntegrationUpdateKind;

        assert_watch_contract(&WatchContract {
            integration: ID,
            stream: "errors".to_owned(),
            kind: IntegrationUpdateKind::Updated,
            entity: "trace",
            overflow_tail: "flagged",
            diff: RETAIN,
        })
        .await;
    }
}
