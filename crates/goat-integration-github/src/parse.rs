use goat_integration::{IntegrationError, IntegrationResult};
use serde_json::Value;

#[derive(Clone, Debug)]
pub struct Item {
    pub key: String,
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub updated_at: String,
    pub is_pr: bool,
    pub raw: Value,
}

impl Item {
    pub fn summary(&self) -> String {
        let kind = if self.is_pr { "PR" } else { "issue" };
        format!(
            "{} {kind} #{} — {}",
            self.repo,
            self.number,
            squeeze(&self.title, 160)
        )
    }
}

pub struct FetchPage {
    pub items: Vec<Item>,
    pub truncated: bool,
}

pub fn parse_page(data: &Value) -> IntegrationResult<FetchPage> {
    let items = parse_items(data)?;
    let total = data.get("total_count").and_then(Value::as_u64);
    let incomplete = data
        .get("incomplete_results")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(FetchPage {
        truncated: incomplete || total.is_some_and(|total| total > items.len() as u64),
        items,
    })
}

pub fn parse_items(data: &Value) -> IntegrationResult<Vec<Item>> {
    item_array(data)
        .ok_or_else(|| {
            IntegrationError::Service(format!("github response has no item list: {data}"))
        })?
        .iter()
        .map(parse_item)
        .collect()
}

fn item_array(data: &Value) -> Option<&Vec<Value>> {
    if let Some(array) = data.as_array() {
        return Some(array);
    }
    ["items", "results", "nodes"]
        .iter()
        .find_map(|key| data.get(key).and_then(Value::as_array))
}

fn parse_item(node: &Value) -> IntegrationResult<Item> {
    let number = node
        .get("number")
        .and_then(Value::as_u64)
        .ok_or_else(|| IntegrationError::Service("github item missing `number`".into()))?;
    let updated_at = string_field(node, "updated_at")
        .ok_or_else(|| IntegrationError::Service("github item missing `updated_at`".into()))?;
    let repo = parse_repo(node);
    Ok(Item {
        key: format!("{repo}#{number}"),
        repo,
        number,
        title: string_field(node, "title").unwrap_or_default(),
        updated_at,
        is_pr: node.get("pull_request").is_some_and(|node| !node.is_null()),
        raw: node.clone(),
    })
}

fn parse_repo(node: &Value) -> String {
    if let Some(full_name) = node
        .get("repository")
        .and_then(|repository| repository.get("full_name"))
        .and_then(Value::as_str)
    {
        return full_name.to_string();
    }
    let Some(url) = string_field(node, "repository_url") else {
        return "?".to_string();
    };
    url.rsplit_once("/repos/")
        .map_or(url.clone(), |(_, tail)| tail.to_string())
}

fn string_field(node: &Value, key: &str) -> Option<String> {
    node.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
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

    fn pr_node() -> Value {
        json!({
            "number": 42,
            "title": "feat: add the github integration",
            "updated_at": "2026-07-28T04:00:00Z",
            "html_url": "https://github.com/goat-agent/goat/pull/42",
            "repository_url": "https://api.github.com/repos/goat-agent/goat",
            "pull_request": { "url": "https://api.github.com/repos/goat-agent/goat/pulls/42" }
        })
    }

    fn issue_node() -> Value {
        json!({
            "number": 7,
            "title": "watcher backs off too slowly",
            "updated_at": "2026-07-28T03:00:00Z",
            "html_url": "https://github.com/goat-agent/goat/issues/7",
            "repository_url": "https://api.github.com/repos/goat-agent/goat"
        })
    }

    #[test]
    fn parses_the_search_issues_envelope() {
        let page = parse_page(&json!({
            "total_count": 2,
            "incomplete_results": false,
            "items": [pr_node(), issue_node()]
        }))
        .unwrap();
        assert_eq!(page.items.len(), 2);
        assert!(!page.truncated);
        assert_eq!(page.items[0].key, "goat-agent/goat#42");
        assert_eq!(page.items[0].repo, "goat-agent/goat");
        assert_eq!(page.items[0].updated_at, "2026-07-28T04:00:00Z");
    }

    #[test]
    fn pull_requests_are_distinguished_from_issues() {
        let items = parse_items(&json!([pr_node(), issue_node()])).unwrap();
        assert!(items[0].is_pr);
        assert!(!items[1].is_pr);
        assert!(items[0].summary().starts_with("goat-agent/goat PR #42 — "));
        assert!(
            items[1]
                .summary()
                .starts_with("goat-agent/goat issue #7 — ")
        );
    }

    #[test]
    fn repo_comes_from_full_name_when_present() {
        let mut node = issue_node();
        node["repository"] = json!({ "full_name": "acme/widgets" });
        assert_eq!(parse_items(&json!([node])).unwrap()[0].repo, "acme/widgets");
    }

    #[test]
    fn an_unreadable_repository_url_still_yields_a_stable_key() {
        let mut node = issue_node();
        node["repository_url"] = json!("https://example.test/nope");
        let item = &parse_items(&json!([node])).unwrap()[0];
        assert_eq!(item.repo, "https://example.test/nope");

        let mut node = issue_node();
        node.as_object_mut().unwrap().remove("repository_url");
        assert_eq!(parse_items(&json!([node])).unwrap()[0].key, "?#7");
    }

    #[test]
    fn a_short_page_against_a_bigger_total_counts_as_truncated() {
        let page = parse_page(&json!({ "total_count": 900, "items": [issue_node()] })).unwrap();
        assert!(page.truncated);

        let page = parse_page(&json!({ "incomplete_results": true, "items": [] })).unwrap();
        assert!(page.truncated);
    }

    #[test]
    fn missing_number_or_timestamp_is_rejected_and_unknown_shapes_error() {
        let mut node = issue_node();
        node.as_object_mut().unwrap().remove("number");
        assert!(parse_items(&json!([node])).is_err());

        let mut node = issue_node();
        node.as_object_mut().unwrap().remove("updated_at");
        assert!(parse_items(&json!([node])).is_err());

        assert!(parse_items(&json!({ "unexpected": true })).is_err());
        assert!(parse_items(&json!("text blob")).is_err());
    }

    #[test]
    fn summary_is_flattened_and_clamped() {
        let mut node = issue_node();
        node["title"] = json!("a  b\n  c");
        assert_eq!(
            parse_items(&json!([node])).unwrap()[0].summary(),
            "goat-agent/goat issue #7 — a b c"
        );

        let mut node = issue_node();
        node["title"] = json!("x".repeat(400));
        assert!(
            parse_items(&json!([node])).unwrap()[0]
                .summary()
                .ends_with('…')
        );
    }
}
