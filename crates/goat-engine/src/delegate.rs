use std::{fmt::Write as _, sync::Arc, sync::atomic::Ordering};

use goat_protocol::{Effort, Event, ModelTarget, TaskId, ToolDisplay};
use goat_provider::{ContentBlock, Message, MessageRole, Provider, ToolDefinition};
use tokio_util::sync::CancellationToken;

use crate::{
    Ctx, LoopEnv, Run,
    accounts::provider_for,
    agent::AgentSpec,
    compaction::ContextTracker,
    conversation::Conversation,
    prompt::compose_child_system,
    rounds::{LoopOutcome, core_loop},
    tools_exec::build_tool_defs,
};

pub(crate) const MAX_CONCURRENT_AGENTS: usize = 8;
pub(crate) const AGENT_TOOL_NAME: &str = "Agent";

pub(crate) fn agent_tool_def(ctx: &Ctx<'_>) -> ToolDefinition {
    let names: Vec<String> = ctx.agents.names();
    let mut description = String::from(
        "Delegate a self-contained task to a sub-agent that runs in its own context with a restricted tool set and returns only its final report. Prefer this for focused investigation or work that would otherwise flood the main context. Issue several Agent calls in one response to run them in parallel. Available agent_type values:",
    );
    for spec in ctx.agents.iter() {
        let _ = write!(description, "\n- {}: {}", spec.name, spec.description);
    }
    ToolDefinition {
        name: AGENT_TOOL_NAME.to_owned(),
        description,
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "agent_type": {
                    "type": "string",
                    "enum": names,
                },
                "prompt": {
                    "type": "string",
                    "description": "A complete, self-contained instruction for the sub-agent. It does not see the conversation, so include all needed context.",
                },
            },
            "required": ["agent_type", "prompt"],
        }),
    }
}

pub(crate) fn agent_call_display(input: &str) -> ToolDisplay {
    #[derive(serde::Deserialize)]
    struct Input {
        agent_type: String,
        prompt: String,
    }
    match serde_json::from_str::<Input>(input) {
        Ok(args) => {
            let prompt = goat_tool::display::flatten(&args.prompt);
            ToolDisplay::primary(goat_tool::display::call_sig(
                AGENT_TOOL_NAME,
                &[args.agent_type.as_str(), prompt.as_str()],
            ))
        }
        Err(_) => goat_tool::display::generic_named(AGENT_TOOL_NAME, input),
    }
}

fn resolve_agent_model(
    ctx: &Ctx<'_>,
    parent: &ModelTarget,
    spec: &AgentSpec,
) -> Option<(Arc<dyn Provider>, String, Option<Effort>)> {
    if let Some(model_id) = &spec.model {
        if let Some(found) = ctx
            .registry
            .all()
            .iter()
            .find(|provider| provider.catalog().contains(&model_id.as_str()))
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
        tracing::warn!(model = %model_id, "agent model not found; inheriting parent model");
    }
    let provider = provider_for(
        ctx,
        &parent.account,
        &goat_provider::ProviderId::from(parent.provider.as_str()),
    )?;
    Some((provider, parent.model.clone(), parent.effort))
}

pub(crate) async fn run_delegation(
    ctx: &Ctx<'_>,
    env: &LoopEnv<'_>,
    input_json: &str,
    parent: TaskId,
    token: &CancellationToken,
) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    struct Input {
        agent_type: String,
        prompt: String,
    }
    let args: Input =
        serde_json::from_str(input_json).map_err(|err| format!("invalid Agent input: {err}"))?;
    let Some(spec) = ctx.agents.get(&args.agent_type) else {
        return Err(format!("unknown agent_type: {}", args.agent_type));
    };
    let Some((provider, model, effort)) = resolve_agent_model(ctx, env.target, spec) else {
        return Err("could not resolve a model for the agent".to_owned());
    };
    let child_target = ModelTarget {
        provider: provider.id().to_string(),
        model,
        account: env.target.account.clone(),
        effort,
    };
    let tool_defs = build_tool_defs(ctx, provider.as_ref(), Some(&spec.tools), false, false);
    let mut conversation = Conversation::new();
    conversation.push(
        Message::text(
            MessageRole::System,
            compose_child_system(&spec.prompt, ctx.instructions),
        ),
        None,
    );
    conversation.push(Message::text(MessageRole::User, args.prompt.clone()), None);
    let mut tracker = ContextTracker::new();
    let child_id = TaskId(ctx.child_ids.fetch_add(1, Ordering::Relaxed));
    let _ = ctx
        .events
        .send(Event::AgentStarted {
            id: child_id,
            parent,
            agent_type: args.agent_type.clone(),
            label: delegation_label(&args.prompt),
        })
        .await;
    let run = Run::child(child_id);
    let child_env = LoopEnv {
        provider: provider.as_ref(),
        target: &child_target,
        tool_defs: &tool_defs,
        cwd: env.cwd,
        allow_delegate: false,
        allow_ask: false,
        exec_policy: crate::agent::tighter(&env.exec_policy, &spec.exec_policy),
    };
    let child_token = token.child_token();
    let outcome = Box::pin(core_loop(
        ctx,
        &run,
        &child_env,
        &child_token,
        &mut conversation,
        &mut tracker,
    ))
    .await;
    let result = match outcome {
        LoopOutcome::Completed => Ok(final_text(conversation.messages())),
        LoopOutcome::Cancelled => Ok("(agent interrupted)".to_owned()),
        LoopOutcome::Failed(message, _) => Err(message),
    };
    let _ = ctx
        .events
        .send(Event::AgentDone {
            id: child_id,
            ok: result.is_ok(),
        })
        .await;
    result
}

fn delegation_label(prompt: &str) -> String {
    let line = prompt.lines().next().unwrap_or("").trim();
    goat_tool::display::truncate_chars(line, 50)
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
    "(agent produced no output)".to_owned()
}

#[cfg(test)]
mod tests {
    use super::{delegation_label, final_text};
    use goat_provider::{Message, MessageRole};

    #[test]
    fn label_uses_first_line_and_truncates() {
        assert_eq!(delegation_label("hello\nworld"), "hello");
        let long = "x".repeat(60);
        let label = delegation_label(&long);
        assert_eq!(label.chars().count(), 51);
        assert!(label.ends_with('…'));
    }

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
        assert_eq!(final_text(&history), "(agent produced no output)");
    }
}
