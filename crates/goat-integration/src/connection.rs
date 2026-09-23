use std::collections::BTreeMap;

use goat_auth::{Credential, CredentialKey, CredentialStore};
use serde_json::Value;

use crate::{IntegrationAuth, IntegrationBinding, IntegrationError, IntegrationMetadata};

pub const KIND_KEY: &str = "kind";
pub const CONNECTION_KEYS: &[&str] = &[KIND_KEY, "host", "client_id"];
pub const PROJECT_USAGE_FILE: &str = "integrations.json";
pub const CLIENT_ID_SLOT: &str = "client_id";
pub const CLIENT_SECRET_SLOT: &str = "client_secret";

#[derive(Clone, Debug, PartialEq)]
pub struct Connection {
    pub name: String,
    pub kind: String,
    pub config: Value,
}

impl Connection {
    pub fn new(name: impl Into<String>, kind: impl Into<String>, config: Value) -> Self {
        Self {
            name: name.into(),
            kind: kind.into(),
            config: without_kind(config),
        }
    }

    pub fn parse(name: &str, entry: &Value) -> Result<Self, IntegrationError> {
        validate_name(name)?;
        let object = match entry {
            Value::Object(object) => object,
            Value::Null => &serde_json::Map::new(),
            _ => {
                return Err(IntegrationError::Config(format!(
                    "integration `{name}` must be a table"
                )));
            }
        };
        let kind = match object.get(KIND_KEY) {
            None => name.to_owned(),
            Some(Value::String(kind)) => kind.clone(),
            Some(_) => {
                return Err(IntegrationError::Config(format!(
                    "integration `{name}`: `{KIND_KEY}` must be a string"
                )));
            }
        };
        if crate::factory_for(&kind).is_none() {
            return Err(IntegrationError::Config(format!(
                "integration `{name}`: unknown kind `{kind}`"
            )));
        }
        Ok(Self::new(name, kind, Value::Object(object.clone())))
    }

    pub fn is_primary(&self) -> bool {
        self.name == self.kind
    }

    pub fn entry(&self) -> Value {
        let mut entry = self.config.as_object().cloned().unwrap_or_default();
        if !self.is_primary() {
            entry.insert(KIND_KEY.to_owned(), Value::String(self.kind.clone()));
        }
        Value::Object(entry)
    }

    pub fn credential_key(&self) -> CredentialKey {
        CredentialKey::integration(&self.kind, &self.name)
    }

    pub fn slot_key(&self, slot: &str) -> CredentialKey {
        CredentialKey::integration_slot(&self.kind, &self.name, slot)
    }

    pub fn credential_keys(&self) -> [CredentialKey; 3] {
        [
            self.credential_key(),
            self.slot_key(CLIENT_ID_SLOT),
            self.slot_key(CLIENT_SECRET_SLOT),
        ]
    }

    pub fn env_var(&self, metadata: &IntegrationMetadata) -> Option<&'static str> {
        metadata.env_var.filter(|_| self.is_primary())
    }

    pub fn binding(&self, usage: &Value) -> IntegrationBinding {
        let mut config = self.config.as_object().cloned().unwrap_or_default();
        if let Some(usage) = usage.as_object() {
            config.extend(
                usage
                    .iter()
                    .filter(|(key, _)| !CONNECTION_KEYS.contains(&key.as_str()))
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
        }
        IntegrationBinding {
            account: self.name.clone(),
            config: Value::Object(config),
        }
    }
}

pub fn reject_connection_keys(usage: &Value) -> Result<(), IntegrationError> {
    let Some(object) = usage.as_object() else {
        return Ok(());
    };
    match CONNECTION_KEYS
        .iter()
        .find(|key| object.contains_key(**key))
    {
        Some(key) => Err(IntegrationError::Config(format!(
            "`{key}` belongs to the connection, not to its use; set it with `goat integration add`"
        ))),
        None => Ok(()),
    }
}

pub fn project_usage_path(project_root: &std::path::Path) -> std::path::PathBuf {
    project_root.join(".goat").join(PROJECT_USAGE_FILE)
}

pub fn load_project_usage(
    project_root: &std::path::Path,
) -> Result<Option<BTreeMap<String, Value>>, IntegrationError> {
    let path = project_usage_path(project_root);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(IntegrationError::Config(format!(
                "reading {}: {e}",
                path.display()
            )));
        }
    };
    serde_json::from_str(&raw)
        .map(Some)
        .map_err(|e| IntegrationError::Config(format!("parsing {}: {e}", path.display())))
}

pub fn tool_prefix(kind_prefix: &str, kind: &str, connection: &str) -> String {
    if connection == kind {
        kind_prefix.to_owned()
    } else {
        format!("{}_", connection.replace('-', "_"))
    }
}

pub fn validate_name(name: &str) -> Result<(), IntegrationError> {
    let valid = !name.is_empty()
        && name.len() <= 32
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && !name.ends_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if valid {
        Ok(())
    } else {
        Err(IntegrationError::Config(format!(
            "`{name}` is not a valid connection name; use lowercase letters, digits and `-`, \
             starting with a letter"
        )))
    }
}

#[derive(Clone, Debug, Default)]
pub struct Connections {
    pub valid: Vec<Connection>,
    pub invalid: Vec<(String, String)>,
}

impl Connections {
    pub fn parse(entries: &BTreeMap<String, Value>) -> Self {
        let mut parsed = Self::default();
        for (name, entry) in entries {
            match Connection::parse(name, entry) {
                Ok(connection) => parsed.valid.push(connection),
                Err(e) => parsed.invalid.push((name.clone(), e.to_string())),
            }
        }
        parsed
    }

    pub fn get(&self, name: &str) -> Option<&Connection> {
        self.valid.iter().find(|connection| connection.name == name)
    }

    pub fn of_kind<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a Connection> + 'a {
        self.valid
            .iter()
            .filter(move |connection| connection.kind == kind)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialSource {
    Stored,
    Environment,
    External,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    Ready(CredentialSource),
    NeedsLogin(String),
}

impl ConnectionState {
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready(_))
    }
}

pub fn connection_state(
    metadata: &IntegrationMetadata,
    connection: &Connection,
    credentials: &CredentialStore,
) -> ConnectionState {
    if metadata.auth == IntegrationAuth::External {
        return ConnectionState::Ready(CredentialSource::External);
    }
    if let Some(var) = connection.env_var(metadata)
        && std::env::var(var).is_ok_and(|value| !value.is_empty())
    {
        return ConnectionState::Ready(CredentialSource::Environment);
    }
    match credentials.get(&connection.credential_key()) {
        Some(Credential::OAuth(tokens))
            if tokens.is_expired() && tokens.refresh_token.is_none() =>
        {
            ConnectionState::NeedsLogin("the token expired".to_owned())
        }
        Some(_) => ConnectionState::Ready(CredentialSource::Stored),
        None if credentials
            .get(&connection.slot_key(CLIENT_ID_SLOT))
            .is_some() =>
        {
            ConnectionState::NeedsLogin("login was not finished".to_owned())
        }
        None => ConnectionState::NeedsLogin("no credential is stored".to_owned()),
    }
}

fn without_kind(config: Value) -> Value {
    match config {
        Value::Object(mut object) => {
            object.remove(KIND_KEY);
            Value::Object(object)
        }
        Value::Null => Value::Object(serde_json::Map::new()),
        other => other,
    }
}
