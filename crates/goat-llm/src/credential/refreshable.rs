use std::marker::PhantomData;
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use serde::{de::DeserializeOwned, Serialize};
use thiserror::Error;
use tokio::sync::Mutex;

use crate::credentials::{CredentialError, CredentialStore};
use crate::ProviderId;

#[async_trait]
pub trait RefreshableCredential:
    Serialize + DeserializeOwned + Clone + Send + Sync + 'static
{
    fn label(&self) -> Option<&str>;
    fn needs_refresh(&self, now: SystemTime) -> bool;
    async fn refresh(&self) -> Result<Self, RefreshError>;
}

#[derive(Debug, Error)]
pub enum RefreshError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("not found")]
    NotFound,
    #[error("transport: {0}")]
    Transport(String),
    #[error("auth: {0}")]
    Auth(String),
    #[error("{0}")]
    Other(String),
}

impl From<CredentialError> for RefreshError {
    fn from(e: CredentialError) -> Self {
        match e {
            CredentialError::Io(e) => RefreshError::Io(e),
            CredentialError::Json(e) => RefreshError::Json(e),
            CredentialError::NotFound => RefreshError::NotFound,
        }
    }
}

pub struct Refreshable<C> {
    store: Arc<dyn CredentialStore>,
    provider: ProviderId,
    label: Option<String>,
    lock: Mutex<()>,
    _marker: PhantomData<C>,
}

impl<C: RefreshableCredential> Refreshable<C> {
    pub fn new(
        store: Arc<dyn CredentialStore>,
        provider: ProviderId,
        label: Option<String>,
    ) -> Self {
        Self {
            store,
            provider,
            label,
            lock: Mutex::new(()),
            _marker: PhantomData,
        }
    }

    fn read_typed(&self) -> Result<C, RefreshError> {
        let raw = self
            .store
            .read(self.provider.clone(), self.label.as_deref())
            .ok_or(RefreshError::NotFound)?;
        Ok(serde_json::from_value(raw)?)
    }

    fn persist(&self, cred: &C) -> Result<(), RefreshError> {
        let v = serde_json::to_value(cred)?;
        self.store
            .write(self.provider.clone(), self.label.as_deref(), v)?;
        Ok(())
    }

    pub async fn current(&self) -> Result<C, RefreshError> {
        let cred = self.read_typed()?;
        if !cred.needs_refresh(SystemTime::now()) {
            return Ok(cred);
        }
        let _guard = self.lock.lock().await;
        let cred = self.read_typed()?;
        if !cred.needs_refresh(SystemTime::now()) {
            return Ok(cred);
        }
        let fresh = cred.refresh().await?;
        self.persist(&fresh)?;
        Ok(fresh)
    }

    pub async fn force_refresh(&self) -> Result<C, RefreshError> {
        let _guard = self.lock.lock().await;
        let cred = self.read_typed()?;
        let fresh = cred.refresh().await?;
        self.persist(&fresh)?;
        Ok(fresh)
    }
}
