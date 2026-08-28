use async_trait::async_trait;

#[async_trait]
pub trait Embedder: Send + Sync + 'static {
    fn identity(&self) -> &str;
    fn dim(&self) -> usize;
    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>>;
}
