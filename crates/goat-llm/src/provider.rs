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

/// Core abstraction for a language model provider.
///
/// Implement this trait in a dedicated `goat-llm-<provider>` crate. All
/// streaming, auth, and provider-specific error mapping must stay inside
/// that crate — do not add shared "quirks" flags to this trait.
///
/// Register your implementation with the inventory macro so the runtime
/// discovers it at link time:
/// ```ignore
/// inventory::submit!(LlmProviderSpec { id: MY_PROVIDER_ID, build: ..., ... });
/// ```
///
/// The runtime wires registered providers into a [`ProviderRegistry`] keyed by
/// [`ProviderId`]; the brain routes each LLM request to the provider whose `id()`
/// matches the model's declared provider.
#[async_trait]
pub trait LlmProvider: Send + Sync + 'static {
    /// Returns the provider identifier. Must match the `ProviderId` constant
    /// declared in your `LlmProviderSpec` so the registry can route to it.
    fn id(&self) -> ProviderId;

    /// Opens a streaming LLM call. The returned stream yields [`LlmChunk`]
    /// items until the model stops or an error occurs. Provider crates must
    /// handle retries and rate-limit mapping internally before surfacing an
    /// [`LlmError`] to callers.
    async fn stream(&self, req: LlmRequest) -> Result<LlmStream, LlmError>;
}

/// Identifies an LLM provider. Uses a `Cow<'static, str>` internally so that
/// compiled-in extension crates pay zero allocation cost.
///
/// # Choosing the right constructor
///
/// | Situation | Use |
/// |-----------|-----|
/// | `pub const ID` in a provider crate | [`ProviderId::from_static`] |
/// | Value deserialized from config/DB at runtime | [`ProviderId::new`] |
///
/// Provider crates **must** declare their identifier as a `pub const` using
/// `from_static` so the runtime can compare IDs without allocating:
/// ```ignore
/// pub const ID: ProviderId = ProviderId::from_static("my-provider");
/// ```
#[derive(Clone, Eq, PartialEq, Hash, Debug)]
pub struct ProviderId(Cow<'static, str>);

impl ProviderId {
    /// Creates a zero-allocation provider ID from a `'static` string literal.
    /// Use this for `pub const` declarations in provider crates.
    pub const fn from_static(s: &'static str) -> Self {
        Self(Cow::Borrowed(s))
    }

    /// Creates an owned provider ID from a runtime-allocated string.
    /// Use this when deserializing from config or DB values.
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
