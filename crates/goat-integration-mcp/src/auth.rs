use goat_auth::{Credential, CredentialKey, CredentialStore};
use goat_integration::{IntegrationError, IntegrationResult};
use goat_mcp::auth::StoredOAuth;
use tracing::info;

use crate::McpService;

pub enum ResolvedAuth {
    Token(String),
    OAuth(StoredOAuth),
}

pub fn resolve(
    service: &McpService,
    credentials: &CredentialStore,
    account: &str,
    client_id: Option<&str>,
) -> IntegrationResult<ResolvedAuth> {
    let name = service.id.as_str();
    let key = CredentialKey::integration(name, account);
    if env_overrides_stored_oauth(service.env_var, credentials, &key) {
        info!(
            integration = name,
            env_var = service.env_var,
            "token from the environment overrides the stored oauth credential",
        );
    }
    match credentials.resolve(&key, service.env_var) {
        Some(Credential::ApiKey(secret) | Credential::ApiKeyWithEndpoint { secret, .. }) => Ok(
            ResolvedAuth::Token(header_value(service.auth_scheme, secret.expose())),
        ),
        Some(Credential::OAuth(_)) => {
            let client_id = client_id.ok_or_else(|| {
                IntegrationError::Config(format!(
                    "{name} connection missing `client_id`; run `goat integration add {name}`"
                ))
            })?;
            Ok(ResolvedAuth::OAuth(StoredOAuth::new(
                credentials.clone(),
                key,
                client_id.to_owned(),
            )))
        }
        None => Err(IntegrationError::Auth(missing_credential(
            name,
            account,
            service.env_var,
        ))),
    }
}

fn missing_credential(name: &str, account: &str, env_var: Option<&str>) -> String {
    let base =
        format!("no {name} credential for account `{account}`; run `goat integration add {name}`");
    match env_var {
        Some(var) => format!("{base} or set {var}"),
        None => base,
    }
}

pub fn header_value(scheme: Option<&str>, token: &str) -> String {
    let trimmed = token.trim();
    match scheme {
        Some(scheme) if !trimmed.contains(char::is_whitespace) => format!("{scheme} {trimmed}"),
        _ => trimmed.to_owned(),
    }
}

fn env_overrides_stored_oauth(
    env_var: Option<&str>,
    credentials: &CredentialStore,
    key: &CredentialKey,
) -> bool {
    env_var.is_some_and(|var| std::env::var(var).is_ok_and(|value| !value.is_empty()))
        && matches!(credentials.get(key), Some(Credential::OAuth(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scheme_is_prefixed_and_the_token_is_trimmed() {
        assert_eq!(header_value(Some("Bearer"), "  abc  "), "Bearer abc");
        assert_eq!(
            header_value(Some("Sentry-Bearer"), "abc"),
            "Sentry-Bearer abc"
        );
    }

    #[test]
    fn a_newline_pasted_into_a_token_never_reaches_the_wire() {
        assert_eq!(header_value(Some("Bearer"), "abc\n"), "Bearer abc");
        assert_eq!(header_value(None, "abc\n"), "abc");
    }

    #[test]
    fn a_preformed_header_passes_through_untouched() {
        assert_eq!(header_value(Some("Bearer"), "Bearer abc"), "Bearer abc");
    }

    #[test]
    fn a_service_without_a_scheme_sends_the_raw_token() {
        assert_eq!(header_value(None, "abc"), "abc");
    }

    #[test]
    fn the_missing_credential_message_mentions_the_env_var_when_there_is_one() {
        let with = missing_credential("sentry", "default", Some("GOAT_SENTRY_ACCESS_TOKEN"));
        assert!(with.contains("goat integration add sentry"));
        assert!(with.contains("GOAT_SENTRY_ACCESS_TOKEN"));
        let without = missing_credential("notion", "default", None);
        assert!(without.contains("goat integration add notion"));
        assert!(!without.contains("set "));
    }
}
