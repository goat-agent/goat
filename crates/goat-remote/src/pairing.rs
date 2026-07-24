use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, Notify};

const CODE_BYTES: usize = 16;
const DEFAULT_TTL: Duration = Duration::from_mins(3);

#[derive(Clone)]
pub struct Pairing {
    inner: Arc<Mutex<HashMap<String, Pending>>>,
    ttl: Duration,
    changed: Arc<Notify>,
}

struct Pending {
    label: String,
    expires_at: Instant,
}

pub struct Claim {
    pub label: String,
}

impl Default for Pairing {
    fn default() -> Self {
        Self::new(DEFAULT_TTL)
    }
}

impl Pairing {
    pub fn new(ttl: Duration) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            ttl,
            changed: Arc::new(Notify::new()),
        }
    }

    pub fn changed(&self) -> Arc<Notify> {
        self.changed.clone()
    }

    pub async fn has_pending(&self) -> bool {
        let mut guard = self.inner.lock().await;
        Self::sweep(&mut guard);
        !guard.is_empty()
    }

    pub async fn mint(&self, label: String) -> String {
        let bytes: [u8; CODE_BYTES] = rand::random();
        let code = encode(&bytes);
        let mut guard = self.inner.lock().await;
        Self::sweep(&mut guard);
        guard.insert(
            code.clone(),
            Pending {
                label,
                expires_at: Instant::now() + self.ttl,
            },
        );
        drop(guard);
        self.changed.notify_waiters();
        code
    }

    pub async fn claim(&self, code: &str) -> Option<Claim> {
        let mut guard = self.inner.lock().await;
        Self::sweep(&mut guard);
        let pending = guard.remove(code)?;
        if pending.expires_at <= Instant::now() {
            return None;
        }
        Some(Claim {
            label: pending.label,
        })
    }

    fn sweep(map: &mut HashMap<String, Pending>) {
        let now = Instant::now();
        map.retain(|_, p| p.expires_at > now);
    }
}

fn encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}
