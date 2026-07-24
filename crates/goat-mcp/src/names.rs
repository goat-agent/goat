use std::collections::HashSet;

pub fn sanitize_component(input: &str) -> String {
    let mut output = String::new();
    let mut last_was_sep = false;
    for ch in input.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            output.push(ch);
            last_was_sep = false;
        } else if !last_was_sep && !output.is_empty() {
            output.push('_');
            last_was_sep = true;
        }
    }
    while output.ends_with('_') {
        output.pop();
    }
    if output.is_empty() {
        "unnamed".to_owned()
    } else {
        output
    }
}

pub fn exposed_tool_name(server: &str, tool: &str) -> String {
    format!(
        "mcp__{}__{}",
        sanitize_component(server),
        sanitize_component(tool)
    )
}

pub(crate) fn unique_tool_name(used: &mut HashSet<String>, server: &str, tool: &str) -> String {
    let base = exposed_tool_name(server, tool);
    if used.insert(base.clone()) {
        return base;
    }
    let mut index = 2;
    loop {
        let candidate = format!("{base}_{index}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        index += 1;
    }
}
