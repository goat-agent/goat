use serde::{Deserialize, Serialize};

use crate::Model;

#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug, Serialize, Deserialize)]
pub struct BlockId(pub u32);

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input: u32,
    pub output: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    ToolUse,
    Stop,
    Refused,
    Error,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum LlmChunk {
    MessageStart {
        id: String,
        model: Model,
        input_tokens: u32,
    },
    TextDelta {
        block: BlockId,
        text: String,
    },
    ReasoningDelta {
        block: BlockId,
        text: String,
    },
    ToolCallStart {
        block: BlockId,
        tool_call_id: String,
        name: String,
    },
    ToolCallDelta {
        block: BlockId,
        args_json_fragment: String,
    },
    BlockEnd {
        block: BlockId,
    },
    MessageEnd {
        stop: StopReason,
        usage: Usage,
    },
}
