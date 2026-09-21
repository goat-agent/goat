use std::borrow::Cow;

use serde::Serialize;

use crate::error::ToolError;

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

impl std::borrow::Borrow<str> for ToolName {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl PartialEq<str> for ToolName {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for ToolName {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

pub(crate) fn validate_name(name: &str) -> Result<(), ToolError> {
    if name.is_empty() || name.len() > 128 {
        return Err(ToolError::invalid_input(format!(
            "invalid tool name `{name}`: must be 1..=128 characters"
        )));
    }
    let ok = name
        .chars()
        .all(|c| matches!(c, 'A'..='Z' | 'a'..='z' | '0'..='9' | '_' | '-'));
    if !ok {
        return Err(ToolError::invalid_input(format!(
            "invalid tool name `{name}`: only ASCII letters, digits, underscore, and hyphen are allowed"
        )));
    }
    Ok(())
}
