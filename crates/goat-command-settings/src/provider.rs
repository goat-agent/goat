use goat_command::{Command, CommandEffect, CommandInvocation, Session};

use crate::config::open_config;

pub struct Provider;

impl Command for Provider {
    fn name(&self) -> &'static str {
        "provider"
    }

    fn description(&self) -> &'static str {
        "manage model providers"
    }

    fn run(&self, _invocation: CommandInvocation, session: &mut dyn Session) -> CommandEffect {
        open_config(session)
    }
}
