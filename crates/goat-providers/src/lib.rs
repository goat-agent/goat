use std::sync::Arc;

use goat_auth::{CredentialKey, CredentialStore, TokenSet};
use goat_config::ProviderSpecs;
use goat_provider::{
    AuthMethod, Dialect, EndpointValidator, Provider, ProviderId, ProviderSpec, ProviderSpecConfig,
    validate_override_endpoint, validate_user_endpoint,
};
pub use goat_provider_builtin as builtin;
use goat_provider_builtin::rows;

pub const DEFAULT_ACCOUNT: &str = "default";

#[derive(Clone, Copy)]
enum Special {
    Codex,
    KimiCode,
    Xai,
    Devin,
}

impl Special {
    fn id(self) -> &'static str {
        match self {
            Self::Codex => "openai-codex",
            Self::KimiCode => "kimi-code",
            Self::Xai => "xai",
            Self::Devin => "devin",
        }
    }

    fn build(self, store: &CredentialStore, account: &str) -> Arc<dyn Provider> {
        match self {
            Self::Codex => Arc::new(goat_provider_openai_codex::build(store, account)),
            Self::KimiCode => Arc::new(goat_provider_kimi_code::build(store, account)),
            Self::Xai => Arc::new(goat_provider_xai::build(store, account)),
            Self::Devin => Arc::new(goat_provider_devin::build(store, account)),
        }
    }
}

#[derive(Clone, Copy)]
enum Builtin {
    Row(&'static builtin::Row),
    Anthropic,
    Gemini,
    Special(Special),
}

impl Builtin {
    fn id(self) -> &'static str {
        match self {
            Self::Row(row) => row.id,
            Self::Anthropic => goat_provider_anthropic::PROVIDER_ID,
            Self::Gemini => goat_provider_gemini::PROVIDER_ID,
            Self::Special(special) => special.id(),
        }
    }
}

const BUILTINS: &[Builtin] = &[
    Builtin::Row(&rows::OPENAI),
    Builtin::Special(Special::Codex),
    Builtin::Anthropic,
    Builtin::Gemini,
    Builtin::Row(&rows::OPENROUTER),
    Builtin::Row(&rows::GROQ),
    Builtin::Row(&rows::DEEPSEEK),
    Builtin::Special(Special::Xai),
    Builtin::Row(&rows::MISTRAL),
    Builtin::Row(&rows::ZAI),
    Builtin::Row(&rows::ZAI_CODING),
    Builtin::Row(&rows::KIMI),
    Builtin::Special(Special::KimiCode),
    Builtin::Row(&rows::QWEN),
    Builtin::Row(&rows::MINIMAX),
    Builtin::Row(&rows::VERCEL),
    Builtin::Row(&rows::OLLAMA),
    Builtin::Row(&rows::LMSTUDIO),
    Builtin::Row(&rows::LLAMA_CPP),
    Builtin::Special(Special::Devin),
];

pub struct InvalidSpec {
    pub id: String,
    pub reason: String,
}

pub struct Registry {
    providers: Vec<Arc<dyn Provider>>,
    invalid: Vec<InvalidSpec>,
}

impl Registry {
    pub fn new(store: &CredentialStore, specs: &ProviderSpecs) -> Self {
        Self::load(store, specs, DEFAULT_ACCOUNT)
    }

    pub fn load(store: &CredentialStore, specs: &ProviderSpecs, account: &str) -> Self {
        Self::load_metered(store, specs, account, None)
    }

    pub fn load_metered(
        store: &CredentialStore,
        specs: &ProviderSpecs,
        account: &str,
        meter: Option<goat_proxy::Meter>,
    ) -> Self {
        let entries = specs.load();
        let mut providers: Vec<Arc<dyn Provider>> = Vec::new();
        let mut invalid = Vec::new();
        for entry in BUILTINS {
            let id = entry.id();
            let patch = entries
                .iter()
                .find(|(key, _)| goat_model::canonicalize_provider_id(key) == id)
                .map(|(_, config)| config);
            if patch.is_some_and(|patch| patch.disabled) {
                continue;
            }
            match entry {
                Builtin::Special(special) => {
                    if patch.is_some_and(ProviderSpecConfig::has_connection_fields) {
                        invalid.push(InvalidSpec {
                            id: id.to_owned(),
                            reason: "only `disabled` and `name` overrides are supported for this provider".to_owned(),
                        });
                    }
                    providers.push(special.build(store, account));
                }
                Builtin::Row(row) => match builtin::spec_for(row, patch, store, account) {
                    Ok(spec) => providers.push(build_spec(&spec, Some(row), store, account)),
                    Err(reason) => {
                        invalid.push(InvalidSpec {
                            id: id.to_owned(),
                            reason,
                        });
                        providers.push(build_spec(
                            &builtin::base_spec(row),
                            Some(row),
                            store,
                            account,
                        ));
                    }
                },
                Builtin::Anthropic => {
                    let mut spec = goat_provider_anthropic::default_spec();
                    match patch_builtin(&mut spec, patch, validate_override_endpoint) {
                        Ok(()) => {
                            spec.credentials_usable = spec.resolve_endpoint(
                                store,
                                &CredentialKey::model(id, account),
                                validate_override_endpoint,
                            );
                            providers.push(build_spec(&spec, None, store, account));
                        }
                        Err(reason) => {
                            invalid.push(InvalidSpec {
                                id: id.to_owned(),
                                reason,
                            });
                            providers.push(build_spec(
                                &goat_provider_anthropic::default_spec(),
                                None,
                                store,
                                account,
                            ));
                        }
                    }
                }
                Builtin::Gemini => {
                    let mut spec = goat_provider_gemini::default_spec();
                    match patch_builtin(&mut spec, patch, validate_override_endpoint) {
                        Ok(()) => {
                            spec.credentials_usable = spec.resolve_endpoint(
                                store,
                                &CredentialKey::model(id, account),
                                validate_override_endpoint,
                            );
                            providers.push(build_spec(&spec, None, store, account));
                        }
                        Err(reason) => {
                            invalid.push(InvalidSpec {
                                id: id.to_owned(),
                                reason,
                            });
                            providers.push(build_spec(
                                &goat_provider_gemini::default_spec(),
                                None,
                                store,
                                account,
                            ));
                        }
                    }
                }
            }
        }
        for (id, config) in &entries {
            if BUILTINS
                .iter()
                .any(|entry| entry.id() == goat_model::canonicalize_provider_id(id))
            {
                continue;
            }
            if config.disabled {
                continue;
            }
            match build_custom(id, config, store, account) {
                Ok(provider) => providers.push(provider),
                Err(reason) => invalid.push(InvalidSpec {
                    id: id.clone(),
                    reason,
                }),
            }
        }
        let providers = match meter {
            Some(meter) => providers
                .into_iter()
                .map(|provider| meter.wrap(provider, account))
                .collect(),
            None => providers,
        };
        Self { providers, invalid }
    }

    pub fn from_providers(providers: Vec<Arc<dyn Provider>>) -> Self {
        Self {
            providers,
            invalid: Vec::new(),
        }
    }

    pub fn get(&self, id: &ProviderId) -> Option<Arc<dyn Provider>> {
        self.providers.iter().find(|p| &p.id() == id).cloned()
    }

    pub fn all(&self) -> &[Arc<dyn Provider>] {
        &self.providers
    }

    pub fn invalid(&self) -> &[InvalidSpec] {
        &self.invalid
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

fn patch_builtin(
    spec: &mut ProviderSpec,
    patch: Option<&ProviderSpecConfig>,
    validate: EndpointValidator,
) -> Result<(), String> {
    let Some(patch) = patch else {
        return Ok(());
    };
    if let Some(endpoint) = &patch.endpoint {
        validate(endpoint).map_err(|err| format!("invalid endpoint: {err}"))?;
    }
    spec.apply_patch(patch);
    Ok(())
}

fn build_spec(
    spec: &ProviderSpec,
    row: Option<&'static builtin::Row>,
    store: &CredentialStore,
    account: &str,
) -> Arc<dyn Provider> {
    match spec.dialect {
        Dialect::Chat | Dialect::Responses => builtin::build_openai_spec(row, spec, store, account),
        Dialect::Anthropic => Arc::new(goat_provider_anthropic::build_connected(
            spec, store, account, true,
        )),
        Dialect::Gemini => Arc::new(goat_provider_gemini::build_connected(spec, store, account)),
    }
}

fn build_custom(
    id: &str,
    config: &ProviderSpecConfig,
    store: &CredentialStore,
    account: &str,
) -> Result<Arc<dyn Provider>, String> {
    builtin::validate_id(id)?;
    let mut spec = ProviderSpec::custom(id, config)?;
    spec.credentials_usable = spec.resolve_endpoint(
        store,
        &CredentialKey::model(id, account),
        validate_user_endpoint,
    );
    Ok(build_spec(&spec, None, store, account))
}

pub fn is_builtin_id(id: &str) -> bool {
    BUILTINS
        .iter()
        .any(|entry| entry.id() == goat_model::canonicalize_provider_id(id))
}

pub fn validate_spec_entry(id: &str, config: &ProviderSpecConfig) -> Result<(), String> {
    let canonical = goat_model::canonicalize_provider_id(id);
    match BUILTINS.iter().find(|entry| entry.id() == canonical) {
        Some(Builtin::Special(_)) => {
            if config.has_connection_fields() {
                Err(format!("{id} only accepts `disabled` and `name` overrides"))
            } else {
                Ok(())
            }
        }
        Some(Builtin::Row(row)) => {
            if let Some(endpoint) = &config.endpoint {
                let validate = if matches!(row.auth, AuthMethod::None) {
                    validate_user_endpoint
                } else {
                    validate_override_endpoint
                };
                validate(endpoint).map_err(|err| format!("invalid endpoint: {err}"))?;
            }
            Ok(())
        }
        Some(_) => {
            if let Some(endpoint) = &config.endpoint {
                validate_override_endpoint(endpoint)
                    .map_err(|err| format!("invalid endpoint: {err}"))?;
            }
            Ok(())
        }
        None => {
            builtin::validate_id(id)?;
            match &config.endpoint {
                Some(endpoint) => {
                    validate_user_endpoint(endpoint)?;
                    Ok(())
                }
                None => Err("custom providers need an endpoint".to_owned()),
            }
        }
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
            match metadata.endpoint_override {
                Some(over) => writeln!(
                    out,
                    "  endpoint_override env {:?} default {:?} validate {}",
                    over.env_var,
                    over.default,
                    over.validate.is_some()
                )
                .unwrap(),
                None => writeln!(out, "  endpoint_override none").unwrap(),
            }
            if let Some(connection) = provider.connection() {
                writeln!(
                    out,
                    "  connection {} {} {} {:?} {:?} {:?}",
                    connection.dialect,
                    connection.endpoint,
                    connection.source.as_str(),
                    connection.auth_scheme.map(|s| s.as_str()),
                    connection.env_key,
                    connection.name,
                )
                .unwrap();
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
        let specs = goat_config::ProviderSpecs::at(path.with_extension("toml"));
        Registry::new(&goat_auth::CredentialStore::new(path), &specs)
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

    fn no_specs(name: &str) -> goat_config::ProviderSpecs {
        let path = std::env::temp_dir().join(name);
        let _ = std::fs::remove_file(&path);
        goat_config::ProviderSpecs::at(path)
    }

    #[test]
    fn user_providers_join_the_registry() {
        let store_path = std::env::temp_dir().join("goat-providers-user-store.json");
        let config_path = std::env::temp_dir().join("goat-providers-user-config.toml");
        let _ = std::fs::remove_file(&store_path);
        std::fs::write(
            &config_path,
            "[providers.my-proxy]\nendpoint = \"http://localhost:9/v1\"\n\n\
             [providers.openai]\nendpoint = \"https://localhost:9/v1\"\n\n\
             [providers.\"Bad Name\"]\nendpoint = \"http://localhost:9/v1\"\n",
        )
        .unwrap();
        let store = goat_auth::CredentialStore::new(store_path);
        let specs = goat_config::ProviderSpecs::at(config_path);
        let registry = Registry::new(&store, &specs);
        assert_eq!(registry.all().len(), 21);
        let custom = registry
            .get(&ProviderId::from("my-proxy"))
            .expect("custom provider");
        assert_eq!(custom.capabilities().auth, AuthMethod::None);
        assert!(custom.authenticated());
        assert_eq!(custom.metadata().validation, "custom");
        let openai = registry
            .get(&ProviderId::from("openai"))
            .expect("openai stays builtin");
        assert_eq!(openai.metadata().validation, "network");
        assert_eq!(
            openai.connection().expect("connection").endpoint,
            "https://localhost:9/v1",
            "the builtin-id entry patches the builtin endpoint"
        );
        assert_eq!(registry.invalid().len(), 1);
        assert_eq!(registry.invalid()[0].id, "Bad Name");
    }

    #[test]
    fn builtin_registers_known_providers() {
        let store = goat_auth::CredentialStore::new(
            std::env::temp_dir().join("goat-providers-registry-test.json"),
        );
        let registry = Registry::new(&store, &no_specs("goat-providers-registry-nouser.toml"));
        assert_eq!(registry.all().len(), 20);
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
        assert_eq!(
            registry
                .get(&ProviderId::from("devin"))
                .expect("devin provider")
                .capabilities()
                .auth,
            AuthMethod::ApiKeyOrOAuth
        );
        assert!(registry.get(&ProviderId::from("qwen")).is_some());
        assert!(registry.get(&ProviderId::from("minimax")).is_some());
        assert!(registry.get(&ProviderId::from("vercel")).is_some());
        assert!(registry.get(&ProviderId::from("ollama")).is_some());
        assert!(registry.get(&ProviderId::from("does-not-exist")).is_none());
    }
}
