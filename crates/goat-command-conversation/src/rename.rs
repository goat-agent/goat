use goat_command::{
    Command, CommandEffect, CommandInvocation, CommandShape, ParameterSpec, ParameterValue,
};

pub struct Rename;

impl Command for Rename {
    fn name(&self) -> &'static str {
        "rename"
    }

    fn description(&self) -> &'static str {
        "rename the current conversation"
    }

    fn shape(&self) -> CommandShape {
        CommandShape::Parameters(vec![ParameterSpec {
            name: "title".to_owned(),
            description: "new conversation title".to_owned(),
            required: true,
            value: ParameterValue::TextTail,
        }])
    }

    fn run(&self, invocation: CommandInvocation) -> CommandEffect {
        CommandEffect::RenameConversation(invocation.text("title").unwrap_or_default().to_owned())
    }
}
