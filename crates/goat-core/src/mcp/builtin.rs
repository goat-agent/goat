pub mod computer;
mod gmail;
pub(crate) mod google;
mod mime;

use std::sync::Arc;

use rmcp::service::{RoleServer, RunningService, ServiceExt};

use super::Context;

const PIPE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    Gmail,
    Computer,
}

#[derive(Debug)]
pub(crate) enum Running {
    Gmail(RunningService<RoleServer, gmail::Gmail>),
    Computer(RunningService<RoleServer, computer::Computer>),
}

impl Builtin {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "gmail" => Some(Self::Gmail),
            "computer" => Some(Self::Computer),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gmail => "gmail",
            Self::Computer => "computer",
        }
    }

    pub fn needs_sign_in(self) -> bool {
        match self {
            Self::Gmail => true,
            Self::Computer => false,
        }
    }

    pub fn forget_grants(self, logins: &crate::store::Logins) -> Result<(), crate::store::Error> {
        match self {
            Self::Gmail => Ok(()),
            Self::Computer => computer::forget_grants(logins),
        }
    }

    pub fn missing_permissions(self, ask: bool) -> Result<Vec<&'static str>, String> {
        match self {
            Self::Gmail => Ok(Vec::new()),
            Self::Computer => computer::missing_permissions(ask),
        }
    }

    pub(crate) fn scopes(self) -> &'static [&'static str] {
        match self {
            Self::Gmail => gmail::SCOPES,
            Self::Computer => &[],
        }
    }

    pub(crate) async fn serve(
        self,
        pipe: tokio::io::DuplexStream,
        context: &Context,
        key: String,
    ) -> Result<Running, String> {
        let failed = |err| format!("{} did not start: {err}", self.as_str());
        match self {
            Self::Gmail => {
                let access = google::Access::new(context, key);
                let server =
                    gmail::Gmail::new(&context.endpoints.gmail, context.http.clone(), access);
                server.serve(pipe).await.map(Running::Gmail).map_err(failed)
            }
            Self::Computer => {
                let desktop = computer::start(&context.home.join(".goat")).map_err(|fault| {
                    format!("{} did not start: {}", self.as_str(), fault.message())
                })?;
                let server = computer::Computer::new(
                    desktop,
                    Arc::clone(&context.logins),
                    &context.home.join(".goat"),
                );
                server
                    .serve(pipe)
                    .await
                    .map(Running::Computer)
                    .map_err(failed)
            }
        }
    }
}

impl Running {
    pub(crate) async fn cancel(self) {
        match self {
            Self::Gmail(service) => drop(service.cancel().await),
            Self::Computer(service) => drop(service.cancel().await),
        }
    }
}

pub(crate) fn pipe() -> (tokio::io::DuplexStream, tokio::io::DuplexStream) {
    tokio::io::duplex(PIPE_BYTES)
}
