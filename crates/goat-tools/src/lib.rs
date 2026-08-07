use goat_tool::ToolRegistry;

pub fn builtin() -> ToolRegistry {
    let mut tools = goat_tool_fs::all();
    tools.extend(goat_tool_shell::all());
    tools.extend(goat_tool_search::all());
    tools.extend(goat_tool_skill::all());
    tools.extend(goat_tool_web::all());
    ToolRegistry::new(tools)
}

#[cfg(test)]
mod tests {
    #[test]
    fn builtin_registers_all_tools() {
        let registry = super::builtin();
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
        let registry = super::builtin();
        let specs = registry.specs();
        let names: Vec<&str> = specs.iter().map(|spec| spec.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
        assert_eq!(specs.len(), 9);
    }

    #[test]
    fn registry_accepts_dynamic_tools() {
        let registry = super::builtin().with_many(Vec::new());
        assert!(registry.get("Read").is_some());
    }

    #[test]
    fn unknown_tool_is_none() {
        let registry = super::builtin();
        assert!(registry.get("Nonexistent").is_none());
    }
}
