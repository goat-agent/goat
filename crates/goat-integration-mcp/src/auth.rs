use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use goat_auth::{Credential, CredentialKey, CredentialStore};
use goat_integration::{IntegrationError, IntegrationResult};
use goat_mcp::auth::StoredOAuth;
use tracing::info;

use crate::{AuthScheme, CredentialSpec};

pub enum ResolvedAuth {
    Token(String),
    OAuth(StoredOAuth),
}

pub fn resolve(
    name: &str,
    spec: &CredentialSpec,
    credentials: &CredentialStore,
    account: &str,
    client_id: Option<&str>,
) -> IntegrationResult<ResolvedAuth> {
    let key = CredentialKey::integration(name, account);
    if env_overrides_stored_oauth(spec.env_var, credentials, &key) {
        info!(
            integration = name,
            env_var = spec.env_var,
            "token from the environment overrides the stored oauth credential",
        );
    }
    match credentials.resolve(&key, spec.env_var) {
        Some(Credential::ApiKey(secret) | Credential::ApiKeyWithEndpoint { secret, .. }) => Ok(
            ResolvedAuth::Token(header_value(spec.scheme, secret.expose())),
        ),
        Some(Credential::OAuth(tokens)) => {
            if tokens.client_id.is_none() && client_id.is_none() {
                return Err(IntegrationError::Config(format!(
                    "{name} connection missing `client_id`; run `goat integration add {name}`"
                )));
            }
            Ok(ResolvedAuth::OAuth(StoredOAuth::new(
                credentials.clone(),
                key,
                client_id.map(str::to_owned),
            )))
        }
        None => Err(IntegrationError::Auth(missing_credential(
            name,
            account,
            spec.env_var,
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

pub fn header_value(scheme: AuthScheme, token: &str) -> String {
    let trimmed = token.trim();
    if trimmed.contains(char::is_whitespace) {
        return trimmed.to_owned();
    }
    match scheme {
        AuthScheme::Raw => trimmed.to_owned(),
        AuthScheme::Bearer => format!("Bearer {trimmed}"),
        AuthScheme::Custom(scheme) => format!("{scheme} {trimmed}"),
        AuthScheme::Basic => format!("Basic {}", basic_token(trimmed)),
    }
}

fn basic_token(trimmed: &str) -> String {
    if trimmed.contains(':') {
        STANDARD.encode(trimmed)
    } else {
        trimmed.to_owned()
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
        assert_eq!(header_value(AuthScheme::Bearer, "  abc  "), "Bearer abc");
        assert_eq!(
            header_value(AuthScheme::Custom("Sentry-Bearer"), "abc"),
            "Sentry-Bearer abc"
        );
    }

    #[test]
    fn a_newline_pasted_into_a_token_never_reaches_the_wire() {
        assert_eq!(header_value(AuthScheme::Bearer, "abc\n"), "Bearer abc");
        assert_eq!(header_value(AuthScheme::Raw, "abc\n"), "abc");
    }

    #[test]
    fn a_preformed_header_passes_through_untouched() {
        assert_eq!(header_value(AuthScheme::Bearer, "Bearer abc"), "Bearer abc");
        assert_eq!(header_value(AuthScheme::Basic, "Basic abc"), "Basic abc");
    }

    #[test]
    fn a_raw_service_sends_the_raw_token() {
        assert_eq!(header_value(AuthScheme::Raw, "abc"), "abc");
    }

    #[test]
    fn a_colon_joined_pair_is_base64_encoded_for_basic() {
        assert_eq!(
            header_value(AuthScheme::Basic, "pk-lf-1:sk-lf-2"),
            format!("Basic {}", STANDARD.encode("pk-lf-1:sk-lf-2"))
        );
    }

    #[test]
    fn an_already_encoded_basic_token_is_not_encoded_twice() {
        let encoded = STANDARD.encode("pk:sk");
        assert_eq!(
            header_value(AuthScheme::Basic, &encoded),
            format!("Basic {encoded}")
        );
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
