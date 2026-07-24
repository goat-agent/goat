use goat_auth::{CredentialKey, CredentialStore};
use goat_provider::{AuthMethod, Effort, ProviderId, ProviderMetadata};

use crate::OpenAiCompatProvider;

pub fn enforce_https_host(base_url: &str, allowed_host: &str) -> Result<(), String> {
    let url = base_url.trim_end_matches('/');
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| "hosted providers require https".to_owned())?;
    let actual = rest.split('/').next().unwrap_or_default();
    if actual == allowed_host || actual.ends_with(&format!(".{allowed_host}")) {
        Ok(())
    } else {
        Err(format!("invalid hosted provider host: {actual}"))
    }
}

pub fn api_key(
    store: &CredentialStore,
    account: &str,
    provider_id: &'static str,
    base_url: &'static str,
    allowed_host: &'static str,
    env_var: &'static str,
) -> OpenAiCompatProvider {
    enforce_https_host(base_url, allowed_host).expect("hosted provider base URL");
    let key = CredentialKey::model(provider_id, account);
    let bearer = store
        .resolve(&key, Some(env_var))
        .map(|cred| cred.bearer().to_owned());
    OpenAiCompatProvider::new(
        ProviderId::from(provider_id),
        base_url,
        bearer,
        AuthMethod::ApiKey,
    )
    .with_metadata(ProviderMetadata {
        env_var: Some(env_var),
        validation: "network",
        endpoint: None,
        oauth: Some("not supported"),
        login_endpoint: None,
        setup: &[],
    })
}

pub fn no_vision(_id: &str) -> bool {
    false
}

pub fn no_efforts(_model: &str) -> Vec<Effort> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::enforce_https_host;

    #[test]
    fn enforces_https_and_allowed_host() {
        assert!(enforce_https_host("https://openrouter.ai/api/v1/", "openrouter.ai").is_ok());
        assert!(enforce_https_host("http://openrouter.ai/api/v1", "openrouter.ai").is_err());
        assert!(enforce_https_host("https://example.com/api/v1", "openrouter.ai").is_err());
        assert!(enforce_https_host("https://api.z.ai/api/coding/paas/v4", "api.z.ai").is_ok());
    }
}
