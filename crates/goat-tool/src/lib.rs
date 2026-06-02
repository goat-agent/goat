use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
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

/// Handles a single tool invocation inside a brain turn.
///
/// Implement this trait in a `goat-tool-<name>` crate. The brain resolves
/// the tool name to a handler via the [`ToolRegistry`] and calls [`call`]
/// once per tool invocation per round.
///
/// [`call`] must never panic. Return a [`ToolOutput`] with a descriptive
/// error message if the tool cannot complete; the brain forwards the error
/// text back to the model so it can decide how to proceed.
///
/// Register your tool via:
/// ```ignore
/// inventory::submit!(ToolFactory { name: MY_TOOL_NAME, default_enabled: true, spec: ..., ctor: ... });
/// ```
#[async_trait]
pub trait ToolHandler: Send + Sync + 'static {
    /// Executes the tool. `ctx` carries the conversation, persona, and store
    /// references; `call` contains the tool name and validated JSON arguments.
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

    /// Insert a pre-built handler. Use this for tools whose constructor
    /// requires runtime-injected state (for example a database handle)
    /// and therefore cannot be expressed as a stateless `ToolFactory`.
    pub fn insert_handler(
        &mut self,
        spec: ToolSpec,
        handler: Arc<dyn ToolHandler>,
        default_enabled: bool,
    ) {
        validate_name(spec.name.as_str()).expect("invalid tool spec name");
        self.by_name.insert(
            spec.name.clone(),
            RegisteredTool {
                spec,
                handler,
                default_enabled,
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

    pub fn default_tool_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .by_name
            .values()
            .filter(|t| t.default_enabled)
            .map(|t| t.spec.name.as_str().to_string())
            .collect();
        names.sort();
        names
    }

    pub fn validate_default_selectors(&self, selectors: &[String]) -> ToolResultValue<()> {
        validate_tool_selectors(selectors, self.default_tool_names())
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

pub fn selector_allows(tool_name: &str, selectors: &[String]) -> bool {
    selector_allows_with_empty_default(tool_name, selectors, false)
}

pub fn selector_allows_empty_denies(tool_name: &str, selectors: &[String]) -> bool {
    selector_allows_with_empty_default(tool_name, selectors, false)
}

fn selector_allows_with_empty_default(
    tool_name: &str,
    selectors: &[String],
    empty_default: bool,
) -> bool {
    if selectors.is_empty() {
        return empty_default;
    }
    let mut allowed = false;
    let mut denied = false;
    for selector in selectors {
        let selector = selector.trim();
        if selector.is_empty() {
            continue;
        }
        if selector == "*" {
            allowed = true;
        } else if let Some(denied_name) = selector.strip_prefix('!') {
            if denied_name == tool_name || denied_name == "*" {
                denied = true;
            }
        } else if selector == tool_name {
            allowed = true;
        }
    }
    allowed && !denied
}

pub fn validate_tool_selectors(
    selectors: &[String],
    known_tools: impl IntoIterator<Item = String>,
) -> ToolResultValue<()> {
    let known_tools: HashSet<String> = known_tools.into_iter().collect();
    for selector in selectors {
        validate_tool_selector(selector, &known_tools)?;
    }
    Ok(())
}

fn validate_tool_selector(selector: &str, known_tools: &HashSet<String>) -> ToolResultValue<()> {
    let selector = selector.trim();
    if selector.is_empty() {
        return Err(ToolError::InvalidInput(
            "tool selector must not be empty".to_string(),
        ));
    }
    if selector == "*" || selector == "!*" {
        return Ok(());
    }
    let name = selector.strip_prefix('!').unwrap_or(selector);
    validate_name(name)?;
    if !known_tools.contains(name) {
        return Err(ToolError::InvalidInput(format!(
            "unknown tool selector: {selector}"
        )));
    }
    Ok(())
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

    #[test]
    fn selector_star_allows_tools() {
        assert!(selector_allows("shell", &["*".to_string()]));
        assert!(selector_allows("read", &["*".to_string()]));
    }

    #[test]
    fn selector_negation_wins() {
        let selectors = vec!["*".to_string(), "!shell".to_string()];
        assert!(!selector_allows("shell", &selectors));
        assert!(selector_allows("read", &selectors));
    }

    #[test]
    fn selector_allowlist_excludes_others() {
        let selectors = vec!["read".to_string(), "grep".to_string()];
        assert!(selector_allows("read", &selectors));
        assert!(selector_allows("grep", &selectors));
        assert!(!selector_allows("shell", &selectors));
    }

    #[test]
    fn validates_unknown_selector() {
        let err =
            validate_tool_selectors(&["bash".to_string()], vec!["shell".to_string()]).unwrap_err();
        assert!(err.to_string().contains("unknown tool selector"));
    }
}
