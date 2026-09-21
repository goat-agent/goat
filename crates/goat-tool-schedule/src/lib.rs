use std::borrow::Cow;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use goat_loop::cron_expr;
use goat_loop::scheduler::SchedulerHandle;
use goat_store::{NewSchedule, ScheduleKind, Store};
use goat_tool::{Tool, ToolCall, ToolContext, ToolFuture, ToolName, ToolOutput, ToolRegistry};
use goat_types::SCHEDULE_FALLBACK_TIMEZONE;
use serde::Deserialize;
use serde_json::json;

pub const SCHEDULE_ONCE: ToolName = ToolName::from_static("schedule_once");
pub const SCHEDULE_CRON: ToolName = ToolName::from_static("schedule_cron");
pub const CANCEL_TASK: ToolName = ToolName::from_static("cancel_task");
pub const LIST_TASKS: ToolName = ToolName::from_static("list_tasks");

const PREVIEW_OCCURRENCES: usize = 5;

pub fn register(registry: &mut ToolRegistry, store: Arc<dyn Store>, scheduler: SchedulerHandle) {
    registry.insert(Arc::new(ScheduleOnceTool {
        store: store.clone(),
        scheduler: scheduler.clone(),
    }));
    registry.insert(Arc::new(ScheduleCronTool {
        store: store.clone(),
        scheduler,
    }));
    registry.insert(Arc::new(CancelTaskTool {
        store: store.clone(),
    }));
    registry.insert(Arc::new(ListTasksTool { store }));
}

#[derive(Debug, Deserialize)]
struct ScheduleOnceArgs {
    due_at: String,
    task: String,
    tools: Vec<String>,
    #[serde(default)]
    timezone: Option<String>,
}

pub struct ScheduleOnceTool {
    store: Arc<dyn Store>,
    scheduler: SchedulerHandle,
}

impl Tool for ScheduleOnceTool {
    fn name(&self) -> ToolName {
        SCHEDULE_ONCE.clone()
    }

    fn description(&self) -> Cow<'static, str> {
        "Schedules a one-shot task to fire once at the given time. The fire opens a fresh \
         autonomous turn with no conversation attached, so write the task as a complete \
         note to your future self."
            .into()
    }

    fn parameters(&self) -> serde_json::Value {
        schedule_once_parameters()
    }

    fn call<'a>(&'a self, call: &'a ToolCall, ctx: ToolContext<'a>) -> ToolFuture<'a> {
        Box::pin(async move {
            let ac = ctx.agent_context()?;
            let args: ScheduleOnceArgs = match serde_json::from_value(call.arguments.clone()) {
                Ok(a) => a,
                Err(e) => {
                    return Ok(ToolOutput::error(format!(
                        "invalid schedule_once input: {e}"
                    )));
                }
            };
            if args.task.trim().is_empty() {
                return Ok(ToolOutput::error("task must not be empty"));
            }
            let due_at = match DateTime::parse_from_rfc3339(&args.due_at) {
                Ok(d) => d.with_timezone(&Utc),
                Err(e) => {
                    return Ok(ToolOutput::error(format!(
                        "invalid due_at (RFC3339 required): {e}"
                    )));
                }
            };
            let (_, timezone_name) = match selected_timezone(args.timezone.as_deref()) {
                Ok(timezone) => timezone,
                Err(error) => return Ok(ToolOutput::error(error)),
            };
            let now = Utc::now();
            if due_at <= now {
                return Ok(ToolOutput::error(format!(
                    "due_at must be in the future (got {} <= now {})",
                    due_at.to_rfc3339(),
                    now.to_rfc3339()
                )));
            }
            let similar = similar_summaries(&*self.store, ac.id, &args.task).await;
            let new = NewSchedule {
                agent: ac.id,
                instruction: args.task.clone(),
                tools: args.tools,
                origin_conv: ac.conversation.clone(),
                schedule: ScheduleKind::Once(due_at),
                timezone: Some(timezone_name.clone()),
                created_by_msg_id: None,
            };
            let schedule_id = match self.store.insert_schedule(new).await {
                Ok(id) => id,
                Err(e) => {
                    return Ok(ToolOutput::error(format!("insert_schedule failed: {e}")));
                }
            };
            if let Err(e) = self
                .store
                .insert_schedule_run(schedule_id, due_at, args.task.clone())
                .await
            {
                return Ok(ToolOutput::error(format!(
                    "insert_schedule_run failed: {e}"
                )));
            }
            self.scheduler.schedule(due_at);
            Ok(ToolOutput::structured(json!({
                "task_id": schedule_id,
                "schedule_kind": "once",
                "due_at": due_at.to_rfc3339(),
                "timezone": timezone_name,
                "similar_existing": similar,
            })))
        })
    }
}

#[derive(Debug, Deserialize)]
struct ScheduleCronArgs {
    cron: String,
    task: String,
    tools: Vec<String>,
    #[serde(default)]
    timezone: Option<String>,
    #[serde(default)]
    first_at: Option<String>,
}

pub struct ScheduleCronTool {
    store: Arc<dyn Store>,
    scheduler: SchedulerHandle,
}

impl Tool for ScheduleCronTool {
    fn name(&self) -> ToolName {
        SCHEDULE_CRON.clone()
    }

    fn description(&self) -> Cow<'static, str> {
        "Schedules a recurring task using a 5-field cron expression. Each fire opens a \
         fresh autonomous turn with no conversation attached, so write the task as a \
         complete note to your future self."
            .into()
    }

    fn parameters(&self) -> serde_json::Value {
        schedule_cron_parameters()
    }

    fn call<'a>(&'a self, call: &'a ToolCall, ctx: ToolContext<'a>) -> ToolFuture<'a> {
        Box::pin(async move {
            let ac = ctx.agent_context()?;
            let args: ScheduleCronArgs = match serde_json::from_value(call.arguments.clone()) {
                Ok(a) => a,
                Err(e) => {
                    return Ok(ToolOutput::error(format!(
                        "invalid schedule_cron input: {e}"
                    )));
                }
            };
            if args.task.trim().is_empty() {
                return Ok(ToolOutput::error("task must not be empty"));
            }
            let schedule = match cron_expr::parse(&args.cron) {
                Ok(schedule) => schedule,
                Err(error) => return Ok(ToolOutput::error(format!("invalid cron: {error}"))),
            };
            let (timezone, timezone_name) = match selected_timezone(args.timezone.as_deref()) {
                Ok(timezone) => timezone,
                Err(error) => return Ok(ToolOutput::error(error)),
            };
            let now = Utc::now();
            let explicit_first = args.first_at.is_some();
            let first_at = if let Some(raw) = args.first_at.as_deref() {
                match DateTime::parse_from_rfc3339(raw) {
                    Ok(date) => date.with_timezone(&Utc),
                    Err(error) => {
                        return Ok(ToolOutput::error(format!("invalid first_at: {error}")));
                    }
                }
            } else {
                match cron_expr::next_after(&schedule, now, timezone) {
                    Some(date) => date,
                    None => {
                        return Ok(ToolOutput::error("cron has no future occurrences"));
                    }
                }
            };
            if first_at <= now {
                return Ok(ToolOutput::error("first_at must be in the future"));
            }
            if explicit_first && !cron_expr::includes(&schedule, first_at, timezone) {
                return Ok(ToolOutput::error(format!(
                    "first_at is not an occurrence of this cron in {timezone_name}"
                )));
            }
            let preview_dates = if explicit_first {
                let mut dates = vec![first_at];
                dates.extend(cron_expr::upcoming(
                    &schedule,
                    first_at,
                    PREVIEW_OCCURRENCES - 1,
                    timezone,
                ));
                dates
            } else {
                cron_expr::upcoming(&schedule, now, PREVIEW_OCCURRENCES, timezone)
            };
            let preview: Vec<String> = preview_dates
                .into_iter()
                .map(|date| date.to_rfc3339())
                .collect();
            let similar = similar_summaries(&*self.store, ac.id, &args.task).await;
            let new = NewSchedule {
                agent: ac.id,
                instruction: args.task.clone(),
                tools: args.tools,
                origin_conv: ac.conversation.clone(),
                schedule: ScheduleKind::Cron(args.cron.clone()),
                timezone: Some(timezone_name.clone()),
                created_by_msg_id: None,
            };
            let schedule_id = match self.store.insert_schedule(new).await {
                Ok(id) => id,
                Err(e) => {
                    return Ok(ToolOutput::error(format!("insert_schedule failed: {e}")));
                }
            };
            if let Err(e) = self
                .store
                .insert_schedule_run(schedule_id, first_at, args.task.clone())
                .await
            {
                return Ok(ToolOutput::error(format!(
                    "insert_schedule_run failed: {e}"
                )));
            }
            self.scheduler.schedule(first_at);
            Ok(ToolOutput::structured(json!({
                "task_id": schedule_id,
                "schedule_kind": "cron",
                "cron": args.cron,
                "timezone": timezone_name,
                "first_at": first_at.to_rfc3339(),
                "preview": preview,
                "similar_existing": similar,
            })))
        })
    }
}

#[derive(Debug, Deserialize)]
struct CancelTaskArgs {
    #[serde(rename = "task_id")]
    schedule_id: i64,
}

pub struct CancelTaskTool {
    store: Arc<dyn Store>,
}

impl Tool for CancelTaskTool {
    fn name(&self) -> ToolName {
        CANCEL_TASK.clone()
    }

    fn description(&self) -> Cow<'static, str> {
        "Cancels an active scheduled task. \
         Call list_tasks first if you don't already know the id."
            .into()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["task_id"],
            "properties": {
                "task_id": {
                    "type": "integer",
                    "description": "Exact task id."
                }
            }
        })
    }

    fn call<'a>(&'a self, call: &'a ToolCall, _ctx: ToolContext<'a>) -> ToolFuture<'a> {
        Box::pin(async move {
            let args: CancelTaskArgs = match serde_json::from_value(call.arguments.clone()) {
                Ok(a) => a,
                Err(e) => {
                    return Ok(ToolOutput::error(format!("invalid cancel_task input: {e}")));
                }
            };
            match self.store.cancel_schedule(args.schedule_id).await {
                Ok(true) => Ok(ToolOutput::structured(
                    json!({"cancelled": [args.schedule_id]}),
                )),
                Ok(false) => Ok(ToolOutput::error(format!(
                    "no active task with id {}",
                    args.schedule_id
                ))),
                Err(e) => Ok(ToolOutput::error(format!("cancel failed: {e}"))),
            }
        })
    }
}

pub struct ListTasksTool {
    store: Arc<dyn Store>,
}

impl Tool for ListTasksTool {
    fn name(&self) -> ToolName {
        LIST_TASKS.clone()
    }

    fn description(&self) -> Cow<'static, str> {
        "Lists all active scheduled tasks for this agent.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {}
        })
    }

    fn call<'a>(&'a self, _call: &'a ToolCall, ctx: ToolContext<'a>) -> ToolFuture<'a> {
        Box::pin(async move {
            let ac = ctx.agent_context()?;
            match self.store.list_active_schedules(ac.id).await {
                Ok(rows) => {
                    let entries: Vec<_> = rows
                        .into_iter()
                        .map(|(schedule, next_at)| {
                            let (kind, schedule_summary) = match &schedule.schedule {
                                ScheduleKind::Once(at) => ("once", at.to_rfc3339()),
                                ScheduleKind::Cron(expr) => ("cron", expr.clone()),
                            };
                            json!({
                                "id": schedule.id,
                                "kind": kind,
                                "task": schedule.instruction,
                                "schedule": schedule_summary,
                                "timezone": schedule.timezone.as_deref().unwrap_or("host-local"),
                                "next_at": next_at.map(|d| d.to_rfc3339()),
                                "tools": schedule.tools,
                            })
                        })
                        .collect();
                    Ok(ToolOutput::structured(json!({"tasks": entries})))
                }
                Err(e) => Ok(ToolOutput::error(format!(
                    "list_active_schedules failed: {e}"
                ))),
            }
        })
    }
}

fn schedule_once_parameters() -> serde_json::Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["due_at", "task", "tools"],
        "properties": {
            "due_at": {
                "type": "string",
                "description": "When the task fires (RFC 3339)."
            },
            "timezone": {
                "type": "string",
                "default": "UTC",
                "description": "IANA timezone recording the owner's time context. Defaults to the agent's configured timezone, or UTC when none is configured."
            },
            "task": {
                "type": "string",
                "description": "What you will do at that fire moment."
            },
            "tools": {
                "type": "array",
                "items": {"type": "string"},
                "description": "Tool selectors you may call at fire time. Use [\"*\"] for all agent-allowed non-schedule tools, [] for no tools, or names/negations such as [\"read\", \"grep\"] or [\"*\", \"!shell\"]."
            }
        }
    })
}

fn schedule_cron_parameters() -> serde_json::Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["cron", "task", "tools"],
        "properties": {
            "cron": {
                "type": "string",
                "description": "5-field cron evaluated in timezone: minute hour day month day-of-week (day-of-week 0=Sun..6=Sat)."
            },
            "timezone": {
                "type": "string",
                "default": "UTC",
                "description": "IANA timezone for every cron occurrence. Defaults to the agent's configured timezone, or UTC when none is configured."
            },
            "task": {
                "type": "string",
                "description": "What you will do at each fire moment."
            },
            "tools": {
                "type": "array",
                "items": {"type": "string"},
                "description": "Tool selectors you may call at fire time. Use [\"*\"] for all agent-allowed non-schedule tools, [] for no tools, or names/negations such as [\"read\", \"grep\"] or [\"*\", \"!shell\"]."
            },
            "first_at": {
                "type": "string",
                "description": "Optional RFC 3339 first occurrence. It must match the cron in timezone."
            }
        }
    })
}

fn selected_timezone(requested: Option<&str>) -> Result<(cron_expr::CronTimezone, String), String> {
    let requested = requested.unwrap_or(SCHEDULE_FALLBACK_TIMEZONE);
    let timezone = cron_expr::parse_timezone(Some(requested)).map_err(|error| error.to_string())?;
    let cron_expr::CronTimezone::Named(named) = timezone else {
        return Err("timezone must be an IANA timezone".to_string());
    };
    Ok((timezone, named.to_string()))
}

async fn similar_summaries(
    store: &dyn Store,
    agent: goat_types::AgentId,
    incoming: &str,
) -> Vec<serde_json::Value> {
    const NEEDLE_CHARS: usize = 30;
    let needle: String = incoming.chars().take(NEEDLE_CHARS).collect();
    let needle = needle.trim();
    if needle.is_empty() {
        return Vec::new();
    }
    match store.similar_active_schedules(agent, needle).await {
        Ok(rows) => rows
            .into_iter()
            .take(5)
            .map(|t| {
                json!({
                    "id": t.id,
                    "task": t.instruction,
                })
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use goat_loop::scheduler::SchedulerHandle;
    use goat_store::SqliteStore;
    use goat_tool::{AgentContext, ToolReadState, ToolSandbox};
    use goat_types::{AgentId, ChannelId, ConversationId, InstanceId};
    use tokio_util::sync::CancellationToken;

    async fn setup() -> (
        Arc<dyn Store>,
        ToolSandbox,
        CancellationToken,
        AgentContext,
        AgentId,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let root = dir.path().to_path_buf();
        std::mem::forget(dir);
        let store = Arc::new(SqliteStore::open(&path).await.unwrap()) as Arc<dyn Store>;
        let agent_id = AgentId::new();
        store.ensure_agent(agent_id, "dev", "dev").await.unwrap();
        let conv = ConversationId::new(ChannelId::new("discord"), InstanceId::new(), "chat:1");
        store.ensure_conversation(&conv, agent_id).await.unwrap();
        let sandbox = ToolSandbox::rooted(&root).unwrap();
        let token = CancellationToken::new();
        let agent = AgentContext {
            id: agent_id,
            slug: "dev".into(),
            conversation: conv,
            audience: None,
            read_state: ToolReadState::default(),
        };
        (store, sandbox, token, agent, agent_id)
    }

    fn call_once(due_at: &str, text: &str) -> ToolCall {
        ToolCall {
            call_id: "c".into(),
            name: SCHEDULE_ONCE,
            arguments: json!({
                "due_at": due_at,
                "task": text,
                "tools": ["shell"],
            }),
        }
    }

    #[tokio::test]
    async fn schedule_once_rejects_past_due() {
        let (store, sandbox, token, agent, _) = setup().await;
        let ctx = ToolContext::agent(&sandbox, &token, &agent);
        let tool = ScheduleOnceTool {
            store,
            scheduler: SchedulerHandle::detached(),
        };
        let past = (Utc::now() - Duration::hours(1)).to_rfc3339();
        let out = tool.call(&call_once(&past, "ping"), ctx).await.unwrap();
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn schedule_once_accepts_future_due() {
        let (store, sandbox, token, agent, agent_id) = setup().await;
        let ctx = ToolContext::agent(&sandbox, &token, &agent);
        let tool = ScheduleOnceTool {
            store: store.clone(),
            scheduler: SchedulerHandle::detached(),
        };
        let future = (Utc::now() + Duration::minutes(10)).to_rfc3339();
        let out = tool
            .call(&call_once(&future, "ping example.com from staging"), ctx)
            .await
            .unwrap();
        assert!(!out.is_error, "got error: {}", out.text_for_model());
        let active = store.list_active_schedules(agent_id).await.unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(
            active[0].0.timezone.as_deref(),
            Some(SCHEDULE_FALLBACK_TIMEZONE)
        );
    }

    #[tokio::test]
    async fn schedule_once_rejects_empty_task() {
        let (store, sandbox, token, agent, _) = setup().await;
        let ctx = ToolContext::agent(&sandbox, &token, &agent);
        let tool = ScheduleOnceTool {
            store,
            scheduler: SchedulerHandle::detached(),
        };
        let future = (Utc::now() + Duration::minutes(10)).to_rfc3339();
        let out = tool.call(&call_once(&future, "   "), ctx).await.unwrap();
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn schedule_cron_rejects_invalid_expr() {
        let (store, sandbox, token, agent, _) = setup().await;
        let ctx = ToolContext::agent(&sandbox, &token, &agent);
        let tool = ScheduleCronTool {
            store,
            scheduler: SchedulerHandle::detached(),
        };
        let out = tool
            .call(
                &ToolCall {
                    call_id: "c".into(),
                    name: SCHEDULE_CRON,
                    arguments: json!({
                        "cron": "99 * * * *",
                        "task": "weekly task",
                        "tools": [],
                    }),
                },
                ctx,
            )
            .await
            .unwrap();
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn schedule_cron_includes_preview() {
        let (store, sandbox, token, agent, _) = setup().await;
        let ctx = ToolContext::agent(&sandbox, &token, &agent);
        let tool = ScheduleCronTool {
            store,
            scheduler: SchedulerHandle::detached(),
        };
        let out = tool
            .call(
                &ToolCall {
                    call_id: "c".into(),
                    name: SCHEDULE_CRON,
                    arguments: json!({
                        "cron": "0 7 * * 1",
                        "task": "weekly summary",
                        "tools": ["read", "grep"],
                    }),
                },
                ctx,
            )
            .await
            .unwrap();
        assert!(!out.is_error, "got error: {}", out.text_for_model());
        let preview = out
            .structured_content
            .as_ref()
            .unwrap()
            .get("preview")
            .and_then(|v| v.as_array())
            .expect("preview must be present");
        assert_eq!(preview.len(), PREVIEW_OCCURRENCES);
    }

    #[tokio::test]
    async fn schedule_cron_rejects_disagreeing_first_at() {
        let (store, sandbox, token, agent, _) = setup().await;
        let ctx = ToolContext::agent(&sandbox, &token, &agent);
        let tool = ScheduleCronTool {
            store,
            scheduler: SchedulerHandle::detached(),
        };
        let out = tool
            .call(
                &ToolCall {
                    call_id: "c".into(),
                    name: SCHEDULE_CRON,
                    arguments: json!({
                        "cron": "0 9 * * *",
                        "timezone": "Asia/Seoul",
                        "first_at": "2099-01-01T09:00:00Z",
                        "task": "daily summary",
                        "tools": [],
                    }),
                },
                ctx,
            )
            .await
            .unwrap();
        assert!(out.is_error);
        assert!(out.text_for_model().contains("not an occurrence"));
    }

    #[test]
    fn schedule_schemas_expose_utc_fallback() {
        for parameters in [schedule_once_parameters(), schedule_cron_parameters()] {
            assert_eq!(
                parameters["properties"]["timezone"]["default"],
                SCHEDULE_FALLBACK_TIMEZONE
            );
        }
    }

    #[tokio::test]
    async fn cancel_by_id_succeeds() {
        let (store, sandbox, token, agent, agent_id) = setup().await;
        let ctx = ToolContext::agent(&sandbox, &token, &agent);
        let once = ScheduleOnceTool {
            store: store.clone(),
            scheduler: SchedulerHandle::detached(),
        };
        let future = (Utc::now() + Duration::minutes(10)).to_rfc3339();
        once.call(&call_once(&future, "loadtest staging"), ctx)
            .await
            .unwrap();
        let active_before = store.list_active_schedules(agent_id).await.unwrap();
        let schedule_id = active_before[0].0.id;

        let cancel = CancelTaskTool {
            store: store.clone(),
        };
        let out = cancel
            .call(
                &ToolCall {
                    call_id: "c".into(),
                    name: CANCEL_TASK,
                    arguments: json!({"task_id": schedule_id}),
                },
                ctx,
            )
            .await
            .unwrap();
        assert!(!out.is_error, "got error: {}", out.text_for_model());
        let active = store.list_active_schedules(agent_id).await.unwrap();
        assert!(active.is_empty());
    }

    #[tokio::test]
    async fn cancel_requires_task_id() {
        let (store, sandbox, token, agent, _) = setup().await;
        let ctx = ToolContext::agent(&sandbox, &token, &agent);
        let cancel = CancelTaskTool { store };
        let out = cancel
            .call(
                &ToolCall {
                    call_id: "c".into(),
                    name: CANCEL_TASK,
                    arguments: json!({}),
                },
                ctx,
            )
            .await
            .unwrap();
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn list_tasks_returns_active() {
        let (store, sandbox, token, agent, _) = setup().await;
        let ctx = ToolContext::agent(&sandbox, &token, &agent);
        let once = ScheduleOnceTool {
            store: store.clone(),
            scheduler: SchedulerHandle::detached(),
        };
        let future = (Utc::now() + Duration::minutes(10)).to_rfc3339();
        once.call(&call_once(&future, "do thing"), ctx)
            .await
            .unwrap();

        let list = ListTasksTool {
            store: store.clone(),
        };
        let out = list
            .call(
                &ToolCall {
                    call_id: "c".into(),
                    name: LIST_TASKS,
                    arguments: json!({}),
                },
                ctx,
            )
            .await
            .unwrap();
        assert!(!out.is_error);
        let tasks = out
            .structured_content
            .unwrap()
            .get("tasks")
            .unwrap()
            .as_array()
            .unwrap()
            .len();
        assert_eq!(tasks, 1);
    }
}
