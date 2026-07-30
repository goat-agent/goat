pub mod diff;
pub mod schema;
pub mod watch;

#[cfg(feature = "test-support")]
pub mod test_support;

pub use schema::drop_placeholder_args;

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use goat_agent_tool::{ToolName, ToolRegistry};
use goat_auth::CredentialStore;
use goat_bus::EventBus;
use goat_store::{NewObservation, Store, StoreError};
use goat_types::{AgentId, Event, IntegrationId};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tracing::warn;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum IntegrationError {
    #[error("auth: {0}")]
    Auth(String),
    #[error("config: {0}")]
    Config(String),
    #[error("service: {0}")]
    Service(String),
    #[error("store: {0}")]
    Store(String),
}

pub type IntegrationResult<T> = Result<T, IntegrationError>;

#[derive(Clone, Debug)]
pub struct IntegrationBinding {
    pub account: String,
    pub config: serde_json::Value,
}

impl IntegrationBinding {
    pub fn from_config(config: serde_json::Value) -> Self {
        let account = config
            .get("account")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("default")
            .to_string();
        Self { account, config }
    }
}

pub type BindingMap = HashMap<AgentId, IntegrationBinding>;

#[derive(Clone)]
pub struct IntegrationRuntime {
    pub credentials: CredentialStore,
    pub store: Arc<dyn Store>,
    pub bus: EventBus,
}

impl IntegrationRuntime {
    pub async fn load_state(
        &self,
        agent: AgentId,
        integration: &IntegrationId,
        account: &str,
        stream: &str,
    ) -> IntegrationResult<Option<String>> {
        self.store
            .integration_state(agent, integration.as_str(), account, stream)
            .await
            .map_err(|e| store_err(&e))
    }

    pub async fn save_state(
        &self,
        agent: AgentId,
        integration: &IntegrationId,
        account: &str,
        stream: &str,
        state: &str,
    ) -> IntegrationResult<()> {
        self.store
            .set_integration_state(agent, integration.as_str(), account, stream, state)
            .await
            .map_err(|e| store_err(&e))
    }

    pub async fn record_observation(
        &self,
        agent: AgentId,
        integration: &IntegrationId,
        account: &str,
        external_ref: &str,
        kind: &str,
        payload: serde_json::Value,
    ) -> IntegrationResult<i64> {
        self.store
            .record_observation(NewObservation {
                agent,
                integration: integration.as_str().to_string(),
                account: account.to_string(),
                external_ref: external_ref.to_string(),
                kind: kind.to_string(),
                payload,
            })
            .await
            .map_err(|e| store_err(&e))
    }

    pub fn publish(&self, event: Event) {
        self.bus.publish(event);
    }

    pub async fn paused(&self) -> bool {
        self.store.is_paused().await.unwrap_or(false)
    }
}

fn store_err(e: &StoreError) -> IntegrationError {
    IntegrationError::Store(e.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntegrationAuth {
    Secret,
    OAuth,
    External,
}

#[derive(Clone, Copy, Debug)]
pub struct IntegrationMetadata {
    pub id: &'static str,
    pub display: &'static str,
    pub auth: IntegrationAuth,
    pub secret_label: &'static str,
    pub env_var: Option<&'static str>,
    pub setup: &'static str,
}

#[async_trait]
pub trait Integration: Send + Sync + 'static {
    fn id(&self) -> IntegrationId;

    fn metadata(&self) -> IntegrationMetadata;

    async fn register_tools(
        &self,
        registry: &mut ToolRegistry,
        runtime: &IntegrationRuntime,
        bindings: Arc<BindingMap>,
    ) -> Vec<ToolName>;

    fn spawn_watcher(
        &self,
        agent: AgentId,
        binding: IntegrationBinding,
        runtime: IntegrationRuntime,
        cancel: CancellationToken,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let _ = (agent, binding, runtime, cancel);
        None
    }

    async fn verify(
        &self,
        config: &serde_json::Value,
        credentials: &CredentialStore,
    ) -> IntegrationResult<String>;

    async fn oauth_login(
        &self,
        credentials: &CredentialStore,
        account: &str,
        present_url: &(dyn for<'a> Fn(&'a str) + Send + Sync),
    ) -> IntegrationResult<serde_json::Value> {
        let _ = (credentials, account, present_url);
        Err(IntegrationError::Config(
            "oauth login is not supported by this integration".into(),
        ))
    }
}

pub struct IntegrationFactory {
    pub id: IntegrationId,
    pub ctor: fn() -> Arc<dyn Integration>,
    pub validate_config: fn(&serde_json::Value) -> IntegrationResult<()>,
}

inventory::collect!(IntegrationFactory);

pub fn factories() -> Vec<&'static IntegrationFactory> {
    inventory::iter::<IntegrationFactory>().collect()
}

pub fn factory_for(id: &str) -> Option<&'static IntegrationFactory> {
    inventory::iter::<IntegrationFactory>().find(|f| f.id.as_str() == id)
}

pub fn registry_from_inventory() -> HashMap<String, Arc<dyn Integration>> {
    let mut map: HashMap<String, Arc<dyn Integration>> = HashMap::new();
    for factory in inventory::iter::<IntegrationFactory>() {
        let id = factory.id.as_str().to_string();
        if map.contains_key(&id) {
            warn!(integration = %id, "duplicate integration factory ignored");
            continue;
        }
        map.insert(id, (factory.ctor)());
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use goat_store::SqliteStore;

    async fn runtime() -> (tempfile::TempDir, IntegrationRuntime) {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(&dir.path().join("goat.db"))
            .await
            .unwrap();
        let rt = IntegrationRuntime {
            credentials: CredentialStore::new(dir.path().join("credentials.json")),
            store: Arc::new(store),
            bus: EventBus::new(),
        };
        (dir, rt)
    }

    #[test]
    fn binding_from_config_defaults_account() {
        let b = IntegrationBinding::from_config(serde_json::json!({}));
        assert_eq!(b.account, "default");
        let b = IntegrationBinding::from_config(serde_json::json!({ "account": "work" }));
        assert_eq!(b.account, "work");
    }

    #[tokio::test]
    async fn observation_round_trip_through_runtime() {
        let (_d, rt) = runtime().await;
        let agent = AgentId::from_slug("test");
        rt.store.ensure_agent(agent, "test", "test").await.unwrap();
        let id = rt
            .record_observation(
                agent,
                &IntegrationId::from_static("linear"),
                "default",
                "linear/default:issue:US-1",
                "assigned",
                serde_json::json!({ "id": "US-1" }),
            )
            .await
            .unwrap();
        let record = rt.store.get_observation(id).await.unwrap().unwrap();
        assert_eq!(record.external_ref, "linear/default:issue:US-1");
        assert_eq!(record.payload["id"], "US-1");
    }

    #[tokio::test]
    async fn state_round_trip_through_runtime() {
        let (_d, rt) = runtime().await;
        let agent = AgentId::from_slug("test");
        rt.store.ensure_agent(agent, "test", "test").await.unwrap();
        let id = IntegrationId::from_static("linear");

        assert!(
            rt.load_state(agent, &id, "default", "assigned")
                .await
                .unwrap()
                .is_none()
        );
        rt.save_state(agent, &id, "default", "assigned", "{\"w\":1}")
            .await
            .unwrap();
        assert_eq!(
            rt.load_state(agent, &id, "default", "assigned")
                .await
                .unwrap()
                .as_deref(),
            Some("{\"w\":1}")
        );
    }
}
