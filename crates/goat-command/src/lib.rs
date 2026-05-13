use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct CommandName(Cow<'static, str>);

impl CommandName {
    pub const fn from_static(name: &'static str) -> Self {
        Self(Cow::Borrowed(name))
    }

    pub fn new(name: impl Into<String>) -> Result<Self, CommandError> {
        let name = name.into();
        validate_name(&name)?;
        Ok(Self(Cow::Owned(name)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CommandName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_str().fmt(f)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CommandSpec {
    pub name: CommandName,
    pub description: String,
    pub args: CommandArgs,
}

impl CommandSpec {
    pub fn raw_string(name: CommandName, description: impl Into<String>) -> Self {
        Self {
            name,
            description: description.into(),
            args: CommandArgs::RawString {
                name: "args".to_string(),
                description: "Optional command arguments".to_string(),
                required: false,
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum CommandArgs {
    None,
    RawString {
        name: String,
        description: String,
        required: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CommandCall {
    pub call_id: String,
    pub name: CommandName,
    pub args: String,
    pub raw: serde_json::Value,
}

impl CommandCall {
    pub fn new(
        call_id: impl Into<String>,
        name: CommandName,
        args: impl Into<String>,
        raw: serde_json::Value,
    ) -> Self {
        Self {
            call_id: call_id.into(),
            name,
            args: args.into(),
            raw,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CommandOutput {
    /// Inject this content as the current user turn before the LLM completes.
    Query { content: String },
    /// Send this text directly without an LLM completion.
    Reply { text: String },
    /// Acknowledge and do nothing.
    Skip,
}

#[async_trait]
pub trait CommandHandler: Send + Sync + 'static {
    async fn call(&self, call: CommandCall) -> Result<CommandOutput, CommandError>;
}

pub struct CommandRegistration {
    pub spec: CommandSpec,
    pub handler: Arc<dyn CommandHandler>,
}

#[derive(Default)]
pub struct CommandRegistry {
    commands: HashMap<CommandName, CommandRegistration>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(
        &mut self,
        spec: CommandSpec,
        handler: Arc<dyn CommandHandler>,
    ) -> Result<(), CommandError> {
        if self.commands.contains_key(&spec.name) {
            return Err(CommandError::Duplicate(spec.name.as_str().to_string()));
        }
        self.commands
            .insert(spec.name.clone(), CommandRegistration { spec, handler });
        Ok(())
    }

    pub fn specs(&self) -> Vec<CommandSpec> {
        let mut specs = self
            .commands
            .values()
            .map(|registration| registration.spec.clone())
            .collect::<Vec<_>>();
        specs.sort_by(|a, b| a.name.cmp(&b.name));
        specs
    }

    pub async fn call(&self, call: CommandCall) -> Result<CommandOutput, CommandError> {
        let Some(registration) = self.commands.get(&call.name) else {
            return Err(CommandError::NotFound(call.name.as_str().to_string()));
        };
        registration.handler.call(call).await
    }
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CommandProviderContext {
    pub goat_root: PathBuf,
    pub persona: uuid::Uuid,
}

impl CommandProviderContext {
    pub fn new(goat_root: PathBuf, persona: uuid::Uuid) -> Self {
        Self { goat_root, persona }
    }
}

pub struct CommandFactory {
    pub id: &'static str,
    pub register: fn(&mut CommandRegistry, CommandProviderContext),
}

inventory::collect!(CommandFactory);

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CommandError {
    #[error("invalid command name `{0}`")]
    InvalidName(String),
    #[error("duplicate command `{0}`")]
    Duplicate(String),
    #[error("unknown command `{0}`")]
    NotFound(String),
    #[error("command failed: {0}")]
    Failed(String),
}

fn validate_name(name: &str) -> Result<(), CommandError> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return Err(CommandError::InvalidName(name.to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    #[async_trait]
    impl CommandHandler for Echo {
        async fn call(&self, call: CommandCall) -> Result<CommandOutput, CommandError> {
            Ok(CommandOutput::Query { content: call.args })
        }
    }

    #[test]
    fn rejects_platform_unsafe_names() {
        assert!(CommandName::new("skill").is_ok());
        assert!(CommandName::new("remind-me").is_ok());
        assert!(CommandName::new("skill.activate").is_err());
        assert!(CommandName::new("").is_err());
    }

    #[tokio::test]
    async fn registry_rejects_duplicate_names() {
        let mut registry = CommandRegistry::new();
        let spec = CommandSpec::raw_string(CommandName::new("skill").unwrap(), "Run skill");
        registry.insert(spec.clone(), Arc::new(Echo)).unwrap();
        assert!(matches!(
            registry.insert(spec, Arc::new(Echo)),
            Err(CommandError::Duplicate(name)) if name == "skill"
        ));
    }
}
