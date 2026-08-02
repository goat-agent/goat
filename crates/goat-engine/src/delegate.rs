use std::{fmt::Write as _, sync::Arc, sync::atomic::Ordering};

use goat_protocol::{
    Effort, Event, ModelTarget, SubagentGroupMember, TaskId, ToolCallId, ToolDisplay,
};
use goat_provider::{ContentBlock, Message, MessageRole, Provider, ToolDefinition};
use tokio_util::sync::CancellationToken;

use crate::{
    Ctx, LoopEnv, Run,
    accounts::provider_for,
    compaction::ContextTracker,
    conversation::Conversation,
    prompt::compose_child_system,
    rounds::{LoopOutcome, core_loop},
    subagent::SubagentSpec,
    tools_exec::build_tool_defs,
};

pub(crate) const MAX_CONCURRENT_SUBAGENTS: usize = 8;
pub(crate) const SUBAGENT_TOOL_NAME: &str = "Subagent";
pub(crate) const SUBAGENT_KILL_TOOL_NAME: &str = "SubagentKill";

#[derive(serde::Deserialize)]
struct SubagentInput {
    subagent_type: String,
    name: String,
    prompt: String,
    #[serde(default)]
    background: bool,
}

#[derive(serde::Deserialize)]
struct KillInput {
    run: goat_protocol::RunId,
}

pub(crate) fn subagent_tool_def(ctx: &Ctx) -> ToolDefinition {
    let names: Vec<String> = ctx.subagents.names();
    let mut description = String::from(
        "Delegate a self-contained task to a subagent that runs in its own context with a restricted tool set and returns only its final report. Prefer this for focused investigation or work that would otherwise flood the main context. Issue several Subagent calls in one response to run them in parallel. Set background=true to detach it instead: the call returns a run id immediately and a fresh turn wakes you with the report when it finishes, so if you have other work to get on with, detach it and end your turn rather than waiting. Stop a detached one with SubagentKill. Available subagent_type values:",
    );
    for spec in ctx.subagents.iter() {
        let _ = write!(description, "\n- {}: {}", spec.name, spec.description);
    }
    ToolDefinition {
        name: SUBAGENT_TOOL_NAME.to_owned(),
        description,
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "subagent_type": {
                    "type": "string",
                    "enum": names,
                },
                "name": {
                    "type": "string",
                    "description": "A short name for this run, a few words naming the job rather than restating the instruction — it is what every view of the run is labelled with.",
                },
                "prompt": {
                    "type": "string",
                    "description": "A complete, self-contained instruction for the subagent. It does not see the conversation, so include all needed context.",
                },
                "background": {
                    "type": "boolean",
                    "description": "detach it and return a run id instead of waiting for the report (default false)",
                },
            },
            "required": ["subagent_type", "name", "prompt"],
        }),
    }
}

pub(crate) fn subagent_kill_tool_def() -> ToolDefinition {
    ToolDefinition {
        name: SUBAGENT_KILL_TOOL_NAME.to_owned(),
        description: "Stop a detached subagent run started with Subagent(background=true). It will not report back.".to_owned(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "run": {"type": "string", "description": "run id from Subagent(background=true)"}
            },
            "required": ["run"],
        }),
    }
}

pub(crate) async fn run_subagent_kill(ctx: &Ctx, input_json: &str) -> Result<String, String> {
    let args: KillInput =
        serde_json::from_str(input_json).map_err(|err| format!("invalid input: {err}"))?;
    ctx.background
        .kill(args.run, Some(crate::background::Kind::Subagent))
        .await?;
    Ok(format!("Killed subagent run #{}.", args.run))
}

pub(crate) fn subagent_kill_display(input: &str) -> ToolDisplay {
    match serde_json::from_str::<KillInput>(input) {
        Ok(args) => ToolDisplay::primary(format!("SubagentKill(#{})", args.run)),
        Err(_) => ToolDisplay::primary(SUBAGENT_KILL_TOOL_NAME.to_owned()),
    }
}

pub(crate) fn subagent_call_display(input: &str) -> ToolDisplay {
    match serde_json::from_str::<SubagentInput>(input) {
        Ok(args) => {
            let mut parts = Vec::with_capacity(3);
            if args.background {
                parts.push("background");
            }
            parts.push(args.subagent_type.as_str());
            parts.push(args.name.as_str());
            ToolDisplay::primary(goat_tool::display::call_sig(SUBAGENT_TOOL_NAME, &parts))
        }
        Err(_) => goat_tool::display::generic_named(SUBAGENT_TOOL_NAME, input),
    }
}

pub(crate) fn wants_background(input_json: &str) -> bool {
    serde_json::from_str::<SubagentInput>(input_json).is_ok_and(|args| args.background)
}

pub(crate) fn subagent_group_member(call: ToolCallId, input: &str) -> SubagentGroupMember {
    match serde_json::from_str::<SubagentInput>(input) {
        Ok(args) => SubagentGroupMember {
            call,
            subagent_type: args.subagent_type,
            label: args.name,
            background: args.background,
        },
        Err(_) => SubagentGroupMember {
            call,
            subagent_type: "subagent".to_owned(),
            label: "subagent".to_owned(),
            background: false,
        },
    }
}

fn resolve_subagent_model(
    ctx: &Ctx,
    parent: &ModelTarget,
    spec: &SubagentSpec,
) -> Option<(Arc<dyn Provider>, String, Option<Effort>)> {
    if let Some(model_id) = &spec.model {
        if let Some(found) = ctx
            .registry()
            .all()
            .iter()
            .find(|provider| provider.list_models().iter().any(|id| id == model_id))
        {
            let provider_id = found.id().to_string();
            let provider = provider_for(
                ctx,
                &parent.account,
                &goat_provider::ProviderId::from(provider_id.as_str()),
            )
            .unwrap_or_else(|| found.clone());
            let effort = spec
                .effort
                .or_else(|| provider.efforts(model_id).into_iter().next());
            return Some((provider, model_id.clone(), effort));
        }
        tracing::warn!(model = %model_id, "subagent model not found; inheriting parent model");
    }
    let provider = provider_for(
        ctx,
        &parent.account,
        &goat_provider::ProviderId::from(parent.provider.as_str()),
    )?;
    Some((provider, parent.model.clone(), parent.effort))
}

struct Origin {
    target: ModelTarget,
    cwd: std::path::PathBuf,
    exec_policy: goat_tool::SandboxPolicy,
}

impl Origin {
    fn of(env: &LoopEnv) -> Self {
        Self {
            target: env.target.clone(),
            cwd: env.cwd.clone(),
            exec_policy: env.exec_policy.clone(),
        }
    }
}

pub(crate) async fn run_delegation(
    ctx: &Ctx,
    env: &LoopEnv,
    input_json: &str,
    parent: TaskId,
    call: ToolCallId,
    token: &CancellationToken,
) -> Result<String, String> {
    let args: SubagentInput =
        serde_json::from_str(input_json).map_err(|err| format!("invalid Subagent input: {err}"))?;
    if ctx.subagents.get(&args.subagent_type).is_none() {
        return Err(format!("unknown subagent_type: {}", args.subagent_type));
    }
    let origin = Origin::of(env);
    if args.background {
        return detach(ctx, origin, args, parent, call).await;
    }
    let child_token = token.child_token();
    run_child(ctx, &origin, &args, parent, call, &child_token).await
}

async fn detach(
    ctx: &Ctx,
    origin: Origin,
    args: SubagentInput,
    parent: TaskId,
    call: ToolCallId,
) -> Result<String, String> {
    let cancel = CancellationToken::new();
    let subagent_type = args.subagent_type.clone();
    let run_id = ctx
        .background
        .register_subagent(&args.name, cancel.clone())
        .await;
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let permit = ctx.semaphore.clone().acquire_owned().await;
        let result = if cancel.is_cancelled() {
            Err("subagent interrupted".to_owned())
        } else {
            run_child(&ctx, &origin, &args, parent, call, &cancel).await
        };
        drop(permit);
        ctx.background.finish_subagent(run_id, result).await;
    });
    Ok(format!(
        "Started subagent run #{run_id} ({subagent_type}). A fresh turn will wake you with its report when it finishes, so if you have other work to get on with, do that and end your turn instead of waiting. Stop it with SubagentKill(run={run_id})."
    ))
}

type ChildFuture<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>;

fn run_child<'a>(
    ctx: &'a Ctx,
    origin: &'a Origin,
    args: &'a SubagentInput,
    parent: TaskId,
    call: ToolCallId,
    token: &'a CancellationToken,
) -> ChildFuture<'a> {
    Box::pin(run_child_inner(ctx, origin, args, parent, call, token))
}

async fn run_child_inner(
    ctx: &Ctx,
    origin: &Origin,
    args: &SubagentInput,
    parent: TaskId,
    call: ToolCallId,
    token: &CancellationToken,
) -> Result<String, String> {
    let Some(spec) = ctx.subagents.get(&args.subagent_type) else {
        return Err(format!("unknown subagent_type: {}", args.subagent_type));
    };
    let Some((provider, model, effort)) = resolve_subagent_model(ctx, &origin.target, spec) else {
        return Err("could not resolve a model for the subagent".to_owned());
    };
    let child_target = ModelTarget {
        provider: provider.id().to_string(),
        model,
        account: origin.target.account.clone(),
        effort,
    };
    let tool_defs = build_tool_defs(
        ctx,
        provider.as_ref(),
        Some(&spec.tools),
        false,
        false,
        false,
    );
    let mut conversation = Conversation::new();
    conversation.push(
        Message::text(
            MessageRole::System,
            compose_child_system(&spec.prompt, ctx.instructions.as_deref()),
        ),
        None,
    );
    conversation.push(Message::text(MessageRole::User, args.prompt.clone()), None);
    let mut tracker = ContextTracker::new();
    let child_id = TaskId(ctx.child_ids.fetch_add(1, Ordering::Relaxed));
    let _ = ctx
        .events
        .send(Event::SubagentStarted {
            id: child_id,
            parent,
            call,
            subagent_type: args.subagent_type.clone(),
            label: args.name.clone(),
        })
        .await;
    let run = Run::child(child_id);
    let child_env = LoopEnv {
        provider,
        target: child_target,
        tool_defs,
        cwd: origin.cwd.clone(),
        allow_delegate: false,
        allow_ask: false,
        plan: false,
        plan_path: None,
        exec_policy: crate::subagent::tighter(&origin.exec_policy, &spec.exec_policy),
    };
    let outcome = Box::pin(core_loop(
        ctx,
        &run,
        &child_env,
        token,
        &mut conversation,
        &mut tracker,
    ))
    .await;
    let result = match outcome {
        LoopOutcome::Completed => Ok(final_text(conversation.messages())),
        LoopOutcome::Cancelled => Err("subagent interrupted".to_owned()),
        LoopOutcome::Failed(message, _) => Err(message),
    };
    let _ = ctx
        .events
        .send(Event::SubagentDone {
            id: child_id,
            ok: result.is_ok(),
        })
        .await;
    result
}

fn final_text(history: &[Message]) -> String {
    for message in history.iter().rev() {
        if message.role == MessageRole::Assistant {
            let mut text = String::new();
            for block in &message.content {
                if let ContentBlock::Text { text: chunk } = block {
                    text.push_str(chunk);
                }
            }
            if !text.trim().is_empty() {
                return text;
            }
        }
    }
    "(subagent produced no output)".to_owned()
}

#[cfg(test)]
mod tests {
    use super::final_text;
    use goat_provider::{Message, MessageRole};

    #[test]
    fn final_text_picks_last_nonempty_assistant() {
        let history = vec![
            Message::text(MessageRole::User, "ask"),
            Message::text(MessageRole::Assistant, "first"),
            Message::text(MessageRole::Assistant, "   "),
        ];
        assert_eq!(final_text(&history), "first");
    }

    #[test]
    fn final_text_falls_back_when_no_output() {
        let history = vec![Message::text(MessageRole::User, "ask")];
        assert_eq!(final_text(&history), "(subagent produced no output)");
    }
}
