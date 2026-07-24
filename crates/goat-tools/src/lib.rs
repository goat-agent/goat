use std::collections::HashMap;

use goat_tool::{Tool, ToolSpec};

pub struct ToolRegistry {
    tools: HashMap<&'static str, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn builtin() -> Self {
        let mut tools: HashMap<&'static str, Box<dyn Tool>> = HashMap::new();
        for tool in builtin_tools() {
            tools.insert(tool.name(), tool);
        }
        Self { tools }
    }

    #[must_use]
    pub fn with(mut self, tool: Box<dyn Tool>) -> Self {
        self.tools.insert(tool.name(), tool);
        self
    }

    #[must_use]
    pub fn with_many(mut self, tools: Vec<Box<dyn Tool>>) -> Self {
        for tool in tools {
            self.tools.insert(tool.name(), tool);
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
}

fn builtin_tools() -> Vec<Box<dyn Tool>> {
    let mut tools = goat_tool_fs::all();
    tools.extend(goat_tool_shell::all());
    tools.extend(goat_tool_search::all());
    tools.extend(goat_tool_skill::all());
    tools.extend(goat_tool_web::all());
    tools
}

#[cfg(test)]
mod tests {
    use super::ToolRegistry;

    #[test]
    fn builtin_registers_all_tools() {
        let registry = ToolRegistry::builtin();
        for name in [
            "Read",
            "Write",
            "Edit",
            "Bash",
            "Grep",
            "Glob",
            "Skill",
            "WebFetch",
            "WebSearch",
        ] {
            assert!(registry.get(name).is_some(), "missing tool: {name}");
        }
    }

    #[test]
    fn specs_are_sorted_by_name() {
        let registry = ToolRegistry::builtin();
        let specs = registry.specs();
        let names: Vec<&str> = specs.iter().map(|spec| spec.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
        assert_eq!(specs.len(), 9);
    }

    #[test]
    fn with_many_registers_dynamic_tools() {
        let registry = ToolRegistry::builtin().with_many(Vec::new());
        assert!(registry.get("Read").is_some());
    }

    #[test]
    fn unknown_tool_is_none() {
        let registry = ToolRegistry::builtin();
        assert!(registry.get("Nonexistent").is_none());
    }
}
