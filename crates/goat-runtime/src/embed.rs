use anyhow::{Result, anyhow};
use async_trait::async_trait;
use goat_auth::CredentialStore;
use goat_embedding::{EmbeddingProvider, OpenAiEmbeddingProvider};
use goat_memory::Embedder;

pub struct OpenAiEmbedderAdapter {
    provider: OpenAiEmbeddingProvider,
    model: String,
    dim: usize,
}

impl OpenAiEmbedderAdapter {
    pub async fn new(store: CredentialStore, model: String) -> Result<Self> {
        let provider = OpenAiEmbeddingProvider::new(store);
        let probe = provider
            .embed(&model, "dimension probe")
            .await
            .map_err(|e| anyhow!(e.to_string()))?;
        let dim = probe.len();
        Ok(Self {
            provider,
            model,
            dim,
        })
    }
}

#[async_trait]
impl Embedder for OpenAiEmbedderAdapter {
    fn dim(&self) -> usize {
        self.dim
    }

    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        self.provider
            .embed(&self.model, text)
            .await
            .map_err(|e| anyhow!(e.to_string()))
    }
}
