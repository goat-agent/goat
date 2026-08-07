use std::collections::HashMap;

use crate::{Tool, ToolSpec};

pub struct ToolRegistry {
    tools: HashMap<&'static str, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new(tools: Vec<Box<dyn Tool>>) -> Self {
        let mut registry = Self {
            tools: HashMap::new(),
        };
        for tool in tools {
            registry.insert(tool);
        }
        registry
    }

    #[must_use]
    pub fn with(mut self, tool: Box<dyn Tool>) -> Self {
        self.insert(tool);
        self
    }

    #[must_use]
    pub fn with_many(mut self, tools: Vec<Box<dyn Tool>>) -> Self {
        for tool in tools {
            self.insert(tool);
        }
        self
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(AsRef::as_ref)
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        let mut specs: Vec<ToolSpec> = self
            .tools
            .values()
            .map(|tool| ToolSpec {
                name: tool.name(),
                description: tool.description(),
                parameters: tool.parameters(),
            })
            .collect();
        specs.sort_by_key(|spec| spec.name);
        specs
    }

    fn insert(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name(), tool);
    }
}
