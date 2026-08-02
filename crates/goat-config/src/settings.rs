use std::fs;

use serde::{Deserialize, Serialize};

use crate::paths::config_path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeChoice {
    #[default]
    Dark,
    Light,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub theme: ThemeChoice,
    pub computer_use_enabled: bool,
    pub browser_enabled: bool,
    pub mouse_capture_enabled: bool,
    #[serde(alias = "remote")]
    pub devices: DeviceConfig,
    pub remotes: std::collections::BTreeMap<String, RemoteEntry>,
    pub default_remote: Option<String>,
    pub search: SearchConfig,
    pub web_fetch: WebFetchConfig,
    pub proxy: ProxyConfig,
    pub integrations: std::collections::BTreeMap<String, serde_json::Value>,
    pub providers: std::collections::BTreeMap<String, UserProviderConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserProviderConfig {
    pub endpoint: String,
}

#[derive(Clone)]
pub struct UserProviders {
    path: Option<std::path::PathBuf>,
}

impl UserProviders {
    #[must_use]
    pub fn detect() -> Self {
        Self {
            path: config_path(),
        }
    }

    #[must_use]
    pub fn at(path: std::path::PathBuf) -> Self {
        Self { path: Some(path) }
    }

    #[must_use]
    pub fn load(&self) -> std::collections::BTreeMap<String, UserProviderConfig> {
        let Some(path) = &self.path else {
            return std::collections::BTreeMap::new();
        };
        let Ok(raw) = fs::read_to_string(path) else {
            return std::collections::BTreeMap::new();
        };
        serde_json::from_str::<Config>(&raw)
            .map(|config| config.providers)
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchConfig {
    pub default_target: Option<String>,
    pub accounts: Vec<SearchAccountConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WebFetchConfig {
    pub readability: bool,
    pub render_enabled: bool,
    pub max_length: usize,
}

impl Default for WebFetchConfig {
    fn default() -> Self {
        Self {
            readability: true,
            render_enabled: true,
            max_length: 48 * 1024,
        }
    }
}

pub use goat_search_provider::SearchAccountConfig;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceConfig {
    pub bind: String,
    pub advertised: Vec<String>,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:4317".to_owned(),
            advertised: Vec::new(),
        }
    }
}

pub const LOCAL_REMOTE: &str = "local";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteEntry {
    pub host: String,
    pub fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_dir: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProxyConfig {
    pub enabled: bool,
    pub bind: String,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            bind: "127.0.0.1:7777".to_owned(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::default(),
            computer_use_enabled: false,
            browser_enabled: false,
            mouse_capture_enabled: true,
            devices: DeviceConfig::default(),
            remotes: std::collections::BTreeMap::new(),
            default_remote: None,
            search: SearchConfig::default(),
            web_fetch: WebFetchConfig::default(),
            proxy: ProxyConfig::default(),
            integrations: std::collections::BTreeMap::new(),
            providers: std::collections::BTreeMap::new(),
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            return Self::default();
        };
        let Ok(raw) = fs::read_to_string(&path) else {
            return Self::default();
        };
        let Ok(config) = serde_json::from_str::<Self>(&raw) else {
            let _ = fs::rename(&path, path.with_extension("json.corrupt"));
            return Self::default();
        };
        config
    }

    #[cfg(test)]
    pub fn from_json(raw: &str) -> Result<Self, SettingsError> {
        Ok(serde_json::from_str(raw)?)
    }

    pub fn save(&self) -> Result<(), SettingsError> {
        let path = config_path().ok_or(SettingsError::NoHome)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("config json failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("could not resolve home directory")]
    NoHome,
    #[error("config io failed: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::{
        Config, DeviceConfig, ProxyConfig, RemoteEntry, SearchConfig, ThemeChoice, UserProviders,
        WebFetchConfig,
    };

    #[test]
    fn parses_user_providers() {
        let cfg = Config::from_json(
            r#"{ "providers": { "my-proxy": { "endpoint": "https://llm.corp/v1" } } }"#,
        )
        .unwrap();
        assert_eq!(cfg.providers["my-proxy"].endpoint, "https://llm.corp/v1");
    }

    #[test]
    fn user_providers_reads_fresh_from_disk() {
        let path = std::env::temp_dir().join("goat-config-user-providers.json");
        let _ = std::fs::remove_file(&path);
        let user = UserProviders::at(path.clone());
        assert!(user.load().is_empty());
        std::fs::write(
            &path,
            r#"{ "providers": { "my-proxy": { "endpoint": "http://localhost:9/v1" } } }"#,
        )
        .unwrap();
        assert_eq!(user.load()["my-proxy"].endpoint, "http://localhost:9/v1");
        std::fs::write(&path, "{}").unwrap();
        assert!(user.load().is_empty());
    }

    #[test]
    fn defaults_to_dark() {
        assert_eq!(Config::default().theme, ThemeChoice::Dark);
    }

    #[test]
    fn parses_minimal_json() {
        let cfg = Config::from_json(r#"{ "theme": "light" }"#).unwrap();
        assert_eq!(cfg.theme, ThemeChoice::Light);
    }

    #[test]
    fn empty_object_is_default() {
        assert_eq!(Config::from_json("{}").unwrap(), Config::default());
    }

    #[test]
    fn parses_search_config() {
        let cfg = Config::from_json(
            r#"{
                "search": {
                    "default_target": "searxng/home",
                    "accounts": [
                        { "provider": "searxng", "account": "home", "endpoint": "https://search.example.com" }
                    ]
                }
            }"#,
        )
        .unwrap();
        assert_eq!(cfg.search.default_target.as_deref(), Some("searxng/home"));
        assert_eq!(cfg.search.accounts[0].target(), "searxng/home");
    }

    #[test]
    fn proxy_defaults_enabled_on_localhost() {
        let cfg = Config::from_json("{}").unwrap();
        assert!(cfg.proxy.enabled);
        assert_eq!(cfg.proxy.bind, "127.0.0.1:7777");
    }

    #[test]
    fn parses_proxy_overrides() {
        let cfg =
            Config::from_json(r#"{ "proxy": { "enabled": false, "bind": "127.0.0.1:9000" } }"#)
                .unwrap();
        assert!(!cfg.proxy.enabled);
        assert_eq!(cfg.proxy.bind, "127.0.0.1:9000");
    }

    #[test]
    fn round_trips_through_json() {
        let cfg = Config {
            theme: ThemeChoice::Light,
            computer_use_enabled: false,
            browser_enabled: true,
            mouse_capture_enabled: false,
            devices: DeviceConfig::default(),
            remotes: std::collections::BTreeMap::from([(
                "box".to_owned(),
                RemoteEntry {
                    host: "1.2.3.4:4317".to_owned(),
                    fingerprint: "abcdef".to_owned(),
                    last_dir: Some("/srv/work".to_owned()),
                },
            )]),
            default_remote: Some("box".to_owned()),
            search: SearchConfig::default(),
            web_fetch: WebFetchConfig::default(),
            proxy: ProxyConfig::default(),
            integrations: std::collections::BTreeMap::new(),
            providers: std::collections::BTreeMap::new(),
        };
        let raw = serde_json::to_string(&cfg).unwrap();
        assert_eq!(Config::from_json(&raw).unwrap(), cfg);
    }

    #[test]
    fn the_old_remote_key_still_configures_the_listener() {
        let cfg = Config::from_json(r#"{ "remote": { "bind": "0.0.0.0:5000" } }"#).unwrap();
        assert_eq!(cfg.devices.bind, "0.0.0.0:5000");
    }

    #[test]
    fn no_default_remote_means_local() {
        let cfg = Config::default();
        assert!(cfg.default_remote.is_none());
        assert!(cfg.remotes.is_empty());
    }
}
