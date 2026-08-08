mod parse;
mod watch;

use std::sync::Arc;

use async_trait::async_trait;
use goat_agent_tool::{ToolName, ToolRegistry};
use goat_auth::CredentialStore;
use goat_integration::query::{LimitSpec, Residue, TermPolicy, WatchVocabulary};
use goat_integration::{
    BindingMap, CompiledWatch, Integration, IntegrationAuth, IntegrationBinding, IntegrationError,
    IntegrationFactory, IntegrationMetadata, IntegrationResult, IntegrationRuntime, WatchSpec,
};
use goat_types::IntegrationId;
use serde::Deserialize;
use serde_json::Value;

pub const ID: IntegrationId = IntegrationId::from_static("github");

const SETUP: &str = "goat reaches github through the `gh` cli — it holds the credential, goat never stores one.\n\
     install gh, then run `gh auth login`.\n\
     by default the watcher briefs you on review requests (`is:open is:pr review-requested:@me`)\n\
     and assigned items (`is:open assignee:@me`).\n\
     declare workflows in the agent's `watch` section to change that, e.g.\n\
     { \"source\": \"github\", \"query\": \"is:open author:@me label:bug limit:25\" } —\n\
     the query is github's native search syntax and passes through unchanged;\n\
     only `limit:N` is read out to cap the page size (default 50, max 100).";
pub(crate) const MISSING_GH: &str =
    "the `gh` cli is not on PATH; install it and run `gh auth login`";

pub const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 100;

pub const VOCABULARY: WatchVocabulary = WatchVocabulary {
    integration: "github",
    residue: Residue::Keep,
    terms: TermPolicy::Reject,
    limit: Some(LimitSpec {
        default: DEFAULT_LIMIT,
        max: MAX_LIMIT,
    }),
    keys: &[],
};

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GithubBinding {}

pub struct GithubIntegration;

#[async_trait]
impl Integration for GithubIntegration {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn id(&self) -> IntegrationId {
        ID
    }

    fn metadata(&self) -> IntegrationMetadata {
        IntegrationMetadata {
            id: "github",
            display: "GitHub",
            auth: IntegrationAuth::External,
            secret_label: "",
            env_var: None,
            setup: SETUP,
        }
    }

    async fn register_tools(
        &self,
        _registry: &mut ToolRegistry,
        _runtime: &IntegrationRuntime,
        _bindings: Arc<BindingMap>,
    ) -> Vec<ToolName> {
        Vec::new()
    }

    fn default_watch(&self, _binding: &IntegrationBinding) -> Vec<WatchSpec> {
        watch::defaults()
    }

    fn watch_vocabulary(&self) -> Option<&'static goat_integration::query::WatchVocabulary> {
        Some(&VOCABULARY)
    }

    fn compile_watch(
        &self,
        _binding: &IntegrationBinding,
        _runtime: &IntegrationRuntime,
        spec: &WatchSpec,
    ) -> IntegrationResult<CompiledWatch> {
        watch::compile(spec)
    }

    async fn verify(
        &self,
        _config: &Value,
        _credentials: &CredentialStore,
    ) -> IntegrationResult<String> {
        if !goat_github::gh_available() {
            return Err(IntegrationError::Config(MISSING_GH.to_owned()));
        }
        goat_github::cli::login()
            .await
            .map(|handle| format!("gh as {handle}"))
            .map_err(watch::map_error)
    }
}

const MOVED_KEYS: &[&str] = &["watch", "limit"];

fn validate_config(config: &Value) -> IntegrationResult<()> {
    if let Some(object) = config.as_object() {
        for key in MOVED_KEYS {
            if object.contains_key(*key) {
                return Err(IntegrationError::Config(format!(
                    "github binding: `{key}` moved to the agent-level `watch` section; \
                     write {{ \"source\": \"github\", \"query\": \"...\" }} there instead"
                )));
            }
        }
    }
    goat_integration_mcp::validate_binding::<GithubBinding>("github", config)
}

inventory::submit! {
    IntegrationFactory {
        id: ID,
        ctor: || Arc::new(GithubIntegration),
        validate_config,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use goat_integration::query::assert_vocabulary;
    use serde_json::json;

    #[test]
    fn the_vocabulary_holds_its_invariants() {
        assert_vocabulary(&VOCABULARY);
    }

    #[test]
    fn default_watch_declares_the_two_historical_streams() {
        let binding = IntegrationBinding::from_config(json!({}));
        assert_eq!(
            GithubIntegration.default_watch(&binding),
            vec![
                WatchSpec {
                    stream: "review".to_owned(),
                    query: "is:open is:pr review-requested:@me".to_owned(),
                },
                WatchSpec {
                    stream: "assigned".to_owned(),
                    query: "is:open assignee:@me".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn the_binding_keeps_only_connection_keys() {
        assert!(validate_config(&json!({})).is_ok());
        assert!(validate_config(&json!({ "account": "work" })).is_ok());
        assert!(validate_config(&json!("nope")).is_err());
        assert!(validate_config(&json!({ "unknown": true })).is_err());
        assert!(validate_config(&json!({ "wach": [] })).is_err());
    }

    #[test]
    fn an_old_policy_key_points_at_the_watch_section() {
        let err = validate_config(&json!({ "watch": [] })).unwrap_err();
        assert!(err.to_string().contains("agent-level `watch` section"));
        let err = validate_config(&json!({ "limit": 10 })).unwrap_err();
        assert!(err.to_string().contains("watch"));
    }

    #[test]
    fn factory_is_registered_in_inventory() {
        let registry = goat_integration::registry_from_inventory();
        assert!(registry.contains_key("github"));
        assert!(goat_integration::factory_for("github").is_some());
    }

    #[test]
    fn metadata_says_gh_owns_the_credential() {
        let meta = GithubIntegration.metadata();
        assert_eq!(meta.auth, IntegrationAuth::External);
        assert_eq!(meta.env_var, None);
        assert!(meta.setup.contains("gh auth login"));
        assert!(meta.setup.contains("review-requested:@me"));
        assert!(meta.setup.contains("limit:"));
    }

    #[tokio::test]
    async fn the_watcher_honours_the_shared_contract() {
        use goat_integration::diff::REBUILD;
        use goat_integration::test_support::{WatchContract, assert_watch_contract};
        use goat_types::IntegrationUpdateKind;

        assert_watch_contract(&WatchContract {
            integration: ID,
            stream: "review".to_owned(),
            kind: IntegrationUpdateKind::Assigned,
            entity: "item",
            diff: REBUILD,
        })
        .await;
    }
}
