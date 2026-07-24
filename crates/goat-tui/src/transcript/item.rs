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
