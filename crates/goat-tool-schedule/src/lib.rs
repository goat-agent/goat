//! Schedule-related tools (`schedule_once`, `schedule_cron`, `cancel_task`,
//! `list_tasks`) for goat's scheduled-task self-tick.
//!
//! These tools are stateful (they need a [`Store`] handle) and are therefore
//! registered through [`register`] rather than the stateless `inventory`
//! mechanism used by the other tool crates.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use goat_loop::cron_expr;
use goat_loop::scheduler::SchedulerHandle;
use goat_store::{NewScheduledTask, ScheduleKind, Store};
use goat_tool::{ToolCall, ToolContext, ToolHandler, ToolName, ToolOutput, ToolRegistry, ToolSpec};
use serde::Deserialize;
use serde_json::json;

pub const SCHEDULE_ONCE: ToolName = ToolName::from_static("schedule_once");
pub const SCHEDULE_CRON: ToolName = ToolName::from_static("schedule_cron");
pub const CANCEL_TASK: ToolName = ToolName::from_static("cancel_task");
pub const LIST_TASKS: ToolName = ToolName::from_static("list_tasks");

const PREVIEW_OCCURRENCES: usize = 5;

/// Insert the four scheduling tools into the registry, all sharing the
/// same store handle and scheduler handle. Call this once from the
/// runtime after building the inventory-backed registry and the
/// scheduler.
pub fn register(registry: &mut ToolRegistry, store: Arc<dyn Store>, scheduler: SchedulerHandle) {
    registry.insert_handler(
        spec_schedule_once(),
        Arc::new(ScheduleOnceTool {
            store: store.clone(),
            scheduler: scheduler.clone(),
        }),
        true,
    );
    registry.insert_handler(
        spec_schedule_cron(),
        Arc::new(ScheduleCronTool {
            store: store.clone(),
            scheduler: scheduler.clone(),
        }),
        true,
    );
    registry.insert_handler(
        spec_cancel_task(),
        Arc::new(CancelTaskTool {
            store: store.clone(),
        }),
        true,
    );
    registry.insert_handler(spec_list_tasks(), Arc::new(ListTasksTool { store }), true);
}

// --------------------------------------------------------------------------
// schedule_once
// --------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ScheduleOnceArgs {
    due_at: String,
    task: String,
    tools: Vec<String>,
}

pub struct ScheduleOnceTool {
    store: Arc<dyn Store>,
    scheduler: SchedulerHandle,
}

#[async_trait]
impl ToolHandler for ScheduleOnceTool {
    async fn call(&self, ctx: ToolContext, call: ToolCall) -> ToolOutput {
        let args: ScheduleOnceArgs = match serde_json::from_value(call.arguments) {
            Ok(a) => a,
            Err(e) => return ToolOutput::error(format!("invalid schedule_once input: {e}")),
        };
        if args.task.trim().is_empty() {
            return ToolOutput::error("task must not be empty");
        }
        let due_at = match DateTime::parse_from_rfc3339(&args.due_at) {
            Ok(d) => d.with_timezone(&Utc),
            Err(e) => return ToolOutput::error(format!("invalid due_at (RFC3339 required): {e}")),
        };
        let now = Utc::now();
        if due_at <= now {
            return ToolOutput::error(format!(
                "due_at must be in the future (got {} <= now {})",
                due_at.to_rfc3339(),
                now.to_rfc3339()
            ));
        }
        let similar = similar_summaries(&*self.store, ctx.persona, &args.task).await;
        let new = NewScheduledTask {
            persona: ctx.persona,
            task: args.task.clone(),
            tools: args.tools,
            origin_conv: ctx.conversation,
            schedule: ScheduleKind::Once(due_at),
            created_by_msg_id: None,
        };
        let task_id = match self.store.insert_scheduled_task(new).await {
            Ok(id) => id,
            Err(e) => return ToolOutput::error(format!("insert_scheduled_task failed: {e}")),
        };
        if let Err(e) = self
            .store
            .insert_task_run(task_id, due_at, args.task.clone())
            .await
        {
            return ToolOutput::error(format!("insert_task_run failed: {e}"));
        }
        self.scheduler.schedule(due_at);
        ToolOutput::structured(json!({
            "task_id": task_id,
            "schedule_kind": "once",
            "due_at": due_at.to_rfc3339(),
            "similar_existing": similar,
        }))
    }
}

// --------------------------------------------------------------------------
// schedule_cron
// --------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ScheduleCronArgs {
    cron: String,
    task: String,
    tools: Vec<String>,
    #[serde(default)]
    first_at: Option<String>,
}

pub struct ScheduleCronTool {
    store: Arc<dyn Store>,
    scheduler: SchedulerHandle,
}

#[async_trait]
impl ToolHandler for ScheduleCronTool {
    async fn call(&self, ctx: ToolContext, call: ToolCall) -> ToolOutput {
        let args: ScheduleCronArgs = match serde_json::from_value(call.arguments) {
            Ok(a) => a,
            Err(e) => return ToolOutput::error(format!("invalid schedule_cron input: {e}")),
        };
        if args.task.trim().is_empty() {
            return ToolOutput::error("task must not be empty");
        }
        let schedule = match cron_expr::parse(&args.cron) {
            Ok(s) => s,
            Err(e) => return ToolOutput::error(format!("invalid cron: {e}")),
        };
        let now = Utc::now();
        let first_at = if let Some(raw) = args.first_at.as_deref() {
            match DateTime::parse_from_rfc3339(raw) {
                Ok(d) => d.with_timezone(&Utc),
                Err(e) => return ToolOutput::error(format!("invalid first_at: {e}")),
            }
        } else {
            match cron_expr::next_after(&schedule, now) {
                Some(d) => d,
                None => return ToolOutput::error("cron has no future occurrences"),
            }
        };
        if first_at <= now {
            return ToolOutput::error("first_at must be in the future");
        }
        let preview: Vec<String> = cron_expr::upcoming(&schedule, now, PREVIEW_OCCURRENCES)
            .into_iter()
            .map(|d| d.to_rfc3339())
            .collect();
        let similar = similar_summaries(&*self.store, ctx.persona, &args.task).await;
        let new = NewScheduledTask {
            persona: ctx.persona,
            task: args.task.clone(),
            tools: args.tools,
            origin_conv: ctx.conversation,
            schedule: ScheduleKind::Cron(args.cron.clone()),
            created_by_msg_id: None,
        };
        let task_id = match self.store.insert_scheduled_task(new).await {
            Ok(id) => id,
            Err(e) => return ToolOutput::error(format!("insert_scheduled_task failed: {e}")),
        };
        if let Err(e) = self
            .store
            .insert_task_run(task_id, first_at, args.task.clone())
            .await
        {
            return ToolOutput::error(format!("insert_task_run failed: {e}"));
        }
        self.scheduler.schedule(first_at);
        ToolOutput::structured(json!({
            "task_id": task_id,
            "schedule_kind": "cron",
            "cron": args.cron,
            "first_at": first_at.to_rfc3339(),
            "preview": preview,
            "similar_existing": similar,
        }))
    }
}

// --------------------------------------------------------------------------
// cancel_task
// --------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CancelTaskArgs {
    task_id: i64,
}

pub struct CancelTaskTool {
    store: Arc<dyn Store>,
}

#[async_trait]
impl ToolHandler for CancelTaskTool {
    async fn call(&self, _ctx: ToolContext, call: ToolCall) -> ToolOutput {
        let args: CancelTaskArgs = match serde_json::from_value(call.arguments) {
            Ok(a) => a,
            Err(e) => return ToolOutput::error(format!("invalid cancel_task input: {e}")),
        };
        match self.store.cancel_task_by_id(args.task_id).await {
            Ok(true) => ToolOutput::structured(json!({"cancelled": [args.task_id]})),
            Ok(false) => ToolOutput::error(format!("no active task with id {}", args.task_id)),
            Err(e) => ToolOutput::error(format!("cancel failed: {e}")),
        }
    }
}

// --------------------------------------------------------------------------
// list_tasks
// --------------------------------------------------------------------------

pub struct ListTasksTool {
    store: Arc<dyn Store>,
}

#[async_trait]
impl ToolHandler for ListTasksTool {
    async fn call(&self, ctx: ToolContext, _call: ToolCall) -> ToolOutput {
        match self.store.list_active_tasks(ctx.persona).await {
            Ok(rows) => {
                let entries: Vec<_> = rows
                    .into_iter()
                    .map(|(task, next_at)| {
                        let (kind, schedule_summary) = match &task.schedule {
                            ScheduleKind::Once(at) => ("once", at.to_rfc3339()),
                            ScheduleKind::Cron(expr) => ("cron", expr.clone()),
                        };
                        json!({
                            "id": task.id,
                            "kind": kind,
                            "task": task.task,
                            "schedule": schedule_summary,
                            "next_at": next_at.map(|d| d.to_rfc3339()),
                            "tools": task.tools,
                        })
                    })
                    .collect();
                ToolOutput::structured(json!({"tasks": entries}))
            }
            Err(e) => ToolOutput::error(format!("list_active_tasks failed: {e}")),
        }
    }
}

// --------------------------------------------------------------------------
// specs
// --------------------------------------------------------------------------

fn spec_schedule_once() -> ToolSpec {
    ToolSpec::new(
        SCHEDULE_ONCE,
        "Schedules a one-shot task to fire once at the given time.",
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["due_at", "task", "tools"],
            "properties": {
                "due_at": {
                    "type": "string",
                    "description": "When the task fires (RFC 3339)."
                },
                "task": {
                    "type": "string",
                    "description": "What you will do at that fire moment."
                },
                "tools": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Tool selectors you may call at fire time. Use [\"*\"] for all persona-allowed non-schedule tools, [] for no tools, or names/negations such as [\"read\", \"grep\"] or [\"*\", \"!shell\"]."
                }
            }
        }),
    )
}

fn spec_schedule_cron() -> ToolSpec {
    ToolSpec::new(
        SCHEDULE_CRON,
        "Schedules a recurring task using a 5-field cron expression.",
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["cron", "task", "tools"],
            "properties": {
                "cron": {
                    "type": "string",
                    "description": "5-field cron: minute hour day month day-of-week (day-of-week 0=Sun..6=Sat)."
                },
                "task": {
                    "type": "string",
                    "description": "What you will do at each fire moment."
                },
                "tools": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Tool selectors you may call at fire time. Use [\"*\"] for all persona-allowed non-schedule tools, [] for no tools, or names/negations such as [\"read\", \"grep\"] or [\"*\", \"!shell\"]."
                },
                "first_at": {
                    "type": "string",
                    "description": "Optional RFC 3339 override for the first occurrence."
                }
            }
        }),
    )
}

fn spec_cancel_task() -> ToolSpec {
    ToolSpec::new(
        CANCEL_TASK,
        "Cancels an active scheduled task. \
         Call list_tasks first if you don't already know the id.",
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
        }),
    )
}

fn spec_list_tasks() -> ToolSpec {
    ToolSpec::new(
        LIST_TASKS,
        "Lists all active scheduled tasks for this persona.",
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {}
        }),
    )
}

// --------------------------------------------------------------------------
// helpers
// --------------------------------------------------------------------------

/// Returns up to a handful of compact summaries of already-active tasks
/// whose `task` contains the leading words of `incoming`. Used to
/// surface a soft warning in the tool's structured output. Returns an
/// empty list on any error (the registration still proceeds).
async fn similar_summaries(
    store: &dyn Store,
    persona: goat_types::PersonaId,
    incoming: &str,
) -> Vec<serde_json::Value> {
    const NEEDLE_CHARS: usize = 30;
    let needle: String = incoming.chars().take(NEEDLE_CHARS).collect();
    let needle = needle.trim();
    if needle.is_empty() {
        return Vec::new();
    }
    match store.similar_active_tasks(persona, needle).await {
        Ok(rows) => rows
            .into_iter()
            .take(5)
            .map(|t| {
                json!({
                    "id": t.id,
                    "task": t.task,
                })
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

// --------------------------------------------------------------------------
// tests
// --------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use goat_loop::scheduler::SchedulerHandle;
    use goat_store::SqliteStore;
    use goat_tool::{ToolCall, ToolContext, ToolReadState};
    use goat_types::{ChannelId, ConversationId, InstanceId, PersonaId};
    use std::path::PathBuf;

    async fn setup() -> (Arc<dyn Store>, ToolContext, PersonaId) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        std::mem::forget(dir);
        let store = Arc::new(SqliteStore::open(&path).await.unwrap()) as Arc<dyn Store>;
        let persona = PersonaId::new();
        store.ensure_persona(persona, "dev", "dev").await.unwrap();
        let conv = ConversationId::new(ChannelId::new("telegram"), InstanceId::new(), "chat:1");
        store.ensure_conversation(&conv, persona).await.unwrap();
        let ctx = ToolContext {
            persona,
            conversation: conv,
            goat_root: PathBuf::from("/tmp"),
            read_state: ToolReadState::default(),
        };
        (store, ctx, persona)
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
        let (store, ctx, _) = setup().await;
        let tool = ScheduleOnceTool {
            store,
            scheduler: SchedulerHandle::detached(),
        };
        let past = (Utc::now() - Duration::hours(1)).to_rfc3339();
        let out = tool.call(ctx, call_once(&past, "ping")).await;
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn schedule_once_accepts_future_due() {
        let (store, ctx, persona) = setup().await;
        let tool = ScheduleOnceTool {
            store: store.clone(),
            scheduler: SchedulerHandle::detached(),
        };
        let future = (Utc::now() + Duration::minutes(10)).to_rfc3339();
        let out = tool
            .call(ctx, call_once(&future, "ping example.com from staging"))
            .await;
        assert!(!out.is_error, "got error: {out:?}");
        let active = store.list_active_tasks(persona).await.unwrap();
        assert_eq!(active.len(), 1);
    }

    #[tokio::test]
    async fn schedule_once_rejects_empty_task() {
        let (store, ctx, _) = setup().await;
        let tool = ScheduleOnceTool {
            store,
            scheduler: SchedulerHandle::detached(),
        };
        let future = (Utc::now() + Duration::minutes(10)).to_rfc3339();
        let out = tool.call(ctx, call_once(&future, "   ")).await;
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn schedule_cron_rejects_invalid_expr() {
        let (store, ctx, _) = setup().await;
        let tool = ScheduleCronTool {
            store,
            scheduler: SchedulerHandle::detached(),
        };
        let out = tool
            .call(
                ctx,
                ToolCall {
                    call_id: "c".into(),
                    name: SCHEDULE_CRON,
                    arguments: json!({
                        "cron": "99 * * * *",
                        "task": "weekly task",
                        "tools": [],
                    }),
                },
            )
            .await;
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn schedule_cron_includes_preview() {
        let (store, ctx, _) = setup().await;
        let tool = ScheduleCronTool {
            store,
            scheduler: SchedulerHandle::detached(),
        };
        let out = tool
            .call(
                ctx,
                ToolCall {
                    call_id: "c".into(),
                    name: SCHEDULE_CRON,
                    arguments: json!({
                        "cron": "0 7 * * 1",
                        "task": "weekly summary",
                        "tools": ["read", "grep"],
                    }),
                },
            )
            .await;
        assert!(!out.is_error, "got error: {out:?}");
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
    async fn cancel_by_id_succeeds() {
        let (store, ctx, persona) = setup().await;
        let once = ScheduleOnceTool {
            store: store.clone(),
            scheduler: SchedulerHandle::detached(),
        };
        let future = (Utc::now() + Duration::minutes(10)).to_rfc3339();
        once.call(ctx.clone(), call_once(&future, "loadtest staging"))
            .await;
        let active_before = store.list_active_tasks(persona).await.unwrap();
        let task_id = active_before[0].0.id;

        let cancel = CancelTaskTool {
            store: store.clone(),
        };
        let out = cancel
            .call(
                ctx,
                ToolCall {
                    call_id: "c".into(),
                    name: CANCEL_TASK,
                    arguments: json!({"task_id": task_id}),
                },
            )
            .await;
        assert!(!out.is_error, "got error: {out:?}");
        let active = store.list_active_tasks(persona).await.unwrap();
        assert!(active.is_empty());
    }

    #[tokio::test]
    async fn cancel_requires_task_id() {
        let (store, ctx, _) = setup().await;
        let cancel = CancelTaskTool { store };
        let out = cancel
            .call(
                ctx,
                ToolCall {
                    call_id: "c".into(),
                    name: CANCEL_TASK,
                    arguments: json!({}),
                },
            )
            .await;
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn list_tasks_returns_active() {
        let (store, ctx, _) = setup().await;
        let once = ScheduleOnceTool {
            store: store.clone(),
            scheduler: SchedulerHandle::detached(),
        };
        let future = (Utc::now() + Duration::minutes(10)).to_rfc3339();
        once.call(ctx.clone(), call_once(&future, "do thing")).await;

        let list = ListTasksTool {
            store: store.clone(),
        };
        let out = list
            .call(
                ctx,
                ToolCall {
                    call_id: "c".into(),
                    name: LIST_TASKS,
                    arguments: json!({}),
                },
            )
            .await;
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
