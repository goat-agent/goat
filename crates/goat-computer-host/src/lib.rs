pub mod fake;
#[cfg(target_os = "macos")]
pub mod macos;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use goat_api::{
    CapabilityAdvertise, CapabilityAdvertiseParams, CapabilityOffer, ComputerCommand, Empty,
    HostComputer, HostComputerOutput, Method,
};
use goat_wire::envelope::{CallError, ErrorCode, Execution};
use goat_wire::peer::{CallResult, Handler, Request, unknown_method};

pub const CAPABILITY: &str = HostComputer::NAME;
pub const CAPABILITY_VERSION: u16 = HostComputer::VERSION;
pub const MAX_IN_FLIGHT: usize = 1;
pub const HALTED_MESSAGE: &str =
    "computer use was stopped with the global shortcut; re-enable it in goat-desktop";

#[derive(Debug, thiserror::Error)]
pub enum DesktopError {
    #[error("{0}")]
    NotStarted(String),
    #[error("{0}")]
    OutcomeUnknown(String),
}

impl From<DesktopError> for CallError {
    fn from(error: DesktopError) -> Self {
        match error {
            DesktopError::NotStarted(message) => {
                Self::new(ErrorCode::Denied, message).with_execution(Execution::NotStarted)
            }
            DesktopError::OutcomeUnknown(message) => {
                Self::new(ErrorCode::Denied, message).with_execution(Execution::OutcomeUnknown)
            }
        }
    }
}

pub trait Desktop: Send + Sync {
    fn run(&self, command: ComputerCommand) -> Result<HostComputerOutput, DesktopError>;
}

pub struct ComputerHost {
    desktop: Arc<dyn Desktop>,
    halted: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
}

impl ComputerHost {
    pub fn new(desktop: Arc<dyn Desktop>) -> Self {
        Self {
            desktop,
            halted: Arc::new(AtomicBool::new(false)),
            busy: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn halt(&self) {
        self.halted.store(true, Ordering::SeqCst);
    }
    pub fn resume(&self) {
        self.halted.store(false, Ordering::SeqCst);
    }
    pub fn is_halted(&self) -> bool {
        self.halted.load(Ordering::SeqCst)
    }
    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }
}

struct BusyGuard(Arc<AtomicBool>);

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl Handler for ComputerHost {
    async fn call(&self, request: Request) -> CallResult {
        if request.method != CAPABILITY {
            return Err(unknown_method(&request.method, request.version));
        }
        if request.version != CAPABILITY_VERSION {
            return Err(CallError::new(
                ErrorCode::UnsupportedVersion,
                format!(
                    "{}@{} is not served; this peer speaks [{}]",
                    request.method, request.version, CAPABILITY_VERSION
                ),
            )
            .with_execution(Execution::NotStarted));
        }
        if self.is_halted() {
            return Err(DesktopError::NotStarted(HALTED_MESSAGE.into()).into());
        }
        if request.cancel.is_cancelled() {
            return Err(
                CallError::new(ErrorCode::Canceled, "the turn was interrupted")
                    .with_execution(Execution::NotStarted),
            );
        }
        let command = serde_json::from_value(request.params).map_err(|error| {
            CallError::new(ErrorCode::InvalidParams, error.to_string())
                .with_execution(Execution::NotStarted)
        })?;
        if self
            .busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(CallError::new(
                ErrorCode::Conflict,
                "computer is already executing an action",
            )
            .with_execution(Execution::NotStarted));
        }
        let guard = BusyGuard(self.busy.clone());
        let desktop = self.desktop.clone();
        let halted = self.halted.clone();
        let cancel = request.cancel;
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            if halted.load(Ordering::SeqCst) {
                return Err(DesktopError::NotStarted(HALTED_MESSAGE.into()).into());
            }
            if cancel.is_cancelled() {
                return Err(
                    CallError::new(ErrorCode::Canceled, "the turn was interrupted")
                        .with_execution(Execution::NotStarted),
                );
            }
            let result = desktop.run(command).map_err(CallError::from)?;
            serde_json::to_value(result).map_err(|error| {
                CallError::new(ErrorCode::Internal, error.to_string())
                    .with_execution(Execution::OutcomeUnknown)
            })
        })
        .await
        .map_err(|error| {
            CallError::new(ErrorCode::Internal, error.to_string())
                .with_execution(Execution::OutcomeUnknown)
        })?
    }
}

pub fn advertisement(instance: &str, label: &str, boot_epoch: u64) -> CapabilityAdvertiseParams {
    CapabilityAdvertiseParams {
        instance: instance.into(),
        label: label.into(),
        boot_epoch,
        offers: vec![CapabilityOffer {
            id: CAPABILITY.into(),
            version: CAPABILITY_VERSION,
            max_in_flight: MAX_IN_FLIGHT,
        }],
    }
}

pub fn withdrawal(instance: &str, boot_epoch: u64) -> CapabilityAdvertiseParams {
    CapabilityAdvertiseParams {
        instance: instance.into(),
        label: String::new(),
        boot_epoch,
        offers: Vec::new(),
    }
}

pub async fn advertise(
    api: &goat_api::Api,
    params: CapabilityAdvertiseParams,
) -> Result<Empty, CallError> {
    api.call::<CapabilityAdvertise>(params).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use goat_wire::{
        WireConn,
        envelope::{Frame, Role},
        peer::{RejectAll, spawn},
    };
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn halted_actions_do_not_execute_and_resume_restores_delivery() {
        let desktop = Arc::new(fake::FakeDesktop::default());
        desktop.push(Ok(HostComputerOutput::Point { x: 12.0, y: 24.0 }));
        let host = ComputerHost::new(desktop.clone());
        let (stream, _other) = tokio::io::duplex(4096);
        let (sink, source) = WireConn::<_, Frame, Frame>::new(stream).split();
        let peer = spawn(
            Role::Client,
            Box::pin(sink),
            Box::pin(source),
            Arc::new(RejectAll),
            CancellationToken::new(),
        );
        let request = || Request {
            method: CAPABILITY.into(),
            version: CAPABILITY_VERSION,
            params: serde_json::json!({"command": "cursor_position"}),
            peer: peer.handle.clone(),
            cancel: CancellationToken::new(),
        };
        host.halt();
        let error = host.call(request()).await.unwrap_err();
        assert_eq!(error.execution, Some(Execution::NotStarted));
        assert_eq!(error.code, ErrorCode::Denied);
        assert!(desktop.commands().is_empty());
        host.resume();
        assert_eq!(
            host.call(request()).await.unwrap(),
            serde_json::json!({"reply": "point", "x": 12.0, "y": 24.0})
        );
        assert_eq!(desktop.commands(), vec![ComputerCommand::CursorPosition {}]);
        assert!(!host.is_busy());
    }
}
