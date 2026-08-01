use std::time::Instant;

use goat_protocol::{InputAttachment, TaskId, ToolCallId, ToolDisplay, ToolOutcome};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UserMessage {
    pub text: String,
    pub attachments: Vec<InputAttachment>,
}

pub(crate) struct Working {
    pub elapsed: Option<u64>,
    pub label: Option<String>,
    pub thinking: bool,
    pub tokens: Option<u64>,
}

#[derive(Debug)]
pub(crate) enum ToolStatus {
    Running,
    Done(ToolOutcome),
}

#[derive(Debug)]
pub(crate) enum ShellStatus {
    Running,
    Done(String),
}

#[derive(Debug)]
pub(crate) enum SubagentMemberStatus {
    Pending,
    Running,
    Done(ToolOutcome),
}

#[derive(Debug)]
pub(crate) struct SubagentGroupMemberView {
    pub(crate) call: ToolCallId,
    pub(crate) subagent_type: String,
    pub(crate) label: String,
    pub(crate) background: bool,
    pub(crate) status: SubagentMemberStatus,
    pub(crate) tools: u64,
    pub(crate) tokens: u64,
    pub(crate) started_at: Option<Instant>,
    pub(crate) finished_at: Option<Instant>,
}

#[derive(Debug)]
pub(crate) struct SubagentGroupView {
    pub(crate) parent: TaskId,
    pub(crate) group: ToolCallId,
    pub(crate) members: Vec<SubagentGroupMemberView>,
    pub(crate) started_at: Option<Instant>,
    pub(crate) finished_at: Option<Instant>,
}

#[derive(Debug)]
pub(crate) enum Item {
    User(UserMessage),
    Agent(String),
    Thinking {
        text: String,
        collapsed: bool,
    },
    Tool {
        id: ToolCallId,
        name: String,
        display: ToolDisplay,
        status: ToolStatus,
        image: Option<Box<crate::screenshot::TranscriptImage>>,
    },
    SubagentGroup(SubagentGroupView),
    Shell {
        id: TaskId,
        command: String,
        status: ShellStatus,
    },
    Process {
        command: String,
        output: String,
        running: bool,
        exit_code: Option<i32>,
    },
    Error {
        message: String,
        hint: Option<String>,
    },
    Interrupted,
    Compaction {
        tokens_before: u32,
        tokens_after: u32,
    },
}
