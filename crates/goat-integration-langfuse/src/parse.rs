use goat_integration::{IntegrationError, IntegrationResult};
use serde_json::Value;

pub fn version(value: &Value) -> IntegrationResult<String> {
    if let Some(status) = value.get("status").and_then(Value::as_str)
        && status != "OK"
    {
        return Err(IntegrationError::Service(format!(
            "langfuse reports status `{status}`"
        )));
    }
    Ok(match value.get("version").and_then(Value::as_str) {
        Some(version) => format!("Langfuse {version}"),
        None => "langfuse mcp".to_owned(),
    })
}

pub struct Flagged {
    pub key: String,
    pub trace: Option<String>,
    pub stamp: String,
    pub summary: String,
    pub raw: Value,
}

pub fn parse_observations(result: &Value) -> IntegrationResult<(Vec<Flagged>, Option<u64>)> {
    let data = result
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| result.as_array())
        .ok_or_else(|| IntegrationError::Service("langfuse returned no observation list".into()))?;
    let total = result
        .get("meta")
        .and_then(|meta| meta.get("totalItems"))
        .and_then(Value::as_u64);
    Ok((data.iter().filter_map(flagged).collect(), total))
}

fn flagged(raw: &Value) -> Option<Flagged> {
    let id = raw.get("id").and_then(Value::as_str)?;
    let trace = raw
        .get("traceId")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let stamp = raw
        .get("startTime")
        .and_then(Value::as_str)
        .unwrap_or(id)
        .to_owned();
    Some(Flagged {
        key: id.to_owned(),
        trace,
        stamp,
        summary: summary(raw, id),
        raw: raw.clone(),
    })
}

fn summary(raw: &Value, id: &str) -> String {
    let name = raw.get("name").and_then(Value::as_str).unwrap_or(id);
    let kind = raw
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("observation");
    match raw.get("level").and_then(Value::as_str) {
        Some(level) if level != "DEFAULT" => format!("[{level}] {kind} {name}"),
        _ => format!("{kind} {name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_healthy_deployment_reports_its_version() {
        let described = version(&json!({ "status": "OK", "version": "4.0.0" })).unwrap();
        assert_eq!(described, "Langfuse 4.0.0");
    }

    #[test]
    fn an_unhealthy_status_is_an_error_not_a_name() {
        let err = version(&json!({ "status": "DEGRADED", "version": "4.0.0" })).unwrap_err();
        assert!(matches!(err, IntegrationError::Service(m) if m.contains("DEGRADED")));
    }

    #[test]
    fn a_shape_without_a_version_still_verifies() {
        assert_eq!(version(&json!({})).unwrap(), "langfuse mcp");
    }

    #[test]
    fn observations_come_out_of_the_data_envelope() {
        let (items, total) = parse_observations(&json!({
            "data": [
                { "id": "obs-1", "traceId": "tr-9", "type": "GENERATION",
                  "name": "chat", "level": "ERROR", "startTime": "2026-07-30T00:00:00Z" },
                { "id": "obs-2", "type": "SPAN", "name": "plan" },
                { "no_id": true }
            ],
            "meta": { "totalItems": 40 }
        }))
        .unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].key, "obs-1");
        assert_eq!(items[0].trace.as_deref(), Some("tr-9"));
        assert_eq!(items[0].stamp, "2026-07-30T00:00:00Z");
        assert_eq!(items[0].summary, "[ERROR] GENERATION chat");
        assert_eq!(items[1].trace, None);
        assert_eq!(items[1].stamp, "obs-2");
        assert_eq!(items[1].summary, "SPAN plan");
        assert_eq!(total, Some(40));
    }

    #[test]
    fn a_bare_array_is_accepted_too() {
        let (items, total) =
            parse_observations(&json!([{ "id": "obs-1", "type": "SPAN" }])).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(total, None);
    }

    #[test]
    fn a_shape_without_a_list_is_an_error() {
        assert!(parse_observations(&json!({ "rows": [] })).is_err());
        assert!(parse_observations(&json!("nope")).is_err());
    }

    #[test]
    fn a_default_level_is_left_off_the_summary() {
        let (items, _) = parse_observations(&json!({
            "data": [{ "id": "o", "type": "SPAN", "name": "step", "level": "DEFAULT" }]
        }))
        .unwrap();
        assert_eq!(items[0].summary, "SPAN step");
    }
}
