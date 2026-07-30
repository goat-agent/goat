use std::sync::Arc;

use async_trait::async_trait;
use goat_agent_tool::{ToolHandler, ToolSpec};
use goat_types::AgentId;
use serde::de::DeserializeOwned;

#[async_trait]
pub trait Plugin: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    async fn run(&self, ctx: Ctx) -> anyhow::Result<()>;
}

pub trait ToolSink: Send + Sync + 'static {
    fn expose(&self, agent: AgentId, spec: ToolSpec, handler: Arc<dyn ToolHandler>);
}

#[derive(Clone)]
pub struct Ctx {
    inner: Arc<CtxInner>,
}

struct CtxInner {
    agent: AgentId,
    config: serde_json::Value,
    sink: Arc<dyn ToolSink>,
}

impl Ctx {
    pub fn new(agent: AgentId, config: serde_json::Value, sink: Arc<dyn ToolSink>) -> Self {
        Self {
            inner: Arc::new(CtxInner {
                agent,
                config,
                sink,
            }),
        }
    }

    pub fn agent(&self) -> AgentId {
        self.inner.agent
    }

    pub fn config<T: DeserializeOwned>(&self) -> anyhow::Result<T> {
        Ok(serde_json::from_value(self.inner.config.clone())?)
    }

    pub fn expose(&self, spec: ToolSpec, handler: Arc<dyn ToolHandler>) {
        self.inner.sink.expose(self.inner.agent, spec, handler);
    }
}

pub struct PluginFactory {
    pub name: &'static str,
    pub ctor: fn() -> Arc<dyn Plugin>,
}

inventory::collect!(PluginFactory);
