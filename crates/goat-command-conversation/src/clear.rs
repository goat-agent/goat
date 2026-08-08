use goat_command::{Command, CommandEffect, CommandInvocation};

pub struct Clear;

impl Command for Clear {
    fn name(&self) -> &'static str {
        "clear"
    }

    fn description(&self) -> &'static str {
        "start a new conversation"
    }

    fn run(
        &self,
        _invocation: CommandInvocation,
        _session: &mut dyn goat_command::Session,
    ) -> CommandEffect {
        CommandEffect::Dispatch(vec![goat_protocol::Op::Clear {}])
    }
}
