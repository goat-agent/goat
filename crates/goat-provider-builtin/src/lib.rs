use std::sync::Arc;

use goat_auth::{CredentialKey, CredentialStore};
use goat_provider::{AuthMethod, Effort, ModelListSource, Provider, ProviderId, ProviderMetadata};
use goat_provider_openai_compat::{
    ChatDiscovery, ChatValidation, OpenAiCompatProvider, ResponsesProvider, enforce_https_host,
};

pub mod rows;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wire {
    Chat,
    Responses,
}

pub struct Row {
    pub id: &'static str,
    pub wire: Wire,
    pub base_url: &'static str,
    pub host: Option<&'static str>,
    pub env_var: Option<&'static str>,
    pub auth: AuthMethod,
    pub catalog: &'static [&'static str],
    pub context_windows: &'static [(&'static str, u32)],
    pub model_filter: Option<fn(&str) -> bool>,
    pub vision_filter: Option<fn(&str) -> bool>,
    pub efforts: Option<fn(&str) -> Vec<Effort>>,
    pub effort_wire: Option<fn(Effort) -> Option<&'static str>>,
    pub extra_headers: &'static [(&'static str, &'static str)],
    pub images: bool,
    pub stream_options: bool,
    pub reasoning_effort: bool,
    pub catalog_only: bool,
    pub live_model_list: bool,
    pub search_model: Option<&'static str>,
    pub metadata: ProviderMetadata,
}

impl Row {
    pub const fn hosted(
        id: &'static str,
        base_url: &'static str,
        host: &'static str,
        env_var: &'static str,
    ) -> Self {
        Self {
            id,
            wire: Wire::Chat,
            base_url,
            host: Some(host),
            env_var: Some(env_var),
            auth: AuthMethod::ApiKey,
            catalog: &[],
            context_windows: &[],
            model_filter: None,
            vision_filter: None,
            efforts: None,
            effort_wire: None,
            extra_headers: &[],
            images: true,
            stream_options: true,
            reasoning_effort: true,
            catalog_only: false,
            live_model_list: false,
            search_model: None,
            metadata: ProviderMetadata {
                env_var: Some(env_var),
                validation: "network",
                endpoint: None,
                oauth: Some("not supported"),
                login_endpoint: None,
                setup: &[],
            },
        }
    }

    pub const fn local(id: &'static str, base_url: &'static str) -> Self {
        Self {
            id,
            wire: Wire::Chat,
            base_url,
            host: None,
            env_var: None,
            auth: AuthMethod::None,
            catalog: &[],
            context_windows: &[],
            model_filter: None,
            vision_filter: None,
            efforts: None,
            effort_wire: None,
            extra_headers: &[],
            images: true,
            stream_options: true,
            reasoning_effort: true,
            catalog_only: false,
            live_model_list: false,
            search_model: None,
            metadata: ProviderMetadata {
                env_var: None,
                validation: "local",
                endpoint: Some(base_url),
                oauth: None,
                login_endpoint: None,
                setup: &[],
            },
        }
    }
}

pub fn build(row: &'static Row, store: &CredentialStore, account: &str) -> Arc<dyn Provider> {
    if let Some(host) = row.host {
        enforce_https_host(row.base_url, host).expect("builtin provider base URL");
    }
    let key = CredentialKey::model(row.id, account);
    let (endpoint, credentials_usable) = resolve_endpoint(row, store, &key);
    let bearer = match row.env_var {
        Some(env_var) if credentials_usable => store
            .resolve(&key, Some(env_var))
            .map(|cred| cred.bearer().to_owned()),
        _ => None,
    };
    match row.wire {
        Wire::Chat => Arc::new(build_chat(row, endpoint, bearer)),
        Wire::Responses => Arc::new(build_responses(row, endpoint, bearer)),
    }
}

fn resolve_endpoint(row: &Row, store: &CredentialStore, key: &CredentialKey) -> (String, bool) {
    let Some(login_endpoint) = row.metadata.login_endpoint else {
        return (row.base_url.to_owned(), true);
    };
    let stored = store
        .get(key)
        .and_then(|cred| cred.endpoint().map(str::to_owned));
    match stored {
        Some(raw) => match login_endpoint.validate {
            Some(validate) => match validate(&raw) {
                Ok(endpoint) => (endpoint, true),
                Err(_) => (row.base_url.to_owned(), false),
            },
            None => (raw, true),
        },
        None => (row.base_url.to_owned(), true),
    }
}

fn build_chat(row: &Row, endpoint: String, bearer: Option<String>) -> OpenAiCompatProvider {
    let mut provider =
        OpenAiCompatProvider::new(ProviderId::from(row.id), endpoint, bearer, row.auth)
            .with_catalog(row.catalog)
            .with_context_windows(row.context_windows)
            .with_images(row.images)
            .with_stream_options(row.stream_options)
            .with_reasoning_effort(row.reasoning_effort)
            .with_extra_headers(row.extra_headers.iter().copied())
            .with_metadata(row.metadata);
    if let Some(filter) = row.model_filter {
        provider = provider.with_model_filter(filter);
    }
    if let Some(filter) = row.vision_filter {
        provider = provider.with_vision_filter(filter);
    }
    if let Some(efforts) = row.efforts {
        provider = provider.with_efforts(efforts);
    }
    if let Some(effort_wire) = row.effort_wire {
        provider = provider.with_effort_wire(effort_wire);
    }
    if row.catalog_only {
        provider = provider
            .with_validation(ChatValidation::CatalogOnly)
            .with_discovery(ChatDiscovery::CatalogOnly);
    }
    if row.live_model_list {
        provider = provider.with_model_list_source(ModelListSource::Discover);
    }
    provider
}

fn build_responses(row: &Row, endpoint: String, bearer: Option<String>) -> ResponsesProvider {
    let mut provider = ResponsesProvider::new(ProviderId::from(row.id), endpoint, bearer, row.auth)
        .with_catalog(row.catalog)
        .with_context_windows(row.context_windows)
        .with_extra_headers(row.extra_headers.iter().copied())
        .with_metadata(row.metadata);
    if let Some(filter) = row.model_filter {
        provider = provider.with_model_filter(filter);
    }
    if let Some(filter) = row.vision_filter {
        provider = provider.with_vision_filter(filter);
    }
    if let Some(search_model) = row.search_model {
        provider = provider.with_search_model(search_model);
    }
    provider
}
