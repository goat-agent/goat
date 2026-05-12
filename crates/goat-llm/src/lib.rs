mod chunk;
mod error;
mod key;
mod model;
mod provider;
mod request;
mod response;

pub use chunk::*;
pub use error::*;
pub use key::*;
pub use model::*;
pub use provider::*;
pub use request::*;
pub use response::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_collects_text_and_stop() {
        let mut b = LlmResponseBuilder::default();
        b.push(LlmChunk::MessageStart {
            id: "m1".into(),
            model: Model::new(ProviderId::new("anthropic"), "claude-x"),
            input_tokens: 10,
        });
        b.push(LlmChunk::TextDelta {
            block: BlockId(0),
            text: "hello ".into(),
        });
        b.push(LlmChunk::TextDelta {
            block: BlockId(0),
            text: "world".into(),
        });
        b.push(LlmChunk::BlockEnd { block: BlockId(0) });
        b.push(LlmChunk::MessageEnd {
            stop: StopReason::EndTurn,
            usage: Usage {
                input: 10,
                output: 3,
            },
        });
        let r = b.finish().unwrap();
        assert_eq!(r.text, "hello world");
        assert!(matches!(r.stop, StopReason::EndTurn));
        assert_eq!(r.usage.output, 3);
    }
}
