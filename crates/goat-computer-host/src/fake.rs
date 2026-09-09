use parking_lot::Mutex;
use std::collections::VecDeque;

use goat_api::{ComputerCommand, HostComputerOutput};

use crate::{Desktop, DesktopError};

#[derive(Default)]
pub struct FakeDesktop {
    commands: Mutex<Vec<ComputerCommand>>,
    outputs: Mutex<VecDeque<Result<HostComputerOutput, DesktopError>>>,
}

impl FakeDesktop {
    pub fn push(&self, result: Result<HostComputerOutput, DesktopError>) {
        self.outputs.lock().push_back(result);
    }

    pub fn commands(&self) -> Vec<ComputerCommand> {
        self.commands.lock().clone()
    }
}

impl Desktop for FakeDesktop {
    fn run(&self, command: ComputerCommand) -> Result<HostComputerOutput, DesktopError> {
        self.commands.lock().push(command);
        self.outputs.lock().pop_front().unwrap_or_else(|| {
            Err(DesktopError::NotStarted(
                "no scripted computer result".into(),
            ))
        })
    }
}
