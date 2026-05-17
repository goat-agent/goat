use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use goat_llm::{CredentialEntry, CredentialError, CredentialStore, ProviderId};
use serde_json::Value;

pub struct JsonFileStore {
    path: PathBuf,
    write_lock: Mutex<()>,
}

impl JsonFileStore {
    pub fn open(path: PathBuf) -> Result<Self, CredentialError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(Self {
            path,
            write_lock: Mutex::new(()),
        })
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    fn read_all(&self) -> Result<BTreeMap<String, Vec<Value>>, CredentialError> {
        if !self.path.exists() {
            return Ok(BTreeMap::new());
        }
        let raw = fs::read_to_string(&self.path)?;
        if raw.trim().is_empty() {
            return Ok(BTreeMap::new());
        }
        Ok(serde_json::from_str(&raw)?)
    }

    fn write_all(&self, map: &BTreeMap<String, Vec<Value>>) -> Result<(), CredentialError> {
        let raw = serde_json::to_string_pretty(map)?;
        let parent = self.path.parent().ok_or_else(|| {
            CredentialError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "credentials path has no parent",
            ))
        })?;
        fs::create_dir_all(parent)?;
        let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
        tmp.write_all(raw.as_bytes())?;
        tmp.flush()?;
        tmp.persist(&self.path)
            .map_err(|e| CredentialError::Io(e.error))?;
        Ok(())
    }
}

fn entry_label(v: &Value) -> Option<String> {
    v.get("label").and_then(|l| match l {
        Value::String(s) => Some(s.clone()),
        _ => None,
    })
}

fn matches(v: &Value, label: Option<&str>) -> bool {
    let entry_l = entry_label(v);
    match (label, entry_l) {
        (Some(want), Some(got)) => want == got.as_str(),
        (None, None) => true,
        _ => false,
    }
}

fn inject_label(mut value: Value, label: Option<&str>) -> Value {
    if let Value::Object(map) = &mut value {
        match label {
            Some(l) => {
                map.insert("label".to_string(), Value::String(l.to_string()));
            }
            None => {
                map.insert("label".to_string(), Value::Null);
            }
        }
    }
    value
}

impl CredentialStore for JsonFileStore {
    fn list(&self, provider: ProviderId) -> Vec<CredentialEntry> {
        let Ok(map) = self.read_all() else {
            return Vec::new();
        };
        let Some(entries) = map.get(provider.as_str()) else {
            return Vec::new();
        };
        entries
            .iter()
            .cloned()
            .map(|raw| CredentialEntry {
                label: entry_label(&raw),
                raw,
            })
            .collect()
    }

    fn read(&self, provider: ProviderId, label: Option<&str>) -> Option<Value> {
        let map = self.read_all().ok()?;
        let entries = map.get(provider.as_str())?;
        entries.iter().find(|v| matches(v, label)).cloned()
    }

    fn write(
        &self,
        provider: ProviderId,
        label: Option<&str>,
        value: Value,
    ) -> Result<(), CredentialError> {
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut map = self.read_all()?;
        let value = inject_label(value, label);
        let entries = map.entry(provider.as_str().to_string()).or_default();
        if let Some(slot) = entries.iter_mut().find(|v| matches(v, label)) {
            *slot = value;
        } else {
            entries.push(value);
        }
        self.write_all(&map)
    }

    fn remove(&self, provider: ProviderId, label: Option<&str>) -> Result<(), CredentialError> {
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut map = self.read_all()?;
        let Some(entries) = map.get_mut(provider.as_str()) else {
            return Err(CredentialError::NotFound);
        };
        let before = entries.len();
        entries.retain(|v| !matches(v, label));
        if entries.len() == before {
            return Err(CredentialError::NotFound);
        }
        if entries.is_empty() {
            map.remove(provider.as_str());
        }
        self.write_all(&map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    const OPENAI: ProviderId = ProviderId::from_static("openai");
    const CODEX: ProviderId = ProviderId::from_static("codex");

    fn setup() -> (tempfile::TempDir, JsonFileStore) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        let store = JsonFileStore::open(path).unwrap();
        (dir, store)
    }

    #[test]
    fn write_read_roundtrip() {
        let (_d, store) = setup();
        store
            .write(OPENAI, None, json!({ "api_key": "sk-1" }))
            .unwrap();
        let got = store.read(OPENAI, None).unwrap();
        assert_eq!(got["api_key"], "sk-1");
        assert!(got["label"].is_null());
    }

    #[test]
    fn label_match_four_cases() {
        let (_d, store) = setup();
        store
            .write(OPENAI, None, json!({ "api_key": "k0" }))
            .unwrap();
        store
            .write(OPENAI, Some("work"), json!({ "api_key": "k1" }))
            .unwrap();

        // Some matches
        assert_eq!(store.read(OPENAI, Some("work")).unwrap()["api_key"], "k1");
        // Some no match
        assert!(store.read(OPENAI, Some("missing")).is_none());
        // None matches null-label
        assert_eq!(store.read(OPENAI, None).unwrap()["api_key"], "k0");
        // None no match (provider absent)
        assert!(store
            .read(ProviderId::from_static("gemini"), None)
            .is_none());
    }

    #[test]
    fn list_returns_all_provider_entries() {
        let (_d, store) = setup();
        store
            .write(OPENAI, None, json!({ "api_key": "k0" }))
            .unwrap();
        store
            .write(OPENAI, Some("work"), json!({ "api_key": "k1" }))
            .unwrap();
        let entries = store.list(OPENAI);
        assert_eq!(entries.len(), 2);
        let labels: Vec<_> = entries.iter().map(|e| e.label.clone()).collect();
        assert!(labels.contains(&None));
        assert!(labels.contains(&Some("work".to_string())));
    }

    #[test]
    fn write_replaces_same_label() {
        let (_d, store) = setup();
        store
            .write(OPENAI, Some("work"), json!({ "api_key": "old" }))
            .unwrap();
        store
            .write(OPENAI, Some("work"), json!({ "api_key": "new" }))
            .unwrap();
        let entries = store.list(OPENAI);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].raw["api_key"], "new");
    }

    #[test]
    fn remove_then_not_found() {
        let (_d, store) = setup();
        store
            .write(OPENAI, None, json!({ "api_key": "k0" }))
            .unwrap();
        store.remove(OPENAI, None).unwrap();
        assert!(store.read(OPENAI, None).is_none());
        assert!(matches!(
            store.remove(OPENAI, None),
            Err(CredentialError::NotFound)
        ));
    }

    #[test]
    fn remove_drops_empty_provider() {
        let (_d, store) = setup();
        store
            .write(OPENAI, None, json!({ "api_key": "k0" }))
            .unwrap();
        store.remove(OPENAI, None).unwrap();
        assert!(store.list(OPENAI).is_empty());
    }

    #[test]
    fn multi_provider_isolated() {
        let (_d, store) = setup();
        store
            .write(OPENAI, None, json!({ "api_key": "sk-o" }))
            .unwrap();
        store
            .write(CODEX, None, json!({ "access_token": "tk" }))
            .unwrap();
        assert_eq!(store.read(OPENAI, None).unwrap()["api_key"], "sk-o");
        assert_eq!(store.read(CODEX, None).unwrap()["access_token"], "tk");
    }

    #[test]
    fn legacy_schema_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        std::fs::write(
            &path,
            r#"{"openai":[{"api_key":"sk-legacy","label":"work"}]}"#,
        )
        .unwrap();
        let store = JsonFileStore::open(path).unwrap();
        let got = store.read(OPENAI, Some("work")).unwrap();
        assert_eq!(got["api_key"], "sk-legacy");
    }

    #[test]
    fn concurrent_writes_serialize() {
        use std::thread;
        let (_d, store) = setup();
        let store = Arc::new(store);
        let mut handles = Vec::new();
        for i in 0..10 {
            let s = store.clone();
            handles.push(thread::spawn(move || {
                s.write(
                    OPENAI,
                    Some(&format!("k{i}")),
                    serde_json::json!({ "api_key": format!("sk-{i}") }),
                )
                .unwrap();
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let entries = store.list(OPENAI);
        assert_eq!(entries.len(), 10);
    }
}
