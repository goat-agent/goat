mod parse;
mod watch;

use std::sync::Arc;

use async_trait::async_trait;
use goat_agent_tool::{ToolName, ToolRegistry};
use goat_auth::CredentialStore;
use goat_integration::{
    BindingMap, Integration, IntegrationAuth, IntegrationBinding, IntegrationError,
    IntegrationFactory, IntegrationMetadata, IntegrationResult, IntegrationRuntime,
};
use goat_types::{IntegrationId, ProfileId};
use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

pub const ID: IntegrationId = IntegrationId::from_static("github");

const SETUP: &str = "goat reaches github through the `gh` cli — it holds the credential, goat never stores one.\ninstall gh, then run `gh auth login`.";
const MISSING_GH: &str = "the `gh` cli is not on PATH; install it and run `gh auth login`";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchStream {
    pub stream: String,
    pub query: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GithubBinding {
    #[serde(default)]
    watch: Option<Vec<WatchEntry>>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WatchEntry {
    stream: String,
    query: String,
}

impl GithubBinding {
    pub(crate) fn read(config: &Value) -> Self {
        goat_integration_mcp::read_binding(config)
    }

    pub(crate) fn streams(&self) -> Vec<WatchStream> {
        let Some(entries) = &self.watch else {
            return default_streams();
        };
        entries
            .iter()
            .filter(|entry| !entry.stream.trim().is_empty() && !entry.query.trim().is_empty())
            .map(|entry| WatchStream {
                stream: entry.stream.trim().to_owned(),
                query: entry.query.trim().to_owned(),
            })
            .collect()
    }

    pub(crate) fn limit(&self) -> usize {
        self.limit
            .filter(|limit| *limit > 0)
            .unwrap_or(watch::DEFAULT_LIMIT)
    }
}

pub fn default_streams() -> Vec<WatchStream> {
    vec![
        WatchStream {
            stream: "review".to_owned(),
            query: "is:open is:pr review-requested:@me".to_owned(),
        },
        WatchStream {
            stream: "assigned".to_owned(),
            query: "is:open assignee:@me".to_owned(),
        },
    ]
}

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
        let handles = watch::spawn_all(persona, &binding, &runtime, &cancel);
        if handles.is_empty() {
            return None;
        }
        Some(tokio::spawn(async move {
            for handle in handles {
                let _ = handle.await;
            }
        }))
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

fn validate_config(config: &Value) -> IntegrationResult<()> {
    goat_integration_mcp::validate_binding::<GithubBinding>("github", config)?;
    let settings = GithubBinding::read(config);
    let streams = settings.streams();
    let mut seen = std::collections::BTreeSet::new();
    for entry in &streams {
        if !seen.insert(entry.stream.clone()) {
            return Err(IntegrationError::Config(format!(
                "github binding: `watch` repeats the stream `{}`; stream names key stored state and must be unique",
                entry.stream
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
    fn an_absent_watch_key_means_the_two_default_streams() {
        assert_eq!(GithubBinding::read(&json!({})).streams(), default_streams());
    }

    #[test]
    fn an_explicit_empty_watch_list_turns_the_watcher_off() {
        assert!(
            GithubBinding::read(&json!({ "watch": [] }))
                .streams()
                .is_empty()
        );
    }

    #[test]
    fn declared_streams_replace_the_defaults() {
        let read = GithubBinding::read(&json!({
            "watch": [{ "stream": "mine", "query": "is:open author:@me" }]
        }));
        assert_eq!(
            read.streams(),
            vec![WatchStream {
                stream: "mine".to_owned(),
                query: "is:open author:@me".to_owned()
            }]
        );
    }

    #[test]
    fn a_repeated_stream_name_is_rejected_because_it_keys_stored_state() {
        let config = json!({
            "watch": [
                { "stream": "mine", "query": "a" },
                { "stream": "mine", "query": "b" }
            ]
        });
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn the_binding_is_typo_checked() {
        assert!(validate_config(&json!({})).is_ok());
        assert!(validate_config(&json!({ "account": "work" })).is_ok());
        assert!(validate_config(&json!({ "limit": 10 })).is_ok());
        assert!(validate_config(&json!("nope")).is_err());
        assert!(validate_config(&json!({ "limit": "ten" })).is_err());
        assert!(validate_config(&json!({ "watch": [{ "stream": "s" }] })).is_err());
        assert!(validate_config(&json!({ "wach": [] })).is_err());
    }

    #[test]
    fn a_zero_or_absent_limit_falls_back_to_the_default() {
        assert_eq!(
            GithubBinding::read(&json!({})).limit(),
            watch::DEFAULT_LIMIT
        );
        assert_eq!(
            GithubBinding::read(&json!({ "limit": 0 })).limit(),
            watch::DEFAULT_LIMIT
        );
        assert_eq!(GithubBinding::read(&json!({ "limit": 10 })).limit(), 10);
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
    }
}
