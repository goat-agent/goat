mod body;
mod diagnostics;
mod embedding;
mod error;
mod models;
mod provider;
mod stream;

use std::sync::Arc;

use goat_llm::{
    summarize_api_key, ApiKeyPool, CredentialStore, EmbeddingProvider, EmbeddingProviderSpec,
    LlmProvider, LlmProviderSpec, ProviderId, SecretPrompt,
};

pub use embedding::OpenAiEmbeddingProvider;
pub use provider::OpenAiProvider;

pub const ID: ProviderId = ProviderId::from_static("openai");

fn build(store: Arc<dyn CredentialStore>) -> Arc<dyn LlmProvider> {
    Arc::new(OpenAiProvider::new(ApiKeyPool::new(store, ID)))
}

fn build_embedding(store: Arc<dyn CredentialStore>) -> Arc<dyn EmbeddingProvider> {
    Arc::new(OpenAiEmbeddingProvider::new(ApiKeyPool::new(store, ID)))
}

static SETUP: SecretPrompt = SecretPrompt {
    description: "OpenAI API key",
    json_key: "api_key",
    hint: "sk-...",
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

inventory::submit! {
    EmbeddingProviderSpec {
        id: ID,
        build: build_embedding,
    }
}
