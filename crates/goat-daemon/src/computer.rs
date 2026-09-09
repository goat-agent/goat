use std::sync::Arc;

use goat_api::{ComputerCommand, Holder, HostComputer, Method};
use goat_capability::{Broker, DEFAULT_CALL_DEADLINE};
use goat_tool_computer::{ComputerError, Transport, TransportFuture};
use goat_wire::envelope::Execution;

pub const CAPABILITY: &str = HostComputer::NAME;

pub struct ComputerRelay {
    broker: Arc<Broker>,
    holder: Holder,
}

impl ComputerRelay {
    #[must_use]
    pub fn new(broker: Arc<Broker>, holder: Holder) -> Self {
        Self { broker, holder }
    }
}

impl Transport for ComputerRelay {
    fn call(&self, command: ComputerCommand) -> TransportFuture<'_> {
        Box::pin(async move {
            let params = serde_json::to_value(&command)
                .map_err(|err| ComputerError::Message(err.to_string()))?;
            let value = self
                .broker
                .invoke(&self.holder, CAPABILITY, params, DEFAULT_CALL_DEADLINE)
                .await
                .map_err(|err| {
                    let execution = match err.execution {
                        Some(Execution::NotStarted) => "not_started",
                        Some(Execution::KnownFailed) => "known_failed",
                        Some(Execution::OutcomeUnknown) => "outcome_unknown",
                        None => "unspecified",
                    };
                    ComputerError::Message(format!("{err}; execution={execution}"))
                })?;
            serde_json::from_value(value).map_err(|err| ComputerError::Message(err.to_string()))
        })
    }
}
