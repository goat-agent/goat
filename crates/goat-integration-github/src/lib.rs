mod diff;
mod gh;
mod parse;
mod watcher;

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use goat_agent_tool::{ToolName, ToolRegistry};
use goat_auth::CredentialStore;
use goat_integration::{
    BindingMap, Integration, IntegrationAuth, IntegrationBinding, IntegrationError,
    IntegrationFactory, IntegrationMetadata, IntegrationResult, IntegrationRuntime,
};
use goat_types::{IntegrationId, ProfileId};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use watcher::{DEFAULT_LIMIT, GhFetch, WatchQuery, default_queries};

pub const ID: IntegrationId = IntegrationId::from_static("github");

const SETUP: &str = "goat reaches github through the `gh` cli — it holds the credential, goat never stores one.\ninstall gh, then run `gh auth login`.";

const MISSING_GH: &str = "the `gh` cli is not on PATH; install it and run `gh auth login`";

pub struct GithubIntegration;

#[async_trait]
impl Integration for GithubIntegration {
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
            has_watcher: true,
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

    fn spawn_watcher(
        &self,
        persona: ProfileId,
        binding: IntegrationBinding,
        runtime: IntegrationRuntime,
        cancel: CancellationToken,
    ) -> Option<tokio::task::JoinHandle<()>> {
        if !goat_github::gh_available() {
            warn!(profile = %persona, "github watcher disabled; {MISSING_GH}");
            return None;
        }
        let queries = queries_from(&binding.config);
        if queries.is_empty() {
            warn!(
                profile = %persona,
                "github watcher disabled; the agent's github binding declares no `watch` entries",
            );
            return None;
        }
        let fetch = GhFetch {
            limit: limit_from(&binding.config),
        };
        Some(tokio::spawn(watcher::run(
            persona,
            runtime,
            binding.account,
            queries,
            fetch,
            cancel,
        )))
    }

    async fn verify(
        &self,
        _config: &Value,
        _credentials: &CredentialStore,
    ) -> IntegrationResult<String> {
        if !goat_github::gh_available() {
            return Err(IntegrationError::Config(MISSING_GH.into()));
        }
        gh::login().await
    }
}

fn queries_from(config: &Value) -> Vec<WatchQuery> {
    let Some(entries) = config.get("watch").and_then(Value::as_array) else {
        return default_queries();
    };
    entries
        .iter()
        .filter_map(|entry| {
            Some(WatchQuery {
                stream: string_setting(entry, "stream")?,
                query: string_setting(entry, "query")?,
            })
        })
        .collect()
}

fn limit_from(config: &Value) -> usize {
    config
        .get("limit")
        .and_then(Value::as_u64)
        .and_then(|limit| usize::try_from(limit).ok())
        .filter(|limit| *limit > 0)
        .unwrap_or(DEFAULT_LIMIT)
}

fn string_setting(node: &Value, key: &str) -> Option<String> {
    node.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn validate_config(config: &Value) -> IntegrationResult<()> {
    let obj = config
        .as_object()
        .ok_or_else(|| IntegrationError::Config("github binding must be an object".into()))?;
    if let Some(account) = obj.get("account")
        && !account.is_string()
    {
        return Err(IntegrationError::Config(
            "`account` must be a string".into(),
        ));
    }
    if let Some(limit) = obj.get("limit")
        && limit.as_u64().is_none_or(|limit| limit == 0)
    {
        return Err(IntegrationError::Config(
            "`limit` must be a positive integer".into(),
        ));
    }
    let Some(watch) = obj.get("watch") else {
        return Ok(());
    };
    let entries = watch
        .as_array()
        .ok_or_else(|| IntegrationError::Config("`watch` must be an array".into()))?;
    let mut streams = BTreeSet::new();
    for entry in entries {
        if !entry.is_object() {
            return Err(IntegrationError::Config(
                "each `watch` entry must be an object".into(),
            ));
        }
        for key in ["stream", "query"] {
            if string_setting(entry, key).is_none() {
                return Err(IntegrationError::Config(format!(
                    "each `watch` entry needs a non-empty string `{key}`"
                )));
            }
        }
        let stream = string_setting(entry, "stream").unwrap_or_default();
        if !streams.insert(stream.clone()) {
            return Err(IntegrationError::Config(format!(
                "`watch` reuses the stream name `{stream}`; stream names key stored state and must be unique"
            )));
        }
    }
    Ok(())
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
    use serde_json::json;

    #[test]
    fn validate_config_accepts_valid_bindings() {
        assert!(validate_config(&json!({})).is_ok());
        assert!(validate_config(&json!({ "account": "work", "limit": 25 })).is_ok());
        assert!(
            validate_config(&json!({
                "watch": [
                    { "stream": "review", "query": "is:open review-requested:@me" },
                    { "stream": "mine", "query": "is:open author:@me" }
                ]
            }))
            .is_ok()
        );
        assert!(
            validate_config(&json!({ "watch": [] })).is_ok(),
            "an empty watch list is how you turn the watcher off",
        );
    }

    #[test]
    fn validate_config_rejects_malformed_bindings() {
        assert!(validate_config(&json!("nope")).is_err());
        assert!(validate_config(&json!({ "account": 3 })).is_err());
        assert!(validate_config(&json!({ "limit": 0 })).is_err());
        assert!(validate_config(&json!({ "limit": -1 })).is_err());
        assert!(validate_config(&json!({ "watch": "is:open" })).is_err());
        assert!(validate_config(&json!({ "watch": ["is:open"] })).is_err());
        assert!(validate_config(&json!({ "watch": [{ "stream": "review" }] })).is_err());
        assert!(
            validate_config(&json!({ "watch": [{ "stream": " ", "query": "is:open" }] })).is_err()
        );
    }

    #[test]
    fn duplicate_stream_names_are_rejected_because_they_key_stored_state() {
        let err = validate_config(&json!({
            "watch": [
                { "stream": "review", "query": "is:open review-requested:@me" },
                { "stream": "review", "query": "is:open assignee:@me" }
            ]
        }))
        .unwrap_err();
        assert!(err.to_string().contains("review"));
    }

    #[test]
    fn an_absent_watch_list_falls_back_to_the_defaults() {
        assert_eq!(queries_from(&json!({})), default_queries());
        assert_eq!(
            queries_from(
                &json!({ "watch": [{ "stream": "mine", "query": "is:open author:@me" }] })
            ),
            vec![WatchQuery {
                stream: "mine".into(),
                query: "is:open author:@me".into(),
            }],
        );
        assert!(queries_from(&json!({ "watch": [] })).is_empty());
    }

    #[test]
    fn limit_falls_back_when_absent_or_unusable() {
        assert_eq!(limit_from(&json!({})), DEFAULT_LIMIT);
        assert_eq!(limit_from(&json!({ "limit": 10 })), 10);
        assert_eq!(limit_from(&json!({ "limit": 0 })), DEFAULT_LIMIT);
        assert_eq!(limit_from(&json!({ "limit": "many" })), DEFAULT_LIMIT);
    }

    #[test]
    fn factory_is_registered_in_inventory() {
        let registry = goat_integration::registry_from_inventory();
        assert!(registry.contains_key("github"));
        assert!(goat_integration::factory_for("github").is_some());
    }

    #[test]
    fn metadata_keeps_the_credential_outside_goat() {
        let meta = GithubIntegration.metadata();
        assert!(matches!(meta.auth, IntegrationAuth::External));
        assert_eq!(meta.env_var, None);
        assert!(meta.setup.contains("gh auth login"));
        assert!(meta.has_watcher);
    }

    #[tokio::test]
    async fn the_integration_contributes_no_tools() {
        let dir = tempfile::tempdir().unwrap();
        let store = goat_store::SqliteStore::open(&dir.path().join("goat.db"))
            .await
            .unwrap();
        let runtime = IntegrationRuntime {
            credentials: CredentialStore::new(dir.path().join("credentials.json")),
            store: Arc::new(store),
            bus: goat_bus::EventBus::new(),
        };
        let mut registry = ToolRegistry::from_inventory();
        let before = registry.default_specs().len();
        let names = GithubIntegration
            .register_tools(&mut registry, &runtime, Arc::new(BindingMap::new()))
            .await;
        assert!(names.is_empty(), "the agent reaches github through `shell`");
        assert_eq!(registry.default_specs().len(), before);
    }
}
