use std::sync::Arc;

use serde_json::Value;
use thiserror::Error;

use crate::ProviderId;

pub trait CredentialStore: Send + Sync + 'static {
    fn list(&self, provider: ProviderId) -> Vec<CredentialEntry>;
    fn read(&self, provider: ProviderId, label: Option<&str>) -> Option<Value>;
    fn write(
        &self,
        provider: ProviderId,
        label: Option<&str>,
        value: Value,
    ) -> Result<(), CredentialError>;
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
