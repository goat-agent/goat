use std::sync::Arc;

use async_trait::async_trait;
use goat_agent_tool::{
    ToolCall, ToolCaller, ToolHandler, ToolName, ToolOutput, ToolRegistry, ToolSpec,
};
use goat_store::{ObservationRecord, Store};
use serde::Deserialize;
use serde_json::{Value, json};

pub const OBSERVATION: ToolName = ToolName::from_static("observation");

const DEFAULT_LIMIT: i64 = 5;
const MAX_LIMIT: i64 = 50;

pub fn register(registry: &mut ToolRegistry, store: Arc<dyn Store>) {
    registry.insert_handler(spec(), Arc::new(ObservationTool { store }), true);
}

fn spec() -> ToolSpec {
    ToolSpec::new(
        OBSERVATION,
        "Read what an integration watcher actually saw. A watcher polls an integration \
         bound to this agent and records each sighting losslessly as an observation. \
         Integration briefings cite an observation reference such as `observation:42`; \
         pass that id here to get the raw payload back. Pass an external_ref instead to \
         get the recorded history for one item, newest first.",
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "integer",
                    "description": "Observation id, as cited in a briefing (`observation:<id>`)."
                },
                "external_ref": {
                    "type": "string",
                    "description": "Stable item reference, e.g. `linear/default:issue:US-1`."
                },
                "integration": {
                    "type": "string",
                    "description": "Integration id. Required with external_ref, e.g. `linear`."
                },
                "limit": {
                    "type": "integer",
                    "description": "How many records to return for an external_ref lookup."
                }
            }
        }),
    )
}

#[derive(Debug, Deserialize)]
struct Args {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    external_ref: Option<String>,
    #[serde(default)]
    integration: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

struct ObservationTool {
    store: Arc<dyn Store>,
}

#[async_trait]
impl ToolHandler for ObservationTool {
    async fn call(&self, ctx: ToolCaller, call: ToolCall) -> ToolOutput {
        let args: Args = match serde_json::from_value(call.arguments) {
            Ok(args) => args,
            Err(e) => return ToolOutput::error(format!("invalid observation arguments: {e}")),
        };

        if let Some(id) = args.id {
            return match self.store.get_observation(id).await {
                Ok(Some(record)) if record.agent == ctx.agent => {
                    ToolOutput::structured(render(&record))
                }
                Ok(_) => ToolOutput::error(format!("no observation {id} for this agent")),
                Err(e) => ToolOutput::error(format!("failed to read observation {id}: {e}")),
            };
        }

        let Some(external_ref) = args.external_ref else {
            return ToolOutput::error(
                "pass either `id` (from a briefing's observation reference) or `external_ref` \
                 together with `integration`"
                    .to_owned(),
            );
        };
        let Some(integration) = args.integration.or_else(|| integration_of(&external_ref)) else {
            return ToolOutput::error(
                "`integration` is required when looking up by external_ref".to_owned(),
            );
        };
        let limit = args.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        match self
            .store
            .observations_by_ref(ctx.agent, &integration, &external_ref, limit)
            .await
        {
            Ok(records) => ToolOutput::structured(json!({
                "external_ref": external_ref,
                "count": records.len(),
                "observations": records.iter().map(render).collect::<Vec<_>>(),
            })),
            Err(e) => ToolOutput::error(format!("failed to read observations: {e}")),
        }
    }
}

fn integration_of(external_ref: &str) -> Option<String> {
    let prefix = external_ref.split('/').next()?;
    if prefix.is_empty() || prefix == external_ref {
        None
    } else {
        Some(prefix.to_owned())
    }
}

fn render(record: &ObservationRecord) -> Value {
    json!({
        "id": record.id,
        "integration": record.integration,
        "account": record.account,
        "external_ref": record.external_ref,
        "kind": record.kind,
        "observed_at": record.observed_at.to_rfc3339(),
        "payload": record.payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use goat_store::{NewObservation, SqliteStore};
    use goat_types::{AgentId, ChannelId, ConversationId, InstanceId};

    async fn store_with(dir: &std::path::Path) -> (Arc<dyn Store>, AgentId) {
        let store = SqliteStore::open(&dir.join("goat.db")).await.unwrap();
        let agent = AgentId::from_slug("test");
        store.ensure_agent(agent, "test", "test").await.unwrap();
        (Arc::new(store), agent)
    }

    fn call(arguments: Value) -> ToolCall {
        ToolCall {
            call_id: "1".to_owned(),
            name: OBSERVATION,
            arguments,
        }
    }

    fn ctx(agent: AgentId) -> ToolCaller {
        ToolCaller {
            agent,
            conversation: ConversationId::new(
                ChannelId::from_static("test"),
                InstanceId::default(),
                "t",
            ),
            audience: None,
            goat_root: std::path::PathBuf::from("/tmp/goat-observation-test"),
            read_state: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }

    #[test]
    fn the_integration_is_inferred_from_a_well_formed_ref() {
        assert_eq!(
            integration_of("linear/default:issue:US-1").as_deref(),
            Some("linear")
        );
        assert_eq!(integration_of("nonsense"), None);
        assert_eq!(integration_of("/leading"), None);
    }

    #[tokio::test]
    async fn a_cited_observation_id_resolves_to_its_payload() {
        let dir = tempfile::tempdir().unwrap();
        let (store, agent) = store_with(dir.path()).await;
        let id = store
            .record_observation(NewObservation {
                agent,
                integration: "linear".to_owned(),
                account: "default".to_owned(),
                external_ref: "linear/default:issue:US-1".to_owned(),
                kind: "assigned".to_owned(),
                payload: json!({ "title": "hammer the api" }),
            })
            .await
            .unwrap();

        let tool = ObservationTool {
            store: store.clone(),
        };
        let out = tool.call(ctx(agent), call(json!({ "id": id }))).await;
        let value = out.structured_content.expect("structured output");
        assert_eq!(value["payload"]["title"], "hammer the api");
        assert_eq!(value["external_ref"], "linear/default:issue:US-1");
    }

    #[tokio::test]
    async fn another_agents_observation_is_not_readable() {
        let dir = tempfile::tempdir().unwrap();
        let (store, agent) = store_with(dir.path()).await;
        let other = AgentId::from_slug("other");
        store.ensure_agent(other, "other", "other").await.unwrap();
        let id = store
            .record_observation(NewObservation {
                agent,
                integration: "linear".to_owned(),
                account: "default".to_owned(),
                external_ref: "linear/default:issue:US-1".to_owned(),
                kind: "assigned".to_owned(),
                payload: json!({}),
            })
            .await
            .unwrap();

        let tool = ObservationTool { store };
        let out = tool.call(ctx(other), call(json!({ "id": id }))).await;
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn an_external_ref_returns_its_history_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let (store, agent) = store_with(dir.path()).await;
        for n in 0..3 {
            store
                .record_observation(NewObservation {
                    agent,
                    integration: "sentry".to_owned(),
                    account: "default".to_owned(),
                    external_ref: "sentry/default:issue:E-1".to_owned(),
                    kind: "updated".to_owned(),
                    payload: json!({ "seen": n }),
                })
                .await
                .unwrap();
        }

        let tool = ObservationTool { store };
        let out = tool
            .call(
                ctx(agent),
                call(json!({ "external_ref": "sentry/default:issue:E-1" })),
            )
            .await;
        let value = out.structured_content.expect("structured output");
        assert_eq!(value["count"], 3);
        assert_eq!(value["observations"][0]["payload"]["seen"], 2);
    }

    #[tokio::test]
    async fn a_lookup_without_id_or_ref_explains_itself() {
        let dir = tempfile::tempdir().unwrap();
        let (store, agent) = store_with(dir.path()).await;
        let tool = ObservationTool { store };
        let out = tool.call(ctx(agent), call(json!({}))).await;
        assert!(out.is_error);
    }
}
