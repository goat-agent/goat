mod parse;
mod watch;

use std::sync::Arc;

use goat_integration::{IntegrationFactory, IntegrationResult};
use goat_integration_mcp::{AuthScheme, McpService, ServiceUrl, ToolPolicy};
use goat_types::IntegrationId;
use serde::Deserialize;
use serde_json::Value;

pub const ID: IntegrationId = IntegrationId::from_static("sentry");
pub const PREFIX: &str = "sentry_";

const MCP_URL: &str = "https://mcp.sentry.dev/mcp";
const ENV_VAR: &str = "GOAT_SENTRY_ACCESS_TOKEN";

const SETUP: &str = "connects to Sentry's hosted MCP server; a browser window will ask you to approve access.\n\
     the approval screen lists skills — uncheck anything you do not want; `Manage Projects & Teams` grants project and team writes.\n\
     the watcher stays off until you set `organization_slug` in the agent's sentry binding.\n\
     to run headless, or to recover if the browser flow fails, set GOAT_SENTRY_ACCESS_TOKEN to a Sentry user auth token.";

pub fn service() -> McpService {
    McpService::new("sentry", "Sentry", ServiceUrl::Fixed(MCP_URL), SETUP)
        .env_var(ENV_VAR)
        .token_scheme(AuthScheme::Custom("Sentry-Bearer"))
        .tools(ToolPolicy::all(PREFIX))
        .truncation_hint(
            "narrow the time range, request fewer fields, or fetch a single issue instead",
        )
        .watch(watch::spawn)
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SentryBinding {
    #[serde(default, deserialize_with = "meaningful")]
    pub organization_slug: Option<String>,
    #[serde(default, deserialize_with = "meaningful")]
    pub project: Option<String>,
    #[serde(default, deserialize_with = "meaningful")]
    pub query: Option<String>,
    #[serde(default, deserialize_with = "meaningful")]
    pub sort: Option<String>,
}

impl SentryBinding {
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
    goat_integration_mcp::validate_binding::<SentryBinding>("sentry", config)
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
        assert!(
            validate_config(&json!({
                "organization_slug": "acme",
                "project": "backend",
                "query": "is:unresolved",
                "sort": "new"
            }))
            .is_ok()
        );
        assert!(validate_config(&json!("nope")).is_err());
        assert!(validate_config(&json!({ "organization_slug": 3 })).is_err());
        assert!(validate_config(&json!({ "project": ["backend"] })).is_err());
        assert!(validate_config(&json!({ "org_slug": "acme" })).is_err());
    }

    #[test]
    fn factory_is_registered_in_inventory() {
        let registry = goat_integration::registry_from_inventory();
        assert!(registry.contains_key("sentry"));
        assert!(goat_integration::factory_for("sentry").is_some());
    }

    #[test]
    fn metadata_advertises_the_prefixed_environment_override() {
        let meta = service().build().metadata();
        assert_eq!(meta.id, "sentry");
        assert_eq!(meta.display, "Sentry");
        assert_eq!(meta.auth, IntegrationAuth::OAuth);
        assert_eq!(meta.env_var, Some("GOAT_SENTRY_ACCESS_TOKEN"));
        assert!(service().watch.is_some());
        assert!(meta.setup.contains("GOAT_SENTRY_ACCESS_TOKEN"));
        assert!(meta.setup.contains("organization_slug"));
    }

    #[test]
    fn settings_are_trimmed_and_blank_values_ignored() {
        let read = SentryBinding::read(&json!({ "organization_slug": "  ", "project": " api " }));
        assert_eq!(read.organization_slug, None);
        assert_eq!(read.project, Some("api".to_owned()));
    }

    #[test]
    fn the_custom_auth_scheme_is_kept() {
        let service = service();
        assert_eq!(
            service.credential.scheme,
            AuthScheme::Custom("Sentry-Bearer")
        );
        assert_eq!(
            goat_integration_mcp::header_value(service.credential.scheme, " tok \n"),
            "Sentry-Bearer tok"
        );
    }

    #[tokio::test]
    async fn the_watcher_honours_the_shared_contract() {
        use goat_integration::diff::RETAIN;
        use goat_integration::test_support::{WatchContract, assert_watch_contract};
        use goat_types::IntegrationUpdateKind;

        assert_watch_contract(&WatchContract {
            integration: ID,
            stream: watch::STREAM.to_owned(),
            kind: IntegrationUpdateKind::Updated,
            entity: "issue",
            overflow_tail: "issues waiting",
            diff: RETAIN,
        })
        .await;
    }
}
