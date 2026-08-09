use serde_json::Value;

use crate::{IntegrationError, IntegrationResult};

pub const ENVELOPE_KEYS: &[&str] = &[
    "issues", "nodes", "results", "items", "data", "records", "entries", "rows",
];

pub const MORE_KEYS: &[&str] = &["hasNextPage", "has_more", "hasMore", "more"];

#[must_use]
pub fn envelope<'a>(data: &'a Value, extra: &[&str]) -> Option<&'a Vec<Value>> {
    if let Some(array) = data.as_array() {
        return Some(array);
    }
    let keys = || extra.iter().chain(ENVELOPE_KEYS);
    if let Some(array) = keys().find_map(|key| data.get(*key).and_then(Value::as_array)) {
        return Some(array);
    }
    keys()
        .filter_map(|key| data.get(*key))
        .find_map(|nested| keys().find_map(|key| nested.get(*key).and_then(Value::as_array)))
}

pub fn items<'a>(
    service: &str,
    data: &'a Value,
    extra: &[&str],
) -> IntegrationResult<&'a Vec<Value>> {
    envelope(data, extra).ok_or_else(|| {
        IntegrationError::Service(format!("{service} response has no item list: {data}"))
    })
}

#[must_use]
pub fn more(data: &Value) -> bool {
    MORE_KEYS
        .iter()
        .find_map(|key| data.get(*key).and_then(Value::as_bool))
        .unwrap_or(false)
}

#[must_use]
pub fn text(node: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| pluck(node, key))
        .unwrap_or_default()
}

pub fn required(service: &str, node: &Value, keys: &[&str]) -> IntegrationResult<String> {
    keys.iter().find_map(|key| pluck(node, key)).ok_or_else(|| {
        IntegrationError::Service(format!(
            "{service} item is missing every one of `{}`: {node}",
            keys.join("`, `")
        ))
    })
}

#[must_use]
pub fn squeeze(raw: &str, limit: usize) -> String {
    let flat = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= limit {
        return flat;
    }
    let clipped: String = flat.chars().take(limit).collect();
    format!("{clipped}…")
}

fn pluck(node: &Value, key: &str) -> Option<String> {
    let found = key
        .split('.')
        .try_fold(node, |current, part| current.get(part))?;
    match found {
        Value::String(text) if text.trim().is_empty() => None,
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_bare_array_is_its_own_envelope() {
        let bare = json!([{ "id": "1" }]);
        assert_eq!(envelope(&bare, &[]).unwrap().len(), 1);
    }

    #[test]
    fn every_known_wrapper_key_is_unwrapped() {
        for key in ENVELOPE_KEYS {
            let wrapped = json!({ *key: [{ "id": "1" }] });
            assert_eq!(envelope(&wrapped, &[]).unwrap().len(), 1, "{key}");
        }
    }

    #[test]
    fn a_service_specific_key_is_tried_before_the_shared_ones() {
        let wrapped = json!({ "deployments": [{ "id": "1" }], "items": [] });
        assert_eq!(envelope(&wrapped, &["deployments"]).unwrap().len(), 1);
    }

    #[test]
    fn a_missing_list_names_the_service() {
        let err = items("vercel", &json!({ "ok": true }), &[]).unwrap_err();
        assert!(err.to_string().contains("vercel"));
    }

    #[test]
    fn truncation_reads_whichever_flag_the_server_sends() {
        assert!(more(&json!({ "hasNextPage": true })));
        assert!(more(&json!({ "has_more": true })));
        assert!(!more(&json!({ "hasNextPage": false })));
        assert!(!more(&json!([{ "id": "1" }])));
    }

    #[test]
    fn text_takes_the_first_present_key_and_coerces_scalars() {
        let node = json!({ "count": 12, "title": "  ", "name": "fallback" });
        assert_eq!(text(&node, &["title", "name"]), "fallback");
        assert_eq!(text(&node, &["count"]), "12");
        assert_eq!(text(&node, &["absent"]), "");
    }

    #[test]
    fn a_dotted_key_walks_into_nested_objects() {
        let node = json!({ "fields": { "summary": "boom", "status": { "name": "Open" } } });
        assert_eq!(text(&node, &["fields.summary"]), "boom");
        assert_eq!(text(&node, &["fields.status.name"]), "Open");
        assert_eq!(text(&node, &["fields.missing"]), "");
    }

    #[test]
    fn a_required_field_reports_every_key_it_tried() {
        let err = required("jira", &json!({}), &["updated", "updatedAt"]).unwrap_err();
        assert!(err.to_string().contains("updated"));
        assert!(err.to_string().contains("updatedAt"));
    }

    #[test]
    fn squeeze_collapses_whitespace_before_clipping() {
        assert_eq!(squeeze("  a   b \n c ", 80), "a b c");
        assert_eq!(squeeze("abcdef", 3), "abc…");
        assert_eq!(squeeze("abc", 3), "abc");
    }
}
