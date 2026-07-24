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
    pub remote: RemoteConfig,
    pub search: SearchConfig,
    pub web_fetch: WebFetchConfig,
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
pub struct RemoteConfig {
    pub bind: String,
    pub advertised: Vec<String>,
}

impl Default for RemoteConfig {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:4317".to_owned(),
            advertised: Vec::new(),
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
            remote: RemoteConfig::default(),
            search: SearchConfig::default(),
            web_fetch: WebFetchConfig::default(),
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
    use super::{Config, RemoteConfig, SearchConfig, ThemeChoice, WebFetchConfig};

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
    fn round_trips_through_json() {
        let cfg = Config {
            theme: ThemeChoice::Light,
            computer_use_enabled: false,
            browser_enabled: true,
            mouse_capture_enabled: false,
            remote: RemoteConfig::default(),
            search: SearchConfig::default(),
            web_fetch: WebFetchConfig::default(),
        };
        let raw = serde_json::to_string(&cfg).unwrap();
        assert_eq!(Config::from_json(&raw).unwrap(), cfg);
    }
}
