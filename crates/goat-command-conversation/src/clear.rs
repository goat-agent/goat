use goat_command::{Command, CommandEffect, CommandInvocation};

pub struct Clear;

impl Command for Clear {
    fn name(&self) -> &'static str {
        "clear"
    }

    fn description(&self) -> &'static str {
        "start a new conversation"
    }

    fn run(&self, _invocation: CommandInvocation) -> CommandEffect {
        CommandEffect::ClearConversation
    }
}
