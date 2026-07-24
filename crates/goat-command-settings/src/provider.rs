use goat_command::{Command, CommandEffect, CommandInvocation};

pub struct Provider;

impl Command for Provider {
    fn name(&self) -> &'static str {
        "provider"
    }

    fn description(&self) -> &'static str {
        "manage model providers"
    }

    fn run(&self, _invocation: CommandInvocation) -> CommandEffect {
        CommandEffect::OpenConfig
    }
}
