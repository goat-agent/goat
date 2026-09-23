use std::path::Path;
use std::time::Duration;

use goat_api::{
    AdminIntegrationConnectOutput, AdminIntegrationConnectParams, AdminIntegrationRemoveOutput,
    AdminIntegrationStatusOutput, IntegrationConnectionStatus, IntegrationRemoveScope,
    IntegrationState, InvalidIntegrationConnection,
};
use goat_auth::{Credential, CredentialKey, CredentialStore, SecretString};
use goat_integration::{
    CLIENT_ID_SLOT, CLIENT_SECRET_SLOT, Connection, ConnectionState, Connections, CredentialSource,
    IntegrationError,
};
use serde_json::Value;

use crate::manager::CodeSessionHub;

const VERIFY_TIMEOUT: Duration = Duration::from_secs(15);

impl CodeSessionHub {
    pub(crate) async fn connect_integration(
        &self,
        params: AdminIntegrationConnectParams,
    ) -> Result<AdminIntegrationConnectOutput, String> {
        goat_integration::connection::validate_name(&params.name).map_err(|e| e.to_string())?;
        let factory = goat_integration::factory_for(&params.kind)
            .ok_or_else(|| format!("unknown integration `{}`", params.kind))?;
        let integration = (factory.ctor)();
        let metadata = integration.metadata();
        let config_path = self.config_path()?;
        let existing = connections_at(&config_path);
        if let Some(other) = existing.get(&params.name)
            && other.kind != params.kind
        {
            return Err(format!(
                "`{}` is already a {} connection",
                params.name, other.kind
            ));
        }
        let connection = Connection::new(
            &params.name,
            &params.kind,
            connection_config(&metadata, existing.get(&params.name), params.config)?,
        );

        let mut staged = Vec::new();
        if let Some(credential) = params.credential {
            staged.push((connection.credential_key(), Credential::from(credential)));
        }
        if let Some(client) = params.client {
            staged.push((
                connection.slot_key(CLIENT_ID_SLOT),
                Credential::ApiKey(SecretString::from(client.id.as_str())),
            ));
            if let Some(secret) = client.secret.filter(|secret| !secret.trim().is_empty()) {
                staged.push((
                    connection.slot_key(CLIENT_SECRET_SLOT),
                    Credential::ApiKey(SecretString::from(secret.as_str())),
                ));
            }
        }

        let credentials = CredentialStore::new(self.auth_path().to_path_buf());
        let outcome = if params.verify {
            let candidate = credentials.staged(staged.clone());
            let binding = connection.binding(&Value::Null);
            match tokio::time::timeout(VERIFY_TIMEOUT, integration.verify(&binding, &candidate))
                .await
            {
                Ok(Ok(identity)) => {
                    staged = candidate.staged_entries();
                    AdminIntegrationConnectOutput::Verified { identity }
                }
                Ok(Err(IntegrationError::Auth(message) | IntegrationError::Config(message))) => {
                    return Err(format!(
                        "{} rejected the credential ({}); nothing was stored",
                        metadata.display,
                        last_clause(&message)
                    ));
                }
                Ok(Err(other)) => AdminIntegrationConnectOutput::Unverified {
                    message: other.to_string(),
                },
                Err(_) => AdminIntegrationConnectOutput::Unverified {
                    message: format!("verification timed out after {}s", VERIFY_TIMEOUT.as_secs()),
                },
            }
        } else {
            AdminIntegrationConnectOutput::Unverified {
                message: "verification was skipped".to_owned(),
            }
        };

        let _writes = self.config_writes();
        commit(&credentials, &config_path, &connection, staged)?;
        Ok(outcome)
    }

    pub(crate) fn remove_integration(
        &self,
        name: &str,
        scope: IntegrationRemoveScope,
    ) -> Result<AdminIntegrationRemoveOutput, String> {
        let config_path = self.config_path()?;
        let _writes = self.config_writes();
        let connections = connections_at(&config_path);
        let connection = connections
            .get(name)
            .ok_or_else(|| format!("no integration connection named `{name}`"))?;
        let credentials = CredentialStore::new(self.auth_path().to_path_buf());
        let mut removed = credentials
            .remove_many(&connection.credential_keys())
            .map_err(|e| e.to_string())?
            > 0;
        if scope == IntegrationRemoveScope::Connection {
            let mut doc = goat_config::ConfigDocument::load_at(&config_path);
            doc.remove_integration(name);
            doc.save(&config_path)
                .map_err(|e| format!("could not write the config: {e}"))?;
            removed = true;
        }
        Ok(AdminIntegrationRemoveOutput {
            removed,
            used_by: agents_using(self.agents_dir(), name),
        })
    }

    pub(crate) async fn integration_status(
        &self,
        name: Option<&str>,
        verify: bool,
    ) -> Result<AdminIntegrationStatusOutput, String> {
        let connections = connections_at(&self.config_path()?);
        let credentials = CredentialStore::new(self.auth_path().to_path_buf());
        let agents_dir = self.agents_dir();
        let mut checks = tokio::task::JoinSet::new();
        let mut statuses = Vec::new();
        for connection in connections
            .valid
            .iter()
            .filter(|connection| name.is_none_or(|name| connection.name == name))
        {
            let Some(factory) = goat_integration::factory_for(&connection.kind) else {
                continue;
            };
            let integration = (factory.ctor)();
            let state = goat_integration::connection_state(
                &integration.metadata(),
                connection,
                &credentials,
            );
            if verify && state.is_ready() {
                let binding = connection.binding(&Value::Null);
                let credentials = credentials.clone();
                let index = statuses.len();
                checks.spawn(async move {
                    let checked = tokio::time::timeout(
                        VERIFY_TIMEOUT,
                        integration.verify(&binding, &credentials),
                    )
                    .await;
                    (index, checked)
                });
            }
            statuses.push(IntegrationConnectionStatus {
                name: connection.name.clone(),
                kind: connection.kind.clone(),
                config: connection.config.clone(),
                state: wire_state(&state),
                identity: None,
                used_by: agents_using(agents_dir, &connection.name),
            });
        }
        while let Some(joined) = checks.join_next().await {
            let Ok((index, checked)) = joined else {
                continue;
            };
            let status = &mut statuses[index];
            match checked {
                Ok(Ok(identity)) => status.identity = Some(identity),
                Ok(Err(e)) => {
                    status.state = IntegrationState::Failed {
                        message: last_clause(&e.to_string()).to_owned(),
                    };
                }
                Err(_) => {
                    status.state = IntegrationState::Failed {
                        message: format!(
                            "verification timed out after {}s",
                            VERIFY_TIMEOUT.as_secs()
                        ),
                    };
                }
            }
        }
        Ok(AdminIntegrationStatusOutput {
            connections: statuses,
            invalid: connections
                .invalid
                .into_iter()
                .filter(|(invalid, _)| name.is_none_or(|name| invalid == name))
                .map(|(name, reason)| InvalidIntegrationConnection { name, reason })
                .collect(),
        })
    }
}

fn last_clause(message: &str) -> &str {
    message.rsplit("error: ").next().unwrap_or(message).trim()
}

fn connections_at(config_path: &Path) -> Connections {
    Connections::parse(&goat_config::Config::read_at(config_path).integrations)
}

fn connection_config(
    metadata: &goat_integration::IntegrationMetadata,
    existing: Option<&Connection>,
    patch: Value,
) -> Result<Value, String> {
    let mut config = existing
        .and_then(|connection| connection.config.as_object().cloned())
        .unwrap_or_default();
    let patch = match patch {
        Value::Null => serde_json::Map::new(),
        Value::Object(patch) => patch,
        _ => return Err("the connection config must be an object".to_owned()),
    };
    for (key, value) in patch {
        if !metadata
            .connection_keys
            .iter()
            .any(|known| known.name == key)
        {
            return Err(format!(
                "{} has no connection setting `{key}`",
                metadata.display
            ));
        }
        if value.is_null() {
            config.remove(&key);
        } else {
            config.insert(key, value);
        }
    }
    Ok(Value::Object(config))
}

fn commit(
    credentials: &CredentialStore,
    config_path: &Path,
    connection: &Connection,
    staged: Vec<(CredentialKey, Credential)>,
) -> Result<(), String> {
    let keys: Vec<CredentialKey> = staged.iter().map(|(key, _)| key.clone()).collect();
    let previous: Vec<(CredentialKey, Option<Credential>)> = keys
        .iter()
        .map(|key| (key.clone(), credentials.get(key)))
        .collect();
    credentials
        .store_many(staged)
        .map_err(|e| format!("could not store the credential: {e}"))?;
    let mut doc = goat_config::ConfigDocument::load_at(config_path);
    let written = doc
        .set_integration(&connection.name, &connection.entry())
        .map_err(|e| e.to_string())
        .and_then(|()| {
            doc.save(config_path)
                .map_err(|e| format!("could not write the config: {e}"))
        });
    if let Err(e) = written {
        let (restore, drop): (Vec<_>, Vec<_>) =
            previous.into_iter().partition(|(_, value)| value.is_some());
        let _ = credentials.store_many(
            restore
                .into_iter()
                .filter_map(|(key, value)| value.map(|value| (key, value))),
        );
        let _ = credentials.remove_many(&drop.into_iter().map(|(key, _)| key).collect::<Vec<_>>());
        return Err(e);
    }
    Ok(())
}

fn wire_state(state: &ConnectionState) -> IntegrationState {
    match state {
        ConnectionState::Ready(source) => IntegrationState::Ready {
            source: match source {
                CredentialSource::Stored => "stored",
                CredentialSource::Environment => "environment",
                CredentialSource::External => "external",
            }
            .to_owned(),
        },
        ConnectionState::NeedsLogin(reason) => IntegrationState::NeedsLogin {
            reason: reason.clone(),
        },
    }
}

fn agents_using(agents_dir: Option<&Path>, name: &str) -> Vec<String> {
    let Some(entries) = agents_dir.and_then(|dir| std::fs::read_dir(dir).ok()) else {
        return Vec::new();
    };
    let mut slugs: Vec<String> = entries
        .flatten()
        .filter(|entry| {
            std::fs::read_to_string(entry.path().join(goat_config::AGENT_CONFIG_FILE))
                .ok()
                .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
                .is_some_and(|config| config["integrations"].get(name).is_some())
        })
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .collect();
    slugs.sort();
    slugs
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use goat_api::IntegrationClient;
    use goat_integration::{
        BindingMap, IntegrationAuth, IntegrationBinding, IntegrationFactory, IntegrationMetadata,
        IntegrationResult, IntegrationRuntime,
    };
    use goat_types::IntegrationId;
    use serde_json::json;

    use super::*;

    struct Fake;

    #[async_trait]
    impl goat_integration::Integration for Fake {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn id(&self) -> IntegrationId {
            IntegrationId::from_static("fakesvc")
        }

        fn metadata(&self) -> IntegrationMetadata {
            IntegrationMetadata {
                id: "fakesvc",
                display: "Fake",
                summary: "",
                auth: IntegrationAuth::Secret,
                secret_label: "key",
                env_var: None,
                setup: "",
                preregistered: false,
                tools: true,
                connection_keys: &[goat_integration::HOST_KEY],
                binding_keys: &[],
            }
        }

        async fn register_tools(
            &self,
            _registry: &mut goat_tool::ToolRegistry,
            _runtime: &IntegrationRuntime,
            _bindings: Arc<BindingMap>,
        ) -> Vec<goat_tool::ToolName> {
            Vec::new()
        }

        async fn verify(
            &self,
            binding: &IntegrationBinding,
            credentials: &CredentialStore,
        ) -> IntegrationResult<String> {
            let key = CredentialKey::integration("fakesvc", &binding.account);
            match credentials.get(&key) {
                Some(Credential::ApiKey(secret)) if secret.expose() == "good" => {
                    Ok(format!("{} ok", binding.account))
                }
                Some(Credential::ApiKey(secret)) if secret.expose() == "flaky" => Err(
                    IntegrationError::Service("the service is unreachable".to_owned()),
                ),
                _ => Err(IntegrationError::Auth("rejected".to_owned())),
            }
        }
    }

    inventory::submit! {
        IntegrationFactory {
            id: IntegrationId::from_static("fakesvc"),
            ctor: || Arc::new(Fake),
            validate_config: |_| Ok(()),
        }
    }

    fn hub(dir: &Path) -> CodeSessionHub {
        let hub = CodeSessionHub::new(
            dir.join("credentials.json"),
            goat_config::ProviderSpecs::at(dir.join("config.toml")),
            dir.join("goat.db"),
        );
        hub.set_agents_dir(dir.join("agents"));
        hub
    }

    fn params(name: &str, key: &str) -> AdminIntegrationConnectParams {
        AdminIntegrationConnectParams {
            name: name.to_owned(),
            kind: "fakesvc".to_owned(),
            config: Value::Null,
            credential: Some(Credential::ApiKey(SecretString::from(key)).into()),
            client: None,
            verify: true,
        }
    }

    fn stored(dir: &Path, name: &str) -> Option<Credential> {
        CredentialStore::new(dir.join("credentials.json"))
            .get(&CredentialKey::integration("fakesvc", name))
    }

    #[tokio::test]
    async fn a_rejected_credential_stores_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let hub = hub(dir.path());
        let err = hub
            .connect_integration(params("fakesvc", "bad"))
            .await
            .unwrap_err();
        assert!(err.contains("nothing was stored"), "{err}");
        assert!(stored(dir.path(), "fakesvc").is_none());
        assert!(
            connections_at(&dir.path().join("config.toml"))
                .valid
                .is_empty()
        );
    }

    #[tokio::test]
    async fn a_verified_connection_is_stored_under_its_name() {
        let dir = tempfile::tempdir().unwrap();
        let hub = hub(dir.path());
        let outcome = hub
            .connect_integration(params("fakesvc-work", "good"))
            .await
            .unwrap();
        assert_eq!(
            outcome,
            AdminIntegrationConnectOutput::Verified {
                identity: "fakesvc-work ok".to_owned()
            }
        );
        assert!(stored(dir.path(), "fakesvc-work").is_some());
        let connections = connections_at(&dir.path().join("config.toml"));
        assert_eq!(connections.get("fakesvc-work").unwrap().kind, "fakesvc");
    }

    #[tokio::test]
    async fn an_unreachable_service_stores_the_credential_unverified() {
        let dir = tempfile::tempdir().unwrap();
        let hub = hub(dir.path());
        let outcome = hub
            .connect_integration(params("fakesvc", "flaky"))
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            AdminIntegrationConnectOutput::Unverified { .. }
        ));
        assert!(stored(dir.path(), "fakesvc").is_some());
    }

    #[tokio::test]
    async fn only_declared_connection_keys_are_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let hub = hub(dir.path());
        let mut bad = params("fakesvc", "good");
        bad.config = json!({ "org": "acme" });
        assert!(hub.connect_integration(bad).await.is_err());

        let mut hosted = params("fakesvc", "good");
        hosted.config = json!({ "host": "https://eu.example" });
        hosted.client = Some(IntegrationClient {
            id: "cid".to_owned(),
            secret: None,
        });
        hub.connect_integration(hosted).await.unwrap();
        let connections = connections_at(&dir.path().join("config.toml"));
        assert_eq!(
            connections.get("fakesvc").unwrap().config["host"],
            "https://eu.example"
        );
    }

    #[tokio::test]
    async fn status_reports_state_and_users_and_remove_reports_them_too() {
        let dir = tempfile::tempdir().unwrap();
        let hub = hub(dir.path());
        hub.connect_integration(params("fakesvc", "good"))
            .await
            .unwrap();
        let agent = dir.path().join("agents").join("bot");
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::write(
            agent.join(goat_config::AGENT_CONFIG_FILE),
            r#"{"integrations":{"fakesvc":{}}}"#,
        )
        .unwrap();

        let status = hub.integration_status(None, true).await.unwrap();
        assert_eq!(status.connections.len(), 1);
        assert_eq!(status.connections[0].used_by, vec!["bot".to_owned()]);
        assert_eq!(
            status.connections[0].identity.as_deref(),
            Some("fakesvc ok")
        );

        let logout = hub
            .remove_integration("fakesvc", IntegrationRemoveScope::Credential)
            .unwrap();
        assert!(logout.removed);
        assert_eq!(logout.used_by, vec!["bot".to_owned()]);
        let status = hub.integration_status(None, false).await.unwrap();
        assert!(matches!(
            status.connections[0].state,
            IntegrationState::NeedsLogin { .. }
        ));

        hub.remove_integration("fakesvc", IntegrationRemoveScope::Connection)
            .unwrap();
        assert!(
            hub.integration_status(None, false)
                .await
                .unwrap()
                .connections
                .is_empty()
        );
    }
}
