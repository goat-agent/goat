use std::sync::Arc;

use goat_auth::CredentialStore;
use goat_integration::diff::SETTLE;
use goat_integration::query::{self, QueryError, TokenValue};
use goat_integration::watch::{CompiledWatch, Observed, WatchPage, WatchSource, WatchSpec};
use goat_integration::{IntegrationBinding, IntegrationResult, IntegrationRuntime};
use goat_integration_mcp::McpService;
use goat_types::IntegrationUpdateKind;
use serde_json::{Value, json};

use crate::parse::parse_notes;
use crate::{VOCABULARY, service};

pub const TOOL_LIST_NOTES: &str = "list_notes";

pub fn defaults(_: &IntegrationBinding) -> Vec<WatchSpec> {
    Vec::new()
}

pub fn compile(
    binding: &IntegrationBinding,
    runtime: &IntegrationRuntime,
    spec: &WatchSpec,
) -> IntegrationResult<CompiledWatch> {
    let arguments = plan(&spec.query)?;
    Ok(CompiledWatch {
        kind: IntegrationUpdateKind::Updated,
        entity: "note",
        diff: SETTLE,
        source: Box::new(Notes {
            service: Arc::new(service()),
            credentials: runtime.credentials.clone(),
            binding: binding.clone(),
            arguments,
        }),
    })
}

fn plan(raw: &str) -> Result<Value, QueryError> {
    let resolved = query::resolve(&VOCABULARY, query::parse(raw)?)?;
    if resolved.single("workspace").is_none() && resolved.single("folder").is_none() {
        return Err(QueryError::Invalid(
            "tiro watches nothing without `workspace:<name>` or `folder:<id>`".to_owned(),
        ));
    }
    let mut arguments = json!({});
    if let Some(limit) = resolved.limit {
        arguments["pagination"] = json!({ "size": limit });
    }
    if let Some(workspace) = resolved.single("workspace")
        && let TokenValue::Text(text) = &workspace.value
    {
        arguments["workspaceGuid"] = json!(text);
    }
    if let Some(folder) = resolved.single("folder")
        && let TokenValue::Text(text) = &folder.value
    {
        arguments["filter"] = json!({ "folderId": text });
    }
    Ok(arguments)
}

struct Notes {
    service: Arc<McpService>,
    credentials: CredentialStore,
    binding: IntegrationBinding,
    arguments: Value,
}

impl WatchSource for Notes {
    async fn fetch(&self) -> IntegrationResult<WatchPage> {
        let session = self
            .service
            .connect(&self.credentials, &self.binding)
            .await?;
        let result = self
            .service
            .call(&session, TOOL_LIST_NOTES, self.arguments.clone())
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

    #[test]
    fn there_is_no_default_watch() {
        assert!(defaults(&IntegrationBinding::from_config(json!({}))).is_empty());
    }

    #[test]
    fn a_workspace_query_compiles_to_the_old_request_shape() {
        let arguments = plan("workspace:W1").unwrap();
        assert_eq!(
            arguments,
            json!({ "pagination": { "size": 50 }, "workspaceGuid": "W1" })
        );
    }

    #[test]
    fn a_folder_query_narrows_by_folder_filter() {
        let arguments = plan("folder:F2").unwrap();
        assert_eq!(
            arguments,
            json!({ "pagination": { "size": 50 }, "filter": { "folderId": "F2" } })
        );
    }

    #[test]
    fn workspace_folder_and_limit_narrow_together() {
        let arguments = plan("workspace:W1 folder:F2 limit:25").unwrap();
        assert_eq!(
            arguments,
            json!({
                "pagination": { "size": 25 },
                "workspaceGuid": "W1",
                "filter": { "folderId": "F2" },
            })
        );
    }

    #[test]
    fn a_query_naming_neither_workspace_nor_folder_refuses_to_compile() {
        assert!(matches!(plan(""), Err(QueryError::Invalid(_))));
        assert!(matches!(plan("limit:10"), Err(QueryError::Invalid(_))));
    }

    #[test]
    fn free_text_is_rejected() {
        assert!(matches!(
            plan("workspace:W1 stray"),
            Err(QueryError::FreeText { .. })
        ));
    }

    #[test]
    fn a_typo_names_the_known_keys() {
        let err = plan("workspce:W1").unwrap_err();
        let QueryError::UnknownKey { known, .. } = err else {
            panic!("expected UnknownKey");
        };
        assert_eq!(known, "folder, limit, workspace");
    }

    #[test]
    fn each_key_may_appear_only_once() {
        assert!(matches!(
            plan("workspace:W1 workspace:W2"),
            Err(QueryError::Repeated(_))
        ));
    }
}
