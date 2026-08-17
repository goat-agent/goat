use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{paths::config_path, write_atomic};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeChoice {
    #[default]
    Dark,
    Light,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub browser_enabled: bool,
    #[serde(alias = "remote")]
    pub devices: DeviceConfig,
    pub search: SearchConfig,
    pub web_fetch: WebFetchConfig,
    pub proxy: ProxyConfig,
    pub integrations: BTreeMap<String, serde_json::Value>,
    pub providers: BTreeMap<String, UserProviderConfig>,
    #[serde(flatten)]
    unrecognized: BTreeMap<String, serde_json::Value>,
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

impl Config {
    pub fn load() -> Self {
        config_path().map_or_else(Self::default, |path| Self::load_path(&path))
    }

    fn load_path(path: &Path) -> Self {
        let Ok(raw) = fs::read_to_string(path) else {
            return Self::default();
        };
        let Ok(config) = serde_json::from_str::<Self>(&raw) else {
            let _ = fs::rename(path, path.with_extension("json.corrupt"));
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
        self.save_path(&path)
    }

    fn save_path(&self, path: &Path) -> Result<(), SettingsError> {
        write_atomic(path, serde_json::to_string_pretty(self)?.as_bytes())?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientConfig {
    pub theme: ThemeChoice,
    pub mouse_capture_enabled: bool,
    pub remotes: std::collections::BTreeMap<String, RemoteEntry>,
    pub default_remote: Option<String>,
    #[serde(flatten)]
    unrecognized: BTreeMap<String, serde_json::Value>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::default(),
            mouse_capture_enabled: true,
            remotes: std::collections::BTreeMap::new(),
            default_remote: None,
            unrecognized: BTreeMap::new(),
        }
    }
}

impl ClientConfig {
    #[must_use]
    pub fn load() -> Self {
        let Some(paths) = crate::paths::resolved() else {
            return Self::default();
        };
        Self::load_pair(&paths.client_json, &paths.config_json)
    }

    const OWNED: [&'static str; 4] = [
        "theme",
        "mouse_capture_enabled",
        "remotes",
        "default_remote",
    ];

    fn load_pair(client: &Path, daemon: &Path) -> Self {
        if let Ok(raw) = fs::read_to_string(client) {
            return serde_json::from_str(&raw).unwrap_or_default();
        }
        let Ok(raw) = fs::read_to_string(daemon) else {
            return Self::default();
        };
        let Ok(serde_json::Value::Object(object)) = serde_json::from_str(&raw) else {
            return Self::default();
        };
        let mine: serde_json::Map<_, _> = object
            .into_iter()
            .filter(|(key, _)| Self::OWNED.contains(&key.as_str()))
            .collect();
        let adopted: Self =
            serde_json::from_value(serde_json::Value::Object(mine)).unwrap_or_default();
        if adopted.save_path(client).is_ok() {
            Self::strip_from(daemon, &raw);
        }
        adopted
    }

    fn strip_from(daemon: &Path, raw: &str) {
        let Ok(serde_json::Value::Object(mut object)) = serde_json::from_str(raw) else {
            return;
        };
        if !Self::OWNED.iter().any(|key| object.contains_key(*key)) {
            return;
        }
        for key in Self::OWNED {
            object.remove(key);
        }
        if let Ok(text) = serde_json::to_string_pretty(&object) {
            let _ = write_atomic(daemon, text.as_bytes());
        }
    }

    pub fn save(&self) -> Result<(), SettingsError> {
        let path = crate::paths::client_path().ok_or(SettingsError::NoHome)?;
        self.save_path(&path)
    }

    fn save_path(&self, path: &Path) -> Result<(), SettingsError> {
        write_atomic(path, serde_json::to_string_pretty(self)?.as_bytes())?;
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
    use std::{collections::BTreeMap, fs};

    use super::{
        ClientConfig, Config, DeviceConfig, ProxyConfig, SearchConfig, ThemeChoice, UserProviders,
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
    fn the_client_defaults_to_dark() {
        assert_eq!(ClientConfig::default().theme, ThemeChoice::Dark);
    }

    #[test]
    fn the_client_adopts_its_keys_from_the_daemon_file_once() {
        let directory = tempfile::tempdir().unwrap();
        let daemon = directory.path().join("config.json");
        let client = directory.path().join("client.json");
        fs::write(
            &daemon,
            r#"{ "theme": "light", "default_remote": "box", "browser_enabled": true }"#,
        )
        .unwrap();

        let adopted = ClientConfig::load_pair(&client, &daemon);
        assert_eq!(adopted.theme, ThemeChoice::Light);
        assert_eq!(adopted.default_remote.as_deref(), Some("box"));
        assert!(client.exists(), "the adoption is written once");
        let left: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&daemon).unwrap()).unwrap();
        assert!(
            left.get("theme").is_none() && left.get("default_remote").is_none(),
            "the adopted keys leave the daemon file so they cannot be edited in two places"
        );
        assert_eq!(
            left["browser_enabled"], true,
            "what the daemon owns stays where it was"
        );
        let taken: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&client).unwrap()).unwrap();
        assert!(
            taken.get("browser_enabled").is_none(),
            "adoption takes only the keys the client owns"
        );

        fs::write(&daemon, r#"{ "theme": "dark" }"#).unwrap();
        assert_eq!(
            ClientConfig::load_pair(&client, &daemon).theme,
            ThemeChoice::Light,
            "once client.json exists the daemon file is never consulted again"
        );
    }

    #[test]
    fn the_daemon_file_keeps_what_it_owns() {
        let cfg = Config::from_json(r#"{ "browser_enabled": true }"#).unwrap();
        assert!(cfg.browser_enabled);
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
    fn unknown_top_level_key_survives_mutation_and_save() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        let expected = serde_json::json!({
            "enabled": true,
            "limits": [1, 2, 3]
        });
        fs::write(
            &path,
            serde_json::json!({ "future_extension": expected }).to_string(),
        )
        .unwrap();

        let mut config = Config::load_path(&path);
        assert_eq!(config.unrecognized.len(), 1);
        assert!(config.unrecognized.contains_key("future_extension"));
        config.browser_enabled = true;
        config.save_path(&path).unwrap();
        let reloaded = Config::load_path(&path);

        assert!(reloaded.browser_enabled);
        assert_eq!(reloaded.unrecognized["future_extension"], expected);
        let saved: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(saved["future_extension"], expected);
    }

    #[test]
    fn round_trips_through_json() {
        let cfg = Config {
            browser_enabled: true,
            devices: DeviceConfig::default(),
            search: SearchConfig::default(),
            web_fetch: WebFetchConfig::default(),
            proxy: ProxyConfig::default(),
            integrations: BTreeMap::new(),
            providers: BTreeMap::new(),
            unrecognized: BTreeMap::new(),
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
        let cfg = ClientConfig::default();
        assert!(cfg.default_remote.is_none());
        assert!(cfg.remotes.is_empty());
    }
}
