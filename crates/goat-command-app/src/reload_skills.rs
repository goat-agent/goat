use goat_command::{Command, CommandEffect, CommandInvocation};

pub struct ReloadSkills;

impl Command for ReloadSkills {
    fn name(&self) -> &'static str {
        "reload-skills"
    }

    fn description(&self) -> &'static str {
        "reload skills from disk"
    }

    fn run(
        &self,
        _invocation: CommandInvocation,
        _session: &mut dyn goat_command::Session,
    ) -> CommandEffect {
        CommandEffect::Dispatch(vec![goat_protocol::Op::ReloadSkills {}])
    }
}
