use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use goat_agent_tool::{
    ToolCall, ToolCaller, ToolHandler, ToolName, ToolOutput, ToolRegistry, ToolSpec,
};
use goat_store::{GoalOrigin, GoalStatus, NewGoal, Store};
use serde::Deserialize;
use serde_json::json;

pub const GOAL: ToolName = ToolName::from_static("goal");

pub fn register(registry: &mut ToolRegistry, store: Arc<dyn Store>) {
    registry.insert_handler(spec(), Arc::new(GoalTool { store }), true);
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum GoalCmd {
    Create {
        title: String,
        #[serde(default)]
        detail: Option<String>,
        #[serde(default)]
        priority: Option<i64>,
        #[serde(default)]
        self_formed: bool,
        #[serde(default)]
        review_in_days: Option<i64>,
    },
    Update {
        id: i64,
        status: String,
    },
    Review {
        id: i64,
        #[serde(default)]
        next_in_days: Option<i64>,
    },
    List,
}

struct GoalTool {
    store: Arc<dyn Store>,
}

impl GoalTool {
    async fn run(&self, ctx: &ToolCaller, cmd: GoalCmd) -> ToolOutput {
        match cmd {
            GoalCmd::Create {
                title,
                detail,
                priority,
                self_formed,
                review_in_days,
            } => {
                if title.trim().is_empty() {
                    return ToolOutput::error("goal title must not be empty");
                }
                let new = NewGoal {
                    agent: ctx.agent,
                    title: title.trim().to_string(),
                    detail,
                    priority: priority.unwrap_or(3).clamp(1, 5),
                    origin: if self_formed {
                        GoalOrigin::SelfFormed
                    } else {
                        GoalOrigin::Owner
                    },
                    origin_conv: Some(ctx.conversation.clone()),
                    next_review_at: review_in_days.map(review_at),
                };
                match self.store.create_goal(new).await {
                    Ok(id) => ToolOutput::structured(json!({ "created": id })),
                    Err(e) => ToolOutput::error(format!("create_goal failed: {e}")),
                }
            }
            GoalCmd::Update { id, status } => {
                let Some(status) = parse_status(&status) else {
                    return ToolOutput::error(format!("invalid status: {status:?}"));
                };
                match self.store.update_goal_status(id, status).await {
                    Ok(()) => ToolOutput::structured(json!({ "updated": id })),
                    Err(e) => ToolOutput::error(format!("update_goal_status failed: {e}")),
                }
            }
            GoalCmd::Review { id, next_in_days } => {
                let next = next_in_days.map(review_at);
                match self.store.set_goal_review(id, next).await {
                    Ok(()) => ToolOutput::structured(json!({ "reviewed": id })),
                    Err(e) => ToolOutput::error(format!("set_goal_review failed: {e}")),
                }
            }
            GoalCmd::List => match self.store.active_goals(ctx.agent).await {
                Ok(goals) => {
                    let out: Vec<_> = goals
                        .into_iter()
                        .map(|g| {
                            json!({
                                "id": g.id,
                                "title": g.title,
                                "detail": g.detail,
                                "priority": g.priority,
                                "origin": g.origin.as_str(),
                                "next_review_at": g.next_review_at.map(|t| t.to_rfc3339()),
                            })
                        })
                        .collect();
                    ToolOutput::structured(json!({ "goals": out }))
                }
                Err(e) => ToolOutput::error(format!("active_goals failed: {e}")),
            },
        }
    }
}

fn review_at(days: i64) -> DateTime<Utc> {
    Utc::now() + Duration::days(days.max(0))
}

fn parse_status(s: &str) -> Option<GoalStatus> {
    match s {
        "active" => Some(GoalStatus::Active),
        "blocked" => Some(GoalStatus::Blocked),
        "waiting" => Some(GoalStatus::Waiting),
        "done" => Some(GoalStatus::Done),
        "dropped" => Some(GoalStatus::Dropped),
        _ => None,
    }
}

#[async_trait]
impl ToolHandler for GoalTool {
    async fn call(&self, ctx: ToolCaller, call: ToolCall) -> ToolOutput {
        let cmd: GoalCmd = match serde_json::from_value(call.arguments) {
            Ok(c) => c,
            Err(e) => return ToolOutput::error(format!("invalid goal command: {e}")),
        };
        self.run(&ctx, cmd).await
    }
}

fn spec() -> ToolSpec {
    ToolSpec::new(
        GOAL,
        "Manage your persistent goals (intentions you work toward across \
         conversations). Create a goal when the owner asks for something ongoing \
         or when you decide to pursue something yourself; update its status as \
         it progresses; set a review time to revisit it later; list your active \
         goals.",
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": { "type": "string", "enum": ["create", "update", "review", "list"] },
                "title": { "type": "string" },
                "detail": { "type": "string", "description": "Body / acceptance criteria." },
                "priority": { "type": "integer", "description": "1 (highest) .. 5 (lowest)." },
                "self_formed": { "type": "boolean", "description": "You formed this goal yourself." },
                "review_in_days": { "type": "integer" },
                "id": { "type": "integer" },
                "status": { "type": "string", "enum": ["active","blocked","waiting","done","dropped"] },
                "next_in_days": { "type": "integer" }
            }
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use goat_agent_tool::ToolReadState;
    use goat_store::SqliteStore;
    use goat_types::{AgentId, ChannelId, ConversationId, InstanceId};
    use std::path::PathBuf;

    async fn setup() -> (Arc<dyn Store>, ToolCaller) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("goat.db");
        std::mem::forget(dir);
        let store = SqliteStore::open(&path).await.unwrap();
        let agent = AgentId::new();
        store.ensure_agent(agent, "dev", "dev").await.unwrap();
        let conv = ConversationId::new(ChannelId::new("discord"), InstanceId::new(), "chat:1");
        store.ensure_conversation(&conv, agent).await.unwrap();
        let store: Arc<dyn Store> = Arc::new(store);
        let ctx = ToolCaller {
            agent,
            conversation: conv,
            audience: None,
            goat_root: PathBuf::from("/tmp"),
            read_state: ToolReadState::default(),
        };
        (store, ctx)
    }

    #[tokio::test]
    async fn create_list_update_flow() {
        let (store, ctx) = setup().await;
        let tool = GoalTool { store };

        let out = tool
            .run(
                &ctx,
                GoalCmd::Create {
                    title: "keep deps patched".into(),
                    detail: None,
                    priority: Some(2),
                    self_formed: false,
                    review_in_days: Some(7),
                },
            )
            .await;
        assert!(!out.is_error, "{out:?}");
        let id = out.structured_content.unwrap()["created"].as_i64().unwrap();

        let out = tool.run(&ctx, GoalCmd::List).await;
        let goals = out.structured_content.unwrap();
        assert_eq!(goals["goals"].as_array().unwrap().len(), 1);

        let out = tool
            .run(
                &ctx,
                GoalCmd::Update {
                    id,
                    status: "done".into(),
                },
            )
            .await;
        assert!(!out.is_error);

        let out = tool.run(&ctx, GoalCmd::List).await;
        assert!(
            out.structured_content.unwrap()["goals"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn rejects_empty_title_and_bad_status() {
        let (store, ctx) = setup().await;
        let tool = GoalTool { store };
        assert!(
            tool.run(
                &ctx,
                GoalCmd::Create {
                    title: "  ".into(),
                    detail: None,
                    priority: None,
                    self_formed: true,
                    review_in_days: None
                }
            )
            .await
            .is_error
        );
        assert!(
            tool.run(
                &ctx,
                GoalCmd::Update {
                    id: 1,
                    status: "bogus".into()
                }
            )
            .await
            .is_error
        );
    }
}
