use std::collections::HashMap;
use std::sync::Arc;

use crate::{
    ToolDefinitionContext,
    error::ToolError,
    name::ToolName,
    selectors::validate_tool_selectors,
    spec::ToolSpec,
    tool::{Tool, ToolCall, ToolContext, ToolOutput},
};

#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: HashMap<ToolName, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new(tools: Vec<Arc<dyn Tool>>) -> Self {
        let mut registry = Self::default();
        for tool in tools {
            registry.insert(tool);
        }
        registry
    }

    #[must_use]
    pub fn with(mut self, tool: Arc<dyn Tool>) -> Self {
        self.insert(tool);
        self
    }

    #[must_use]
    pub fn with_many(mut self, tools: Vec<Arc<dyn Tool>>) -> Self {
        for tool in tools {
            self.insert(tool);
        }
        self
    }

    pub fn insert(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name(), tool);
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(AsRef::as_ref)
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.specs_for(ToolDefinitionContext::default())
    }

    pub fn specs_for(&self, context: ToolDefinitionContext) -> Vec<ToolSpec> {
        let mut specs: Vec<ToolSpec> = self
            .tools
            .values()
            .filter_map(|tool| tool.definition(context))
            .collect();
        specs.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
        specs
    }

    pub fn default_specs(&self) -> Vec<ToolSpec> {
        let mut specs: Vec<ToolSpec> = self
            .tools
            .values()
            .filter(|tool| tool.default_enabled())
            .filter_map(|tool| tool.definition(ToolDefinitionContext::default()))
            .collect();
        specs.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
        specs
    }

    pub fn default_tool_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .tools
            .values()
            .filter(|tool| tool.default_enabled())
            .map(|tool| tool.name().as_str().to_string())
            .collect();
        names.sort();
        names
    }

    pub fn validate_default_selectors(&self, selectors: &[String]) -> Result<(), ToolError> {
        validate_tool_selectors(selectors, self.default_tool_names())
    }

    pub async fn call<'a>(&'a self, ctx: ToolContext<'a>, call: &'a ToolCall) -> ToolOutput {
        let Some(tool) = self.tools.get(call.name.as_str()) else {
            return ToolOutput::error(format!("unknown tool: {}", call.name));
        };
        match tool.call(call, ctx).await {
            Ok(output) => output,
            Err(error) => ToolOutput::error(error.to_string()),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}
