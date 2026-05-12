use std::sync::Arc;

use async_trait::async_trait;
use goat_types::PersonaId;
use serde::de::DeserializeOwned;

#[async_trait]
pub trait Plugin: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    async fn run(&self, ctx: Ctx) -> anyhow::Result<()>;
}

#[derive(Clone, Debug)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[async_trait]
pub trait ToolHandler: Send + Sync + 'static {
    async fn call(&self, args: serde_json::Value) -> anyhow::Result<serde_json::Value>;
}

#[derive(Clone)]
pub struct Ctx {
    inner: Arc<CtxInner>,
}

struct CtxInner {
    persona: PersonaId,
    config: serde_json::Value,
    registry: Arc<dyn ToolRegistry>,
}

pub trait ToolRegistry: Send + Sync + 'static {
    fn register(&self, persona: PersonaId, spec: ToolSpec, handler: Arc<dyn ToolHandler>);
}

impl Ctx {
    pub fn new(
        persona: PersonaId,
        config: serde_json::Value,
        registry: Arc<dyn ToolRegistry>,
    ) -> Self {
        Self {
            inner: Arc::new(CtxInner {
                persona,
                config,
                registry,
            }),
        }
    }

    pub fn persona(&self) -> PersonaId {
        self.inner.persona
    }

    pub fn config<T: DeserializeOwned>(&self) -> anyhow::Result<T> {
        Ok(serde_json::from_value(self.inner.config.clone())?)
    }

    pub fn expose(&self, spec: ToolSpec, handler: Arc<dyn ToolHandler>) {
        self.inner
            .registry
            .register(self.inner.persona, spec, handler);
    }

    pub fn spawn<F>(&self, fut: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        tokio::spawn(fut);
    }
}

pub struct PluginFactory {
    pub name: &'static str,
    pub ctor: fn() -> Arc<dyn Plugin>,
}

inventory::collect!(PluginFactory);
