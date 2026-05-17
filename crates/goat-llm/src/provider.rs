use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use serde_json::Value;

use crate::credentials::CredentialStore;
use crate::setup::Setup;
use crate::{LlmChunk, LlmError, LlmRequest};

pub type LlmStream = Pin<Box<dyn Stream<Item = Result<LlmChunk, LlmError>> + Send>>;
pub type ProbeFuture = Pin<Box<dyn Future<Output = std::result::Result<(), String>> + Send>>;
pub type ProbeFn = fn(Value) -> ProbeFuture;
pub type BuildFn = fn(Arc<dyn CredentialStore>) -> Arc<dyn LlmProvider>;
pub type SummarizeFn = fn(&Value) -> String;

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

pub struct LlmProviderSpec {
    pub id: ProviderId,
    pub build: BuildFn,
    pub probe: Option<ProbeFn>,
    pub setup: &'static dyn Setup,
    pub summarize: SummarizeFn,
}

inventory::collect!(LlmProviderSpec);
