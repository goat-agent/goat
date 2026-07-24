use goat_auth::{CredentialKey, CredentialStore};
use goat_provider::{AuthMethod, LoginEndpointMetadata, ProviderId, ProviderMetadata};
use goat_provider_openai_compat::{OpenAiCompatProvider, no_efforts};

pub const PROVIDER_ID: &str = "qwen";

const QWEN_DEFAULT_ENDPOINT: &str = "https://dashscope-us.aliyuncs.com/compatible-mode/v1";

const QWEN_SETUP: &[&str] = &[
    "Qwen DashScope API-key provider.",
    "Default endpoint: https://dashscope-us.aliyuncs.com/compatible-mode/v1",
    "Non-US workspaces: `goat provider login qwen --endpoint <url> --key sk-...`.",
    "Qwen OAuth enrollment is discontinued upstream.",
];

const CATALOG: &[&str] = &[
    "qwen3.7-max",
    "qwen3.7-plus",
    "qwen3.6-flash",
    "qwen3-coder-plus",
    "qwen3-coder-flash",
];

const CONTEXT: &[(&str, u32)] = &[
    ("qwen3.7", 1_000_000),
    ("qwen3.6-flash", 1_000_000),
    ("qwen3-coder", 1_000_000),
];

pub fn build(store: &CredentialStore, account: &str) -> OpenAiCompatProvider {
    let key = CredentialKey::model(PROVIDER_ID, account);
    let stored_endpoint = store
        .get(&key)
        .and_then(|cred| cred.endpoint().map(str::to_owned));
    let endpoint = match stored_endpoint {
        Some(raw) => validate_qwen_endpoint(&raw).ok(),
        None => Some(QWEN_DEFAULT_ENDPOINT.to_owned()),
    };
    let bearer = endpoint.as_ref().and_then(|_| {
        store
            .resolve(&key, Some("DASHSCOPE_API_KEY"))
            .map(|cred| cred.bearer().to_owned())
    });
    OpenAiCompatProvider::new(
        ProviderId::from(PROVIDER_ID),
        endpoint.unwrap_or_else(|| QWEN_DEFAULT_ENDPOINT.to_owned()),
        bearer,
        AuthMethod::ApiKey,
    )
    .with_catalog(CATALOG)
    .with_context_windows(CONTEXT)
    .with_vision_filter(qwen_vision_model)
    .with_efforts(no_efforts)
    .with_reasoning_effort(false)
    .with_metadata(ProviderMetadata {
        env_var: Some("DASHSCOPE_API_KEY"),
        validation: "network",
        endpoint: Some("required for non-US DashScope workspaces"),
        oauth: Some("Qwen OAuth enrollment discontinued"),
        login_endpoint: Some(LoginEndpointMetadata {
            env_var: Some("QWEN_BASE_URL"),
            default: Some(QWEN_DEFAULT_ENDPOINT),
            validate: Some(validate_qwen_endpoint),
        }),
        setup: QWEN_SETUP,
    })
}

pub fn validate_qwen_endpoint(endpoint: &str) -> Result<String, String> {
    let trimmed = endpoint.trim().trim_end_matches('/');
    let url = reqwest::Url::parse(trimmed).map_err(|err| err.to_string())?;
    if url.scheme() != "https" {
        return Err("qwen endpoint must use https".to_owned());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("qwen endpoint must not include userinfo".to_owned());
    }
    let Some(host) = url.host_str() else {
        return Err("qwen endpoint must include a host".to_owned());
    };
    if host.ends_with('.') {
        return Err("qwen endpoint host must not end with a dot".to_owned());
    }
    let allowed_static = [
        "dashscope.aliyuncs.com",
        "dashscope-intl.aliyuncs.com",
        "dashscope-us.aliyuncs.com",
    ];
    let allowed_regions = [
        "cn-beijing.maas.aliyuncs.com",
        "ap-southeast-1.maas.aliyuncs.com",
        "ap-northeast-1.maas.aliyuncs.com",
    ];
    let allowed = allowed_static.contains(&host)
        || allowed_regions.iter().any(|region| {
            host.strip_suffix(region)
                .and_then(|prefix| prefix.strip_suffix('.'))
                .is_some_and(valid_workspace_id)
        });
    if !allowed {
        return Err("qwen endpoint host is not an allowed Alibaba Model Studio host".to_owned());
    }
    if url.port().is_some() {
        return Err("qwen endpoint must not include a custom port".to_owned());
    }
    if url.path() != "/compatible-mode/v1" {
        return Err("qwen endpoint path must be /compatible-mode/v1".to_owned());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("qwen endpoint must not include query or fragment".to_owned());
    }
    Ok(trimmed.to_owned())
}

fn valid_workspace_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn qwen_vision_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    id.contains("qwen3.7")
        || id.contains("qwen-vl")
        || id.contains("qwen2-vl")
        || id.contains("qwen2.5-vl")
}

#[cfg(test)]
mod tests {
    use goat_auth::{Credential, CredentialStore, SecretString};
    use goat_provider::Provider;

    use super::*;

    fn store(name: &str) -> CredentialStore {
        let _ = std::fs::remove_file(std::env::temp_dir().join(name));
        CredentialStore::new(std::env::temp_dir().join(name))
    }

    #[test]
    fn validates_qwen_endpoints() {
        for endpoint in [
            "https://dashscope-us.aliyuncs.com/compatible-mode/v1",
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
            "https://workspace-1.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
            "https://abc123.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1/",
        ] {
            assert_eq!(
                validate_qwen_endpoint(endpoint).unwrap(),
                endpoint.trim_end_matches('/')
            );
        }
        for endpoint in [
            "http://dashscope-us.aliyuncs.com/compatible-mode/v1",
            "https://dashscope-us.aliyuncs.com.evil.test/compatible-mode/v1",
            "https://user@dashscope-us.aliyuncs.com/compatible-mode/v1",
            "https://dashscope-us.aliyuncs.com:444/compatible-mode/v1",
            "https://dashscope-us.aliyuncs.com/v1",
            "https://dashscope-us.aliyuncs.com/compatible-mode/v1?x=1",
            "https://workspace_1.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
            "https://workspace-1.cn-hangzhou.maas.aliyuncs.com/compatible-mode/v1",
        ] {
            assert!(
                validate_qwen_endpoint(endpoint).is_err(),
                "expected rejection for {endpoint}"
            );
        }
    }

    #[test]
    fn invalid_qwen_endpoint_does_not_authenticate() {
        let store = store("goat-provider-qwen-invalid.json");
        store
            .store(
                &CredentialKey::model(PROVIDER_ID, "default"),
                Credential::ApiKeyWithEndpoint {
                    secret: SecretString::from("key".to_owned()),
                    endpoint: "https://example.com/compatible-mode/v1".to_owned(),
                },
            )
            .unwrap();
        let provider = build(&store, "default");
        assert!(!provider.authenticated());
    }

    #[test]
    fn qwen_endpoint_credential_authenticates() {
        let store = store("goat-provider-qwen-valid.json");
        store
            .store(
                &CredentialKey::model(PROVIDER_ID, "default"),
                Credential::ApiKeyWithEndpoint {
                    secret: SecretString::from("key".to_owned()),
                    endpoint: "https://dashscope-us.aliyuncs.com/compatible-mode/v1".to_owned(),
                },
            )
            .unwrap();
        let provider = build(&store, "default");
        assert!(provider.authenticated());
        assert_eq!(
            provider.base_url(),
            "https://dashscope-us.aliyuncs.com/compatible-mode/v1"
        );
    }
}
