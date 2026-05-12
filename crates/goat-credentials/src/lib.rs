use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use goat_llm::{KeyProvider, PooledKey, ProviderId};
use serde::Deserialize;
use thiserror::Error;
use tracing::warn;

#[derive(Debug, Error)]
pub enum CredentialsError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Deserialize)]
pub struct CredentialEntry {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
}

#[derive(Debug, Default)]
pub struct Credentials {
    pub llm: HashMap<ProviderId, Vec<CredentialEntry>>,
}

impl Credentials {
    pub fn load(path: &Path) -> Result<Self, CredentialsError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)?;
        let map: HashMap<String, Vec<serde_json::Value>> = serde_json::from_str(&raw)?;
        let mut llm = HashMap::new();
        for (name, entries) in map {
            let provider = ProviderId::new(name);
            let parsed: Vec<CredentialEntry> = entries
                .into_iter()
                .filter_map(|v| serde_json::from_value(v).ok())
                .collect();
            llm.insert(provider, parsed);
        }
        Ok(Self { llm })
    }
}

pub struct KeyPool {
    state: Mutex<HashMap<ProviderId, ProviderState>>,
}

struct ProviderState {
    keys: Vec<KeyState>,
    next: usize,
}

#[derive(Debug, Clone)]
struct KeyState {
    api_key: String,
    label: Option<String>,
    cooldown_until: Option<Instant>,
}

impl KeyPool {
    pub fn from_credentials(creds: &Credentials) -> Self {
        let mut state: HashMap<ProviderId, ProviderState> = HashMap::new();
        for (provider, entries) in &creds.llm {
            let keys: Vec<KeyState> = entries
                .iter()
                .filter_map(|e| {
                    e.api_key.clone().map(|api_key| KeyState {
                        api_key,
                        label: e.label.clone(),
                        cooldown_until: None,
                    })
                })
                .collect();
            if !keys.is_empty() {
                state.insert(provider.clone(), ProviderState { keys, next: 0 });
            }
        }
        Self {
            state: Mutex::new(state),
        }
    }

    pub fn empty() -> Self {
        Self {
            state: Mutex::new(HashMap::new()),
        }
    }

    pub fn next(&self, provider: ProviderId) -> Option<PooledKey> {
        let mut guard = self.state.lock().ok()?;
        let prov = guard.get_mut(&provider)?;
        if prov.keys.is_empty() {
            return None;
        }
        let now = Instant::now();
        let n = prov.keys.len();
        for _ in 0..n {
            let idx = prov.next % n;
            prov.next = (prov.next + 1) % n;
            let k = &prov.keys[idx];
            if k.cooldown_until.map(|until| until <= now).unwrap_or(true) {
                return Some(PooledKey {
                    api_key: k.api_key.clone(),
                    label: k.label.clone(),
                });
            }
        }
        None
    }

    pub fn report_429(&self, provider: ProviderId, api_key: &str, retry_after: Option<Duration>) {
        let Ok(mut guard) = self.state.lock() else {
            return;
        };
        let Some(prov) = guard.get_mut(&provider) else {
            return;
        };
        let cooldown = retry_after.unwrap_or(Duration::from_secs(30));
        let until = Instant::now() + cooldown;
        for k in prov.keys.iter_mut() {
            if k.api_key == api_key {
                k.cooldown_until = Some(until);
                warn!(
                    provider = %provider,
                    label = ?k.label,
                    secs = cooldown.as_secs(),
                    "key cooled down after 429",
                );
                return;
            }
        }
    }
}

impl KeyProvider for KeyPool {
    fn next(&self, provider: ProviderId) -> Option<PooledKey> {
        KeyPool::next(self, provider)
    }

    fn report_429(&self, provider: ProviderId, api_key: &str, retry_after: Option<Duration>) {
        KeyPool::report_429(self, provider, api_key, retry_after);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool_of(provider: ProviderId, keys: &[&str]) -> KeyPool {
        let creds = Credentials {
            llm: HashMap::from([(
                provider,
                keys.iter()
                    .map(|k| CredentialEntry {
                        label: None,
                        api_key: Some((*k).to_string()),
                    })
                    .collect(),
            )]),
        };
        KeyPool::from_credentials(&creds)
    }

    const OPENAI: ProviderId = ProviderId::from_static("openai");

    #[test]
    fn round_robin_visits_each_key() {
        let pool = pool_of(OPENAI, &["k1", "k2", "k3"]);
        let a = pool.next(OPENAI).unwrap().api_key;
        let b = pool.next(OPENAI).unwrap().api_key;
        let c = pool.next(OPENAI).unwrap().api_key;
        let d = pool.next(OPENAI).unwrap().api_key;
        assert_eq!(a, "k1");
        assert_eq!(b, "k2");
        assert_eq!(c, "k3");
        assert_eq!(d, "k1");
    }

    #[test]
    fn cooldown_skips_key() {
        let pool = pool_of(OPENAI, &["k1", "k2"]);
        pool.report_429(OPENAI, "k1", Some(Duration::from_secs(60)));
        let a = pool.next(OPENAI).unwrap().api_key;
        let b = pool.next(OPENAI).unwrap().api_key;
        assert_eq!(a, "k2");
        assert_eq!(b, "k2");
    }

    #[test]
    fn empty_pool_returns_none() {
        let pool = KeyPool::empty();
        assert!(pool.next(OPENAI).is_none());
    }

    #[test]
    fn parses_credentials_json() {
        let json = r#"{
            "openai": [
                { "api_key": "sk-1" },
                { "api_key": "sk-2", "label": "work" }
            ],
            "google": [
                { "refresh_token": "rt", "client_id": "ci", "client_secret": "cs" }
            ]
        }"#;
        let map: HashMap<String, Vec<CredentialEntry>> = serde_json::from_str(json).unwrap();
        assert_eq!(map["openai"].len(), 2);
        assert_eq!(map["openai"][1].api_key.as_deref(), Some("sk-2"));
        assert_eq!(map["openai"][1].label.as_deref(), Some("work"));
    }
}
