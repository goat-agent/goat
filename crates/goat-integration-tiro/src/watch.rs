use std::sync::Arc;

use goat_auth::CredentialStore;
use goat_integration::diff::SETTLE;
use goat_integration::watch::{Observed, Watch, WatchPage, WatchSource, run};
use goat_integration::{IntegrationBinding, IntegrationResult, IntegrationRuntime};
use goat_integration_mcp::McpService;
use goat_types::{IntegrationUpdateKind, ProfileId};
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::parse::parse_notes;
use crate::{TiroBinding, service};

pub const STREAM: &str = "notes";
pub const TOOL_LIST_NOTES: &str = "list_notes";
const PAGE_SIZE: u64 = 50;

pub fn spawn(
    persona: ProfileId,
    binding: &IntegrationBinding,
    runtime: &IntegrationRuntime,
    cancel: CancellationToken,
) -> Option<JoinHandle<()>> {
    let settings = TiroBinding::read(&binding.config);
    if settings.workspace.is_none() && settings.folder_id.is_none() {
        warn!(
            profile = %persona,
            "tiro watcher disabled; set `workspace` or `folder_id` in the agent's tiro binding",
        );
        return None;
    }
    let source = Notes {
        service: Arc::new(service()),
        credentials: runtime.credentials.clone(),
        binding: binding.clone(),
        workspace: settings.workspace,
        folder_id: settings.folder_id,
    };
    let watch = Watch::new(
        crate::ID,
        STREAM,
        IntegrationUpdateKind::Updated,
        "note",
        "notes waiting",
        SETTLE,
        source,
    );
    Some(tokio::spawn(run(
        watch,
        persona,
        runtime.clone(),
        binding.account.clone(),
        cancel,
    )))
}

struct Notes {
    service: Arc<McpService>,
    credentials: CredentialStore,
    binding: IntegrationBinding,
    workspace: Option<String>,
    folder_id: Option<String>,
}

impl Notes {
    fn arguments(&self) -> Value {
        let mut arguments = json!({ "pagination": { "size": PAGE_SIZE } });
        if let Some(workspace) = &self.workspace {
            arguments["workspaceGuid"] = json!(workspace);
        }
        if let Some(folder_id) = &self.folder_id {
            arguments["filter"] = json!({ "folderId": folder_id });
        }
        arguments
    }
}

impl WatchSource for Notes {
    async fn fetch(&self) -> IntegrationResult<WatchPage> {
        let session = self
            .service
            .connect(&self.credentials, &self.binding)
            .await?;
        let result = self
            .service
            .call(&session, TOOL_LIST_NOTES, self.arguments())
            .await;
        session.close().await;
        let notes = parse_notes(&result?)?;
        Ok(WatchPage::new(
            notes
                .into_iter()
                .map(|note| {
                    Observed::new(
                        note.key.clone(),
                        note.updated_at.clone(),
                        note.summary(),
                        note.raw,
                    )
                })
                .collect(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(workspace: Option<&str>, folder: Option<&str>) -> Notes {
        Notes {
            service: Arc::new(service()),
            credentials: CredentialStore::new(std::path::PathBuf::from("/tmp/unused.json")),
            binding: IntegrationBinding::from_config(json!({})),
            workspace: workspace.map(str::to_owned),
            folder_id: folder.map(str::to_owned),
        }
    }

    #[test]
    fn a_bare_page_request_carries_only_the_page_size() {
        let arguments = source(None, None).arguments();
        assert_eq!(arguments["pagination"]["size"], 50);
        assert!(arguments.get("workspaceGuid").is_none());
        assert!(arguments.get("filter").is_none());
    }

    #[test]
    fn a_workspace_and_folder_both_narrow_the_listing() {
        let arguments = source(Some("W1"), Some("F2")).arguments();
        assert_eq!(arguments["workspaceGuid"], "W1");
        assert_eq!(arguments["filter"]["folderId"], "F2");
    }
}
