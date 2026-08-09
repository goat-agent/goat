use std::sync::Arc;

use goat_integration::{IntegrationFactory, IntegrationResult};
use goat_integration_mcp::{McpService, NameRule, ServiceUrl, ToolPolicy};
use goat_types::IntegrationId;
use serde::Deserialize;
use serde_json::Value;

pub const ID: IntegrationId = IntegrationId::from_static("stripe");
pub const PREFIX: &str = "stripe_";

const MCP_URL: &str = "https://mcp.stripe.com/";
const ENV_VAR: &str = "GOAT_STRIPE_API_KEY";

const SETUP: &str = "connects to Stripe's hosted MCP server; a browser window will ask you to approve access.\n\
     this integration adds `stripe_*` tools — it does not watch Stripe or brief you on its own.\n\
     to run headless, or to recover if the browser flow fails, set GOAT_STRIPE_API_KEY.\n\
     deletion tools are refused; tighten further with `deny_prefixes` or `deny_suffixes` in the agent's binding";

pub const DENY: &[NameRule] = &[
    NameRule::Prefix("delete_"),
    NameRule::Prefix("delete-"),
    NameRule::Suffix("_delete"),
    NameRule::Suffix("-delete"),
];

pub fn service() -> McpService {
    McpService::new("stripe", "Stripe", ServiceUrl::Fixed(MCP_URL), SETUP)
        .env_var(ENV_VAR)
        .tools(ToolPolicy::all(PREFIX).deny(DENY))
        .truncation_hint(
            "narrow the date range, request fewer objects, or fetch a single object instead",
        )
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StripeBinding {}

fn validate_config(config: &Value) -> IntegrationResult<()> {
    goat_integration_mcp::validate_binding::<StripeBinding>("stripe", config)
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
        assert!(validate_config(&json!("nope")).is_err());
        assert!(validate_config(&json!({ "assignee": "@me" })).is_err());
    }

    #[test]
    fn factory_is_registered_in_inventory() {
        let registry = goat_integration::registry_from_inventory();
        assert!(registry.contains_key("stripe"));
        assert!(goat_integration::factory_for("stripe").is_some());
    }

    #[test]
    fn metadata_says_this_integration_only_brings_tools() {
        let meta = service().build().metadata();
        assert_eq!(meta.id, "stripe");
        assert_eq!(meta.display, "Stripe");
        assert_eq!(meta.auth, IntegrationAuth::OAuth);
        assert_eq!(meta.env_var, Some(ENV_VAR));
        assert!(service().compile.is_none());
        assert!(service().defaults.is_none());
        assert!(meta.setup.contains("does not watch"));
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
