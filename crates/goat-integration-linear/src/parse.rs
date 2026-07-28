use goat_integration::{IntegrationError, IntegrationResult};
use serde_json::Value;

#[derive(Clone, Debug)]
pub struct AssignedIssue {
    pub id: String,
    pub identifier: String,
    pub title: String,
    pub url: String,
    pub updated_at: String,
    pub state: String,
    pub status_type: String,
    pub raw: Value,
}

impl AssignedIssue {
    pub fn is_closed(&self) -> bool {
        matches!(self.status_type.as_str(), "completed" | "canceled")
    }
}

pub struct FetchPage {
    pub issues: Vec<AssignedIssue>,
    pub truncated: bool,
}

pub fn parse_page(data: &Value) -> IntegrationResult<FetchPage> {
    Ok(FetchPage {
        issues: parse_assigned_issues(data)?,
        truncated: data
            .get("hasNextPage")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

pub fn parse_assigned_issues(data: &Value) -> IntegrationResult<Vec<AssignedIssue>> {
    issue_array(data)
        .ok_or_else(|| {
            IntegrationError::Service(format!("linear response has no issue list: {data}"))
        })?
        .iter()
        .map(parse_issue_node)
        .collect()
}

fn issue_array(data: &Value) -> Option<&Vec<Value>> {
    if let Some(array) = data.as_array() {
        return Some(array);
    }
    ["issues", "nodes", "results", "items"]
        .iter()
        .find_map(|key| data.get(key).and_then(Value::as_array))
}

fn parse_issue_node(node: &Value) -> IntegrationResult<AssignedIssue> {
    let field = |key: &str| {
        node.get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| IntegrationError::Service(format!("issue node missing `{key}`")))
    };
    let identifier = node
        .get("identifier")
        .and_then(Value::as_str)
        .map(str::to_string)
        .map_or_else(|| field("id"), Ok)?;
    Ok(AssignedIssue {
        id: node
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or(&identifier)
            .to_string(),
        title: field("title")?,
        url: field("url")?,
        updated_at: field("updatedAt")?,
        state: node
            .get("status")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                node.get("state").and_then(|state| {
                    state.as_str().map(str::to_string).or_else(|| {
                        state
                            .pointer("/name")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                })
            })
            .unwrap_or_else(|| "unknown".to_string()),
        status_type: node
            .get("statusType")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        raw: node.clone(),
        identifier,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(identifier: &str, updated_at: &str) -> Value {
        json!({
            "id": identifier,
            "title": format!("{identifier} title"),
            "url": format!("https://linear.app/goat/issue/{identifier}"),
            "updatedAt": updated_at,
            "status": "In Progress",
            "statusType": "started",
            "assignee": "Mino",
            "description": "workers hammer the API"
        })
    }

    #[test]
    fn page_carries_truncation_flag() {
        let page =
            parse_page(&json!({ "issues": [node("US-1", "t1")], "hasNextPage": true })).unwrap();
        assert!(page.truncated);
        let page = parse_page(&json!([node("US-1", "t1")])).unwrap();
        assert!(!page.truncated);
    }

    #[test]
    fn parses_live_shape_wrapped_in_issues() {
        let wrapped = json!({
            "issues": [node("US-1880", "t1"), node("US-1885", "t2")],
            "hasNextPage": false,
            "cursor": "abc"
        });
        let issues = parse_assigned_issues(&wrapped).unwrap();
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].identifier, "US-1880");
        assert_eq!(issues[0].id, "US-1880");
        assert_eq!(issues[0].state, "In Progress");
        assert_eq!(issues[0].status_type, "started");
        assert_eq!(issues[0].raw["description"], "workers hammer the API");
    }

    #[test]
    fn parses_bare_array_and_alternate_wrappers() {
        let bare = json!([node("US-1", "t1")]);
        assert_eq!(parse_assigned_issues(&bare).unwrap().len(), 1);
        for key in ["nodes", "results", "items"] {
            let wrapped = json!({ key: [node("US-2", "t2")] });
            assert_eq!(
                parse_assigned_issues(&wrapped).unwrap()[0].identifier,
                "US-2"
            );
        }
    }

    #[test]
    fn closed_states_are_detected() {
        let mut done = node("US-1", "t1");
        done["statusType"] = json!("completed");
        let mut canceled = node("US-2", "t2");
        canceled["statusType"] = json!("canceled");
        let issues = parse_assigned_issues(&json!([done, canceled, node("US-3", "t3")])).unwrap();
        assert!(issues[0].is_closed());
        assert!(issues[1].is_closed());
        assert!(!issues[2].is_closed());
    }

    #[test]
    fn tolerates_graphql_style_nodes() {
        let flat = json!([{
            "id": "uuid-9",
            "identifier": "US-9",
            "title": "t",
            "url": "https://linear.app/goat/issue/US-9",
            "updatedAt": "t9",
            "state": { "name": "Todo" }
        }]);
        let issues = parse_assigned_issues(&flat).unwrap();
        assert_eq!(issues[0].identifier, "US-9");
        assert_eq!(issues[0].id, "uuid-9");
        assert_eq!(issues[0].state, "Todo");
        assert_eq!(issues[0].status_type, "unknown");
    }

    #[test]
    fn rejects_missing_required_fields_and_unknown_shapes() {
        assert!(parse_assigned_issues(&json!([{ "id": "x" }])).is_err());
        assert!(parse_assigned_issues(&json!({ "unexpected": true })).is_err());
        assert!(parse_assigned_issues(&json!("text blob")).is_err());
    }
}
