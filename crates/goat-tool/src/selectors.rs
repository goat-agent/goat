use std::collections::HashSet;

use crate::{error::ToolError, name::validate_name};

pub fn selector_allows(tool_name: &str, selectors: &[String]) -> bool {
    if selectors.is_empty() {
        return false;
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
) -> Result<(), ToolError> {
    let known_tools: HashSet<String> = known_tools.into_iter().collect();
    for selector in selectors {
        validate_tool_selector(selector, &known_tools)?;
    }
    Ok(())
}

fn validate_tool_selector(selector: &str, known_tools: &HashSet<String>) -> Result<(), ToolError> {
    let selector = selector.trim();
    if selector.is_empty() {
        return Err(ToolError::invalid_input(
            "tool selector must not be empty".to_string(),
        ));
    }
    if selector == "*" || selector == "!*" {
        return Ok(());
    }
    let name = selector.strip_prefix('!').unwrap_or(selector);
    validate_name(name)?;
    if !known_tools.contains(name) {
        return Err(ToolError::invalid_input(format!(
            "unknown tool selector: {selector}"
        )));
    }
    Ok(())
}
