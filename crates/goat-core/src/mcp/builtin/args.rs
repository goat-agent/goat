use serde_json::{Map, Value};

pub(super) struct Args(pub(super) Map<String, Value>);

impl Args {
    pub(super) fn word(&self, key: &str) -> Option<&str> {
        self.0
            .get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
    }

    pub(super) fn text(&self, key: &str) -> Result<String, String> {
        self.word(key)
            .map(str::to_owned)
            .ok_or_else(|| format!("{key} is required"))
    }

    pub(super) fn list(&self, key: &str) -> Vec<String> {
        self.0
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    }

    pub(super) fn flag(&self, key: &str) -> bool {
        self.0.get(key).and_then(Value::as_bool) == Some(true)
    }

    pub(super) fn has(&self, key: &str) -> bool {
        self.0.get(key).is_some_and(|value| !value.is_null())
    }

    pub(super) fn value(&self, key: &str) -> Option<&Value> {
        self.0.get(key).filter(|value| !value.is_null())
    }

    pub(super) fn objects(&self, key: &str) -> impl Iterator<Item = &Value> {
        self.0
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|item| item.is_object())
    }

    pub(super) fn page(&self, key: &str, default: u64, most: u64) -> u64 {
        self.0
            .get(key)
            .and_then(Value::as_u64)
            .filter(|size| *size > 0)
            .unwrap_or(default)
            .min(most)
    }
}
