use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;

use crate::{KeyProvider, LlmChunk, LlmError, LlmRequest};

pub type LlmStream = Pin<Box<dyn Stream<Item = Result<LlmChunk, LlmError>> + Send>>;
pub type ProbeFuture = Pin<Box<dyn Future<Output = std::result::Result<(), String>> + Send>>;
pub type ProviderProbe = fn(String) -> ProbeFuture;

#[async_trait]
pub trait LlmProvider: Send + Sync + 'static {
    fn id(&self) -> ProviderId;
    async fn stream(&self, req: LlmRequest) -> Result<LlmStream, LlmError>;
}

#[derive(Clone, Eq, PartialEq, Hash, Debug)]
pub struct ProviderId(Cow<'static, str>);

impl ProviderId {
    pub const fn from_static(s: &'static str) -> Self {
        Self(Cow::Borrowed(s))
    }

    pub fn new(s: impl Into<String>) -> Self {
        Self(Cow::Owned(s.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ProviderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

pub struct LlmProviderFactory {
    pub id: ProviderId,
    pub ctor: fn(Arc<dyn KeyProvider>) -> Arc<dyn LlmProvider>,
    pub probe: Option<ProviderProbe>,
}

inventory::collect!(LlmProviderFactory);
