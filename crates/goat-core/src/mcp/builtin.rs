mod calendar;
pub mod computer;
mod contacts;
mod docs;
mod drive;
mod gmail;
pub mod google;
mod mime;
mod sheets;
mod slides;

use std::sync::Arc;

use rmcp::service::{RoleServer, RunningService, ServiceExt};

use super::Context;

const PIPE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    Google(google::Product),
    Computer,
}

#[derive(Debug)]
pub(crate) enum Running {
    Google(RunningService<RoleServer, google::Server>),
    Computer(RunningService<RoleServer, computer::Computer>),
}

impl Builtin {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "computer" => Some(Self::Computer),
            _ => google::Product::parse(name).map(Self::Google),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Google(product) => product.as_str(),
            Self::Computer => "computer",
        }
    }

    pub fn needs_sign_in(self) -> bool {
        match self {
            Self::Google(_) => true,
            Self::Computer => false,
        }
    }

    pub fn forget_grants(self, logins: &crate::store::Logins) -> Result<(), crate::store::Error> {
        match self {
            Self::Google(_) => Ok(()),
            Self::Computer => computer::forget_grants(logins),
        }
    }

    pub fn missing_permissions(self, ask: bool) -> Result<Vec<&'static str>, String> {
        match self {
            Self::Google(_) => Ok(Vec::new()),
            Self::Computer => computer::missing_permissions(ask),
        }
    }

    pub(crate) fn scopes(self) -> &'static [&'static str] {
        match self {
            Self::Google(product) => product.scopes(),
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
            Self::Google(product) => google::Server::new(product, context, key)
                .serve(pipe)
                .await
                .map(Running::Google)
                .map_err(failed),
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
            Self::Google(service) => drop(service.cancel().await),
            Self::Computer(service) => drop(service.cancel().await),
        }
    }
}

pub(crate) fn pipe() -> (tokio::io::DuplexStream, tokio::io::DuplexStream) {
    tokio::io::duplex(PIPE_BYTES)
}
