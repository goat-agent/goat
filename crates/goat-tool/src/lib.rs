use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use goat_types::{ConversationId, PersonaId};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Eq, PartialEq, Hash, Debug, Serialize)]
pub struct ToolName(Cow<'static, str>);

impl ToolName {
    pub const fn from_static(name: &'static str) -> Self {
        Self(Cow::Borrowed(name))
    }

    pub fn new(name: impl Into<String>) -> Result<Self, ToolError> {
        let name = name.into();
        validate_name(&name)?;
        Ok(Self(Cow::Owned(name)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ToolName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("invalid tool name `{name}`: {reason}")]
    InvalidName { name: String, reason: &'static str },
    #[error("tool not found: {0}")]
    NotFound(String),
    #[error("tool denied: {0}")]
    Denied(String),
    #[error("tool input invalid: {0}")]
    InvalidInput(String),
    #[error("tool execution failed: {0}")]
    Execution(String),
}

pub type ToolResultValue<T> = Result<T, ToolError>;

fn validate_name(name: &str) -> ToolResultValue<()> {
    if name.is_empty() || name.len() > 128 {
        return Err(ToolError::InvalidName {
            name: name.to_string(),
            reason: "must be 1..=128 characters",
        });
    }
    let ok = name
        .chars()
        .all(|c| matches!(c, 'A'..='Z' | 'a'..='z' | '0'..='9' | '_' | '-'));
    if !ok {
        return Err(ToolError::InvalidName {
            name: name.to_string(),
            reason: "only ASCII letters, digits, underscore, and hyphen are allowed",
        });
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize)]
pub struct ToolSpec {
    pub name: ToolName,
    pub title: Option<String>,
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
    pub output_schema: Option<serde_json::Value>,
    #[serde(default)]
    pub annotations: serde_json::Value,
}

impl ToolSpec {
    pub fn new(
        name: ToolName,
        description: impl Into<String>,
        input_schema: serde_json::Value,
    ) -> Self {
        Self {
            name,
            title: None,
            description: Some(description.into()),
            input_schema,
            output_schema: None,
            annotations: serde_json::Value::Null,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ToolCall {
    pub call_id: String,
    pub name: ToolName,
    pub arguments: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolContent {
    Text { text: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolOutput {
    pub content: Vec<ToolContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<serde_json::Value>,
    #[serde(default)]
    pub is_error: bool,
}

impl ToolOutput {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![ToolContent::Text { text: text.into() }],
            structured_content: None,
            is_error: false,
        }
    }

    pub fn structured(value: serde_json::Value) -> Self {
        Self {
            content: vec![ToolContent::Text {
                text: value.to_string(),
            }],
            structured_content: Some(value),
            is_error: false,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            content: vec![ToolContent::Text { text: text.into() }],
            structured_content: None,
            is_error: true,
        }
    }

    pub fn text_for_model(&self) -> String {
        let mut out = String::new();
        for part in &self.content {
            match part {
                ToolContent::Text { text } => {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(text);
                }
            }
        }
        if out.is_empty() {
            if let Some(value) = &self.structured_content {
                return value.to_string();
            }
        }
        out
    }
}

#[derive(Clone, Debug)]
pub struct ToolContext {
    pub persona: PersonaId,
    pub conversation: ConversationId,
    pub goat_root: PathBuf,
    pub read_state: ToolReadState,
}

pub type ToolReadState = Arc<Mutex<HashMap<PathBuf, ToolReadSnapshot>>>;

#[derive(Clone, Debug)]
pub struct ToolReadSnapshot {
    pub size: u64,
    pub modified_ms: Option<u128>,
    pub hash: u64,
    pub complete: bool,
}

#[async_trait]
pub trait ToolHandler: Send + Sync + 'static {
    async fn call(&self, ctx: ToolContext, call: ToolCall) -> ToolOutput;
}

pub struct ToolFactory {
    pub name: ToolName,
    pub default_enabled: bool,
    pub spec: fn() -> ToolSpec,
    pub ctor: fn() -> Arc<dyn ToolHandler>,
}

inventory::collect!(ToolFactory);

#[derive(Clone, Default)]
pub struct ToolRegistry {
    by_name: HashMap<ToolName, RegisteredTool>,
}

#[derive(Clone)]
struct RegisteredTool {
    spec: ToolSpec,
    handler: Arc<dyn ToolHandler>,
    default_enabled: bool,
}

impl ToolRegistry {
    pub fn from_inventory() -> Self {
        let mut reg = Self::default();
        for factory in inventory::iter::<ToolFactory>() {
            reg.insert(factory);
        }
        reg
    }

    pub fn insert(&mut self, factory: &ToolFactory) {
        validate_name(factory.name.as_str()).expect("invalid registered tool name");
        let spec = (factory.spec)();
        validate_name(spec.name.as_str()).expect("invalid registered tool spec name");
        self.by_name.insert(
            factory.name.clone(),
            RegisteredTool {
                spec,
                handler: (factory.ctor)(),
                default_enabled: factory.default_enabled,
            },
        );
    }

    pub fn default_specs(&self) -> Vec<ToolSpec> {
        let mut specs: Vec<ToolSpec> = self
            .by_name
            .values()
            .filter(|t| t.default_enabled)
            .map(|t| t.spec.clone())
            .collect();
        specs.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
        specs
    }

    pub async fn call(&self, ctx: ToolContext, call: ToolCall) -> ToolOutput {
        let Some(tool) = self.by_name.get(&call.name) else {
            return ToolOutput::error(format!("unknown tool: {}", call.name));
        };
        tool.handler.call(ctx, call).await
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_provider_safe_names() {
        assert!(ToolName::new("shell").is_ok());
        assert!(ToolName::new("DATA_EXPORT_v2").is_ok());
        assert!(ToolName::new("shell.run").is_err());
        assert!(ToolName::new("bad name").is_err());
    }

    #[test]
    fn no_parameter_schema_is_object() {
        let spec = ToolSpec::new(
            ToolName::new("time_now").unwrap(),
            "Return time",
            serde_json::json!({"type":"object","additionalProperties":false}),
        );
        assert_eq!(spec.input_schema["type"], "object");
    }
}
