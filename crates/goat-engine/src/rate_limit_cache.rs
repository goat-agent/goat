use std::{collections::HashMap, fs, path::Path};

use goat_provider::RateWindow;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedEntry {
    pub windows: Vec<RateWindow>,
    pub cached_at: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RateLimitCache(HashMap<String, PersistedEntry>);

impl RateLimitCache {
    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn to_json(&self) -> Option<String> {
        serde_json::to_string_pretty(self).ok()
    }

    pub fn upsert(
        &mut self,
        provider: &str,
        account: &str,
        windows: Vec<RateWindow>,
        cached_at: i64,
    ) {
        self.0.insert(
            format!("{provider}:{account}"),
            PersistedEntry { windows, cached_at },
        );
    }

    pub fn entries(&self) -> impl Iterator<Item = (&str, &str, &PersistedEntry)> {
        self.0.iter().filter_map(|(key, entry)| {
            let (provider, account) = key.split_once(':')?;
            Some((provider, account, entry))
        })
    }
}

pub fn write(path: &Path, json: &str) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Err(err) = fs::write(path, json) {
        tracing::warn!(%err, "failed to save rate limit cache");
    }
}
