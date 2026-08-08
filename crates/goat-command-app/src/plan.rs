mod screen;

use goat_command::{Command, CommandEffect, CommandInvocation, Session};
use goat_protocol::Op;

pub use screen::PlanScreen;

pub struct Plan;

impl Command for Plan {
    fn name(&self) -> &'static str {
        "plan"
    }

    fn description(&self) -> &'static str {
        "toggle plan mode"
    }

    fn run(&self, _invocation: CommandInvocation, session: &mut dyn Session) -> CommandEffect {
        CommandEffect::Dispatch(vec![Op::SetMode {
            mode: session.mode().toggled(),
        }])
    }
}
