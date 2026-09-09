use std::pin::Pin;

use goat_api::{ComputerCommand, HostComputerOutput};

#[derive(Debug, Clone, thiserror::Error)]
pub enum ComputerError {
    #[error("{0}")]
    Message(String),
}

pub type TransportFuture<'a> =
    Pin<Box<dyn Future<Output = Result<HostComputerOutput, ComputerError>> + Send + 'a>>;

pub trait Transport: Send + Sync {
    fn call(&self, command: ComputerCommand) -> TransportFuture<'_>;
}

pub mod fake {
    use std::collections::VecDeque;

    use parking_lot::Mutex;

    use super::{ComputerCommand, ComputerError, HostComputerOutput, Transport, TransportFuture};

    #[derive(Default)]
    pub struct FakeComputer {
        state: Mutex<State>,
    }

    #[derive(Default)]
    struct State {
        commands: Vec<ComputerCommand>,
        outputs: VecDeque<Result<HostComputerOutput, ComputerError>>,
    }

    impl FakeComputer {
        pub fn push(&self, output: Result<HostComputerOutput, ComputerError>) {
            self.state.lock().outputs.push_back(output);
        }

        pub fn commands(&self) -> Vec<ComputerCommand> {
            self.state.lock().commands.clone()
        }
    }

    impl Transport for FakeComputer {
        fn call(&self, command: ComputerCommand) -> TransportFuture<'_> {
            Box::pin(async move {
                let mut state = self.state.lock();
                state.commands.push(command);
                state.outputs.pop_front().unwrap_or_else(|| {
                    Err(ComputerError::Message(
                        "fake computer has no scripted output".into(),
                    ))
                })
            })
        }
    }
}
