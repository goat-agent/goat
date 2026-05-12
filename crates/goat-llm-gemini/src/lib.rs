mod body;
mod diagnostics;
mod error;
mod models;
mod provider;
mod stream;

use std::sync::Arc;

use goat_llm::{KeyProvider, LlmProvider, LlmProviderFactory, ProviderId};

pub use provider::GeminiProvider;

pub const ID: ProviderId = ProviderId::from_static("gemini");

fn from_keys(keys: Arc<dyn KeyProvider>) -> Arc<dyn LlmProvider> {
    Arc::new(GeminiProvider::new(keys))
}

inventory::submit! {
    LlmProviderFactory { id: ID, ctor: from_keys, probe: Some(diagnostics::probe) }
}
