mod auth;
mod body;
mod credential;
mod diagnostics;
mod error;
mod models;
mod provider;
mod setup;
mod stream;
mod summarize;

use std::sync::Arc;

use goat_llm::{CredentialStore, LlmProvider, LlmProviderSpec, ProviderId, Refreshable};

use crate::provider::CodexProvider;

pub const ID: ProviderId = ProviderId::from_static("codex");

fn build(store: Arc<dyn CredentialStore>) -> Arc<dyn LlmProvider> {
    let refreshable = Refreshable::new(store, ID, None);
    Arc::new(CodexProvider::new(refreshable))
}

static SETUP: setup::PkceSetup = setup::PkceSetup;

inventory::submit! {
    LlmProviderSpec {
        id: ID,
        build,
        probe: Some(diagnostics::probe),
        setup: &SETUP,
        summarize: summarize::summarize,
    }
}
