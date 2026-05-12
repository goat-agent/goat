use serde::{Deserialize, Serialize};

use crate::{LlmChunk, LlmError, Model, StopReason, Usage};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LlmResponse {
    pub text: String,
    pub stop: StopReason,
    pub usage: Usage,
    pub model: Model,
}

#[derive(Default)]
pub struct LlmResponseBuilder {
    text: String,
    stop: Option<StopReason>,
    usage: Option<Usage>,
    model: Option<Model>,
}

impl LlmResponseBuilder {
    pub fn push(&mut self, chunk: LlmChunk) {
        match chunk {
            LlmChunk::MessageStart {
                model,
                input_tokens,
                ..
            } => {
                self.model = Some(model);
                if let Some(u) = self.usage.as_mut() {
                    u.input = input_tokens;
                } else {
                    self.usage = Some(Usage {
                        input: input_tokens,
                        output: 0,
                    });
                }
            }
            LlmChunk::TextDelta { text, .. } => self.text.push_str(&text),
            LlmChunk::MessageEnd { stop, usage } => {
                self.stop = Some(stop);
                if let Some(u) = self.usage.as_mut() {
                    u.output = usage.output;
                    if usage.input > 0 {
                        u.input = usage.input;
                    }
                } else {
                    self.usage = Some(usage);
                }
            }
            _ => {}
        }
    }

    pub fn finish(self) -> Result<LlmResponse, LlmError> {
        let model = self
            .model
            .ok_or_else(|| LlmError::Provider("stream emitted no MessageStart".into()))?;
        Ok(LlmResponse {
            text: self.text,
            stop: self.stop.unwrap_or(StopReason::EndTurn),
            usage: self.usage.unwrap_or_default(),
            model,
        })
    }
}
