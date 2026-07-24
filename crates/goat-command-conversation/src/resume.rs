use goat_command::{
    Command, CommandEffect, CommandInvocation, CommandShape, ParameterSpec, ParameterValue,
};

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

    fn run(&self, invocation: CommandInvocation) -> CommandEffect {
        if let Some(n) = invocation.integer("n") {
            match usize::try_from(n) {
                Ok(n) if n >= 1 => CommandEffect::ResumeIndex(n - 1),
                _ => CommandEffect::Error("resume index must be at least 1".to_owned()),
            }
        } else {
            CommandEffect::OpenThreadPicker
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Resume;
    use goat_command::{Command, CommandEffect, CommandInvocation, ParsedParameter, ParsedValue};

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
        let effect = Resume.run(invocation(Vec::new()));
        assert!(matches!(effect, CommandEffect::OpenThreadPicker));
    }

    #[test]
    fn positive_index_is_zero_based() {
        let effect = Resume.run(invocation(vec![ParsedParameter {
            name: "n".to_owned(),
            value: ParsedValue::Integer(3),
        }]));
        assert!(matches!(effect, CommandEffect::ResumeIndex(2)));
    }

    #[test]
    fn zero_or_negative_is_error() {
        for value in [0, -1] {
            let effect = Resume.run(invocation(vec![ParsedParameter {
                name: "n".to_owned(),
                value: ParsedValue::Integer(value),
            }]));
            assert!(matches!(effect, CommandEffect::Error(_)));
        }
    }
}
