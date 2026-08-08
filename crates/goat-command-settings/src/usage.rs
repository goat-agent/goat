mod screen;

use goat_command::{Command, CommandEffect, CommandInvocation, Session};

pub use screen::UsageScreen;

pub struct Usage;

impl Command for Usage {
    fn name(&self) -> &'static str {
        "usage"
    }

    fn description(&self) -> &'static str {
        "show token usage and rate limits"
    }

    fn run(&self, _invocation: CommandInvocation, session: &mut dyn Session) -> CommandEffect {
        let model = session.current_model().cloned();
        let context_window = model.as_ref().and_then(|target| {
            session
                .models()
                .iter()
                .find(|entry| entry.provider == target.provider && entry.model == target.model)
                .and_then(|entry| entry.context_window)
        });
        CommandEffect::Show(Box::new(UsageScreen::new(
            session.accounts().to_vec(),
            session.usage().clone(),
            context_window,
            model,
        )))
    }
}
