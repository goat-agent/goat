use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{McpError, ServerConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    User,
    Project,
}

impl Scope {
    pub fn account(self, project_root: &Path) -> String {
        match self {
            Self::User => "user".to_owned(),
            Self::Project => project_identity(project_root),
        }
    }
}

impl std::fmt::Display for Scope {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::User => "user",
            Self::Project => "project",
        })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct McpConfig {
    pub servers: BTreeMap<String, ServerConfig>,
}

pub struct ConfigFile {
    pub path: PathBuf,
    pub config: McpConfig,
    original: Option<Vec<u8>>,
}

impl ConfigFile {
    pub fn open(path: PathBuf) -> Result<Self, McpError> {
        match fs::read(&path) {
            Ok(raw) => {
                let config = parse_compatible(&raw)?;
                Ok(Self {
                    path,
                    config,
                    original: Some(raw),
                })
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                path,
                config: McpConfig::default(),
                original: None,
            }),
            Err(error) => Err(error.into()),
        }
    }

    pub fn save(&mut self) -> Result<(), McpError> {
        let current = match fs::read(&self.path) {
            Ok(raw) => Some(raw),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        if current != self.original {
            return Err(McpError::Config(format!(
                "{} changed while it was being edited",
                self.path.display()
            )));
        }
        let mut raw = serde_json::to_vec_pretty(&self.config)?;
        raw.push(b'\n');
        write_atomic(&self.path, &raw)?;
        self.original = Some(raw);
        Ok(())
    }
}

pub fn project_config_path(project_root: &Path) -> PathBuf {
    project_root.join(".goat").join("mcp.json")
}

pub fn project_identity(project_root: &Path) -> String {
    project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

pub fn parse_compatible(raw: &[u8]) -> Result<McpConfig, McpError> {
    let value: serde_json::Value = serde_json::from_slice(raw)?;
    let Some(object) = value.as_object() else {
        return Err(McpError::Config(
            "MCP config must be a JSON object".to_owned(),
        ));
    };
    if let Some(servers) = object
        .get("mcpServers")
        .filter(|servers| is_server_map(servers))
    {
        return Ok(McpConfig {
            servers: serde_json::from_value(servers.clone())?,
        });
    }
    if let Some(servers) = object
        .get("servers")
        .filter(|servers| is_server_map(servers))
    {
        return Ok(McpConfig {
            servers: serde_json::from_value(servers.clone())?,
        });
    }
    Ok(serde_json::from_value(value)?)
}

fn is_server_map(value: &serde_json::Value) -> bool {
    value
        .as_object()
        .is_some_and(|servers| servers.values().all(serde_json::Value::is_object))
}

pub fn validate_server_name(name: &str) -> Result<(), McpError> {
    if name.trim().is_empty() {
        return Err(McpError::Config(
            "MCP server name must not be empty".to_owned(),
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(McpError::Config(
            "MCP server name must not contain control characters".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn write_atomic(path: &Path, raw: &[u8]) -> Result<(), McpError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let name = path.file_name().map_or_else(
        || "mcp.json".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let temporary = parent.join(format!(".{name}.tmp-{}", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temporary)?;
        file.write_all(raw)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    if let Ok(directory) = fs::File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{StdioConfig, ValueSource};

    #[test]
    fn native_config_is_a_direct_server_map() {
        let config =
            parse_compatible(br#"{"filesystem":{"command":"npx","args":["-y","pkg"]}}"#).unwrap();
        assert!(config.servers.contains_key("filesystem"));
        let raw = serde_json::to_string(&config).unwrap();
        assert!(!raw.contains("mcpServers"));
        assert!(!raw.contains("\"servers\""));
    }

    #[test]
    fn legacy_mcp_servers_wrapper_still_loads() {
        let config = parse_compatible(br#"{"mcpServers":{"one":{"command":"x"}}}"#).unwrap();
        assert!(config.servers.contains_key("one"));
    }

    #[test]
    fn wrapper_words_are_valid_native_server_names() {
        let config = parse_compatible(br#"{"servers":{"command":"x"}}"#).unwrap();
        assert!(config.servers.contains_key("servers"));
    }

    #[test]
    fn saving_detects_a_concurrent_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        let mut file = ConfigFile::open(path.clone()).unwrap();
        file.config.servers.insert(
            "one".to_owned(),
            ServerConfig::Stdio(StdioConfig {
                command: "x".to_owned(),
                args: Vec::new(),
                env: BTreeMap::from([("A".to_owned(), ValueSource::Literal("B".to_owned()))]),
            }),
        );
        fs::write(&path, b"{}\n").unwrap();
        assert!(matches!(file.save(), Err(McpError::Config(_))));
    }
}
