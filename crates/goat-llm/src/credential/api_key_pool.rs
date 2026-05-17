use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tracing::warn;

use crate::credentials::CredentialStore;
use crate::ProviderId;

#[derive(Clone, Debug)]
pub struct PooledKey {
    pub api_key: String,
    pub label: Option<String>,
}

pub struct ApiKeyPool {
    store: Arc<dyn CredentialStore>,
    provider: ProviderId,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    cursor: usize,
    cooldown: HashMap<String, Instant>,
}

impl ApiKeyPool {
    pub fn new(store: Arc<dyn CredentialStore>, provider: ProviderId) -> Self {
        Self {
            store,
            provider,
            state: Mutex::new(State::default()),
        }
    }

    pub fn next(&self) -> Option<PooledKey> {
        let entries = self.store.list(self.provider.clone());
        let keys: Vec<(String, Option<String>)> = entries
            .into_iter()
            .filter_map(|e| {
                let api_key = e
                    .raw
                    .get("api_key")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)?;
                Some((api_key, e.label))
            })
            .collect();

        if keys.is_empty() {
            return None;
        }

        let mut state = self.state.lock().ok()?;
        let now = Instant::now();
        state.cooldown.retain(|_, until| *until > now);

        let n = keys.len();
        for _ in 0..n {
            let idx = state.cursor % n;
            state.cursor = (state.cursor + 1) % n;
            let (key, label) = &keys[idx];
            if !state.cooldown.contains_key(key) {
                return Some(PooledKey {
                    api_key: key.clone(),
                    label: label.clone(),
                });
            }
        }
        None
    }

    pub fn report_rate_limit(&self, api_key: &str, retry_after: Option<Duration>) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let cooldown = retry_after.unwrap_or(Duration::from_secs(30));
        state
            .cooldown
            .insert(api_key.to_string(), Instant::now() + cooldown);
        warn!(
            provider = %self.provider,
            secs = cooldown.as_secs(),
            "key cooled down after rate limit",
        );
    }
}
