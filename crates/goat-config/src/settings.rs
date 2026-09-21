use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use goat_provider::ProviderSpecConfig;
use serde::{Deserialize, Serialize};

use crate::{paths::config_path, write_atomic};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeChoice {
    #[default]
    Dark,
    Light,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    #[serde(alias = "remote")]
    pub devices: DeviceConfig,
    pub search: SearchConfig,
    pub web_fetch: WebFetchConfig,
    pub proxy: ProxyConfig,
    pub integrations: BTreeMap<String, serde_json::Value>,
    pub providers: BTreeMap<String, ProviderSpecConfig>,
    #[serde(flatten)]
    unrecognized: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone)]
pub struct ProviderSpecs {
    path: Option<std::path::PathBuf>,
}

impl ProviderSpecs {
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
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    #[must_use]
    pub fn load(&self) -> BTreeMap<String, ProviderSpecConfig> {
        let Some(path) = &self.path else {
            return BTreeMap::new();
        };
        let Ok(raw) = fs::read_to_string(path) else {
            return BTreeMap::new();
        };
        toml::from_str::<Config>(&raw)
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
        let Ok(config) = toml::from_str::<Self>(&raw) else {
            let _ = fs::rename(path, path.with_extension("toml.corrupt"));
            return Self::default();
        };
        config
    }

    #[cfg(test)]
    pub fn from_toml(raw: &str) -> Result<Self, SettingsError> {
        Ok(toml::from_str(raw)?)
    }

    pub fn save_at(&self, path: &Path) -> Result<(), SettingsError> {
        self.save_path(path)
    }

    #[must_use]
    pub fn load_at(path: &Path) -> Self {
        Self::load_path(path)
    }

    fn save_path(&self, path: &Path) -> Result<(), SettingsError> {
        let value = serde_json::to_value(self)?;
        let mut doc = toml_edit::DocumentMut::new();
        if let Some(toml_edit::Item::Table(table)) = json_to_toml_item(&value) {
            *doc.as_table_mut() = table;
        }
        write_atomic(path, doc.to_string().as_bytes())?;
        Ok(())
    }
}

pub struct ConfigDocument {
    doc: toml_edit::DocumentMut,
}

impl ConfigDocument {
    #[must_use]
    pub fn load_at(path: &Path) -> Self {
        let doc = fs::read_to_string(path)
            .ok()
            .and_then(|raw| raw.parse::<toml_edit::DocumentMut>().ok())
            .unwrap_or_default();
        Self { doc }
    }

    pub fn save(&self, path: &Path) -> Result<(), SettingsError> {
        write_atomic(path, self.doc.to_string().as_bytes())?;
        Ok(())
    }

    #[must_use]
    pub fn render(&self) -> String {
        self.doc.to_string()
    }

    pub fn set_provider(
        &mut self,
        name: &str,
        spec: &ProviderSpecConfig,
    ) -> Result<(), SettingsError> {
        let value = serde_json::to_value(spec)?;
        let Some(item) = json_to_toml_item(&value) else {
            return Ok(());
        };
        ensure_table(&mut self.doc, "providers").insert(name, item);
        Ok(())
    }

    pub fn remove_provider(&mut self, name: &str) {
        if let Some(table) = self.doc["providers"].as_table_mut() {
            table.remove(name);
        }
    }

    pub fn set_integration(
        &mut self,
        kind: &str,
        value: &serde_json::Value,
    ) -> Result<(), SettingsError> {
        let Some(item) = json_to_toml_item(value) else {
            return Ok(());
        };
        ensure_table(&mut self.doc, "integrations").insert(kind, item);
        Ok(())
    }

    pub fn remove_integration(&mut self, kind: &str) {
        if let Some(table) = self.doc["integrations"].as_table_mut() {
            table.remove(kind);
        }
    }

    pub fn set_search(&mut self, search: &SearchConfig) -> Result<(), SettingsError> {
        let value = serde_json::to_value(search)?;
        if let Some(item) = json_to_toml_item(&value) {
            self.doc["search"] = item;
        }
        Ok(())
    }
}

fn ensure_table<'a>(doc: &'a mut toml_edit::DocumentMut, key: &str) -> &'a mut toml_edit::Table {
    let item = &mut doc[key];
    if !item.is_table() {
        *item = toml_edit::Item::Table(toml_edit::Table::new());
    }
    item.as_table_mut().expect("just made a table")
}

fn json_to_toml_item(value: &serde_json::Value) -> Option<toml_edit::Item> {
    match value {
        serde_json::Value::Null => None,
        serde_json::Value::Bool(b) => Some(toml_edit::Item::Value((*b).into())),
        serde_json::Value::Number(number) => number
            .as_i64()
            .map(toml_edit::Value::from)
            .or_else(|| {
                number
                    .as_u64()
                    .and_then(|u| i64::try_from(u).ok())
                    .map(toml_edit::Value::from)
            })
            .or_else(|| number.as_f64().map(toml_edit::Value::from))
            .map(toml_edit::Item::Value),
        serde_json::Value::String(text) => Some(toml_edit::Item::Value(text.clone().into())),
        serde_json::Value::Array(items) => {
            if items.iter().all(serde_json::Value::is_object) {
                let mut tables = toml_edit::ArrayOfTables::new();
                for item in items {
                    if let Some(toml_edit::Item::Table(table)) = json_to_toml_item(item) {
                        tables.push(table);
                    }
                }
                Some(toml_edit::Item::ArrayOfTables(tables))
            } else {
                let mut array = toml_edit::Array::new();
                for item in items {
                    if let Some(toml_edit::Item::Value(value)) = json_to_toml_item(item) {
                        array.push(value);
                    }
                }
                Some(toml_edit::Item::Value(toml_edit::Value::Array(array)))
            }
        }
        serde_json::Value::Object(map) => {
            let mut table = toml_edit::Table::new();
            for (key, item) in map {
                if let Some(item) = json_to_toml_item(item) {
                    table.insert(key, item);
                }
            }
            Some(toml_edit::Item::Table(table))
        }
    }
}

fn toml_item_to_json(item: &toml_edit::Item) -> serde_json::Value {
    match item {
        toml_edit::Item::None => serde_json::Value::Null,
        toml_edit::Item::Value(value) => toml_value_to_json(value),
        toml_edit::Item::Table(table) => {
            let mut map = serde_json::Map::new();
            for (key, item) in table {
                map.insert(key.to_owned(), toml_item_to_json(item));
            }
            serde_json::Value::Object(map)
        }
        toml_edit::Item::ArrayOfTables(tables) => serde_json::Value::Array(
            tables
                .iter()
                .map(|table| {
                    let mut map = serde_json::Map::new();
                    for (key, item) in table {
                        map.insert(key.to_owned(), toml_item_to_json(item));
                    }
                    serde_json::Value::Object(map)
                })
                .collect(),
        ),
    }
}

fn toml_value_to_json(value: &toml_edit::Value) -> serde_json::Value {
    match value {
        toml_edit::Value::String(text) => serde_json::Value::String(text.value().clone()),
        toml_edit::Value::Integer(number) => serde_json::json!(*number.value()),
        toml_edit::Value::Float(number) => serde_json::json!(*number.value()),
        toml_edit::Value::Boolean(flag) => serde_json::Value::Bool(*flag.value()),
        toml_edit::Value::Datetime(dt) => serde_json::Value::String(dt.to_string()),
        toml_edit::Value::Array(array) => {
            serde_json::Value::Array(array.iter().map(toml_value_to_json).collect())
        }
        toml_edit::Value::InlineTable(table) => {
            let mut map = serde_json::Map::new();
            for (key, value) in table {
                map.insert(key.to_owned(), toml_value_to_json(value));
            }
            serde_json::Value::Object(map)
        }
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
        Self::load_pair(&paths.client_json, &paths.config_toml)
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
        let Ok(doc) = raw.parse::<toml_edit::DocumentMut>() else {
            return Self::default();
        };
        let mut mine = serde_json::Map::new();
        for key in Self::OWNED {
            if let Some(item) = doc.get(key) {
                mine.insert(key.to_owned(), toml_item_to_json(item));
            }
        }
        if mine.is_empty() {
            return Self::default();
        }
        let adopted: Self =
            serde_json::from_value(serde_json::Value::Object(mine)).unwrap_or_default();
        if adopted.save_path(client).is_ok() {
            Self::strip_from(daemon);
        }
        adopted
    }

    fn strip_from(daemon: &Path) {
        let Ok(raw) = fs::read_to_string(daemon) else {
            return;
        };
        let Ok(mut doc) = raw.parse::<toml_edit::DocumentMut>() else {
            return;
        };
        let mut touched = false;
        for key in Self::OWNED {
            if doc.get(key).is_some() {
                doc[key] = toml_edit::Item::None;
                touched = true;
            }
        }
        if touched {
            let _ = write_atomic(daemon, doc.to_string().as_bytes());
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
    #[error("config toml failed: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("could not resolve home directory")]
    NoHome,
    #[error("config io failed: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs};

    use goat_provider::ProviderSpecConfig;

    use super::{
        ClientConfig, Config, ConfigDocument, DeviceConfig, ProviderSpecs, ProxyConfig,
        SearchConfig, ThemeChoice, WebFetchConfig,
    };

    #[test]
    fn parses_provider_specs() {
        let cfg = Config::from_toml(
            r#"
[providers.my-proxy]
endpoint = "https://llm.corp/v1"
wire = "responses"
headers = { X-Team = "infra" }
"#,
        )
        .unwrap();
        let spec = &cfg.providers["my-proxy"];
        assert_eq!(spec.endpoint.as_deref(), Some("https://llm.corp/v1"));
        assert_eq!(spec.wire, Some(goat_provider::Dialect::Responses));
        assert_eq!(spec.headers["X-Team"], "infra");
    }

    #[test]
    fn provider_specs_reads_fresh_from_disk() {
        let path = std::env::temp_dir().join("goat-config-provider-specs.toml");
        let _ = std::fs::remove_file(&path);
        let specs = ProviderSpecs::at(path.clone());
        assert!(specs.load().is_empty());
        std::fs::write(
            &path,
            "[providers.my-proxy]\nendpoint = \"http://localhost:9/v1\"\n",
        )
        .unwrap();
        assert_eq!(
            specs.load()["my-proxy"].endpoint.as_deref(),
            Some("http://localhost:9/v1")
        );
        std::fs::write(&path, "").unwrap();
        assert!(specs.load().is_empty());
    }

    #[test]
    fn the_client_defaults_to_dark() {
        assert_eq!(ClientConfig::default().theme, ThemeChoice::Dark);
    }

    #[test]
    fn the_client_adopts_its_keys_from_the_daemon_file_once() {
        let directory = tempfile::tempdir().unwrap();
        let daemon = directory.path().join("config.toml");
        let client = directory.path().join("client.json");
        fs::write(
            &daemon,
            "theme = \"light\"\ndefault_remote = \"box\"\n[proxy]\nbind = \"127.0.0.1:9000\"\n",
        )
        .unwrap();

        let adopted = ClientConfig::load_pair(&client, &daemon);
        assert_eq!(adopted.theme, ThemeChoice::Light);
        assert_eq!(adopted.default_remote.as_deref(), Some("box"));
        assert!(client.exists(), "the adoption is written once");
        let left: toml_edit::DocumentMut = fs::read_to_string(&daemon).unwrap().parse().unwrap();
        assert!(
            left.get("theme").is_none() && left.get("default_remote").is_none(),
            "the adopted keys leave the daemon file so they cannot be edited in two places"
        );
        assert_eq!(
            left["proxy"]["bind"].as_str(),
            Some("127.0.0.1:9000"),
            "what the daemon owns stays where it was"
        );
        let taken: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&client).unwrap()).unwrap();
        assert!(
            taken.get("proxy").is_none(),
            "adoption takes only the keys the client owns"
        );

        fs::write(&daemon, "theme = \"dark\"\n").unwrap();
        assert_eq!(
            ClientConfig::load_pair(&client, &daemon).theme,
            ThemeChoice::Light,
            "once client.json exists the daemon file is never consulted again"
        );
    }

    #[test]
    fn empty_document_is_default() {
        assert_eq!(Config::from_toml("").unwrap(), Config::default());
    }

    #[test]
    fn parses_search_config() {
        let cfg = Config::from_toml(
            r#"
[search]
default_target = "searxng/home"
[[search.accounts]]
provider = "searxng"
account = "home"
endpoint = "https://search.example.com"
"#,
        )
        .unwrap();
        assert_eq!(cfg.search.default_target.as_deref(), Some("searxng/home"));
        assert_eq!(cfg.search.accounts[0].target(), "searxng/home");
    }

    #[test]
    fn proxy_defaults_enabled_on_localhost() {
        let cfg = Config::from_toml("").unwrap();
        assert!(cfg.proxy.enabled);
        assert_eq!(cfg.proxy.bind, "127.0.0.1:7777");
    }

    #[test]
    fn parses_proxy_overrides() {
        let cfg =
            Config::from_toml("[proxy]\nenabled = false\nbind = \"127.0.0.1:9000\"\n").unwrap();
        assert!(!cfg.proxy.enabled);
        assert_eq!(cfg.proxy.bind, "127.0.0.1:9000");
    }

    #[test]
    fn document_edits_preserve_comments() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(
            &path,
            "# my providers\n[providers]\n\n# keep me\n[proxy]\nbind = \"127.0.0.1:7777\"\n",
        )
        .unwrap();
        let mut doc = ConfigDocument::load_at(&path);
        let spec = ProviderSpecConfig {
            endpoint: Some("https://relay.example".to_owned()),
            wire: Some(goat_provider::Dialect::Anthropic),
            ..ProviderSpecConfig::default()
        };
        doc.set_provider("claude-relay", &spec).unwrap();
        doc.save(&path).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("# my providers"));
        assert!(raw.contains("# keep me"));
        assert!(raw.contains("claude-relay"));
        let cfg = Config::load_at(&path);
        assert_eq!(
            cfg.providers["claude-relay"].endpoint.as_deref(),
            Some("https://relay.example")
        );
        assert_eq!(
            cfg.providers["claude-relay"].wire,
            Some(goat_provider::Dialect::Anthropic)
        );
    }

    #[test]
    fn document_removes_provider_entries() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(
            &path,
            "[providers.a]\nendpoint = \"https://a.example\"\n[providers.b]\nendpoint = \"https://b.example\"\n",
        )
        .unwrap();
        let mut doc = ConfigDocument::load_at(&path);
        doc.remove_provider("a");
        doc.save(&path).unwrap();
        let cfg = Config::load_at(&path);
        assert!(!cfg.providers.contains_key("a"));
        assert!(cfg.providers.contains_key("b"));
    }

    #[test]
    fn round_trips_through_toml() {
        let cfg = Config {
            devices: DeviceConfig::default(),
            search: SearchConfig::default(),
            web_fetch: WebFetchConfig::default(),
            proxy: ProxyConfig::default(),
            integrations: BTreeMap::new(),
            providers: BTreeMap::new(),
            unrecognized: BTreeMap::new(),
        };
        let raw = toml::to_string(&cfg).unwrap();
        assert_eq!(Config::from_toml(&raw).unwrap(), cfg);
    }

    #[test]
    fn the_old_remote_key_still_configures_the_listener() {
        let cfg = Config::from_toml("[remote]\nbind = \"0.0.0.0:5000\"\n").unwrap();
        assert_eq!(cfg.devices.bind, "0.0.0.0:5000");
    }

    #[test]
    fn no_default_remote_means_local() {
        let cfg = ClientConfig::default();
        assert!(cfg.default_remote.is_none());
        assert!(cfg.remotes.is_empty());
    }
}
