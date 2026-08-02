use goat_command::{Command, CommandEffect, CommandInvocation};

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

    fn run(&self, _invocation: CommandInvocation) -> CommandEffect {
        CommandEffect::OpenRewind
    }
}

#[cfg(test)]
mod tests {
    use goat_command::{Command, CommandEffect, CommandInvocation};

    use super::Rewind;

    #[test]
    fn opens_rewind_picker() {
        let effect = Rewind.run(CommandInvocation {
            name: "rewind".to_owned(),
            subcommand: None,
            raw: "/rewind".to_owned(),
            raw_args: String::new(),
            parameters: Vec::new(),
        });
        assert!(matches!(effect, CommandEffect::OpenRewind));
    }
}
