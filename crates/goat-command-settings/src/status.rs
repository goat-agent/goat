use goat_command::{Command, CommandEffect, CommandInvocation};

pub struct Status;

impl Command for Status {
    fn name(&self) -> &'static str {
        "status"
    }

    fn description(&self) -> &'static str {
        "show session, thread, and daemon status"
    }

    fn run(
        &self,
        _invocation: CommandInvocation,
        _session: &mut dyn goat_command::Session,
    ) -> CommandEffect {
        CommandEffect::OpenStatus
    }
}
