use goat_integration::diff::REBUILD;
use goat_integration::query::{self, QueryError, SelfRefStyle};
use goat_integration::watch::{CompiledWatch, Observed, WatchPage, WatchSource, WatchSpec};
use goat_integration::{IntegrationError, IntegrationResult};
use goat_types::IntegrationUpdateKind;

use crate::parse::{parse_items, truncated};
use crate::{DEFAULT_LIMIT, MISSING_GH, VOCABULARY};

pub fn defaults() -> Vec<WatchSpec> {
    vec![
        WatchSpec {
            stream: "review".to_owned(),
            query: "is:open is:pr review-requested:@me".to_owned(),
        },
        WatchSpec {
            stream: "assigned".to_owned(),
            query: "is:open assignee:@me".to_owned(),
        },
    ]
}

#[derive(Debug, PartialEq, Eq)]
struct Plan {
    query: String,
    limit: usize,
}

pub fn compile(spec: &WatchSpec) -> IntegrationResult<CompiledWatch> {
    if !goat_github::gh_available() {
        return Err(IntegrationError::Config(MISSING_GH.to_owned()));
    }
    let plan = plan(&spec.query)?;
    Ok(CompiledWatch {
        kind: IntegrationUpdateKind::Assigned,
        entity: "item",
        diff: REBUILD,
        source: Box::new(Search {
            query: plan.query,
            limit: plan.limit,
        }),
    })
}

fn plan(raw: &str) -> Result<Plan, QueryError> {
    let resolved = query::resolve(&VOCABULARY, query::parse(raw)?)?;
    Ok(Plan {
        query: query::render(&resolved.residue, SelfRefStyle::Native),
        limit: resolved.limit.unwrap_or(DEFAULT_LIMIT),
    })
}

struct Search {
    query: String,
    limit: usize,
}

impl WatchSource for Search {
    async fn fetch(&self) -> IntegrationResult<WatchPage> {
        let value = goat_github::cli::search_issues(&self.query, self.limit)
            .await
            .map_err(map_error)?;
        let items = parse_items(&value)?;
        Ok(WatchPage {
            items: items
                .into_iter()
                .map(|item| {
                    Observed::new(
                        item.key.clone(),
                        item.updated_at.clone(),
                        item.summary(),
                        item.raw,
                    )
                })
                .collect(),
            truncated: Some(truncated(&value)),
        })
    }
}

pub fn map_error(error: goat_github::cli::GhError) -> IntegrationError {
    match error {
        goat_github::cli::GhError::Auth(detail) => IntegrationError::Auth(format!(
            "github needs the gh cli signed in ({detail}); run `gh auth login`"
        )),
        goat_github::cli::GhError::Failed(detail) => {
            IntegrationError::Service(format!("github search failed: {detail}"))
        }
        other => IntegrationError::Service(format!("github search failed: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_default_stream_compiles_byte_identically_to_its_historical_query() {
        let specs = defaults();
        let streams: Vec<&str> = specs.iter().map(|spec| spec.stream.as_str()).collect();
        assert_eq!(streams, ["review", "assigned"]);
        assert_eq!(specs[0].query, "is:open is:pr review-requested:@me");
        assert_eq!(specs[1].query, "is:open assignee:@me");
        for spec in &specs {
            let plan = plan(&spec.query).unwrap();
            assert_eq!(plan.query, spec.query);
            assert_eq!(plan.limit, DEFAULT_LIMIT);
        }
    }

    #[test]
    fn a_limit_token_is_extracted_and_the_rest_passes_through() {
        let plan = plan("is:open assignee:@me limit:25").unwrap();
        assert_eq!(plan.query, "is:open assignee:@me");
        assert_eq!(plan.limit, 25);
    }

    #[test]
    fn native_github_syntax_passes_through_untouched() {
        let raw = r#"repo:goat-agent/goat -label:wip "exact phrase" involves:@me comments:>5"#;
        assert_eq!(plan(raw).unwrap().query, raw);
    }

    #[test]
    fn limit_violations_are_loud() {
        assert!(matches!(
            plan("limit:0"),
            Err(QueryError::LimitRange { .. })
        ));
        assert!(matches!(
            plan("limit:101"),
            Err(QueryError::LimitRange { .. })
        ));
        assert!(matches!(
            plan("limit:many"),
            Err(QueryError::LimitRange { .. })
        ));
        assert!(matches!(
            plan("limit:5 limit:6"),
            Err(QueryError::Repeated(_))
        ));
    }

    #[test]
    fn an_auth_failure_points_at_the_gh_cli_not_a_goat_command() {
        let mapped = map_error(goat_github::cli::GhError::Auth("HTTP 401".to_owned()));
        let IntegrationError::Auth(message) = mapped else {
            panic!("expected an auth error");
        };
        assert!(message.contains("gh auth login"));
        assert!(!message.contains("goat integration add"));
    }

    #[test]
    fn any_other_failure_stays_a_service_error() {
        let mapped = map_error(goat_github::cli::GhError::Failed("boom".to_owned()));
        assert!(matches!(mapped, IntegrationError::Service(m) if m.contains("boom")));
    }
}
