use std::sync::Arc;

use goat_auth::{CredentialStore, TokenSet};
use goat_provider::{Provider, ProviderId};

pub const DEFAULT_ACCOUNT: &str = "default";

pub struct Registry {
    providers: Vec<Arc<dyn Provider>>,
}

impl Registry {
    pub fn new(store: &CredentialStore) -> Self {
        Self::load(store, DEFAULT_ACCOUNT)
    }

    pub fn load(store: &CredentialStore, account: &str) -> Self {
        let providers: Vec<Arc<dyn Provider>> = vec![
            Arc::new(goat_provider_openai::build(store, account)),
            Arc::new(goat_provider_openai_codex::build(store, account)),
            Arc::new(goat_provider_anthropic::build(store, account)),
            Arc::new(goat_provider_gemini::build(store, account)),
            Arc::new(goat_provider_openrouter::build(store, account)),
            Arc::new(goat_provider_groq::build(store, account)),
            Arc::new(goat_provider_deepseek::build(store, account)),
            Arc::new(goat_provider_xai::build(store, account)),
            Arc::new(goat_provider_mistral::build(store, account)),
            Arc::new(goat_provider_zai::build(store, account)),
            Arc::new(goat_provider_zai_coding::build(store, account)),
            Arc::new(goat_provider_kimi::build(store, account)),
            Arc::new(goat_provider_kimi_code::build(store, account)),
            Arc::new(goat_provider_qwen::build(store, account)),
            Arc::new(goat_provider_local::ollama()),
            Arc::new(goat_provider_local::lmstudio()),
            Arc::new(goat_provider_local::llama_cpp()),
        ];
        Self { providers }
    }

    pub fn from_providers(providers: Vec<Arc<dyn Provider>>) -> Self {
        Self { providers }
    }

    pub fn get(&self, id: &ProviderId) -> Option<Arc<dyn Provider>> {
        self.providers.iter().find(|p| &p.id() == id).cloned()
    }

    pub fn all(&self) -> &[Arc<dyn Provider>] {
        &self.providers
    }

    pub async fn login(
        &self,
        provider: &str,
        status: tokio::sync::mpsc::Sender<String>,
    ) -> Result<TokenSet, String> {
        let p = self
            .get(&ProviderId::from(provider))
            .ok_or_else(|| format!("unknown provider: {provider}"))?;
        p.login(status)
            .await
            .unwrap_or_else(|err| Err(err.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use goat_provider::{AuthMethod, ProviderId};

    use super::Registry;

    #[test]
    fn builtin_registers_known_providers() {
        let store = goat_auth::CredentialStore::new(
            std::env::temp_dir().join("goat-providers-registry-test.json"),
        );
        let registry = Registry::new(&store);
        assert_eq!(registry.all().len(), 17);
        assert!(registry.get(&ProviderId::from("anthropic")).is_some());
        assert!(registry.get(&ProviderId::from("openrouter")).is_some());
        assert!(registry.get(&ProviderId::from("groq")).is_some());
        assert!(registry.get(&ProviderId::from("deepseek")).is_some());
        let xai = registry
            .get(&ProviderId::from("xai"))
            .expect("xai provider");
        assert_eq!(xai.capabilities().auth, AuthMethod::ApiKeyOrOAuth);
        assert_eq!(
            xai.metadata().oauth,
            Some("browser or device code (SuperGrok / X Premium+)")
        );
        assert!(registry.get(&ProviderId::from("mistral")).is_some());
        assert!(registry.get(&ProviderId::from("zai")).is_some());
        assert!(registry.get(&ProviderId::from("zai-coding")).is_some());
        assert!(registry.get(&ProviderId::from("kimi")).is_some());
        assert!(registry.get(&ProviderId::from("kimi-code")).is_some());
        assert!(registry.get(&ProviderId::from("qwen")).is_some());
        assert!(registry.get(&ProviderId::from("ollama")).is_some());
        assert!(registry.get(&ProviderId::from("does-not-exist")).is_none());
    }
}
