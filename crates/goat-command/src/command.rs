use crate::{CommandEffect, CommandInvocation, CommandShape, CommandSpec};

pub trait Command: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn aliases(&self) -> &'static [&'static str] {
        &[]
    }
    fn shape(&self) -> CommandShape {
        CommandShape::Empty
    }
    fn run(&self, invocation: CommandInvocation) -> CommandEffect;
    fn spec(&self) -> CommandSpec {
        CommandSpec {
            name: self.name().to_owned(),
            description: self.description().to_owned(),
            aliases: self
                .aliases()
                .iter()
                .map(|alias| (*alias).to_owned())
                .collect(),
            shape: self.shape(),
        }
    }
}
