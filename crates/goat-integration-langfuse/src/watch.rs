use std::sync::Arc;

use goat_auth::CredentialStore;
use goat_integration::diff::RETAIN;
use goat_integration::query::{self, Comparison, QueryError, Token, TokenKind, TokenValue};
use goat_integration::watch::{CompiledWatch, Observed, WatchPage, WatchSource, WatchSpec};
use goat_integration::{IntegrationBinding, IntegrationResult, IntegrationRuntime};
use goat_integration_mcp::McpService;
use goat_types::IntegrationUpdateKind;
use serde_json::{Value, json};

use crate::parse::parse_observations;
use crate::{VOCABULARY, service};

pub const TOOL_LIST_OBSERVATIONS: &str = "listObservations";
pub const DEFAULT_LIMIT: u32 = 25;

pub fn defaults(_: &IntegrationBinding) -> Vec<WatchSpec> {
    Vec::new()
}

#[derive(Debug, PartialEq, Eq)]
struct Plan {
    filter: Vec<Value>,
    limit: u32,
}

pub fn compile(
    binding: &IntegrationBinding,
    runtime: &IntegrationRuntime,
    spec: &WatchSpec,
) -> IntegrationResult<CompiledWatch> {
    let plan = plan(&spec.query)?;
    Ok(CompiledWatch {
        kind: IntegrationUpdateKind::Updated,
        entity: "trace",
        diff: RETAIN,
        source: Box::new(ObservationSearch {
            service: Arc::new(service()),
            credentials: runtime.credentials.clone(),
            binding: binding.clone(),
            filter: plan.filter,
            limit: plan.limit,
        }),
    })
}

fn plan(raw: &str) -> Result<Plan, QueryError> {
    let resolved = query::resolve(&VOCABULARY, query::parse(raw)?)?;
    let filter = resolved
        .residue
        .iter()
        .map(filter_of)
        .collect::<Result<Vec<Value>, QueryError>>()?;
    let limit = resolved
        .limit
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(DEFAULT_LIMIT);
    Ok(Plan { filter, limit })
}

fn filter_of(token: &Token) -> Result<Value, QueryError> {
    let TokenKind::Pair { key, value } = &token.kind else {
        return Err(QueryError::Invalid(format!(
            "langfuse takes key:value filters only; `{}` is free text — write e.g. level:ERROR",
            token.raw
        )));
    };
    let TokenValue::Text(text) = value else {
        return Err(QueryError::Invalid(format!(
            "`{key}:@me` has no meaning here; langfuse takes key:value filters only"
        )));
    };
    let (comparison, rest) = query::split_comparison(text);
    if rest.is_empty() {
        return Err(QueryError::Invalid(format!(
            "`{}` needs a value after the comparison",
            token.raw
        )));
    }
    Ok(json!({
        "column": key,
        "operator": operator(comparison, token.negated, &token.raw)?,
        "value": rest,
    }))
}

fn operator(comparison: Comparison, negated: bool, raw: &str) -> Result<&'static str, QueryError> {
    match (comparison, negated) {
        (Comparison::Eq, false) => Ok("="),
        (Comparison::Eq, true) => Ok("!="),
        (Comparison::Gt, false) => Ok(">"),
        (Comparison::Gte, false) => Ok(">="),
        (Comparison::Lt, false) => Ok("<"),
        (Comparison::Lte, false) => Ok("<="),
        (_, true) => Err(QueryError::Invalid(format!(
            "`{raw}` negates a comparison; write the opposite comparison instead"
        ))),
    }
}

struct ObservationSearch {
    service: Arc<McpService>,
    credentials: CredentialStore,
    binding: IntegrationBinding,
    filter: Vec<Value>,
    limit: u32,
}

impl ObservationSearch {
    fn arguments(&self) -> Value {
        json!({
            "filter": self.filter,
            "limit": self.limit,
        })
    }
}

impl WatchSource for ObservationSearch {
    async fn fetch(&self) -> IntegrationResult<WatchPage> {
        let session = self
            .service
            .connect(&self.credentials, &self.binding)
            .await?;
        let result = self
            .service
            .call(&session, TOOL_LIST_OBSERVATIONS, self.arguments())
            .await;
        session.close().await;
        let (flagged, total) = parse_observations(&result?)?;
        let shown = flagged.len();
        let items = flagged
            .into_iter()
            .map(|item| {
                let observed = Observed::new(item.key, item.stamp, item.summary, item.raw);
                match item.trace {
                    Some(trace) => observed.with_reference(trace),
                    None => observed,
                }
            })
            .collect();
        let page = WatchPage::new(items);
        Ok(match total {
            Some(total) => page.with_truncated(total > shown as u64),
            None => page,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_level_filter_compiles_to_a_structured_column() {
        let plan = plan("level:ERROR").unwrap();
        assert_eq!(
            plan.filter,
            vec![json!({ "column": "level", "operator": "=", "value": "ERROR" })]
        );
        assert_eq!(plan.limit, 25);
    }

    #[test]
    fn a_negated_pair_becomes_not_equals() {
        let plan = plan("-level:DEBUG").unwrap();
        assert_eq!(
            plan.filter,
            vec![json!({ "column": "level", "operator": "!=", "value": "DEBUG" })]
        );
    }

    #[test]
    fn comparisons_map_to_langfuse_operators() {
        let op = |q: &str| plan(q).unwrap().filter[0]["operator"].clone();
        assert_eq!(op("timestamp:>2026-01-01"), json!(">"));
        assert_eq!(op("timestamp:>=2026-01-01"), json!(">="));
        assert_eq!(op("timestamp:<2026-01-01"), json!("<"));
        assert_eq!(op("timestamp:<=2026-01-01"), json!("<="));
        assert_eq!(
            plan("timestamp:>2026-01-01").unwrap().filter[0]["value"],
            json!("2026-01-01")
        );
    }

    #[test]
    fn several_pairs_accumulate_into_one_filter() {
        let plan = plan("level:ERROR type:GENERATION").unwrap();
        assert_eq!(
            plan.filter,
            vec![
                json!({ "column": "level", "operator": "=", "value": "ERROR" }),
                json!({ "column": "type", "operator": "=", "value": "GENERATION" }),
            ]
        );
    }

    #[test]
    fn a_negated_comparison_is_rejected() {
        let err = plan("-timestamp:>2026-01-01").unwrap_err();
        assert!(matches!(err, QueryError::Invalid(m) if m.contains("opposite comparison")));
    }

    #[test]
    fn free_text_is_rejected() {
        let err = plan("boom").unwrap_err();
        assert!(matches!(err, QueryError::Invalid(m) if m.contains("key:value filters only")));
    }

    #[test]
    fn a_selfref_is_rejected() {
        let err = plan("user:@me").unwrap_err();
        assert!(matches!(err, QueryError::Invalid(m) if m.contains("key:value filters only")));
        assert!(matches!(plan("@me"), Err(QueryError::Invalid(_))));
    }

    #[test]
    fn a_comparison_without_a_value_is_rejected() {
        let err = plan("timestamp:>").unwrap_err();
        assert!(matches!(err, QueryError::Invalid(m) if m.contains("needs a value")));
    }

    #[test]
    fn an_explicit_limit_overrides_the_default() {
        assert_eq!(plan("level:ERROR limit:100").unwrap().limit, 100);
        assert!(matches!(
            plan("limit:0"),
            Err(QueryError::LimitRange { .. })
        ));
        assert!(matches!(
            plan("limit:9999"),
            Err(QueryError::LimitRange { .. })
        ));
    }

    #[test]
    fn an_empty_query_still_lists_observations() {
        let plan = plan("").unwrap();
        assert!(plan.filter.is_empty());
        assert_eq!(plan.limit, DEFAULT_LIMIT);
    }

    #[test]
    fn the_compiled_filter_is_passed_to_list_observations() {
        let plan = plan("level:ERROR limit:25").unwrap();
        let search = ObservationSearch {
            service: Arc::new(service()),
            credentials: CredentialStore::new(std::path::PathBuf::from("/tmp/unused.json")),
            binding: IntegrationBinding::from_config(json!({})),
            filter: plan.filter,
            limit: plan.limit,
        };
        let arguments = search.arguments();
        assert_eq!(
            arguments["filter"],
            json!([{ "column": "level", "operator": "=", "value": "ERROR" }])
        );
        assert_eq!(arguments["limit"], 25);
    }
}
