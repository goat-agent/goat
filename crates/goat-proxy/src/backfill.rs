use std::collections::HashMap;
use std::path::Path;

use goat_provider::{RateLimitSnapshot, RateWindow};
use goat_store::ProxyStore;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct PersistedEntry {
    windows: Vec<RateWindow>,
    cached_at: i64,
}

pub async fn backfill_rate_limits(store: &ProxyStore, path: &Path) -> usize {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return 0;
    };
    let cache: HashMap<String, PersistedEntry> = serde_json::from_str(&raw).unwrap_or_default();
    let mut applied = 0;
    for (key, entry) in cache {
        let Some((provider, account)) = key.split_once(':') else {
            continue;
        };
        let snapshot = RateLimitSnapshot {
            windows: entry.windows,
            representative: None,
        };
        let Ok(json) = serde_json::to_string(&snapshot) else {
            continue;
        };
        match store
            .upsert_rate_limits_if_newer(provider, account, &json, entry.cached_at)
            .await
        {
            Ok(true) => applied += 1,
            Ok(false) => {}
            Err(err) => tracing::warn!(%err, "proxy rate limit backfill failed"),
        }
    }
    applied
}

#[cfg(test)]
mod tests {
    use super::backfill_rate_limits;

    fn write_cache(dir: &std::path::Path, json: &str) -> std::path::PathBuf {
        let path = dir.join("rate_limits.json");
        std::fs::write(&path, json).unwrap();
        path
    }

    #[tokio::test]
    async fn backfill_imports_cached_snapshots_once() {
        let store = goat_store::ProxyStore::open_in_memory().await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = write_cache(
            dir.path(),
            r#"{
                "anthropic:default": {
                    "windows": [{"label": "5h", "used_percent": 42.0, "resets_at": 9999}],
                    "cached_at": 1000
                },
                "openai-codex:personal": {
                    "windows": [{"label": "weekly", "used_percent": 10.0, "resets_at": null}],
                    "cached_at": 2000
                }
            }"#,
        );

        let applied = backfill_rate_limits(&store, &path).await;
        assert_eq!(applied, 2);

        let rows = store.latest_rate_limits().await.unwrap();
        assert_eq!(rows.len(), 2);
        let anthropic = rows.iter().find(|r| r.provider == "anthropic").unwrap();
        assert_eq!(anthropic.account, "default");
        assert_eq!(anthropic.updated_at, 1000);
        let snapshot: goat_provider::RateLimitSnapshot =
            serde_json::from_str(&anthropic.snapshot).unwrap();
        assert_eq!(snapshot.windows[0].label, "5h");
        assert!((snapshot.windows[0].used_percent - 42.0).abs() < f32::EPSILON);

        let path = write_cache(
            dir.path(),
            r#"{
                "anthropic:default": {
                    "windows": [{"label": "5h", "used_percent": 99.0, "resets_at": 9999}],
                    "cached_at": 500
                }
            }"#,
        );
        let applied = backfill_rate_limits(&store, &path).await;
        assert_eq!(applied, 0);
        let rows = store.latest_rate_limits().await.unwrap();
        let anthropic = rows.iter().find(|r| r.provider == "anthropic").unwrap();
        assert_eq!(anthropic.updated_at, 1000);
    }

    #[tokio::test]
    async fn backfill_tolerates_missing_and_malformed_files() {
        let store = goat_store::ProxyStore::open_in_memory().await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.json");
        assert_eq!(backfill_rate_limits(&store, &missing).await, 0);

        let path = write_cache(dir.path(), "not json at all");
        assert_eq!(backfill_rate_limits(&store, &path).await, 0);
        assert!(store.latest_rate_limits().await.unwrap().is_empty());
    }
}
