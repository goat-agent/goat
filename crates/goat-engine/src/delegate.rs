use std::{sync::Arc, sync::Weak, sync::atomic::Ordering};

use goat_protocol::{Effort, Event, ModelTarget, TaskId, ToolCallId};
use goat_provider::{ContentBlock, Message, MessageRole, Provider};
use goat_tool_delegate::{
    DelegateFuture, DelegateInvocation, DelegateRequest, DelegateResult, DelegationService,
};
use tokio_util::sync::CancellationToken;

use crate::{
    LoopEnv, Run, SessionContext,
    accounts::provider_for,
    compaction::ContextTracker,
    conversation::Conversation,
    prompt::compose_child_system,
    rounds::{LoopOutcome, core_loop},
    subagent::SubagentSpec,
    tools_exec::build_tool_defs,
};

pub(crate) const MAX_CONCURRENT_SUBAGENTS: usize = 8;

pub(crate) struct EngineDelegationService {
    shared: std::sync::Mutex<Weak<crate::SessionServices>>,
}

impl EngineDelegationService {
    pub(crate) fn new() -> Self {
        Self {
            shared: std::sync::Mutex::new(Weak::new()),
        }
    }

    pub(crate) fn attach(&self, ctx: &SessionContext) {
        *self
            .shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::downgrade(&ctx.0);
    }

    fn context(&self) -> Result<SessionContext, String> {
        self.shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .upgrade()
            .map(SessionContext)
            .ok_or_else(|| "delegation service unavailable".to_owned())
    }
}

fn resolve_subagent_model(
    ctx: &SessionContext,
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

impl DelegationService for EngineDelegationService {
    fn run<'a>(
        &'a self,
        request: DelegateRequest,
        invocation: DelegateInvocation<'a>,
    ) -> DelegateFuture<'a, DelegateResult> {
        Box::pin(async move {
            let ctx = self.context()?;
            let env = invocation
                .host
                .and_then(|host| host.downcast_ref::<LoopEnv>())
                .ok_or_else(|| "delegation environment unavailable".to_owned())?;
            if ctx.subagents.get(&request.subagent_type).is_none() {
                return Err(format!("unknown subagent_type: {}", request.subagent_type));
            }
            let origin = Origin::of(env);
            if request.background {
                let run = detach(
                    &ctx,
                    origin,
                    request,
                    invocation.run_label,
                    invocation.parent,
                    invocation.call,
                )
                .await;
                return Ok(DelegateResult::Started(run));
            }
            let permit = tokio::select! {
                biased;
                () = invocation.cancellation.cancelled() => None,
                acquired = ctx.semaphore.acquire() => acquired.ok(),
            };
            let Some(_permit) = permit else {
                return Err("subagent interrupted".to_owned());
            };
            let child_token = invocation.cancellation.child_token();
            run_child(
                &ctx,
                &origin,
                &request,
                invocation.parent,
                invocation.call,
                &child_token,
            )
            .await
            .map(DelegateResult::Completed)
        })
    }

    fn kill(&self, run: goat_protocol::RunId) -> DelegateFuture<'_, ()> {
        Box::pin(async move {
            self.context()?
                .background
                .kill(run, Some(crate::background::Kind::Child))
                .await
        })
    }

    fn group_started(
        &self,
        parent: TaskId,
        group: ToolCallId,
        members: Vec<goat_protocol::SubagentGroupMember>,
    ) -> DelegateFuture<'_, ()> {
        Box::pin(async move {
            let _ = self
                .context()?
                .events
                .send(Event::SubagentGroupStarted {
                    id: parent,
                    group,
                    members,
                })
                .await;
            Ok(())
        })
    }
}

async fn detach(
    ctx: &SessionContext,
    origin: Origin,
    args: DelegateRequest,
    label: &str,
    parent: TaskId,
    call: ToolCallId,
) -> goat_protocol::RunId {
    let cancel = CancellationToken::new();
    let run_id = ctx
        .background
        .register_child_labeled(&args.name, cancel.clone(), label)
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
        ctx.background.finish_child(run_id, result).await;
    });
    run_id
}

type ChildFuture<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>;

fn run_child<'a>(
    ctx: &'a SessionContext,
    origin: &'a Origin,
    args: &'a DelegateRequest,
    parent: TaskId,
    call: ToolCallId,
    token: &'a CancellationToken,
) -> ChildFuture<'a> {
    Box::pin(run_child_inner(ctx, origin, args, parent, call, token))
}

async fn run_child_inner(
    ctx: &SessionContext,
    origin: &Origin,
    args: &DelegateRequest,
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
        interactive: false,
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
