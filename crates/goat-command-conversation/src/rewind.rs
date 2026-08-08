mod screen;

use goat_command::{Command, CommandEffect, CommandInvocation, Session};
use goat_protocol::NotifyKind;

pub use screen::RewindScreen;

pub struct Rewind;

impl Command for Rewind {
    fn name(&self) -> &'static str {
        "rewind"
    }

    fn description(&self) -> &'static str {
        "restore code or conversation to an earlier prompt"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["checkpoint", "undo"]
    }

    fn run(&self, _invocation: CommandInvocation, session: &mut dyn Session) -> CommandEffect {
        if session.is_busy() || session.queued_len() > 0 {
            session.notify(
                NotifyKind::Info,
                "finish or interrupt the current task before rewinding".to_owned(),
            );
            CommandEffect::Noop
        } else {
            CommandEffect::Show(Box::new(RewindScreen::new(Vec::new())))
        }
    }
}

#[cfg(test)]
mod tests {
    use goat_command::{Command, CommandEffect, CommandInvocation};

    use super::Rewind;

    #[test]
    fn opens_rewind_screen() {
        let effect = Rewind.run(
            CommandInvocation {
                name: "rewind".to_owned(),
                subcommand: None,
                raw: "/rewind".to_owned(),
                raw_args: String::new(),
                parameters: Vec::new(),
            },
            &mut goat_command::EmptySession::default(),
        );
        assert!(matches!(effect, CommandEffect::Show(_)));
    }
}
