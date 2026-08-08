use std::sync::Arc;

use goat_tool::ToolRegistry;
use goat_tool_ask::{AskTool, QuestionBroker, QuestionFuture};
use goat_tool_delegate::{
    AgentSpec, DelegateFuture, DelegateInvocation, DelegateResult, DelegationService,
};
use goat_tool_plan::{PlanFuture, PlanService, PlanSubmission, ProposePlanTool};
use goat_tool_search::{NativeSearchFuture, NativeSearchRequest, NativeSearchService};
use goat_tool_shell::{BackgroundFuture, BackgroundProcessService, ProcessChunk, ProcessStart};

pub struct BuiltinCapabilities {
    pub questions: Arc<dyn QuestionBroker>,
    pub processes: Arc<dyn BackgroundProcessService>,
    pub agents: Vec<AgentSpec>,
    pub delegation: Arc<dyn DelegationService>,
    pub native_search: Arc<dyn NativeSearchService>,
    pub plans: Arc<dyn PlanService>,
}

struct UnavailableQuestions;
struct UnavailableProcesses;
struct UnavailableDelegation;
struct UnavailableNativeSearch;
struct UnavailablePlans;

impl QuestionBroker for UnavailableQuestions {
    fn ask<'a>(
        &'a self,
        _task: goat_protocol::TaskId,
        _call: goat_protocol::ToolCallId,
        _questions: Vec<goat_protocol::AskQuestion>,
        _cancellation: &'a tokio_util::sync::CancellationToken,
    ) -> QuestionFuture<'a> {
        Box::pin(async { Err("question broker unavailable".to_owned()) })
    }
}

impl BackgroundProcessService for UnavailableProcesses {
    fn start<'a>(
        &'a self,
        _request: ProcessStart,
        _cancellation: &'a tokio_util::sync::CancellationToken,
    ) -> BackgroundFuture<'a, goat_protocol::RunId> {
        Box::pin(async { Err("background process service unavailable".to_owned()) })
    }

    fn output(&self, _run: goat_protocol::RunId) -> BackgroundFuture<'_, ProcessChunk> {
        Box::pin(async { Err("background process service unavailable".to_owned()) })
    }

    fn input(&self, _run: goat_protocol::RunId, _text: String) -> BackgroundFuture<'_, ()> {
        Box::pin(async { Err("background process service unavailable".to_owned()) })
    }

    fn kill(&self, _run: goat_protocol::RunId) -> BackgroundFuture<'_, ()> {
        Box::pin(async { Err("background process service unavailable".to_owned()) })
    }
}

impl DelegationService for UnavailableDelegation {
    fn run<'a>(
        &'a self,
        _request: goat_tool_delegate::DelegateRequest,
        _invocation: DelegateInvocation<'a>,
    ) -> DelegateFuture<'a, DelegateResult> {
        Box::pin(async { Err("delegation service unavailable".to_owned()) })
    }

    fn kill(&self, _run: goat_protocol::RunId) -> DelegateFuture<'_, ()> {
        Box::pin(async { Err("delegation service unavailable".to_owned()) })
    }

    fn group_started(
        &self,
        _parent: goat_protocol::TaskId,
        _group: goat_protocol::ToolCallId,
        _members: Vec<goat_protocol::SubagentGroupMember>,
    ) -> DelegateFuture<'_, ()> {
        Box::pin(async { Err("delegation service unavailable".to_owned()) })
    }
}

impl NativeSearchService for UnavailableNativeSearch {
    fn search<'a>(
        &'a self,
        _request: NativeSearchRequest,
        _invocation: goat_tool::ToolInvocation<'a>,
    ) -> NativeSearchFuture<'a> {
        Box::pin(async { Err("native search service unavailable".to_owned()) })
    }
}

impl PlanService for UnavailablePlans {
    fn path(
        &self,
        _host: Option<&(dyn std::any::Any + Send + Sync)>,
    ) -> Option<std::path::PathBuf> {
        None
    }

    fn submit(&self, _submission: PlanSubmission) -> PlanFuture<'_> {
        Box::pin(async { Err("plan service unavailable".to_owned()) })
    }
}

impl Default for BuiltinCapabilities {
    fn default() -> Self {
        Self {
            questions: Arc::new(UnavailableQuestions),
            processes: Arc::new(UnavailableProcesses),
            agents: Vec::new(),
            delegation: Arc::new(UnavailableDelegation),
            native_search: Arc::new(UnavailableNativeSearch),
            plans: Arc::new(UnavailablePlans),
        }
    }
}

pub fn builtin() -> ToolRegistry {
    builtin_with(BuiltinCapabilities::default())
}

pub fn builtin_with(capabilities: BuiltinCapabilities) -> ToolRegistry {
    let mut tools = goat_tool_fs::all();
    tools.extend(goat_tool_shell::all_with_background(capabilities.processes));
    tools.extend(goat_tool_delegate::tools(
        capabilities.agents,
        capabilities.delegation,
    ));
    tools.extend(goat_tool_search::all_with_native(
        capabilities.native_search,
    ));
    tools.extend(goat_tool_skill::all());
    tools.extend(goat_tool_web::all());
    tools.push(Box::new(AskTool::new(capabilities.questions)));
    tools.push(Box::new(ProposePlanTool::new(capabilities.plans)));
    ToolRegistry::new(tools)
}

#[cfg(test)]
mod tests {
    #[test]
    fn builtin_registers_all_tools() {
        let registry = super::builtin();
        for name in [
            "Read",
            "Write",
            "Edit",
            "Bash",
            "Grep",
            "Glob",
            "Skill",
            "WebFetch",
            "WebSearch",
        ] {
            assert!(registry.get(name).is_some(), "missing tool: {name}");
        }
    }

    #[test]
    fn specs_are_sorted_by_name() {
        let registry = super::builtin();
        let specs = registry.specs();
        let names: Vec<&str> = specs.iter().map(|spec| spec.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
        assert_eq!(specs.len(), 9);
    }

    #[test]
    fn registry_accepts_dynamic_tools() {
        let registry = super::builtin().with_many(Vec::new());
        assert!(registry.get("Read").is_some());
    }

    #[test]
    fn unknown_tool_is_none() {
        let registry = super::builtin();
        assert!(registry.get("Nonexistent").is_none());
    }
}
