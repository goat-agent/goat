use std::sync::Arc;

use goat_integration::{IntegrationFactory, IntegrationResult};
use goat_integration_mcp::{McpService, NameRule, ServiceUrl, ToolPolicy};
use goat_types::IntegrationId;
use serde::Deserialize;
use serde_json::Value;

pub const ID: IntegrationId = IntegrationId::from_static("gcalendar");
pub const PREFIX: &str = "gcalendar_";

const MCP_URL: &str = "https://calendarmcp.googleapis.com/mcp/v1";
const ENV_VAR: &str = "GOAT_GCALENDAR_TOKEN";

const SETUP: &str = "connects to Google's hosted Google Calendar MCP server, which is in the Workspace Developer Preview and does not register clients dynamically.\n\
     in a Google Cloud project, enable calendarmcp.googleapis.com, configure the OAuth consent screen, and create an OAuth client; `goat integration add gcalendar` then asks for its id and secret before opening the browser.\n\
     this integration adds `gcalendar_*` tools — it does not watch Google Calendar or brief you on its own.\n\
     to run headless, set GOAT_GCALENDAR_TOKEN.\n\
     deletion and trash tools are refused; tighten further with `deny_prefixes` or `deny_suffixes` in the agent's binding";

pub const SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/calendar.readonly",
    "https://www.googleapis.com/auth/calendar.events",
];

pub const DENY: &[NameRule] = &[
    NameRule::Prefix("delete_"),
    NameRule::Prefix("delete-"),
    NameRule::Suffix("_delete"),
    NameRule::Suffix("-delete"),
    NameRule::Prefix("trash"),
];

pub fn service() -> McpService {
    McpService::new(
        "gcalendar",
        "Google Calendar",
        ServiceUrl::Fixed(MCP_URL),
        SETUP,
    )
    .oauth(SCOPES)
    .preregistered()
    .env_var(ENV_VAR)
    .tools(ToolPolicy::all(PREFIX).deny(DENY))
    .truncation_hint("narrow the time window or ask for one calendar and call again")
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GcalendarBinding {}

fn validate_config(config: &Value) -> IntegrationResult<()> {
    goat_integration_mcp::validate_binding::<GcalendarBinding>("gcalendar", config)
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
        assert!(validate_config(&json!("nope")).is_err());
        assert!(validate_config(&json!({ "calendar": "primary" })).is_err());
    }

    #[test]
    fn factory_is_registered_in_inventory() {
        let registry = goat_integration::registry_from_inventory();
        assert!(registry.contains_key("gcalendar"));
        assert!(goat_integration::factory_for("gcalendar").is_some());
    }

    #[test]
    fn the_service_asks_for_an_oauth_client_of_its_own() {
        let meta = service().build().metadata();
        assert_eq!(meta.id, "gcalendar");
        assert_eq!(meta.display, "Google Calendar");
        assert_eq!(meta.auth, IntegrationAuth::OAuth);
        assert!(meta.preregistered);
        assert!(meta.setup.contains("Google Cloud"));
        assert!(service().compile.is_none());
    }

    #[test]
    fn the_requested_scopes_are_declared_on_the_descriptor() {
        assert!(!service().credential.scopes.is_empty());
        assert!(
            service()
                .credential
                .scopes
                .iter()
                .all(|scope| scope.starts_with("https://www.googleapis.com/auth/"))
        );
    }

    #[test]
    fn the_tool_policy_exposes_everything_but_deletion() {
        let policy = service().tools;
        assert_eq!(policy.prefix, PREFIX);
        assert!(matches!(policy.enable, Enable::All));
        assert!(policy.deny.contains(&NameRule::Prefix("delete_")));
        assert!(policy.deny.contains(&NameRule::Prefix("trash")));
    }
}
