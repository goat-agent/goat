use goat_command::{Command, CommandEffect, CommandInvocation};

pub struct Exit;

impl Command for Exit {
    fn name(&self) -> &'static str {
        "exit"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["quit"]
    }

    fn description(&self) -> &'static str {
        "quit goat-code"
    }

    fn run(&self, _invocation: CommandInvocation) -> CommandEffect {
        CommandEffect::Quit
    }
}
