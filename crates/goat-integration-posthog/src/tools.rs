use goat_integration_mcp::{CachedTool, ToolDisposition, normalized};
use serde_json::Value;

use crate::{PREFIX, PosthogBinding};

pub const DENY_SUFFIXES: &[&str] = &["-delete", "-destroy"];
const MAX_TOOL_NAME_LEN: usize = 64;

pub const ENABLED_TOOLS: &[&str] = &[
    "execute-sql",
    "insight-query",
    "docs-search",
    "project-get",
    "organization-get",
    "query-error-tracking-issues-list",
    "query-error-tracking-issue",
    "query-error-tracking-issue-events",
    "feature-flag-get-all",
    "feature-flag-get-definition",
    "feature-flag-get-definition-by-key",
    "feature-flags-status-retrieve",
    "create-feature-flag",
    "update-feature-flag",
    "query-logs",
    "logs-count",
    "logs-patterns",
    "dashboards-get-all",
    "dashboard-get",
    "dashboard-insights-run",
    "annotations-list",
    "annotation-create",
    "get-llm-total-costs-for-project",
    "llma-personal-spend",
    "experiment-get-all",
    "experiment-get",
    "experiment-results-get",
];

pub fn disposition(tool: &CachedTool, config: &Value) -> ToolDisposition {
    let denied = PosthogBinding::read(config)
        .deny_suffixes
        .unwrap_or_else(|| DENY_SUFFIXES.iter().map(|s| (*s).to_owned()).collect());
    if denied.iter().any(|suffix| tool.name.ends_with(suffix)) {
        return ToolDisposition::Skip;
    }
    if !schema_is_usable(&tool.input_schema) {
        return ToolDisposition::Skip;
    }
    if PREFIX.len() + tool.name.len() > MAX_TOOL_NAME_LEN {
        return ToolDisposition::Skip;
    }
    if ENABLED_TOOLS
        .iter()
        .any(|wanted| normalized(PREFIX, wanted) == normalized(PREFIX, &tool.name))
    {
        ToolDisposition::Enabled
    } else {
        ToolDisposition::Deferred
    }
}

fn schema_is_usable(schema: &Value) -> bool {
    schema.is_object()
        && serde_json::to_string(schema)
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .is_some_and(|back| &back == schema)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(name: &str) -> CachedTool {
        CachedTool {
            name: name.to_owned(),
            description: String::new(),
            input_schema: json!({ "type": "object" }),
        }
    }

    #[test]
    fn an_allowlisted_tool_is_enabled_and_anything_else_is_deferred() {
        assert_eq!(
            disposition(&tool("execute-sql"), &json!({})),
            ToolDisposition::Enabled
        );
        assert_eq!(
            disposition(&tool("something-experimental"), &json!({})),
            ToolDisposition::Deferred
        );
    }

    #[test]
    fn the_allowlist_matches_across_naming_styles() {
        assert_eq!(
            disposition(&tool("execute_sql"), &json!({})),
            ToolDisposition::Enabled
        );
    }

    #[test]
    fn destructive_suffixes_are_skipped_entirely() {
        assert_eq!(
            disposition(&tool("insight-delete"), &json!({})),
            ToolDisposition::Skip
        );
        assert_eq!(
            disposition(&tool("dashboard-destroy"), &json!({})),
            ToolDisposition::Skip
        );
    }

    #[test]
    fn the_owner_can_widen_or_narrow_the_deny_list() {
        let config = json!({ "deny_suffixes": ["-sql"] });
        assert_eq!(
            disposition(&tool("execute-sql"), &config),
            ToolDisposition::Skip
        );
        assert_eq!(
            disposition(&tool("insight-delete"), &config),
            ToolDisposition::Deferred
        );
    }

    #[test]
    fn an_unusable_schema_is_skipped_rather_than_registered() {
        let mut broken = tool("execute-sql");
        broken.input_schema = json!("not an object");
        assert_eq!(disposition(&broken, &json!({})), ToolDisposition::Skip);
    }

    #[test]
    fn an_over_long_name_is_skipped() {
        assert_eq!(
            disposition(&tool(&"x".repeat(MAX_TOOL_NAME_LEN)), &json!({})),
            ToolDisposition::Skip
        );
    }
}
