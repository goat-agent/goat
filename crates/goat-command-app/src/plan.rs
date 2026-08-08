use goat_command::{Command, CommandEffect, CommandInvocation};

pub struct Plan;

impl Command for Plan {
    fn name(&self) -> &'static str {
        "plan"
    }

    fn description(&self) -> &'static str {
        "toggle plan mode"
    }

    fn run(
        &self,
        _invocation: CommandInvocation,
        _session: &mut dyn goat_command::Session,
    ) -> CommandEffect {
        CommandEffect::TogglePlanMode
    }
}
