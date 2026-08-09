use std::sync::Arc;

use goat_integration::{IntegrationFactory, IntegrationResult};
use goat_integration_mcp::{McpService, NameRule, ServiceUrl, ToolPolicy};
use goat_types::IntegrationId;
use serde::Deserialize;
use serde_json::Value;

pub const ID: IntegrationId = IntegrationId::from_static("gmail");
pub const PREFIX: &str = "gmail_";

const MCP_URL: &str = "https://gmailmcp.googleapis.com/mcp/v1";
const ENV_VAR: &str = "GOAT_GMAIL_TOKEN";

const SETUP: &str = "connects to Google's hosted Gmail MCP server, which is in the Workspace Developer Preview and does not register clients dynamically.\n\
     in a Google Cloud project, enable gmailmcp.googleapis.com, configure the OAuth consent screen, and create an OAuth client; `goat integration add gmail` then asks for its id and secret before opening the browser.\n\
     this integration adds `gmail_*` tools — it does not watch Gmail or brief you on its own.\n\
     to run headless, set GOAT_GMAIL_TOKEN.\n\
     deletion and trash tools are refused; tighten further with `deny_prefixes` or `deny_suffixes` in the agent's binding";

pub const SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/gmail.readonly",
    "https://www.googleapis.com/auth/gmail.compose",
];

pub const DENY: &[NameRule] = &[
    NameRule::Prefix("delete_"),
    NameRule::Prefix("delete-"),
    NameRule::Suffix("_delete"),
    NameRule::Suffix("-delete"),
    NameRule::Prefix("trash"),
];

pub fn service() -> McpService {
    McpService::new("gmail", "Gmail", ServiceUrl::Fixed(MCP_URL), SETUP)
        .oauth(SCOPES)
        .preregistered()
        .env_var(ENV_VAR)
        .tools(ToolPolicy::all(PREFIX).deny(DENY))
        .truncation_hint("narrow the search query or request fewer messages and call again")
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GmailBinding {}

fn validate_config(config: &Value) -> IntegrationResult<()> {
    goat_integration_mcp::validate_binding::<GmailBinding>("gmail", config)
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
        assert!(registry.contains_key("gmail"));
        assert!(goat_integration::factory_for("gmail").is_some());
    }

    #[test]
    fn the_service_asks_for_an_oauth_client_of_its_own() {
        let meta = service().build().metadata();
        assert_eq!(meta.id, "gmail");
        assert_eq!(meta.display, "Gmail");
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
