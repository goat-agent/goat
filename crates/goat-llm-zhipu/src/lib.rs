mod body;
mod diagnostics;
mod error;
mod models;
mod provider;
mod stream;

use std::sync::Arc;

use goat_llm::{
    summarize_api_key, ApiKeyPool, CredentialStore, LlmProvider, LlmProviderSpec, ProviderId,
    SecretPrompt,
};

pub use provider::ZhipuProvider;

pub const ID: ProviderId = ProviderId::from_static("zhipu");

fn build(store: Arc<dyn CredentialStore>) -> Arc<dyn LlmProvider> {
    Arc::new(ZhipuProvider::new(ApiKeyPool::new(store, ID)))
}

static SETUP: SecretPrompt = SecretPrompt {
    description: "Zhipu API key",
    json_key: "api_key",
    hint: "...",
};

inventory::submit! {
    LlmProviderSpec {
        id: ID,
        build,
        probe: Some(diagnostics::probe),
        setup: &SETUP,
        summarize: summarize_api_key,
    }
}
