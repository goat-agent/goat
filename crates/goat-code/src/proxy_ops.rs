use std::sync::Arc;

use goat_auth::{Credential, CredentialKey, CredentialStore, SecretString, TokenSet};
use goat_provider::{AuthMethod, ProviderId, ProviderMetadata};
use goat_providers::Registry;
use goat_proxy::{AccountOps, ProviderMeta};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub struct RegistryAccountOps {
    creds: CredentialStore,
}

impl RegistryAccountOps {
    pub fn new(creds: CredentialStore) -> Arc<Self> {
        Arc::new(Self { creds })
    }
}

fn auth_label(method: AuthMethod) -> &'static str {
    match method {
        AuthMethod::None => "none",
        AuthMethod::ApiKey => "api_key",
        AuthMethod::OAuth => "oauth",
        AuthMethod::ApiKeyOrOAuth => "api_key_or_oauth",
    }
}

fn api_key_credential(
    secret: &str,
    endpoint: Option<&str>,
    metadata: ProviderMetadata,
) -> Result<Credential, String> {
    let Some(endpoint_metadata) = metadata.login_endpoint else {
        if endpoint.is_some_and(|value| !value.trim().is_empty()) {
            return Err("endpoint is not supported for this provider".to_owned());
        }
        return Ok(Credential::ApiKey(SecretString::from(secret)));
    };
    let endpoint = endpoint
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            endpoint_metadata
                .env_var
                .and_then(|var| std::env::var(var).ok())
        })
        .or_else(|| endpoint_metadata.default.map(str::to_owned))
        .ok_or_else(|| "endpoint is required for this provider".to_owned())?;
    let endpoint = match endpoint_metadata.validate {
        Some(validate) => validate(&endpoint)?,
        None => endpoint,
    };
    Ok(Credential::ApiKeyWithEndpoint {
        secret: SecretString::from(secret),
        endpoint,
    })
}

#[async_trait::async_trait]
impl AccountOps for RegistryAccountOps {
    fn providers(&self) -> Vec<ProviderMeta> {
        let registry = Registry::new(&self.creds);
        registry
            .all()
            .iter()
            .map(|provider| {
                let metadata = provider.metadata();
                ProviderMeta {
                    id: provider.id().to_string(),
                    auth: auth_label(provider.capabilities().auth).to_owned(),
                    oauth_note: metadata.oauth.map(str::to_owned),
                    setup: metadata.setup.iter().map(|s| (*s).to_owned()).collect(),
                    env_var: metadata.env_var.map(str::to_owned),
                    endpoint_default: metadata
                        .login_endpoint
                        .and_then(|endpoint| endpoint.default)
                        .map(str::to_owned),
                    endpoint_env_var: metadata
                        .login_endpoint
                        .and_then(|endpoint| endpoint.env_var)
                        .map(str::to_owned),
                }
            })
            .collect()
    }

    async fn store_api_key(
        &self,
        provider: &str,
        account: &str,
        secret: &str,
        endpoint: Option<&str>,
    ) -> Result<(), String> {
        let registry = Registry::new(&self.creds);
        let handle = registry
            .all()
            .iter()
            .find(|candidate| candidate.id().to_string() == provider)
            .cloned()
            .ok_or_else(|| format!("unknown provider: {provider}"))?;
        let credential = api_key_credential(secret, endpoint, handle.metadata())?;
        self.creds
            .store(&CredentialKey::model(provider, account), credential)
            .map_err(|err| err.to_string())
    }

    async fn remove(&self, provider: &str, account: &str) -> Result<bool, String> {
        self.creds
            .remove(&CredentialKey::model(provider, account))
            .map_err(|err| err.to_string())
    }

    async fn verify(&self, provider: &str, account: &str) -> Result<usize, String> {
        let registry = Registry::load(&self.creds, account);
        let Some(handle) = registry.get(&ProviderId::from(provider)) else {
            return Ok(0);
        };
        let (tx, mut rx) = mpsc::channel(32);
        let discover = handle.discover(tx);
        let mut count = 0usize;
        let collect = async {
            while rx.recv().await.is_some() {
                count += 1;
            }
        };
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), collect).await;
        discover.abort();
        Ok(count)
    }

    fn oauth_login(
        &self,
        provider: &str,
        status: mpsc::Sender<String>,
    ) -> JoinHandle<Result<TokenSet, String>> {
        let registry = Registry::new(&self.creds);
        let provider = provider.to_owned();
        tokio::spawn(async move { registry.login(&provider, status).await })
    }
}
