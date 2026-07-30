mod tools;

use std::collections::HashMap;
use std::sync::Arc;

use goat_integration::{IntegrationFactory, IntegrationResult};
use goat_integration_mcp::McpService;
use goat_types::IntegrationId;
use serde::Deserialize;
use serde_json::Value;

pub const ID: IntegrationId = IntegrationId::from_static("posthog");
pub const PREFIX: &str = "posthog_";

const MCP_URL: &str = "https://mcp.posthog.com/mcp";
const ENV_VAR: &str = "GOAT_POSTHOG_API_KEY";
const PROJECT_HEADER: &str = "x-posthog-project-id";
const ORGANIZATION_HEADER: &str = "x-posthog-organization-id";

const SETUP: &str = "connects to PostHog's hosted MCP server; a browser window will ask you to approve access.\n\
     this integration adds `posthog_*` tools — it does not watch PostHog or brief you on its own.\n\
     to run headless, or to recover if the browser flow fails, set GOAT_POSTHOG_API_KEY to a PostHog personal API key (phx-…).\n\
     with more than one project, add `\"project_id\": \"<id>\"` to the agent's posthog binding in ~/.goat/agents/<slug>/config.json";

pub const SCOPES: &[&str] = &[
    "openid",
    "profile",
    "email",
    "organization:read",
    "project:read",
    "user:read",
    "query:read",
    "insight:read",
    "dashboard:read",
    "error_tracking:read",
    "error_tracking:write",
    "feature_flag:read",
    "feature_flag:write",
    "experiment:read",
    "logs:read",
    "annotation:read",
    "annotation:write",
    "llm_analytics:read",
];

pub fn service() -> McpService {
    McpService::new("posthog", "PostHog", MCP_URL, SETUP)
        .with_env_var(ENV_VAR)
        .with_scopes(SCOPES)
        .with_tool_prefix(PREFIX)
        .with_headers(scope_headers)
        .with_tool_filter(tools::disposition)
        .with_truncation_hint(
            "add a LIMIT, select fewer columns, or narrow the date range and call again",
        )
}

fn scope_headers(config: &Value) -> HashMap<String, String> {
    let settings = PosthogBinding::read(config);
    let mut headers = HashMap::new();
    if let Some(project) = settings.project_id {
        headers.insert(PROJECT_HEADER.to_owned(), project);
    }
    if let Some(organization) = settings.organization_id {
        headers.insert(ORGANIZATION_HEADER.to_owned(), organization);
    }
    headers
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PosthogBinding {
    #[serde(default, deserialize_with = "meaningful")]
    pub project_id: Option<String>,
    #[serde(default, deserialize_with = "meaningful")]
    pub organization_id: Option<String>,
    #[serde(default)]
    pub deny_suffixes: Option<Vec<String>>,
}

impl PosthogBinding {
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
    goat_integration_mcp::validate_binding::<PosthogBinding>("posthog", config)
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
    use serde_json::json;

    #[test]
    fn the_binding_is_typo_checked() {
        assert!(validate_config(&json!({})).is_ok());
        assert!(validate_config(&json!({ "account": "work", "client_id": "cid" })).is_ok());
        assert!(validate_config(&json!({ "project_id": "1", "organization_id": "2" })).is_ok());
        assert!(validate_config(&json!({ "deny_suffixes": ["-delete"] })).is_ok());
        assert!(validate_config(&json!("nope")).is_err());
        assert!(validate_config(&json!({ "project_id": 3 })).is_err());
        assert!(validate_config(&json!({ "projectid": "1" })).is_err());
    }

    #[test]
    fn factory_is_registered_in_inventory() {
        let registry = goat_integration::registry_from_inventory();
        assert!(registry.contains_key("posthog"));
        assert!(goat_integration::factory_for("posthog").is_some());
    }

    #[test]
    fn metadata_says_this_integration_only_brings_tools() {
        let meta = service().build().metadata();
        assert_eq!(meta.id, "posthog");
        assert_eq!(meta.display, "PostHog");
        assert_eq!(meta.auth, IntegrationAuth::OAuth);
        assert_eq!(meta.env_var, Some("GOAT_POSTHOG_API_KEY"));
        assert!(!meta.has_watcher);
        assert!(meta.setup.contains("does not watch PostHog"));
    }

    #[test]
    fn scope_headers_are_sent_only_when_configured() {
        assert!(scope_headers(&json!({})).is_empty());
        let headers = scope_headers(&json!({ "project_id": " 12345 ", "organization_id": "org" }));
        assert_eq!(
            headers.get(PROJECT_HEADER).map(String::as_str),
            Some("12345")
        );
        assert_eq!(
            headers.get(ORGANIZATION_HEADER).map(String::as_str),
            Some("org")
        );
    }

    #[test]
    fn the_requested_scopes_are_declared_on_the_descriptor() {
        assert!(service().scopes.contains(&"query:read"));
        assert!(service().scopes.len() > 10);
    }
}
