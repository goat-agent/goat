use goat_command::{Command, CommandEffect, CommandInvocation};

pub struct Help;

impl Command for Help {
    fn name(&self) -> &'static str {
        "help"
    }

    fn description(&self) -> &'static str {
        "show keybindings and commands"
    }

    fn run(&self, _invocation: CommandInvocation) -> CommandEffect {
        CommandEffect::ShowHelp
    }
}
