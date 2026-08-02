use std::sync::Arc;

use goat_auth::{CredentialStore, TokenSet};
use goat_config::UserProviders;
use goat_provider::{Provider, ProviderId};
pub use goat_provider_builtin as builtin;
use goat_provider_builtin::rows;

pub const DEFAULT_ACCOUNT: &str = "default";

pub struct Registry {
    providers: Vec<Arc<dyn Provider>>,
}

impl Registry {
    pub fn new(store: &CredentialStore, user: &UserProviders) -> Self {
        Self::load(store, user, DEFAULT_ACCOUNT)
    }

    pub fn load(store: &CredentialStore, user: &UserProviders, account: &str) -> Self {
        Self::load_metered(store, user, account, None)
    }

    pub fn load_metered(
        store: &CredentialStore,
        user: &UserProviders,
        account: &str,
        meter: Option<goat_proxy::Meter>,
    ) -> Self {
        let mut providers: Vec<Arc<dyn Provider>> = vec![
            builtin::build(&rows::OPENAI, store, account),
            Arc::new(goat_provider_openai_codex::build(store, account)),
            Arc::new(goat_provider_anthropic::build(store, account)),
            Arc::new(goat_provider_gemini::build(store, account)),
            builtin::build(&rows::OPENROUTER, store, account),
            builtin::build(&rows::GROQ, store, account),
            builtin::build(&rows::DEEPSEEK, store, account),
            Arc::new(goat_provider_xai::build(store, account)),
            builtin::build(&rows::MISTRAL, store, account),
            builtin::build(&rows::ZAI, store, account),
            builtin::build(&rows::ZAI_CODING, store, account),
            builtin::build(&rows::KIMI, store, account),
            Arc::new(goat_provider_kimi_code::build(store, account)),
            builtin::build(&rows::QWEN, store, account),
            builtin::build(&rows::MINIMAX, store, account),
            builtin::build(&rows::VERCEL, store, account),
            builtin::build(&rows::OLLAMA, store, account),
            builtin::build(&rows::LMSTUDIO, store, account),
            builtin::build(&rows::LLAMA_CPP, store, account),
        ];
        for (id, config) in user.load() {
            if providers.iter().any(|provider| provider.id().0 == id) {
                continue;
            }
            if let Some(provider) = builtin::user(&id, &config.endpoint, store, account) {
                providers.push(provider);
            }
        }
        let providers = match meter {
            Some(meter) => providers
                .into_iter()
                .map(|provider| meter.wrap(provider, account))
                .collect(),
            None => providers,
        };
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
mod fingerprint {
    use std::fmt::Write as _;

    use goat_provider::{AuthMethod, ModelListSource};

    use super::Registry;

    const FIXTURE: &str = include_str!("registry_fingerprint.txt");

    fn auth_label(auth: AuthMethod) -> &'static str {
        match auth {
            AuthMethod::None => "none",
            AuthMethod::ApiKey => "api_key",
            AuthMethod::OAuth => "oauth",
            AuthMethod::ApiKeyOrOAuth => "api_key_or_oauth",
        }
    }

    fn source_label(source: ModelListSource) -> &'static str {
        match source {
            ModelListSource::Catalog => "catalog",
            ModelListSource::Discover => "discover",
        }
    }

    fn render(registry: &Registry) -> String {
        let mut out = String::new();
        for provider in registry.all() {
            let caps = provider.capabilities();
            let metadata = provider.metadata();
            writeln!(out, "provider {}", provider.id()).unwrap();
            writeln!(
                out,
                "  auth {} tools {} images {} web_search {} verifies {} source {}",
                auth_label(caps.auth),
                caps.tools,
                caps.images,
                provider.supports_web_search(),
                provider.verifies_credentials(),
                source_label(provider.model_list_source()),
            )
            .unwrap();
            writeln!(
                out,
                "  env {:?} validation {:?} endpoint {:?} oauth {:?}",
                metadata.env_var, metadata.validation, metadata.endpoint, metadata.oauth
            )
            .unwrap();
            match metadata.login_endpoint {
                Some(login) => writeln!(
                    out,
                    "  login_endpoint env {:?} default {:?} validate {}",
                    login.env_var,
                    login.default,
                    login.validate.is_some()
                )
                .unwrap(),
                None => writeln!(out, "  login_endpoint none").unwrap(),
            }
            for line in metadata.setup {
                writeln!(out, "  setup {line}").unwrap();
            }
            for model in provider.list_models() {
                let efforts = provider
                    .efforts(&model)
                    .iter()
                    .map(|effort| effort.as_str())
                    .collect::<Vec<_>>()
                    .join(",");
                let efforts = if efforts.is_empty() {
                    "-".to_owned()
                } else {
                    efforts
                };
                writeln!(
                    out,
                    "  model {model} ctx {:?} images {} efforts {efforts}",
                    provider.context_window(&model),
                    provider.supports_images(&model),
                )
                .unwrap();
            }
        }
        out
    }

    fn registry(name: &str) -> Registry {
        let path = std::env::temp_dir().join(name);
        let _ = std::fs::remove_file(&path);
        let user = goat_config::UserProviders::at(path.with_extension("config.json"));
        Registry::new(&goat_auth::CredentialStore::new(path), &user)
    }

    #[test]
    fn matches_fixture() {
        assert_eq!(
            render(&registry("goat-providers-fingerprint.json")),
            FIXTURE
        );
    }

    #[test]
    #[ignore = "rewrites registry_fingerprint.txt; run after a deliberate provider change"]
    fn regenerate() {
        let rendered = render(&registry("goat-providers-fingerprint-regen.json"));
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/registry_fingerprint.txt");
        std::fs::write(path, rendered).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use goat_provider::{AuthMethod, ProviderId};

    use super::Registry;

    fn no_user(name: &str) -> goat_config::UserProviders {
        let path = std::env::temp_dir().join(name);
        let _ = std::fs::remove_file(&path);
        goat_config::UserProviders::at(path)
    }

    #[test]
    fn user_providers_join_the_registry() {
        let store_path = std::env::temp_dir().join("goat-providers-user-store.json");
        let config_path = std::env::temp_dir().join("goat-providers-user-config.json");
        let _ = std::fs::remove_file(&store_path);
        std::fs::write(
            &config_path,
            r#"{ "providers": {
                "my-proxy": { "endpoint": "http://localhost:9/v1" },
                "openai": { "endpoint": "http://localhost:9/v1" },
                "Bad Name": { "endpoint": "http://localhost:9/v1" }
            } }"#,
        )
        .unwrap();
        let store = goat_auth::CredentialStore::new(store_path);
        let user = goat_config::UserProviders::at(config_path);
        let registry = Registry::new(&store, &user);
        assert_eq!(registry.all().len(), 20);
        let custom = registry
            .get(&ProviderId::from("my-proxy"))
            .expect("custom provider");
        assert_eq!(custom.capabilities().auth, AuthMethod::None);
        assert!(custom.authenticated());
        assert_eq!(custom.metadata().validation, "custom");
        assert_eq!(
            registry
                .get(&ProviderId::from("openai"))
                .expect("openai stays builtin")
                .metadata()
                .validation,
            "network"
        );
    }

    #[test]
    fn builtin_registers_known_providers() {
        let store = goat_auth::CredentialStore::new(
            std::env::temp_dir().join("goat-providers-registry-test.json"),
        );
        let registry = Registry::new(&store, &no_user("goat-providers-registry-nouser.json"));
        assert_eq!(registry.all().len(), 19);
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
        assert!(registry.get(&ProviderId::from("minimax")).is_some());
        assert!(registry.get(&ProviderId::from("vercel")).is_some());
        assert!(registry.get(&ProviderId::from("ollama")).is_some());
        assert!(registry.get(&ProviderId::from("does-not-exist")).is_none());
    }
}
