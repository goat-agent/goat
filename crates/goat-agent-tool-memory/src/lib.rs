use std::sync::Arc;

use async_trait::async_trait;
use goat_agent_tool::{
    ToolCall, ToolContext, ToolHandler, ToolName, ToolOutput, ToolRegistry, ToolSpec,
};
use goat_memory::facts::FactOrigin;
use goat_memory::{MemoryEngine, NewFact, Scope};
use serde::Deserialize;
use serde_json::json;

pub const MEMORY: ToolName = ToolName::from_static("memory");
pub const MEMORY_SEARCH: ToolName = ToolName::from_static("memory_search");
pub const FACT: ToolName = ToolName::from_static("fact");

const DEFAULT_RECALL_K: usize = 6;

pub fn register(registry: &mut ToolRegistry, engine: Arc<MemoryEngine>) {
    registry.insert_handler(
        spec_memory(),
        Arc::new(MemoryTool {
            engine: engine.clone(),
        }),
        true,
    );
    registry.insert_handler(
        spec_memory_search(),
        Arc::new(SearchTool {
            engine: engine.clone(),
        }),
        true,
    );
    registry.insert_handler(spec_fact(), Arc::new(FactTool { engine }), true);
}

fn parse_mem_path(path: &str) -> Result<(Scope, String), String> {
    let trimmed = path.trim_start_matches('/');
    let rest = trimmed
        .strip_prefix("memories/")
        .ok_or_else(|| format!("path must start with /memories/: {path:?}"))?;
    let mut parts = rest.splitn(2, '/');
    let scope_seg = parts.next().unwrap_or("");
    let rel = parts.next().unwrap_or("");
    if rel.is_empty() {
        return Err(format!(
            "path must include a file under the scope: {path:?}"
        ));
    }
    let scope = match scope_seg {
        "owner" => Scope::Owner,
        "self" => Scope::Self_,
        other => Scope::domain(other).map_err(|e| e.to_string())?,
    };
    Ok((scope, rel.to_string()))
}

#[derive(Debug, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
enum MemoryCmd {
    View {
        path: String,
        view_range: Option<[usize; 2]>,
    },
    Create {
        path: String,
        text: String,
    },
    StrReplace {
        path: String,
        old_str: String,
        new_str: String,
    },
    Insert {
        path: String,
        line: usize,
        text: String,
    },
    Delete {
        path: String,
    },
    Rename {
        path: String,
        new_path: String,
    },
}

struct MemoryTool {
    engine: Arc<MemoryEngine>,
}

impl MemoryTool {
    async fn run(&self, cmd: MemoryCmd) -> ToolOutput {
        let files = self.engine.files();
        match cmd {
            MemoryCmd::View { path, view_range } => {
                let (scope, rel) = match parse_mem_path(&path) {
                    Ok(v) => v,
                    Err(e) => return ToolOutput::error(e),
                };
                let range = view_range.map(|[a, b]| (a, b));
                match files.view(&scope, &rel, range).await {
                    Ok(text) => ToolOutput::text(text),
                    Err(e) => ToolOutput::error(format!("view failed: {e}")),
                }
            }
            MemoryCmd::Create { path, text } => {
                let (scope, rel) = match parse_mem_path(&path) {
                    Ok(v) => v,
                    Err(e) => return ToolOutput::error(e),
                };
                if let Err(e) = self.engine.files().create(&scope, &rel, &text).await {
                    return ToolOutput::error(format!("create failed: {e}"));
                }
                self.reindex(&scope, &rel).await;
                ToolOutput::structured(json!({ "created": path }))
            }
            MemoryCmd::StrReplace {
                path,
                old_str,
                new_str,
            } => {
                let (scope, rel) = match parse_mem_path(&path) {
                    Ok(v) => v,
                    Err(e) => return ToolOutput::error(e),
                };
                if let Err(e) = self
                    .engine
                    .files()
                    .str_replace(&scope, &rel, &old_str, &new_str)
                    .await
                {
                    return ToolOutput::error(format!("str_replace failed: {e}"));
                }
                self.reindex(&scope, &rel).await;
                ToolOutput::structured(json!({ "edited": path }))
            }
            MemoryCmd::Insert { path, line, text } => {
                let (scope, rel) = match parse_mem_path(&path) {
                    Ok(v) => v,
                    Err(e) => return ToolOutput::error(e),
                };
                if let Err(e) = self.engine.files().insert(&scope, &rel, line, &text).await {
                    return ToolOutput::error(format!("insert failed: {e}"));
                }
                self.reindex(&scope, &rel).await;
                ToolOutput::structured(json!({ "inserted": path }))
            }
            MemoryCmd::Delete { path } => {
                let (scope, rel) = match parse_mem_path(&path) {
                    Ok(v) => v,
                    Err(e) => return ToolOutput::error(e),
                };
                if let Err(e) = self.engine.files().delete(&scope, &rel).await {
                    return ToolOutput::error(format!("delete failed: {e}"));
                }
                self.reindex(&scope, &rel).await;
                ToolOutput::structured(json!({ "deleted": path }))
            }
            MemoryCmd::Rename { path, new_path } => {
                let (scope, rel) = match parse_mem_path(&path) {
                    Ok(v) => v,
                    Err(e) => return ToolOutput::error(e),
                };
                let (scope2, rel2) = match parse_mem_path(&new_path) {
                    Ok(v) => v,
                    Err(e) => return ToolOutput::error(e),
                };
                if scope != scope2 {
                    return ToolOutput::error("rename across scopes is not allowed");
                }
                if let Err(e) = self.engine.files().rename(&scope, &rel, &rel2).await {
                    return ToolOutput::error(format!("rename failed: {e}"));
                }
                self.reindex(&scope, &rel).await;
                self.reindex(&scope, &rel2).await;
                ToolOutput::structured(json!({ "renamed": new_path }))
            }
        }
    }

    async fn reindex(&self, scope: &Scope, rel: &str) {
        if let Err(e) = self.engine.reindex_file(scope, rel).await {
            tracing::warn!(error = %e, "memory: reindex_file failed");
        }
    }
}

#[async_trait]
impl ToolHandler for MemoryTool {
    async fn call(&self, _ctx: ToolContext, call: ToolCall) -> ToolOutput {
        let cmd: MemoryCmd = match serde_json::from_value(call.arguments) {
            Ok(c) => c,
            Err(e) => return ToolOutput::error(format!("invalid memory command: {e}")),
        };
        self.run(cmd).await
    }
}

#[derive(Debug, Deserialize)]
struct SearchArgs {
    query: String,
    #[serde(default)]
    k: Option<usize>,
}

struct SearchTool {
    engine: Arc<MemoryEngine>,
}

#[async_trait]
impl ToolHandler for SearchTool {
    async fn call(&self, _ctx: ToolContext, call: ToolCall) -> ToolOutput {
        let args: SearchArgs = match serde_json::from_value(call.arguments) {
            Ok(a) => a,
            Err(e) => return ToolOutput::error(format!("invalid search input: {e}")),
        };
        let k = args.k.unwrap_or(DEFAULT_RECALL_K);
        let scopes = [Scope::Owner, Scope::Self_];
        match self.engine.recall(&scopes, &args.query, k).await {
            Ok(hits) => {
                let results: Vec<_> = hits
                    .into_iter()
                    .map(|h| {
                        json!({
                            "text": h.text,
                            "kind": h.kind,
                            "scope": h.scope.as_key(),
                            "source": h.source_ref,
                            "score": h.score,
                        })
                    })
                    .collect();
                ToolOutput::structured(json!({ "results": results }))
            }
            Err(e) => ToolOutput::error(format!("search failed: {e}")),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum FactCmd {
    Assert {
        text: String,
        #[serde(default)]
        scope: Option<String>,
        #[serde(default)]
        subject: Option<String>,
        #[serde(default)]
        importance: Option<f32>,
    },
    Invalidate {
        id: i64,
        #[serde(default)]
        superseded_by: Option<i64>,
    },
    List {
        #[serde(default)]
        scope: Option<String>,
        #[serde(default)]
        subject: Option<String>,
    },
}

struct FactTool {
    engine: Arc<MemoryEngine>,
}

fn scope_from_opt(s: Option<&str>) -> Result<Scope, String> {
    match s {
        None | Some("owner") => Ok(Scope::Owner),
        Some("self") => Ok(Scope::Self_),
        Some(other) => Scope::domain(other).map_err(|e| e.to_string()),
    }
}

#[async_trait]
impl ToolHandler for FactTool {
    async fn call(&self, _ctx: ToolContext, call: ToolCall) -> ToolOutput {
        let cmd: FactCmd = match serde_json::from_value(call.arguments) {
            Ok(c) => c,
            Err(e) => return ToolOutput::error(format!("invalid fact command: {e}")),
        };
        match cmd {
            FactCmd::Assert {
                text,
                scope,
                subject,
                importance,
            } => {
                let scope = match scope_from_opt(scope.as_deref()) {
                    Ok(s) => s,
                    Err(e) => return ToolOutput::error(e),
                };
                if text.trim().is_empty() {
                    return ToolOutput::error("fact text must not be empty");
                }
                let nf = NewFact {
                    scope,
                    subject,
                    text: text.trim().to_string(),
                    origin: FactOrigin::OwnerStated,
                    source_kind: "message".into(),
                    source_ref: "tool".into(),
                    importance: importance.unwrap_or(0.6).clamp(0.0, 1.0),
                };
                match self.engine.assert_fact(&nf).await {
                    Ok(id) => ToolOutput::structured(json!({ "asserted": id })),
                    Err(e) => ToolOutput::error(format!("assert failed: {e}")),
                }
            }
            FactCmd::Invalidate { id, superseded_by } => {
                match self.engine.invalidate_fact(id, superseded_by).await {
                    Ok(()) => ToolOutput::structured(json!({ "invalidated": id })),
                    Err(e) => ToolOutput::error(format!("invalidate failed: {e}")),
                }
            }
            FactCmd::List { scope, subject } => {
                let scope = match scope_from_opt(scope.as_deref()) {
                    Ok(s) => s,
                    Err(e) => return ToolOutput::error(e),
                };
                match self
                    .engine
                    .current_facts(&scope, subject.as_deref(), 50)
                    .await
                {
                    Ok(facts) => {
                        let out: Vec<_> = facts
                            .into_iter()
                            .map(|f| {
                                json!({
                                    "id": f.id,
                                    "text": f.text,
                                    "subject": f.subject,
                                    "origin": f.origin.as_str(),
                                    "stated_at": f.stated_at.to_rfc3339(),
                                })
                            })
                            .collect();
                        ToolOutput::structured(json!({ "facts": out }))
                    }
                    Err(e) => ToolOutput::error(format!("list failed: {e}")),
                }
            }
        }
    }
}

fn spec_memory() -> ToolSpec {
    ToolSpec::new(
        MEMORY,
        "Read and edit your long-term memory files. Paths are \
         /memories/<scope>/<file>, where <scope> is 'owner' (about the user, \
         shared), 'self' (your own memory), or a domain name. Prose lives in \
         markdown files; use the `fact` tool for discrete claims.",
        json!({
            "type": "object",
            "required": ["command", "path"],
            "properties": {
                "command": {
                    "type": "string",
                    "enum": ["view", "create", "str_replace", "insert", "delete", "rename"]
                },
                "path": { "type": "string", "description": "e.g. /memories/owner/core/profile.md" },
                "text": { "type": "string" },
                "old_str": { "type": "string" },
                "new_str": { "type": "string" },
                "line": { "type": "integer" },
                "new_path": { "type": "string" },
                "view_range": { "type": "array", "items": { "type": "integer" } }
            }
        }),
    )
}

fn spec_memory_search() -> ToolSpec {
    ToolSpec::new(
        MEMORY_SEARCH,
        "Search your long-term memory (owner + self) for anything relevant to a \
         query. Returns file excerpts and facts with provenance.",
        json!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": { "type": "string" },
                "k": { "type": "integer", "description": "Max results (default 6)." }
            }
        }),
    )
}

fn spec_fact() -> ToolSpec {
    ToolSpec::new(
        FACT,
        "Record or revise a discrete durable claim (a fact). Assert when the \
         user states something durable; invalidate when it is no longer true \
         (this preserves history rather than deleting).",
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": { "type": "string", "enum": ["assert", "invalidate", "list"] },
                "text": { "type": "string" },
                "scope": { "type": "string", "description": "owner | self | <domain> (default owner)" },
                "subject": { "type": "string", "description": "optional entity, e.g. \"wife\"" },
                "importance": { "type": "number" },
                "id": { "type": "integer" },
                "superseded_by": { "type": "integer" }
            }
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use goat_agent_tool::ToolReadState;
    use goat_store::SqliteStore;
    use goat_types::{ChannelId, ConversationId, InstanceId, ProfileId};
    use std::path::PathBuf;

    async fn setup() -> (Arc<MemoryEngine>, ToolContext, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("goat.db");
        let pool = SqliteStore::open(&path).await.unwrap().pool();
        let engine = Arc::new(
            MemoryEngine::open(pool, dir.path(), None, 180.0)
                .await
                .unwrap(),
        );
        let ctx = ToolContext {
            persona: ProfileId::new(),
            conversation: ConversationId::new(
                ChannelId::new("telegram"),
                InstanceId::new(),
                "chat:1",
            ),
            goat_root: PathBuf::from("/tmp"),
            read_state: ToolReadState::default(),
        };
        (engine, ctx, dir)
    }

    #[tokio::test]
    async fn memory_create_and_search() {
        let (engine, ctx, _d) = setup().await;
        let mt = MemoryTool {
            engine: engine.clone(),
        };
        let out = mt
            .call(
                ctx.clone(),
                ToolCall {
                    call_id: "c".into(),
                    name: MEMORY,
                    arguments: json!({
                        "command": "create",
                        "path": "/memories/owner/core/profile.md",
                        "text": "## Profile\nThe owner is a sailor"
                    }),
                },
            )
            .await;
        assert!(!out.is_error, "{out:?}");

        let st = SearchTool { engine };
        let out = st
            .call(
                ctx,
                ToolCall {
                    call_id: "c".into(),
                    name: MEMORY_SEARCH,
                    arguments: json!({ "query": "sailor" }),
                },
            )
            .await;
        assert!(!out.is_error);
        let results = out.structured_content.unwrap();
        assert!(!results["results"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn fact_assert_list_invalidate() {
        let (engine, ctx, _d) = setup().await;
        let ft = FactTool { engine };
        let out = ft
            .call(
                ctx.clone(),
                ToolCall {
                    call_id: "c".into(),
                    name: FACT,
                    arguments: json!({ "action": "assert", "text": "owner likes tea", "subject": "drink" }),
                },
            )
            .await;
        assert!(!out.is_error, "{out:?}");
        let id = out.structured_content.unwrap()["asserted"]
            .as_i64()
            .unwrap();

        let out = ft
            .call(
                ctx.clone(),
                ToolCall {
                    call_id: "c".into(),
                    name: FACT,
                    arguments: json!({ "action": "list" }),
                },
            )
            .await;
        let facts = out.structured_content.unwrap();
        assert_eq!(facts["facts"].as_array().unwrap().len(), 1);

        let out = ft
            .call(
                ctx,
                ToolCall {
                    call_id: "c".into(),
                    name: FACT,
                    arguments: json!({ "action": "invalidate", "id": id }),
                },
            )
            .await;
        assert!(!out.is_error);
    }

    #[tokio::test]
    async fn rejects_bad_path() {
        let (engine, ctx, _d) = setup().await;
        let mt = MemoryTool { engine };
        let out = mt
            .call(
                ctx,
                ToolCall {
                    call_id: "c".into(),
                    name: MEMORY,
                    arguments: json!({ "command": "view", "path": "/etc/passwd" }),
                },
            )
            .await;
        assert!(out.is_error);
    }
}
