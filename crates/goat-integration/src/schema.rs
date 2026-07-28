use serde_json::{Map, Value};

pub fn drop_placeholder_args(args: Value) -> Value {
    let Value::Object(fields) = args else {
        return args;
    };
    Value::Object(
        fields
            .into_iter()
            .filter(|(_, value)| !is_placeholder(value))
            .collect::<Map<String, Value>>(),
    )
}

fn is_placeholder(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(text) => text.is_empty(),
        Value::Array(items) => items.is_empty(),
        Value::Object(fields) => fields.is_empty(),
        Value::Number(number) => number.as_f64().is_some_and(|number| number == 0.0),
        Value::Bool(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn recorded_list_issues_call_keeps_only_meaningful_filters() {
        let args = json!({
            "assignee": "me",
            "createdAt": "",
            "cursor": "",
            "cycle": "",
            "delegate": "",
            "includeArchived": true,
            "label": "",
            "limit": 250,
            "orderBy": "updatedAt",
            "parentId": "",
            "priority": 0,
            "project": "",
            "query": "",
            "release": "",
            "state": "",
            "team": "",
            "updatedAt": ""
        });
        assert_eq!(
            drop_placeholder_args(args),
            json!({
                "assignee": "me",
                "includeArchived": true,
                "limit": 250,
                "orderBy": "updatedAt"
            })
        );
    }

    #[test]
    fn zero_is_dropped_but_false_is_kept() {
        let args = json!({ "priority": 0, "estimate": 0.0, "includeArchived": false });
        assert_eq!(
            drop_placeholder_args(args),
            json!({ "includeArchived": false })
        );
    }

    #[test]
    fn write_call_loses_placeholders_but_keeps_the_edit() {
        let args = json!({
            "id": "US-1876",
            "state": "Done",
            "title": "",
            "description": "",
            "labels": [],
            "priority": 0
        });
        assert_eq!(
            drop_placeholder_args(args),
            json!({ "id": "US-1876", "state": "Done" })
        );
    }

    #[test]
    fn nested_values_are_untouched() {
        let args = json!({ "links": [{ "title": "", "url": "https://x", "rank": 0 }] });
        assert_eq!(
            drop_placeholder_args(args.clone()),
            args,
            "nesting is left to the service to interpret"
        );
    }

    #[test]
    fn non_object_arguments_pass_through() {
        assert_eq!(drop_placeholder_args(json!("me")), json!("me"));
        assert_eq!(drop_placeholder_args(Value::Null), Value::Null);
    }
}
