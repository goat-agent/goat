//! Memory tools (`remember`, `recall`) backed by [`goat_memory`].
//!
//! These are stateful (they hold a [`MemoryStore`] handle and a per-persona
//! [`Embedder`] map) and are therefore registered through [`register`] rather
//! than the stateless `inventory` mechanism, mirroring `goat-tool-schedule`.
//!
//! `remember` writes durable core facts and needs no embedder. `recall`
//! performs a vector search over episodic memory and is only useful when the
//! calling persona has an embedder configured; without one it falls back to
//! returning core facts.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use goat_memory::{Embedder, EpisodicKind, MemoryStore};
use goat_tool::{ToolCall, ToolContext, ToolHandler, ToolName, ToolOutput, ToolRegistry, ToolSpec};
use goat_types::PersonaId;
use serde::Deserialize;
use serde_json::json;

pub const REMEMBER: ToolName = ToolName::from_static("remember");
pub const RECALL: ToolName = ToolName::from_static("recall");

/// Per-persona embedder map. The tool registry is shared across all personas,
/// so the embedder cannot be baked into a single handler; it is resolved from
/// this map by `ctx.persona` at call time.
pub type EmbedderMap = Arc<HashMap<PersonaId, Arc<dyn Embedder>>>;

const RECALL_K: usize = 5;
const MIN_RECALL_SCORE: f32 = 0.5;
const SNIPPET_CHARS: usize = 240;

/// Insert the `remember` and `recall` tools, sharing the memory store and the
/// per-persona embedder map.
pub fn register(registry: &mut ToolRegistry, memory: Arc<dyn MemoryStore>, embedders: EmbedderMap) {
    registry.insert_handler(
        spec_remember(),
        Arc::new(RememberTool {
            memory: memory.clone(),
        }),
        true,
    );
    registry.insert_handler(
        spec_recall(),
        Arc::new(RecallTool { memory, embedders }),
        true,
    );
}

// --------------------------------------------------------------------------
// remember
// --------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct RememberArgs {
    slug: String,
    text: String,
}

pub struct RememberTool {
    memory: Arc<dyn MemoryStore>,
}

#[async_trait]
impl ToolHandler for RememberTool {
    async fn call(&self, ctx: ToolContext, call: ToolCall) -> ToolOutput {
        let args: RememberArgs = match serde_json::from_value(call.arguments) {
            Ok(a) => a,
            Err(e) => return ToolOutput::error(format!("invalid remember input: {e}")),
        };
        let slug = args.slug.trim();
        if slug.is_empty() {
            return ToolOutput::error("slug must not be empty");
        }
        if args.text.trim().is_empty() {
            return ToolOutput::error("text must not be empty");
        }
        match self
            .memory
            .upsert_core(ctx.persona, slug, args.text.trim())
            .await
        {
            Ok(()) => ToolOutput::structured(json!({ "remembered": slug })),
            Err(e) => ToolOutput::error(format!("upsert_core failed: {e}")),
        }
    }
}

// --------------------------------------------------------------------------
// recall
// --------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct RecallArgs {
    query: String,
}

pub struct RecallTool {
    memory: Arc<dyn MemoryStore>,
    embedders: EmbedderMap,
}

#[async_trait]
impl ToolHandler for RecallTool {
    async fn call(&self, ctx: ToolContext, call: ToolCall) -> ToolOutput {
        let args: RecallArgs = match serde_json::from_value(call.arguments) {
            Ok(a) => a,
            Err(e) => return ToolOutput::error(format!("invalid recall input: {e}")),
        };

        let core: Vec<serde_json::Value> = match self.memory.core_blocks(ctx.persona).await {
            Ok(blocks) => blocks
                .into_iter()
                .map(|b| json!({ "slug": b.slug, "text": b.text }))
                .collect(),
            Err(e) => return ToolOutput::error(format!("core_blocks failed: {e}")),
        };

        let mut recalled: Vec<serde_json::Value> = Vec::new();
        if let Some(embedder) = self.embedders.get(&ctx.persona) {
            match embedder.embed(&args.query).await {
                Ok(query_vec) => {
                    match self
                        .memory
                        .search_episodic(ctx.persona, &query_vec, RECALL_K)
                        .await
                    {
                        Ok(hits) => {
                            recalled = hits
                                .into_iter()
                                .filter(|e| e.score.unwrap_or(0.0) >= MIN_RECALL_SCORE)
                                .map(|e| {
                                    json!({
                                        "kind": kind_label(e.kind),
                                        "text": snippet(&e.text),
                                        "score": e.score,
                                    })
                                })
                                .collect();
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "recall: search_episodic failed");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "recall: embedding failed");
                }
            }
        }

        ToolOutput::structured(json!({
            "core": core,
            "recalled": recalled,
        }))
    }
}

fn kind_label(kind: EpisodicKind) -> &'static str {
    match kind {
        EpisodicKind::User => "user",
        EpisodicKind::Assistant => "you",
        EpisodicKind::Observation => "note",
    }
}

fn snippet(text: &str) -> String {
    let mut out: String = text.chars().take(SNIPPET_CHARS).collect();
    if text.chars().count() > SNIPPET_CHARS {
        out.push('…');
    }
    out
}

// --------------------------------------------------------------------------
// specs
// --------------------------------------------------------------------------

fn spec_remember() -> ToolSpec {
    ToolSpec::new(
        REMEMBER,
        "Persists a durable fact about the user or context under a stable key. \
         Use for things worth remembering across conversations (preferences, \
         names, ongoing goals). Re-using a slug overwrites that fact.",
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["slug", "text"],
            "properties": {
                "slug": {
                    "type": "string",
                    "description": "Short stable key, e.g. \"timezone\" or \"project_goal\"."
                },
                "text": {
                    "type": "string",
                    "description": "The fact to remember."
                }
            }
        }),
    )
}

fn spec_recall() -> ToolSpec {
    ToolSpec::new(
        RECALL,
        "Searches your long-term memory for context relevant to a query. \
         Returns durable facts plus excerpts from earlier conversations.",
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["query"],
            "properties": {
                "query": {
                    "type": "string",
                    "description": "What you want to remember about."
                }
            }
        }),
    )
}

// --------------------------------------------------------------------------
// tests
// --------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use goat_memory::SqliteMemory;
    use goat_store::{SqliteStore, Store};
    use goat_tool::ToolReadState;
    use goat_types::{ChannelId, ConversationId, InstanceId};
    use std::path::PathBuf;

    async fn setup() -> (Arc<dyn MemoryStore>, ToolContext, PersonaId) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("goat.db");
        std::mem::forget(dir);
        let store = SqliteStore::open(&path).await.unwrap();
        let persona = PersonaId::new();
        store.ensure_persona(persona, "dev", "dev").await.unwrap();
        let conv = ConversationId::new(ChannelId::new("telegram"), InstanceId::new(), "chat:1");
        store.ensure_conversation(&conv, persona).await.unwrap();
        let memory: Arc<dyn MemoryStore> = Arc::new(SqliteMemory::from_pool(store.pool()));
        let ctx = ToolContext {
            persona,
            conversation: conv,
            goat_root: PathBuf::from("/tmp"),
            read_state: ToolReadState::default(),
        };
        (memory, ctx, persona)
    }

    #[tokio::test]
    async fn remember_then_recall_returns_core() {
        let (memory, ctx, _) = setup().await;
        let remember = RememberTool {
            memory: memory.clone(),
        };
        let out = remember
            .call(
                ctx.clone(),
                ToolCall {
                    call_id: "c".into(),
                    name: REMEMBER,
                    arguments: json!({ "slug": "tz", "text": "user is in KST" }),
                },
            )
            .await;
        assert!(!out.is_error, "got error: {out:?}");

        let recall = RecallTool {
            memory,
            embedders: Arc::new(HashMap::new()),
        };
        let out = recall
            .call(
                ctx,
                ToolCall {
                    call_id: "c".into(),
                    name: RECALL,
                    arguments: json!({ "query": "where is the user" }),
                },
            )
            .await;
        assert!(!out.is_error);
        let core = out.structured_content.unwrap();
        let arr = core.get("core").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["slug"], "tz");
    }

    #[tokio::test]
    async fn remember_rejects_empty_slug() {
        let (memory, ctx, _) = setup().await;
        let remember = RememberTool { memory };
        let out = remember
            .call(
                ctx,
                ToolCall {
                    call_id: "c".into(),
                    name: REMEMBER,
                    arguments: json!({ "slug": "  ", "text": "x" }),
                },
            )
            .await;
        assert!(out.is_error);
    }
}
