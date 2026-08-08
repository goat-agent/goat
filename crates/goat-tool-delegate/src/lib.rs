use std::{any::Any, future::Future, pin::Pin, sync::Arc};

use goat_protocol::{
    RunId, SubagentGroupEntry, SubagentGroupMember, TaskId, ToolCallId, ToolDisplay, ToolOutcome,
    TranscriptEntry,
};
use goat_tool::{
    Tool, ToolBatchCall, ToolBatchFuture, ToolBatchInvocation, ToolDefinitionContext, ToolError,
    ToolFuture, ToolHistoryGroup, ToolInvocation, ToolOutput, ToolSandbox, ToolSpec,
};
use tokio_util::sync::CancellationToken;

pub const DELEGATE_TOOL_NAME: &str = "Subagent";
pub const KILL_TOOL_NAME: &str = "SubagentKill";

#[derive(Clone)]
pub struct AgentSpec {
    pub name: String,
    pub description: String,
}

#[derive(serde::Deserialize)]
pub struct DelegateRequest {
    pub subagent_type: String,
    pub name: String,
    pub prompt: String,
    #[serde(default)]
    pub background: bool,
}

#[derive(serde::Deserialize)]
struct KillRequest {
    run: RunId,
}

pub struct DelegateInvocation<'a> {
    pub run_label: &'static str,
    pub parent: TaskId,
    pub call: ToolCallId,
    pub cancellation: &'a CancellationToken,
    pub host: Option<&'a (dyn Any + Send + Sync)>,
}

pub enum DelegateResult {
    Completed(String),
    Started(RunId),
}

pub type DelegateFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'a>>;

pub trait DelegationService: Send + Sync {
    fn run<'a>(
        &'a self,
        request: DelegateRequest,
        invocation: DelegateInvocation<'a>,
    ) -> DelegateFuture<'a, DelegateResult>;

    fn kill(&self, run: RunId) -> DelegateFuture<'_, ()>;

    fn group_started(
        &self,
        parent: TaskId,
        group: ToolCallId,
        members: Vec<SubagentGroupMember>,
    ) -> DelegateFuture<'_, ()>;
}

pub struct DelegateTool {
    agents: Vec<AgentSpec>,
    service: Arc<dyn DelegationService>,
}

impl DelegateTool {
    pub fn new(agents: Vec<AgentSpec>, service: Arc<dyn DelegationService>) -> Self {
        Self { agents, service }
    }
}

impl Tool for DelegateTool {
    fn name(&self) -> &'static str {
        DELEGATE_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Delegate a self-contained task to a subagent"
    }

    fn parameters(&self) -> serde_json::Value {
        delegate_parameters(&self.agents)
    }

    fn run<'a>(&'a self, _input: &'a str, _ctx: &'a ToolSandbox) -> ToolFuture<'a> {
        Box::pin(async { Err(ToolError::execution("delegation invocation is unavailable")) })
    }

    fn invoke<'a>(
        &'a self,
        input: &'a str,
        _ctx: &'a ToolSandbox,
        invocation: ToolInvocation<'a>,
    ) -> ToolFuture<'a> {
        Box::pin(async move {
            let request: DelegateRequest = serde_json::from_str(input).map_err(ToolError::from)?;
            if !self
                .agents
                .iter()
                .any(|agent| agent.name == request.subagent_type)
            {
                return Err(ToolError::invalid_input(format!(
                    "unknown subagent_type: {}",
                    request.subagent_type
                )));
            }
            let subagent_type = request.subagent_type.clone();
            let result = self
                .service
                .run(
                    request,
                    DelegateInvocation {
                        run_label: "subagent",
                        parent: invocation.task,
                        call: invocation.call,
                        cancellation: invocation.cancellation,
                        host: invocation.host,
                    },
                )
                .await
                .map_err(ToolError::execution)?;
            let text = match result {
                DelegateResult::Completed(report) => report,
                DelegateResult::Started(run) => format!(
                    "Started subagent run #{run} ({subagent_type}). A fresh turn will wake you with its report when it finishes, so if you have other work to get on with, do that and end your turn instead of waiting. Stop it with SubagentKill(run={run})."
                ),
            };
            Ok(ToolOutput::text(text))
        })
    }

    fn enabled(&self, context: ToolDefinitionContext) -> bool {
        context.top_level && !self.agents.is_empty()
    }

    fn definition(&self, context: ToolDefinitionContext) -> Option<ToolSpec> {
        self.enabled(context).then(|| ToolSpec {
            name: self.name(),
            description: delegate_description(&self.agents),
            parameters: self.parameters(),
        })
    }

    fn display_input(&self, input: &str) -> ToolDisplay {
        match serde_json::from_str::<DelegateRequest>(input) {
            Ok(args) => {
                let mut parts = Vec::with_capacity(3);
                if args.background {
                    parts.push("background");
                }
                parts.push(args.subagent_type.as_str());
                parts.push(args.name.as_str());
                ToolDisplay::primary(goat_tool::display::call_sig(DELEGATE_TOOL_NAME, &parts))
            }
            Err(_) => goat_tool::display::generic_named(DELEGATE_TOOL_NAME, input),
        }
    }

    fn batch_started<'a>(
        &'a self,
        calls: &'a [ToolBatchCall<'a>],
        invocation: ToolBatchInvocation,
    ) -> ToolBatchFuture<'a> {
        Box::pin(async move {
            let Some(members) = group_members(calls) else {
                return;
            };
            let group = members[0].call;
            let _ = self
                .service
                .group_started(invocation.task, group, members)
                .await;
        })
    }

    fn history_group(&self, calls: &[ToolBatchCall<'_>]) -> Option<Box<dyn ToolHistoryGroup>> {
        let members = group_members(calls)?;
        Some(Box::new(DelegateHistoryGroup {
            group: members[0].call,
            members,
        }))
    }
}

pub struct KillDelegateTool {
    enabled: bool,
    service: Arc<dyn DelegationService>,
}

impl KillDelegateTool {
    pub fn new(enabled: bool, service: Arc<dyn DelegationService>) -> Self {
        Self { enabled, service }
    }
}

impl Tool for KillDelegateTool {
    fn name(&self) -> &'static str {
        KILL_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Stop a detached subagent run started with Subagent(background=true). It will not report back."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "run": {"type": "string", "description": "run id from Subagent(background=true)"}
            },
            "required": ["run"]
        })
    }

    fn run<'a>(&'a self, input: &'a str, _ctx: &'a ToolSandbox) -> ToolFuture<'a> {
        Box::pin(async move {
            let request: KillRequest = serde_json::from_str(input).map_err(ToolError::from)?;
            self.service
                .kill(request.run)
                .await
                .map_err(ToolError::execution)?;
            Ok(ToolOutput::text(format!(
                "Killed subagent run #{}.",
                request.run
            )))
        })
    }

    fn enabled(&self, context: ToolDefinitionContext) -> bool {
        context.top_level && self.enabled
    }

    fn display_input(&self, input: &str) -> ToolDisplay {
        match serde_json::from_str::<KillRequest>(input) {
            Ok(args) => ToolDisplay::primary(format!("SubagentKill(#{})", args.run)),
            Err(_) => ToolDisplay::primary(KILL_TOOL_NAME.to_owned()),
        }
    }
}

pub fn tools(agents: Vec<AgentSpec>, service: Arc<dyn DelegationService>) -> Vec<Box<dyn Tool>> {
    let enabled = !agents.is_empty();
    vec![
        Box::new(DelegateTool::new(agents, service.clone())),
        Box::new(KillDelegateTool::new(enabled, service)),
    ]
}

fn group_member(call: ToolCallId, input: &str) -> SubagentGroupMember {
    match serde_json::from_str::<DelegateRequest>(input) {
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

fn group_members(calls: &[ToolBatchCall<'_>]) -> Option<Vec<SubagentGroupMember>> {
    if calls.len() < 2 {
        return None;
    }
    Some(
        calls
            .iter()
            .map(|call| group_member(call.call, call.input))
            .collect(),
    )
}

struct DelegateHistoryGroup {
    group: ToolCallId,
    members: Vec<SubagentGroupMember>,
}

impl ToolHistoryGroup for DelegateHistoryGroup {
    fn entry(&self, outcomes: Vec<ToolOutcome>) -> TranscriptEntry {
        let members = self
            .members
            .iter()
            .cloned()
            .zip(outcomes)
            .map(|(member, outcome)| SubagentGroupEntry { member, outcome })
            .collect();
        TranscriptEntry::SubagentGroup {
            group: self.group,
            members,
        }
    }
}

fn delegate_description(agents: &[AgentSpec]) -> String {
    let mut description = String::from(
        "Delegate a self-contained task to a subagent that runs in its own context with a restricted tool set and returns only its final report. Prefer this for focused investigation or work that would otherwise flood the main context. Issue several Subagent calls in one response to run them in parallel. Set background=true to detach it instead: the call returns a run id immediately and a fresh turn wakes you with the report when it finishes, so if you have other work to get on with, detach it and end your turn rather than waiting. Stop a detached one with SubagentKill. Available subagent_type values:",
    );
    for agent in agents {
        description.push_str("\n- ");
        description.push_str(&agent.name);
        description.push_str(": ");
        description.push_str(&agent.description);
    }
    description
}

fn delegate_parameters(agents: &[AgentSpec]) -> serde_json::Value {
    let names: Vec<&str> = agents.iter().map(|agent| agent.name.as_str()).collect();
    serde_json::json!({
        "type": "object",
        "properties": {
            "subagent_type": {
                "type": "string",
                "enum": names,
            },
            "name": {
                "type": "string",
                "description": "A short name for this run, a few words naming the job rather than restating the instruction — it is what every view of the run is labelled with."
            },
            "prompt": {
                "type": "string",
                "description": "A complete, self-contained instruction for the subagent. It does not see the conversation, so include all needed context."
            },
            "background": {
                "type": "boolean",
                "description": "detach it and return a run id instead of waiting for the report (default false)"
            }
        },
        "required": ["subagent_type", "name", "prompt"]
    })
}

#[cfg(test)]
mod tests {
    use goat_protocol::ToolCallId;
    use goat_tool::ToolBatchCall;

    use super::group_members;

    const BLOCKING: &str = r#"{"subagent_type":"explore","name":"n","prompt":"p"}"#;

    #[test]
    fn a_parallel_batch_builds_one_group() {
        let calls = [
            ToolBatchCall {
                call: ToolCallId(1),
                input: BLOCKING,
            },
            ToolBatchCall {
                call: ToolCallId(2),
                input: BLOCKING,
            },
        ];
        let members = group_members(&calls).unwrap();
        assert_eq!(members.len(), 2);
        assert_eq!(members[0].call, ToolCallId(1));
        assert_eq!(members[1].call, ToolCallId(2));
    }

    #[test]
    fn a_lone_call_never_builds_a_group() {
        let calls = [ToolBatchCall {
            call: ToolCallId(1),
            input: BLOCKING,
        }];
        assert!(group_members(&calls).is_none());
    }
}
