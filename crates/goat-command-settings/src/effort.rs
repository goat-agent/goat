use goat_command::{
    Command, CommandEffect, CommandInvocation, CommandShape, ParameterSpec, ParameterValue,
};

pub struct Effort;

impl Command for Effort {
    fn name(&self) -> &'static str {
        "effort"
    }

    fn description(&self) -> &'static str {
        "set reasoning effort"
    }

    fn shape(&self) -> CommandShape {
        CommandShape::Parameters(vec![ParameterSpec {
            name: "level".to_owned(),
            description: "reasoning effort level".to_owned(),
            required: false,
            value: ParameterValue::Word,
        }])
    }

    fn run(&self, invocation: CommandInvocation) -> CommandEffect {
        if let Some(level) = invocation.text("level") {
            CommandEffect::SelectEffort(level.to_ascii_lowercase())
        } else {
            CommandEffect::OpenEffortPicker
        }
    }
}
