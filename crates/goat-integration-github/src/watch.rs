use goat_integration::diff::REBUILD;
use goat_integration::watch::{Observed, Watch, WatchPage, WatchSource, run};
use goat_integration::{
    IntegrationBinding, IntegrationError, IntegrationResult, IntegrationRuntime,
};
use goat_types::{IntegrationUpdateKind, ProfileId};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::GithubBinding;
use crate::parse::{parse_items, truncated};

pub const DEFAULT_LIMIT: usize = 50;

pub fn spawn_all(
    persona: ProfileId,
    binding: &IntegrationBinding,
    runtime: &IntegrationRuntime,
    cancel: &CancellationToken,
) -> Vec<JoinHandle<()>> {
    if !goat_github::gh_available() {
        warn!(
            profile = %persona,
            "github watcher disabled; the `gh` cli is not on PATH",
        );
        return Vec::new();
    }
    let settings = GithubBinding::read(&binding.config);
    let queries = settings.streams();
    if queries.is_empty() {
        warn!(
            profile = %persona,
            "github watcher disabled; the agent's github binding declares no `watch` entries",
        );
        return Vec::new();
    }
    let limit = settings.limit();
    queries
        .into_iter()
        .map(|entry| {
            let watch = Watch::new(
                crate::ID,
                entry.stream.clone(),
                IntegrationUpdateKind::Assigned,
                "item",
                "waiting on you",
                REBUILD,
                Search {
                    query: entry.query,
                    limit,
                },
            );
            tokio::spawn(run(
                watch,
                persona,
                runtime.clone(),
                binding.account.clone(),
                cancel.clone(),
            ))
        })
        .collect()
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
