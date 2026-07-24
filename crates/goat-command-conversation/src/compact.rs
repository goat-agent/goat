use goat_command::{
    Command, CommandEffect, CommandInvocation, CommandShape, ParameterSpec, ParameterValue,
};

pub struct Compact;

impl Command for Compact {
    fn name(&self) -> &'static str {
        "compact"
    }

    fn description(&self) -> &'static str {
        "summarize the conversation to free context"
    }

    fn shape(&self) -> CommandShape {
        CommandShape::Parameters(vec![ParameterSpec {
            name: "focus".to_owned(),
            description: "optional summarization focus".to_owned(),
            required: false,
            value: ParameterValue::TextTail,
        }])
    }

    fn run(&self, invocation: CommandInvocation) -> CommandEffect {
        CommandEffect::CompactConversation(invocation.text("focus").map(str::to_owned))
    }
}
