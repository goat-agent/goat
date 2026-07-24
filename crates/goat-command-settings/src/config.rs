use goat_command::{Command, CommandEffect, CommandInvocation};

pub struct Config;

impl Command for Config {
    fn name(&self) -> &'static str {
        "config"
    }

    fn description(&self) -> &'static str {
        "configure providers and settings"
    }

    fn run(&self, _invocation: CommandInvocation) -> CommandEffect {
        CommandEffect::OpenConfig
    }
}
