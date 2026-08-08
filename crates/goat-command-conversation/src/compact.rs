use goat_command::{
    Command, CommandEffect, CommandInvocation, CommandShape, ParameterSpec, ParameterValue,
};

pub struct Compact;

impl Command for Compact {
    fn name(&self) -> &'static str {
        "compact"
    }

    fn description(&self) -> &'static str {
        "summarize the conversation to free context"
    }

    fn shape(&self) -> CommandShape {
        CommandShape::Parameters(vec![ParameterSpec {
            name: "focus".to_owned(),
            description: "optional summarization focus".to_owned(),
            required: false,
            value: ParameterValue::TextTail,
        }])
    }

    fn run(
        &self,
        invocation: CommandInvocation,
        session: &mut dyn goat_command::Session,
    ) -> CommandEffect {
        if session.is_busy() {
            session.notify(
                goat_protocol::NotifyKind::Info,
                "will compact after the current task".to_owned(),
            );
        }
        CommandEffect::Dispatch(vec![goat_protocol::Op::Compact {
            id: session.allocate_task(),
            instructions: invocation.text("focus").map(str::to_owned),
        }])
    }
}
