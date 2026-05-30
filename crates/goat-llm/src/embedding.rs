use std::sync::Arc;

use async_trait::async_trait;

use crate::credentials::CredentialStore;
use crate::{LlmError, ProviderId};

/// Text embedding capability, parallel to [`crate::LlmProvider`] but kept
/// separate: chat streaming and embeddings are distinct endpoints and not
/// every provider offers both (Anthropic, for example, has no embeddings
/// API). Only provider crates that implement embeddings register an
/// [`EmbeddingProviderSpec`].
#[async_trait]
pub trait EmbeddingProvider: Send + Sync + 'static {
    fn id(&self) -> ProviderId;

    /// Embed a single text into a dense vector using `model`.
    async fn embed(&self, model: &str, text: &str) -> Result<Vec<f32>, LlmError>;
}

pub type BuildEmbeddingFn = fn(Arc<dyn CredentialStore>) -> Arc<dyn EmbeddingProvider>;

/// Inventory entry mirroring [`crate::LlmProviderSpec`]. Discovered at boot by
/// the runtime to assemble the set of usable embedding providers.
pub struct EmbeddingProviderSpec {
    pub id: ProviderId,
    pub build: BuildEmbeddingFn,
}

inventory::collect!(EmbeddingProviderSpec);
