use std::sync::Arc;

use serde_json::Value;
use thiserror::Error;

use crate::ProviderId;

/// Stores and retrieves API credentials keyed by provider and optional label.
///
/// The production implementation (`goat-credentials::JsonFileStore`) writes to
/// `~/.goat/credentials.json` with `0o600` permissions and uses atomic
/// `NamedTempFile` + `persist` writes. This trait exists so tests and future
/// secret backends can substitute without touching provider logic.
///
/// Secrets must never appear in environment variables or `.env` files; only
/// this store's backing file is an accepted secret location.
pub trait CredentialStore: Send + Sync + 'static {
    /// Lists all credential entries for a provider (without revealing values).
    fn list(&self, provider: ProviderId) -> Vec<CredentialEntry>;

    /// Reads the credential value for a provider, optionally filtered by label.
    /// Returns `None` if no matching entry exists.
    fn read(&self, provider: ProviderId, label: Option<&str>) -> Option<Value>;

    /// Persists a credential. The implementation must write atomically so a
    /// concurrent read never sees a partial file.
    fn write(
        &self,
        provider: ProviderId,
        label: Option<&str>,
        value: Value,
    ) -> Result<(), CredentialError>;

    /// Removes a stored credential. Returns `CredentialError::NotFound` if
    /// the entry did not exist.
    fn remove(&self, provider: ProviderId, label: Option<&str>) -> Result<(), CredentialError>;
}

#[derive(Clone, Debug)]
pub struct CredentialEntry {
    pub label: Option<String>,
    pub raw: Value,
}

#[derive(Debug, Error)]
pub enum CredentialError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("not found")]
    NotFound,
}

pub type SharedCredentialStore = Arc<dyn CredentialStore>;
