use std::fmt::Write as _;

use goat_integration::{IntegrationError, IntegrationResult};
use serde_json::Value;

const SUMMARY_LIMIT: usize = 160;
const UNTITLED: &str = "(untitled)";

#[derive(Clone, Debug)]
pub struct ViewRow {
    pub id: String,
    pub title: String,
    pub url: Option<String>,
    pub edited_at: String,
    pub raw: Value,
}

impl ViewRow {
    pub fn summary(&self) -> String {
        let squeezed = self.title.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut summary = if squeezed.is_empty() {
            UNTITLED.to_string()
        } else if squeezed.chars().count() > SUMMARY_LIMIT {
            let clipped: String = squeezed.chars().take(SUMMARY_LIMIT).collect();
            format!("{clipped}…")
        } else {
            squeezed
        };
        if let Some(url) = &self.url {
            let _ = write!(summary, " — {url}");
        }
        summary
    }
}

pub struct FetchPage {
    pub rows: Vec<ViewRow>,
    pub truncated: bool,
}

pub fn parse_page(data: &Value) -> IntegrationResult<FetchPage> {
    Ok(FetchPage {
        rows: parse_rows(data)?,
        truncated: ["has_more", "hasMore"]
            .iter()
            .find_map(|key| data.get(key).and_then(Value::as_bool))
            .unwrap_or(false),
    })
}

pub fn parse_rows(data: &Value) -> IntegrationResult<Vec<ViewRow>> {
    row_array(data)
        .ok_or_else(|| {
            IntegrationError::Service(format!("notion response has no row list: {data}"))
        })?
        .iter()
        .map(parse_row)
        .collect()
}

fn row_array(data: &Value) -> Option<&Vec<Value>> {
    if let Some(array) = data.as_array() {
        return Some(array);
    }
    ["results", "rows", "data", "items", "nodes"]
        .iter()
        .find_map(|key| data.get(key).and_then(Value::as_array))
}

fn parse_row(node: &Value) -> IntegrationResult<ViewRow> {
    let url = ["url", "public_url", "page_url"]
        .iter()
        .find_map(|key| node.get(key).and_then(Value::as_str))
        .map(str::to_string);
    let id = ["id", "page_id", "pageId"]
        .iter()
        .find_map(|key| node.get(key).and_then(Value::as_str))
        .map(str::to_string)
        .or_else(|| url.as_deref().and_then(id_from_url))
        .ok_or_else(|| IntegrationError::Service(format!("notion row missing `id`: {node}")))?;
    let edited_at = ["last_edited_time", "last_edited", "lastEditedTime"]
        .iter()
        .find_map(|key| node.get(key).and_then(Value::as_str))
        .unwrap_or_default()
        .to_string();
    Ok(ViewRow {
        title: extract_title(node).unwrap_or_else(|| UNTITLED.to_string()),
        id,
        url,
        edited_at,
        raw: node.clone(),
    })
}

fn id_from_url(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let last = path.rsplit('/').find(|segment| !segment.is_empty())?;
    let tail = last.rsplit('-').next().unwrap_or(last);
    let compact: String = tail.chars().filter(|c| *c != '-').collect();
    if compact.len() == 32 && compact.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(compact)
    } else {
        None
    }
}

fn extract_title(node: &Value) -> Option<String> {
    for key in ["title", "Name", "name", "Title"] {
        if let Some(value) = node.get(key)
            && let Some(text) = text_of(value)
        {
            return Some(text);
        }
    }
    let properties = node.get("properties")?.as_object()?;
    for key in ["Name", "Title", "name", "title"] {
        if let Some(value) = properties.get(key)
            && let Some(text) = text_of(value)
        {
            return Some(text);
        }
    }
    properties.values().find_map(title_property)
}

fn title_property(value: &Value) -> Option<String> {
    let rich = value.get("title")?;
    let text = text_of(rich)?;
    if text.is_empty() { None } else { Some(text) }
}

fn text_of(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Array(items) => {
            let joined = items
                .iter()
                .filter_map(rich_text_fragment)
                .collect::<String>();
            if joined.is_empty() {
                None
            } else {
                Some(joined)
            }
        }
        Value::Object(_) => value.get("title").and_then(text_of),
        _ => None,
    }
}

fn rich_text_fragment(item: &Value) -> Option<String> {
    if let Some(text) = item.as_str() {
        return Some(text.to_string());
    }
    item.get("plain_text")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            item.get("text")
                .and_then(|text| text.get("content"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_notion_api_page_shape() {
        let data = json!({
            "results": [{
                "id": "1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d",
                "url": "https://www.notion.so/Spec-1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d",
                "last_edited_time": "2026-07-28T10:00:00.000Z",
                "properties": {
                    "Status": { "select": { "name": "Review" } },
                    "Name": { "title": [{ "plain_text": "Ship the watcher" }] }
                }
            }],
            "has_more": false
        });
        let page = parse_page(&data).unwrap();
        assert!(!page.truncated);
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].id, "1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d");
        assert_eq!(page.rows[0].title, "Ship the watcher");
        assert_eq!(page.rows[0].edited_at, "2026-07-28T10:00:00.000Z");
        assert_eq!(
            page.rows[0].raw["properties"]["Status"]["select"]["name"],
            "Review"
        );
    }

    #[test]
    fn parses_flattened_row_shape() {
        let data = json!([{ "id": "abc", "Name": "Flat row", "last_edited_time": "t1" }]);
        let rows = parse_rows(&data).unwrap();
        assert_eq!(rows[0].id, "abc");
        assert_eq!(rows[0].title, "Flat row");
    }

    #[test]
    fn accepts_alternate_envelopes_and_truncation_flag() {
        for key in ["rows", "data", "items", "nodes"] {
            let wrapped = json!({ key: [{ "id": "x", "title": "T" }] });
            assert_eq!(parse_rows(&wrapped).unwrap()[0].title, "T");
        }
        let page = parse_page(&json!({ "results": [], "has_more": true })).unwrap();
        assert!(page.truncated);
    }

    #[test]
    fn falls_back_to_any_title_property() {
        let data = json!([{
            "id": "x",
            "properties": {
                "Owner": { "people": [] },
                "Headline": { "title": [{ "text": { "content": "Derived" } }] }
            }
        }]);
        assert_eq!(parse_rows(&data).unwrap()[0].title, "Derived");
    }

    #[test]
    fn derives_id_from_url_when_absent() {
        let data = json!([{
            "url": "https://www.notion.so/team/Some-Page-1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d?pvs=4",
            "title": "Some Page"
        }]);
        let rows = parse_rows(&data).unwrap();
        assert_eq!(rows[0].id, "1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d");
    }

    #[test]
    fn untitled_rows_still_parse() {
        let rows = parse_rows(&json!([{ "id": "x", "properties": {} }])).unwrap();
        assert_eq!(rows[0].title, UNTITLED);
        assert_eq!(rows[0].summary(), UNTITLED);
    }

    #[test]
    fn rejects_rows_without_any_id_and_unknown_shapes() {
        assert!(parse_rows(&json!([{ "title": "no id" }])).is_err());
        assert!(parse_rows(&json!({ "unexpected": true })).is_err());
        assert!(parse_rows(&json!("text blob")).is_err());
    }

    #[test]
    fn summary_squeezes_whitespace_and_clamps() {
        let row = ViewRow {
            id: "x".into(),
            title: "  spread   over\nlines  ".into(),
            url: None,
            edited_at: String::new(),
            raw: Value::Null,
        };
        assert_eq!(row.summary(), "spread over lines");

        let long = ViewRow {
            id: "x".into(),
            title: "n".repeat(400),
            url: None,
            edited_at: String::new(),
            raw: Value::Null,
        };
        let summary = long.summary();
        assert_eq!(summary.chars().count(), SUMMARY_LIMIT + 1);
        assert!(summary.ends_with('…'));
    }

    #[test]
    fn summary_appends_the_page_url_when_present() {
        let row = ViewRow {
            id: "x".into(),
            title: "Ship the watcher".into(),
            url: Some("https://www.notion.so/Ship-x".into()),
            edited_at: String::new(),
            raw: Value::Null,
        };
        assert_eq!(
            row.summary(),
            "Ship the watcher — https://www.notion.so/Ship-x"
        );
    }
}
