mod screen;

use goat_command::{Command, CommandEffect, CommandInvocation, Session};

pub use screen::ConfigScreen;

pub struct Config;

pub(crate) fn open_config(session: &mut dyn Session) -> CommandEffect {
    let snapshot = session.snapshot();
    CommandEffect::Show(Box::new(ConfigScreen::new(
        session.accounts().to_vec(),
        snapshot.dark_theme,
        snapshot.mouse_capture,
        snapshot.computer_use,
        snapshot.browser,
    )))
}

impl Command for Config {
    fn name(&self) -> &'static str {
        "config"
    }

    fn description(&self) -> &'static str {
        "configure providers and settings"
    }

    fn run(&self, _invocation: CommandInvocation, session: &mut dyn Session) -> CommandEffect {
        open_config(session)
    }
}
