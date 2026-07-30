use std::time::Duration;

use rmcp::RoleClient;
use rmcp::model::{ClientCapabilities, ClientInfo, ErrorCode, Implementation, ProtocolVersion};
use rmcp::service::{
    ClientInitializeError, ClientLifecycleMode, RunningService, serve_client_with_lifecycle_and_ct,
};
use rmcp::transport::IntoTransport;
use tokio_util::sync::CancellationToken;

use crate::McpError;

const MODERN: ProtocolVersion = ProtocolVersion::V_2026_07_28;
const LEGACY: ProtocolVersion = ProtocolVersion::V_2025_11_25;

pub const START_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) const PREFERRED: Era = Era::Legacy;

pub(crate) type Client = RunningService<RoleClient, ClientInfo>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Era {
    Legacy,
    Modern,
}

#[derive(Debug)]
pub(crate) enum Failed {
    TimedOut,
    Wire(ClientInitializeError),
    Rejected(ClientInitializeError),
    Fatal(ClientInitializeError),
}

pub(crate) async fn open<T, E, A>(era: Era, transport: T) -> Result<Client, Failed>
where
    T: IntoTransport<RoleClient, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    let token = CancellationToken::new();
    let served =
        serve_client_with_lifecycle_and_ct(client_info(), transport, lifecycle(era), token);
    match tokio::time::timeout(START_TIMEOUT, served).await {
        Ok(Ok(client)) => Ok(client),
        Ok(Err(error)) => Err(sort(error)),
        Err(_) => Err(Failed::TimedOut),
    }
}

pub(crate) fn other_era(tried: Era, failure: &Failed) -> Option<Era> {
    match (tried, failure) {
        (_, Failed::Fatal(_) | Failed::Wire(_)) | (Era::Legacy, Failed::TimedOut) => None,
        (Era::Modern, Failed::TimedOut | Failed::Rejected(_)) => Some(Era::Legacy),
        (Era::Legacy, Failed::Rejected(_)) => Some(Era::Modern),
    }
}

pub(crate) fn message(failure: &Failed) -> String {
    match failure {
        Failed::TimedOut => format!("timed out after {}s", START_TIMEOUT.as_secs()),
        Failed::Wire(error) | Failed::Rejected(error) | Failed::Fatal(error) => error.to_string(),
    }
}

pub(crate) fn into_error(server: &str, failure: &Failed) -> McpError {
    McpError::Initialize {
        server: server.to_owned(),
        message: message(failure),
    }
}

fn sort(error: ClientInitializeError) -> Failed {
    if peer_is_modern(&error) || giving_up(&error) {
        Failed::Fatal(error)
    } else if wire_failed(&error) {
        Failed::Wire(error)
    } else {
        Failed::Rejected(error)
    }
}

fn peer_is_modern(error: &ClientInitializeError) -> bool {
    match error {
        ClientInitializeError::JsonRpcError(data) => {
            data.code == ErrorCode::UNSUPPORTED_PROTOCOL_VERSION
        }
        ClientInitializeError::NoCompatibleProtocolVersion { .. } => true,
        _ => false,
    }
}

fn giving_up(error: &ClientInitializeError) -> bool {
    matches!(
        error,
        ClientInitializeError::Cancelled | ClientInitializeError::NoPreferredProtocolVersion
    )
}

fn wire_failed(error: &ClientInitializeError) -> bool {
    matches!(
        error,
        ClientInitializeError::TransportError { .. } | ClientInitializeError::ConnectionClosed(_)
    )
}

fn lifecycle(era: Era) -> ClientLifecycleMode {
    match era {
        Era::Legacy => ClientLifecycleMode::Initialize,
        Era::Modern => ClientLifecycleMode::Discover {
            preferred_versions: vec![MODERN],
        },
    }
}

fn client_info() -> ClientInfo {
    ClientInfo::new(
        ClientCapabilities::default(),
        Implementation::new("goat", env!("CARGO_PKG_VERSION")),
    )
    .with_protocol_version(LEGACY)
}

#[cfg(test)]
mod tests {
    use rmcp::model::ErrorData;

    use super::*;

    fn rejected_with(code: i32) -> Failed {
        sort(ClientInitializeError::JsonRpcError(ErrorData::new(
            ErrorCode(code),
            "boom",
            None,
        )))
    }

    fn closed() -> Failed {
        sort(ClientInitializeError::ConnectionClosed("gone".to_owned()))
    }

    #[test]
    fn a_modern_peer_or_a_shutdown_ends_the_ladder() {
        assert!(matches!(rejected_with(-32022), Failed::Fatal(_)));
        assert!(matches!(
            sort(ClientInitializeError::NoCompatibleProtocolVersion {
                client_supported: vec![MODERN],
                server_supported: vec![LEGACY],
            }),
            Failed::Fatal(_)
        ));
        assert!(matches!(
            sort(ClientInitializeError::Cancelled),
            Failed::Fatal(_)
        ));
    }

    #[test]
    fn a_broken_wire_is_not_an_era_problem() {
        assert!(matches!(closed(), Failed::Wire(_)));
    }

    #[test]
    fn an_unexplained_rejection_is_worth_the_other_era() {
        assert!(matches!(rejected_with(-32000), Failed::Rejected(_)));
        assert!(matches!(rejected_with(-32601), Failed::Rejected(_)));
    }

    #[test]
    fn the_ladder_runs_both_ways() {
        assert_eq!(
            other_era(Era::Legacy, &rejected_with(-32000)),
            Some(Era::Modern)
        );
        assert_eq!(
            other_era(Era::Modern, &rejected_with(-32000)),
            Some(Era::Legacy)
        );
    }

    #[test]
    fn a_settled_or_broken_attempt_is_never_retried() {
        for tried in [Era::Legacy, Era::Modern] {
            assert_eq!(other_era(tried, &rejected_with(-32022)), None);
            assert_eq!(other_era(tried, &closed()), None);
        }
    }

    #[test]
    fn only_a_silent_modern_attempt_falls_back() {
        assert_eq!(other_era(Era::Legacy, &Failed::TimedOut), None);
        assert_eq!(other_era(Era::Modern, &Failed::TimedOut), Some(Era::Legacy));
    }

    #[test]
    fn goat_advertises_the_legacy_revision_under_its_own_name() {
        let info = client_info();
        assert_eq!(info.protocol_version, LEGACY);
        assert_eq!(info.client_info.name, "goat");
        assert_eq!(info.client_info.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn each_era_has_its_own_lifecycle() {
        assert!(matches!(
            lifecycle(Era::Legacy),
            ClientLifecycleMode::Initialize
        ));
        let ClientLifecycleMode::Discover { preferred_versions } = lifecycle(Era::Modern) else {
            panic!("expected the discover lifecycle");
        };
        assert_eq!(preferred_versions, vec![MODERN]);
    }

    #[test]
    fn a_timeout_reports_the_budget_it_spent() {
        let rendered = message(&Failed::TimedOut);
        assert!(rendered.contains(&START_TIMEOUT.as_secs().to_string()));
    }
}
