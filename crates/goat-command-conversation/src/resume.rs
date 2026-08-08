mod screen;

use goat_command::{
    Command, CommandEffect, CommandInvocation, CommandShape, ParameterSpec, ParameterValue, Session,
};
use goat_protocol::NotifyKind;

pub use screen::ResumeScreen;

pub struct Resume;

impl Command for Resume {
    fn name(&self) -> &'static str {
        "resume"
    }

    fn description(&self) -> &'static str {
        "resume a past conversation"
    }

    fn shape(&self) -> CommandShape {
        CommandShape::Parameters(vec![ParameterSpec {
            name: "n".to_owned(),
            description: "conversation number".to_owned(),
            required: false,
            value: ParameterValue::Integer,
        }])
    }

    fn run(&self, invocation: CommandInvocation, session: &mut dyn Session) -> CommandEffect {
        if let Some(index) = invocation.integer("n") {
            match usize::try_from(index) {
                Ok(index) if index >= 1 => {
                    CommandEffect::Show(Box::new(ResumeScreen::indexed(index - 1)))
                }
                _ => {
                    session.notify(
                        NotifyKind::Error,
                        "resume index must be at least 1".to_owned(),
                    );
                    CommandEffect::Noop
                }
            }
        } else {
            CommandEffect::Show(Box::new(ResumeScreen::new(session.threads().to_vec())))
        }
    }
}

#[cfg(test)]
mod tests {
    use goat_command::{Command, CommandEffect, CommandInvocation, ParsedParameter, ParsedValue};

    use super::Resume;

    fn invocation(parameters: Vec<ParsedParameter>) -> CommandInvocation {
        CommandInvocation {
            name: "resume".to_owned(),
            subcommand: None,
            raw: "/resume".to_owned(),
            raw_args: String::new(),
            parameters,
        }
    }

    #[test]
    fn bare_opens_picker() {
        let effect = Resume.run(
            invocation(Vec::new()),
            &mut goat_command::EmptySession::default(),
        );
        assert!(matches!(effect, CommandEffect::Show(_)));
    }

    #[test]
    fn positive_index_opens_hidden_interaction() {
        let effect = Resume.run(
            invocation(vec![ParsedParameter {
                name: "n".to_owned(),
                value: ParsedValue::Integer(3),
            }]),
            &mut goat_command::EmptySession::default(),
        );
        assert!(matches!(effect, CommandEffect::Show(_)));
    }

    #[test]
    fn zero_or_negative_notifies() {
        for value in [0, -1] {
            let mut session = goat_command::EmptySession::default();
            let effect = Resume.run(
                invocation(vec![ParsedParameter {
                    name: "n".to_owned(),
                    value: ParsedValue::Integer(value),
                }]),
                &mut session,
            );
            assert!(matches!(effect, CommandEffect::Noop));
            assert_eq!(session.notifications().len(), 1);
        }
    }
}
