use async_trait::async_trait;

#[async_trait]
pub trait Embedder: Send + Sync + 'static {
    fn dim(&self) -> usize;
    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>>;
}

pub struct DummyEmbedder {
    dim: usize,
}

impl DummyEmbedder {
    pub fn new(dim: usize) -> Self {
        Self { dim }
    }
}

impl Default for DummyEmbedder {
    fn default() -> Self {
        Self { dim: 384 }
    }
}

#[async_trait]
impl Embedder for DummyEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    async fn embed(&self, _text: &str) -> anyhow::Result<Vec<f32>> {
        Ok(vec![0.0; self.dim])
    }
}
