mod chunk;
mod credential;
mod credentials;
mod error;
mod model;
mod provider;
mod request;
mod response;
mod setup;

pub use chunk::*;
pub use credential::*;
pub use credentials::*;
pub use error::*;
pub use model::*;
pub use provider::*;
pub use request::*;
pub use response::*;
pub use setup::*;

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
