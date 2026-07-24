use goat_command::{
    Command, CommandEffect, CommandInvocation, CommandShape, ParameterSpec, ParameterValue,
};

pub struct Model;

impl Command for Model {
    fn name(&self) -> &'static str {
        "model"
    }

    fn description(&self) -> &'static str {
        "switch model"
    }

    fn shape(&self) -> CommandShape {
        CommandShape::Parameters(vec![ParameterSpec {
            name: "name".to_owned(),
            description: "model name".to_owned(),
            required: false,
            value: ParameterValue::TextTail,
        }])
    }

    fn run(&self, invocation: CommandInvocation) -> CommandEffect {
        if let Some(name) = invocation.text("name") {
            CommandEffect::SelectModelNamed(name.to_owned())
        } else {
            CommandEffect::OpenModelPicker
        }
    }
}
