use serde::{Deserialize, Serialize};

use crate::{
    AccountEntry, ConversationSummary, InputAttachment, LoginProvider, Mode, ModelEntry,
    ModelTarget, ProcessExitReason, ProcessInfo, RateLimitSnapshot, RewindDraft, RewindPoint,
    RunId, SkillInfo, SubagentGroupMember, TaskId, ToolCall, ToolCallId, ToolOutcome,
    TranscriptEntry, Usage,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotifyKind {
    Info,
    Success,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type")]
pub enum Event {
    TaskStarted {
        id: TaskId,
    },
    TextDelta {
        id: TaskId,
        chunk: String,
    },
    TextDone {
        id: TaskId,
        text: String,
    },
    ToolStarted {
        id: TaskId,
        call: ToolCall,
    },
    ToolDone {
        id: TaskId,
        call: ToolCallId,
        outcome: ToolOutcome,
    },
    SubagentGroupStarted {
        id: TaskId,
        group: ToolCallId,
        members: Vec<SubagentGroupMember>,
    },
    ShellDone {
        id: TaskId,
        output: String,
    },
    TaskDone {
        id: TaskId,
        interrupted: bool,
    },
    SubagentStarted {
        id: TaskId,
        parent: TaskId,
        call: ToolCallId,
        subagent_type: String,
        label: String,
    },
    SubagentDone {
        id: TaskId,
        ok: bool,
    },
    ModelListChanged {
        entries: Vec<ModelEntry>,
    },
    ModelSelected {
        target: ModelTarget,
    },
    ModeChanged {
        mode: Mode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plan_path: Option<String>,
    },
    PlanProposed {
        id: TaskId,
        call: ToolCallId,
        plan: String,
        path: String,
    },
    ConversationsListed {
        conversations: Vec<ConversationSummary>,
    },
    RewindPointsListed {
        points: Vec<RewindPoint>,
    },
    ConversationRewound {
        draft: RewindDraft,
    },
    FilesListed {
        entries: Vec<String>,
    },
    ConversationRestored {
        target: ModelTarget,
        entries: Vec<TranscriptEntry>,
        context_tokens: Option<u32>,
        compaction_threshold: Option<u32>,
    },
    ThinkingDelta {
        id: TaskId,
        chunk: String,
    },
    LoginProviders {
        providers: Vec<LoginProvider>,
    },
    LoginStatus {
        provider: String,
        message: String,
        done: bool,
        ok: bool,
    },
    AccountsChanged {
        providers: Vec<AccountEntry>,
    },
    SkillsChanged {
        skills: Vec<SkillInfo>,
    },
    Error {
        id: Option<TaskId>,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hint: Option<String>,
    },
    Notify {
        kind: NotifyKind,
        message: String,
    },
    AskStarted {
        id: TaskId,
        call: ToolCallId,
        questions: Vec<AskQuestion>,
    },
    AskDismissed {
        id: TaskId,
        call: ToolCallId,
    },
    Usage {
        id: TaskId,
        provider: String,
        account: String,
        usage: Usage,
        context_window: Option<u32>,
        compaction_threshold: Option<u32>,
    },
    RateLimits {
        provider: String,
        account: String,
        snapshot: RateLimitSnapshot,
        cached_at: i64,
    },
    Retrying {
        id: TaskId,
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resets_at: Option<i64>,
    },
    UserMessage {
        id: TaskId,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        system: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<InputAttachment>,
    },
    MessageDequeued {
        id: TaskId,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        system: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<InputAttachment>,
    },
    CompactionStarted {
        id: TaskId,
    },
    CompactionDone {
        id: TaskId,
        ok: bool,
        tokens_before: u32,
        tokens_after: u32,
        usage: Usage,
    },
    ConversationBound {
        conversation_id: i64,
    },
    ProcessStarted {
        process: RunId,
        command: String,
        watched: bool,
    },
    ProcessOutput {
        process: RunId,
        chunk: String,
    },
    ProcessExited {
        process: RunId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<i32>,
        reason: ProcessExitReason,
    },
    ProcessObserved {
        id: TaskId,
        process: RunId,
        command: String,
        output: String,
    },
    ProcessListChanged {
        processes: Vec<ProcessInfo>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AskOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AskQuestion {
    pub question: String,
    #[serde(default)]
    pub options: Vec<AskOption>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub multiple: bool,
}

#[cfg(test)]
mod tests {
    use super::AskQuestion;

    #[test]
    fn multiple_defaults_to_false_when_absent() {
        let q: AskQuestion = serde_json::from_str(r#"{"question":"Pick?"}"#).unwrap();
        assert!(!q.multiple);
    }

    #[test]
    fn multiple_false_is_omitted_from_serialization() {
        let q = AskQuestion {
            question: "Pick?".to_owned(),
            options: Vec::new(),
            multiple: false,
        };
        let json = serde_json::to_string(&q).unwrap();
        assert!(!json.contains("multiple"));
    }

    #[test]
    fn multiple_true_round_trips() {
        let q: AskQuestion =
            serde_json::from_str(r#"{"question":"Pick?","multiple":true}"#).unwrap();
        assert!(q.multiple);
        let json = serde_json::to_string(&q).unwrap();
        assert!(json.contains("\"multiple\":true"));
    }
}
