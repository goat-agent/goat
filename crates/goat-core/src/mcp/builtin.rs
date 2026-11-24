mod gmail;
pub(crate) mod google;
mod mime;

use rmcp::service::{RoleServer, RunningService, ServiceExt};

use super::Context;

const PIPE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    Gmail,
}

pub(crate) type Running = RunningService<RoleServer, gmail::Gmail>;

impl Builtin {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "gmail" => Some(Self::Gmail),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gmail => "gmail",
        }
    }

    pub(crate) fn scopes(self) -> &'static [&'static str] {
        match self {
            Self::Gmail => gmail::SCOPES,
        }
    }

    pub(crate) async fn serve(
        self,
        pipe: tokio::io::DuplexStream,
        context: &Context,
        key: String,
    ) -> Result<Running, String> {
        let access = google::Access::new(context, key);
        let server = match self {
            Self::Gmail => {
                gmail::Gmail::new(&context.endpoints.gmail, context.http.clone(), access)
            }
        };
        server
            .serve(pipe)
            .await
            .map_err(|err| format!("{} did not start: {err}", self.as_str()))
    }
}

pub(crate) fn pipe() -> (tokio::io::DuplexStream, tokio::io::DuplexStream) {
    tokio::io::duplex(PIPE_BYTES)
}
