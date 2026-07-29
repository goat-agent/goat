use goat_integration::{IntegrationError, IntegrationResult};
use serde_json::Value;

const ENVELOPE_KEYS: [&str; 5] = ["issues", "results", "items", "nodes", "data"];

#[derive(Clone, Debug)]
pub struct Issue {
    pub key: String,
    pub short_id: String,
    pub title: String,
    pub culprit: String,
    pub count: String,
    pub user_count: String,
    pub last_seen: String,
    pub raw: Value,
}

impl Issue {
    pub fn summary(&self) -> String {
        let head = if self.short_id.is_empty() {
            squeeze(&self.title, 160)
        } else {
            format!("{} {}", self.short_id, squeeze(&self.title, 160))
        };
        let mut parts = Vec::new();
        if !self.culprit.is_empty() {
            parts.push(squeeze(&self.culprit, 80));
        }
        if let Some(volume) = self.volume() {
            parts.push(volume);
        }
        if parts.is_empty() {
            head
        } else {
            format!("{head} · {}", parts.join(", "))
        }
    }

    fn volume(&self) -> Option<String> {
        match (self.count.as_str(), self.user_count.as_str()) {
            ("", "") => None,
            (events, "") => Some(format!("{events} events")),
            ("", users) => Some(format!("{users} users")),
            (events, users) => Some(format!("{events} events, {users} users")),
        }
    }
}

pub struct FetchPage {
    pub issues: Vec<Issue>,
}

pub fn parse_page(data: &Value) -> IntegrationResult<FetchPage> {
    Ok(FetchPage {
        issues: parse_issues(data)?,
    })
}

pub fn parse_issues(data: &Value) -> IntegrationResult<Vec<Issue>> {
    issue_array(data)
        .ok_or_else(|| {
            IntegrationError::Service(format!(
                "sentry response has no issue list: {}",
                squeeze(&data.to_string(), 400)
            ))
        })?
        .iter()
        .map(parse_issue)
        .collect()
}

fn issue_array(data: &Value) -> Option<&Vec<Value>> {
    if let Some(array) = data.as_array() {
        return Some(array);
    }
    if let Some(array) = ENVELOPE_KEYS
        .iter()
        .find_map(|key| data.get(key).and_then(Value::as_array))
    {
        return Some(array);
    }
    ENVELOPE_KEYS
        .iter()
        .filter_map(|key| data.get(key))
        .find_map(|nested| {
            ENVELOPE_KEYS
                .iter()
                .find_map(|key| nested.get(key).and_then(Value::as_array))
        })
}

fn parse_issue(node: &Value) -> IntegrationResult<Issue> {
    let short_id = string_field(node, "shortId")
        .or_else(|| string_field(node, "short_id"))
        .unwrap_or_default();
    let id = string_field(node, "id")
        .or_else(|| string_field(node, "issueId"))
        .unwrap_or_default();
    let key = if short_id.is_empty() {
        id.clone()
    } else {
        short_id.clone()
    };
    if key.is_empty() {
        return Err(IntegrationError::Service(
            "sentry issue missing both `id` and `shortId`".into(),
        ));
    }
    Ok(Issue {
        key,
        short_id,
        title: string_field(node, "title")
            .or_else(|| {
                node.get("metadata")
                    .and_then(|meta| string_field(meta, "type"))
            })
            .unwrap_or_default(),
        culprit: string_field(node, "culprit").unwrap_or_default(),
        count: numeric_field(node, "count"),
        user_count: numeric_field(node, "userCount"),
        last_seen: string_field(node, "lastSeen")
            .or_else(|| string_field(node, "last_seen"))
            .unwrap_or_default(),
        raw: node.clone(),
    })
}

fn string_field(node: &Value, key: &str) -> Option<String> {
    node.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn numeric_field(node: &Value, key: &str) -> String {
    match node.get(key) {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Number(value)) => value.to_string(),
        _ => String::new(),
    }
}

fn squeeze(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let kept: String = flat.chars().take(max).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn issue_node(short_id: &str, last_seen: &str) -> Value {
        json!({
            "id": "6916805731",
            "shortId": short_id,
            "title": "TypeError: cannot read property 'id' of undefined",
            "culprit": "app/handlers/checkout in process",
            "count": "42",
            "userCount": 7,
            "level": "error",
            "status": "unresolved",
            "lastSeen": last_seen,
            "permalink": "https://acme.sentry.io/issues/6916805731/"
        })
    }

    #[test]
    fn parses_bare_array_and_wrapper_envelopes() {
        let bare = json!([issue_node("BACKEND-1A", "2026-07-28T00:00:00Z")]);
        assert_eq!(parse_issues(&bare).unwrap().len(), 1);
        for key in ENVELOPE_KEYS {
            let wrapped = json!({ key: [issue_node("BACKEND-2B", "2026-07-28T00:00:00Z")] });
            assert_eq!(parse_issues(&wrapped).unwrap()[0].key, "BACKEND-2B");
        }
    }

    #[test]
    fn parses_a_nested_envelope() {
        let nested =
            json!({ "data": { "issues": [issue_node("BACKEND-3C", "2026-07-28T00:00:00Z")] } });
        assert_eq!(parse_issues(&nested).unwrap()[0].key, "BACKEND-3C");
    }

    #[test]
    fn key_falls_back_to_the_numeric_id() {
        let node = json!([{ "id": "6916805731", "title": "boom" }]);
        let issues = parse_issues(&node).unwrap();
        assert_eq!(issues[0].key, "6916805731");
        assert!(issues[0].short_id.is_empty());
    }

    #[test]
    fn counts_are_accepted_as_strings_or_numbers() {
        let node = json!([{ "id": "1", "count": 9, "userCount": "3", "title": "boom" }]);
        let issue = &parse_issues(&node).unwrap()[0];
        assert_eq!(issue.count, "9");
        assert_eq!(issue.user_count, "3");
    }

    #[test]
    fn unknown_shapes_and_keyless_issues_error() {
        assert!(parse_issues(&json!({ "unexpected": true })).is_err());
        assert!(parse_issues(&json!("text blob")).is_err());
        assert!(parse_issues(&json!([{ "title": "no identity" }])).is_err());
    }

    #[test]
    fn summary_carries_identity_culprit_and_volume() {
        let issue =
            &parse_issues(&json!([issue_node("BACKEND-1A", "2026-07-28T00:00:00Z")])).unwrap()[0];
        assert_eq!(
            issue.summary(),
            "BACKEND-1A TypeError: cannot read property 'id' of undefined · \
             app/handlers/checkout in process, 42 events, 7 users"
        );
    }

    #[test]
    fn summary_degrades_when_fields_are_missing_and_clamps_long_titles() {
        let bare = &parse_issues(&json!([{ "id": "1", "title": "boom" }])).unwrap()[0];
        assert_eq!(bare.summary(), "boom");

        let long = json!([{ "id": "1", "title": "a ".repeat(200) }]);
        assert!(parse_issues(&long).unwrap()[0].summary().ends_with('…'));
    }
}
