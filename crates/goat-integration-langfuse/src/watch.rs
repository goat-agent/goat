use std::sync::Arc;

use goat_auth::CredentialStore;
use goat_integration::diff::RETAIN;
use goat_integration::watch::{Observed, Watch, WatchPage, WatchSource, run};
use goat_integration::{IntegrationBinding, IntegrationResult, IntegrationRuntime};
use goat_integration_mcp::McpService;
use goat_types::{AgentId, IntegrationUpdateKind};
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::parse::parse_observations;
use crate::{LangfuseBinding, service};

pub const TOOL_LIST_OBSERVATIONS: &str = "listObservations";
pub const DEFAULT_LIMIT: u32 = 25;

pub fn spawn(
    agent: AgentId,
    binding: &IntegrationBinding,
    runtime: &IntegrationRuntime,
    cancel: CancellationToken,
) -> Option<JoinHandle<()>> {
    let settings = LangfuseBinding::read(&binding.config);
    if settings.watch.is_empty() {
        warn!(
            agent = %agent,
            "langfuse watcher disabled; declare `watch` streams in the agent's langfuse binding",
        );
        return None;
    }
    let limit = settings.limit.unwrap_or(DEFAULT_LIMIT);
    let shared = Arc::new(service());
    let binding = binding.clone();
    let runtime = runtime.clone();
    Some(tokio::spawn(async move {
        let runs: Vec<JoinHandle<()>> = settings
            .watch
            .into_iter()
            .map(|entry| {
                let source = ObservationSearch {
                    service: shared.clone(),
                    credentials: runtime.credentials.clone(),
                    binding: binding.clone(),
                    filter: entry.filter,
                    limit,
                };
                let watch = Watch::new(
                    crate::ID,
                    entry.stream,
                    IntegrationUpdateKind::Updated,
                    "trace",
                    "flagged",
                    RETAIN,
                    source,
                );
                tokio::spawn(run(
                    watch,
                    agent,
                    runtime.clone(),
                    binding.account.clone(),
                    cancel.clone(),
                ))
            })
            .collect();
        for handle in runs {
            let _ = handle.await;
        }
    }))
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

    fn source(filter: Vec<Value>, limit: u32) -> ObservationSearch {
        ObservationSearch {
            service: Arc::new(service()),
            credentials: CredentialStore::new(std::path::PathBuf::from("/tmp/unused.json")),
            binding: IntegrationBinding::from_config(json!({})),
            filter,
            limit,
        }
    }

    #[test]
    fn the_binding_filter_is_passed_through_verbatim() {
        let filter = vec![json!({ "column": "level", "operator": "=", "value": "ERROR" })];
        let arguments = source(filter.clone(), 25).arguments();
        assert_eq!(arguments["filter"], json!(filter));
        assert_eq!(arguments["limit"], 25);
    }

    #[test]
    fn an_empty_filter_still_lists_observations() {
        let arguments = source(Vec::new(), DEFAULT_LIMIT).arguments();
        assert_eq!(arguments["filter"], json!([]));
        assert_eq!(arguments["limit"], DEFAULT_LIMIT);
    }
}
