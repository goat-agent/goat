use goat_command::{Command, CommandEffect, CommandInvocation};

pub struct Usage;

impl Command for Usage {
    fn name(&self) -> &'static str {
        "usage"
    }

    fn description(&self) -> &'static str {
        "show token usage and rate limits"
    }

    fn run(&self, _invocation: CommandInvocation) -> CommandEffect {
        CommandEffect::OpenUsage
    }
}
