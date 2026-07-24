use serde::{Deserialize, Serialize};

use crate::{InputAttachment, LoginCredential, ModelTarget, ProcessId, TaskId, ToolCallId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type")]
pub enum Op {
    SubmitMessage {
        id: TaskId,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<InputAttachment>,
    },
    SubmitShell {
        id: TaskId,
        command: String,
    },
    Interrupt {
        id: TaskId,
    },
    Clear {},
    SelectModel {
        target: ModelTarget,
    },
    Login {
        provider: String,
        credential: LoginCredential,
    },
    AddAccount {
        provider: String,
        name: String,
        credential: LoginCredential,
    },
    RemoveAccount {
        provider: String,
        name: String,
    },
    ListThreads {},
    Resume {
        thread_id: i64,
    },
    ResumeLatest {},
    RenameThread {
        title: String,
    },
    Answer {
        id: TaskId,
        call: ToolCallId,
        answers: Vec<String>,
    },
    Compact {
        id: TaskId,
        instructions: Option<String>,
    },
    DequeueMessage {
        id: TaskId,
    },
    ProcessKill {
        process: ProcessId,
    },
    ProcessWatch {
        process: ProcessId,
        on: bool,
    },
    Shutdown {},
}
