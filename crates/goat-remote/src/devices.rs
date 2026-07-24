use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, RwLock};

use crate::RemoteError;
use crate::verify::Allowlist;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub label: String,
    pub fingerprint: String,
    pub paired_at: i64,
}

#[derive(Clone)]
pub struct Devices {
    path: PathBuf,
    inner: Arc<RwLock<Vec<Device>>>,
    allowlist: Allowlist,
    changed: Arc<Notify>,
}

impl Devices {
    pub fn load(path: PathBuf) -> Result<Self, RemoteError> {
        let devices = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Vec<Device>>(&bytes)?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(err) => return Err(err.into()),
        };
        let allowlist = Allowlist::default();
        allowlist.replace(devices.iter().map(|d| d.fingerprint.clone()));
        Ok(Self {
            path,
            inner: Arc::new(RwLock::new(devices)),
            allowlist,
            changed: Arc::new(Notify::new()),
        })
    }

    pub fn allowlist(&self) -> Allowlist {
        self.allowlist.clone()
    }

    pub fn changed(&self) -> Arc<Notify> {
        self.changed.clone()
    }

    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.is_empty()
    }

    pub async fn list(&self) -> Vec<Device> {
        self.inner.read().await.clone()
    }

    pub async fn contains_fingerprint(&self, fingerprint: &str) -> bool {
        self.inner
            .read()
            .await
            .iter()
            .any(|d| d.fingerprint == fingerprint)
    }

    pub async fn find_by_fingerprint(&self, fingerprint: &str) -> Option<Device> {
        self.inner
            .read()
            .await
            .iter()
            .find(|d| d.fingerprint == fingerprint)
            .cloned()
    }

    pub async fn enroll(&self, device: Device) -> Result<(), RemoteError> {
        let mut guard = self.inner.write().await;
        guard.retain(|d| d.id != device.id && d.fingerprint != device.fingerprint);
        guard.push(device);
        persist(&self.path, &guard)?;
        self.allowlist
            .replace(guard.iter().map(|d| d.fingerprint.clone()));
        drop(guard);
        self.changed.notify_waiters();
        Ok(())
    }

    pub async fn revoke(&self, id: &str) -> Result<bool, RemoteError> {
        let mut guard = self.inner.write().await;
        let before = guard.len();
        guard.retain(|d| d.id != id);
        if guard.len() == before {
            return Ok(false);
        }
        persist(&self.path, &guard)?;
        self.allowlist
            .replace(guard.iter().map(|d| d.fingerprint.clone()));
        drop(guard);
        self.changed.notify_waiters();
        Ok(true)
    }
}

fn persist(path: &Path, devices: &[Device]) -> Result<(), RemoteError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(devices)?;
    std::fs::write(path, bytes)?;
    Ok(())
}
