use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use goat_auth::{CredentialKey, CredentialStore};

use crate::Effort;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dialect {
    Chat,
    Responses,
    Anthropic,
    Gemini,
}

impl Dialect {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Responses => "responses",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
        }
    }
}

impl fmt::Display for Dialect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthScheme {
    Bearer,
    XApiKey,
    Header(String),
    Query(String),
    None,
}

impl AuthScheme {
    pub fn as_str(&self) -> String {
        match self {
            Self::Bearer => "bearer".to_owned(),
            Self::XApiKey => "x-api-key".to_owned(),
            Self::Header(name) => format!("header:{name}"),
            Self::Query(name) => format!("query:{name}"),
            Self::None => "none".to_owned(),
        }
    }

    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "bearer" => Ok(Self::Bearer),
            "x-api-key" => Ok(Self::XApiKey),
            "none" => Ok(Self::None),
            _ => raw
                .strip_prefix("header:")
                .map(|name| Self::Header(name.to_owned()))
                .or_else(|| {
                    raw.strip_prefix("query:")
                        .map(|name| Self::Query(name.to_owned()))
                })
                .ok_or_else(|| {
                    format!(
                        "auth_scheme must be bearer, x-api-key, none, header:<name>, or query:<name>; got {raw}"
                    )
                }),
        }
    }
}

impl Serialize for AuthScheme {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.as_str())
    }
}

impl<'de> Deserialize<'de> for AuthScheme {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointSource {
    Default,
    Spec,
    Credential,
}

impl EndpointSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Spec => "spec",
            Self::Credential => "credential",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FeatureOverrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_search: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_search: Option<bool>,
}

impl FeatureOverrides {
    pub fn is_empty(&self) -> bool {
        self.web_search.is_none() && self.tool_search.is_none()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderSpecConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wire: Option<Dialect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_scheme: Option<AuthScheme>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env_headers: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub query_params: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog: Option<Vec<String>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub context_windows: BTreeMap<String, u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub efforts: Option<Vec<Effort>>,
    #[serde(skip_serializing_if = "FeatureOverrides::is_empty")]
    pub features: FeatureOverrides,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_model: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub disabled: bool,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(value: &bool) -> bool {
    !*value
}

impl ProviderSpecConfig {
    pub fn has_connection_fields(&self) -> bool {
        self.wire.is_some()
            || self.endpoint.is_some()
            || self.env_key.is_some()
            || self.auth_scheme.is_some()
            || !self.headers.is_empty()
            || !self.env_headers.is_empty()
            || !self.query_params.is_empty()
            || self.catalog.is_some()
            || !self.context_windows.is_empty()
            || self.images.is_some()
            || self.efforts.is_some()
            || !self.features.is_empty()
            || self.search_model.is_some()
    }
}

#[derive(Debug, Clone)]
pub struct ProviderSpec {
    pub id: String,
    pub dialect: Dialect,
    pub endpoint: String,
    pub endpoint_source: EndpointSource,
    pub auth_scheme: Option<AuthScheme>,
    pub env_key: Option<String>,
    pub headers: Vec<(String, String)>,
    pub env_headers: Vec<(String, String)>,
    pub query_params: Vec<(String, String)>,
    pub catalog: Option<Vec<String>>,
    pub context_windows: Vec<(String, u32)>,
    pub images: Option<bool>,
    pub efforts: Option<Vec<Effort>>,
    pub features: FeatureOverrides,
    pub name: Option<String>,
    pub search_model: Option<String>,
    pub custom: bool,
    pub credentials_usable: bool,
}

impl ProviderSpec {
    pub fn apply_patch(&mut self, patch: &ProviderSpecConfig) {
        if let Some(wire) = patch.wire {
            self.dialect = wire;
        }
        if let Some(endpoint) = &patch.endpoint {
            self.endpoint.clone_from(endpoint);
            self.endpoint_source = EndpointSource::Spec;
        }
        if let Some(name) = &patch.name {
            self.name = Some(name.clone());
        }
        if let Some(env_key) = &patch.env_key {
            self.env_key = Some(env_key.clone());
        }
        if let Some(scheme) = &patch.auth_scheme {
            self.auth_scheme = Some(scheme.clone());
        }
        merge_pairs(&mut self.headers, &patch.headers);
        merge_pairs(&mut self.env_headers, &patch.env_headers);
        merge_pairs(&mut self.query_params, &patch.query_params);
        if let Some(catalog) = &patch.catalog {
            self.catalog = Some(catalog.clone());
        }
        merge_windows(&mut self.context_windows, &patch.context_windows);
        if let Some(images) = patch.images {
            self.images = Some(images);
        }
        if let Some(efforts) = &patch.efforts {
            self.efforts = Some(efforts.clone());
        }
        if let Some(web_search) = patch.features.web_search {
            self.features.web_search = Some(web_search);
        }
        if let Some(tool_search) = patch.features.tool_search {
            self.features.tool_search = Some(tool_search);
        }
        if let Some(search_model) = &patch.search_model {
            self.search_model = Some(search_model.clone());
        }
    }

    pub fn custom(id: &str, config: &ProviderSpecConfig) -> Result<Self, String> {
        let endpoint = config
            .endpoint
            .as_deref()
            .ok_or_else(|| format!("provider {id} needs an endpoint"))?;
        let endpoint = validate_user_endpoint(endpoint)?;
        Ok(Self {
            id: id.to_owned(),
            dialect: config.wire.unwrap_or(Dialect::Chat),
            endpoint,
            endpoint_source: EndpointSource::Spec,
            auth_scheme: config.auth_scheme.clone(),
            env_key: config.env_key.clone(),
            headers: map_to_pairs(&config.headers),
            env_headers: map_to_pairs(&config.env_headers),
            query_params: map_to_pairs(&config.query_params),
            catalog: config.catalog.clone(),
            context_windows: map_to_pairs(&config.context_windows),
            images: config.images,
            efforts: config.efforts.clone(),
            features: config.features.clone(),
            name: config.name.clone(),
            search_model: config.search_model.clone(),
            custom: true,
            credentials_usable: true,
        })
    }

    pub fn foreign(&self) -> bool {
        self.custom || self.endpoint_source != EndpointSource::Default
    }

    pub fn web_search_allowed(&self) -> bool {
        self.features.web_search.unwrap_or(!self.foreign())
    }

    pub fn tool_search_allowed(&self) -> bool {
        self.features.tool_search.unwrap_or(!self.foreign())
    }

    pub fn resolved_headers(&self) -> Vec<(String, String)> {
        let mut headers = self.headers.clone();
        for (name, var) in &self.env_headers {
            match std::env::var(var) {
                Ok(value) if !value.is_empty() => headers.push((name.clone(), value)),
                _ => {
                    tracing::warn!(provider = %self.id, header = %name, env = %var, "env header skipped: variable is not set");
                }
            }
        }
        headers
    }

    pub fn context_window(&self, model: &str) -> Option<u32> {
        self.context_windows
            .iter()
            .find_map(|(prefix, window)| model.starts_with(prefix.as_str()).then_some(*window))
    }

    pub fn connection_info(&self) -> ConnectionInfo {
        ConnectionInfo {
            endpoint: self.endpoint.clone(),
            source: self.endpoint_source,
            dialect: self.dialect,
            auth_scheme: self.auth_scheme.clone(),
            env_key: self.env_key.clone(),
            name: self.name.clone(),
        }
    }

    pub fn resolve_endpoint(
        &mut self,
        store: &CredentialStore,
        key: &CredentialKey,
        validate: EndpointValidator,
    ) -> bool {
        let Some(raw) = store
            .get(key)
            .and_then(|cred| cred.endpoint().map(str::to_owned))
        else {
            return true;
        };
        match validate(&raw) {
            Ok(endpoint) => {
                self.endpoint = endpoint;
                self.endpoint_source = EndpointSource::Credential;
                true
            }
            Err(_) => false,
        }
    }
}

fn merge_pairs(base: &mut Vec<(String, String)>, patch: &BTreeMap<String, String>) {
    for (name, value) in patch {
        match base.iter_mut().find(|(key, _)| key == name) {
            Some((_, existing)) => existing.clone_from(value),
            None => base.push((name.clone(), value.clone())),
        }
    }
}

fn merge_windows(base: &mut Vec<(String, u32)>, patch: &BTreeMap<String, u32>) {
    for (prefix, window) in patch {
        match base.iter_mut().find(|(key, _)| key == prefix) {
            Some((_, existing)) => *existing = *window,
            None => base.push((prefix.clone(), *window)),
        }
    }
}

fn map_to_pairs<V: Clone>(map: &BTreeMap<String, V>) -> Vec<(String, V)> {
    map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

#[derive(Debug, Clone)]
pub struct ConnectionInfo {
    pub endpoint: String,
    pub source: EndpointSource,
    pub dialect: Dialect,
    pub auth_scheme: Option<AuthScheme>,
    pub env_key: Option<String>,
    pub name: Option<String>,
}

pub type EndpointValidator = fn(&str) -> Result<String, String>;

#[derive(Debug, Clone, Copy)]
pub struct EndpointOverride {
    pub env_var: Option<&'static str>,
    pub default: Option<&'static str>,
    pub validate: Option<EndpointValidator>,
}

pub fn validate_user_endpoint(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim().trim_end_matches('/');
    let url = url::Url::parse(trimmed).map_err(|err| err.to_string())?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err("endpoint must use http or https".to_owned());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("endpoint must not include userinfo".to_owned());
    }
    if url.host_str().is_none() {
        return Err("endpoint must include a host".to_owned());
    }
    Ok(trimmed.to_owned())
}

pub fn validate_override_endpoint(raw: &str) -> Result<String, String> {
    let endpoint = validate_user_endpoint(raw)?;
    let url = url::Url::parse(&endpoint).map_err(|err| err.to_string())?;
    if url.scheme() == "https" {
        return Ok(endpoint);
    }
    let host = url.host_str().unwrap_or_default();
    if is_loopback_host(host) {
        return Ok(endpoint);
    }
    Err("endpoint must use https, or http with a loopback host".to_owned())
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host == "127.0.0.1"
        || host == "::1"
        || host == "[::1]"
        || host.ends_with(".localhost")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_scheme_round_trips() {
        for (raw, scheme) in [
            ("bearer", AuthScheme::Bearer),
            ("x-api-key", AuthScheme::XApiKey),
            ("none", AuthScheme::None),
            ("header:x-token", AuthScheme::Header("x-token".into())),
            ("query:api-key", AuthScheme::Query("api-key".into())),
        ] {
            assert_eq!(AuthScheme::parse(raw).unwrap(), scheme);
            assert_eq!(scheme.as_str(), raw);
        }
        assert!(AuthScheme::parse("digest").is_err());
    }

    #[test]
    fn patch_merges_maps_and_replaces_scalars() {
        let mut spec = ProviderSpec {
            id: "p".into(),
            dialect: Dialect::Chat,
            endpoint: "https://a.example".into(),
            endpoint_source: EndpointSource::Default,
            auth_scheme: None,
            env_key: Some("A_KEY".into()),
            headers: vec![("X-A".into(), "1".into()), ("X-B".into(), "2".into())],
            env_headers: vec![],
            query_params: vec![],
            catalog: Some(vec!["m1".into()]),
            context_windows: vec![("m".into(), 100)],
            images: None,
            efforts: None,
            features: FeatureOverrides::default(),
            name: None,
            search_model: None,
            custom: false,
            credentials_usable: true,
        };
        let patch = ProviderSpecConfig {
            endpoint: Some("https://b.example".into()),
            headers: BTreeMap::from([("X-B".into(), "9".into()), ("X-C".into(), "3".into())]),
            context_windows: BTreeMap::from([("m".into(), 200), ("n".into(), 50)]),
            catalog: Some(vec!["m2".into()]),
            ..ProviderSpecConfig::default()
        };
        spec.apply_patch(&patch);
        assert_eq!(spec.endpoint, "https://b.example");
        assert_eq!(spec.endpoint_source, EndpointSource::Spec);
        assert_eq!(
            spec.headers,
            vec![
                ("X-A".to_owned(), "1".to_owned()),
                ("X-B".to_owned(), "9".to_owned()),
                ("X-C".to_owned(), "3".to_owned())
            ]
        );
        assert_eq!(
            spec.context_windows,
            vec![("m".to_owned(), 200), ("n".to_owned(), 50)]
        );
        assert_eq!(spec.catalog, Some(vec!["m2".to_owned()]));
        assert!(spec.foreign());
    }

    #[test]
    fn custom_requires_endpoint() {
        let config = ProviderSpecConfig::default();
        assert!(ProviderSpec::custom("x", &config).is_err());
        let config = ProviderSpecConfig {
            endpoint: Some("https://relay.example/v1".into()),
            wire: Some(Dialect::Anthropic),
            ..ProviderSpecConfig::default()
        };
        let spec = ProviderSpec::custom("relay", &config).unwrap();
        assert_eq!(spec.dialect, Dialect::Anthropic);
        assert!(spec.custom);
        assert!(spec.foreign());
    }

    #[test]
    fn override_endpoint_requires_https_or_loopback() {
        assert!(validate_override_endpoint("https://gw.example").is_ok());
        assert!(validate_override_endpoint("http://localhost:8080").is_ok());
        assert!(validate_override_endpoint("http://127.0.0.1:1").is_ok());
        assert!(validate_override_endpoint("http://192.168.0.5").is_err());
    }
}
