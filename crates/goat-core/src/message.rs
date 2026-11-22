use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;

use std::fmt::Write as _;

use crate::account;
use crate::image::Image;
use crate::tool::Kind;

crate::id::define!(Id);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    Tool,
    Summary,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
            Self::Summary => "summary",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "user" => Some(Self::User),
            "assistant" => Some(Self::Assistant),
            "tool" => Some(Self::Tool),
            "summary" => Some(Self::Summary),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    pub model: Box<str>,
    pub account: account::Id,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub id: Id,
    pub role: Role,
    pub origin: Option<Origin>,
    pub parts: Vec<Part>,
}

impl Message {
    pub fn new(role: Role, origin: Option<Origin>, parts: Vec<Part>) -> Self {
        Self {
            id: Id::generate(),
            role,
            origin,
            parts,
        }
    }

    pub fn user(text: &str) -> Self {
        Self::new(Role::User, None, vec![Part::text(text)])
    }

    pub fn from_origin(&self, origin: &Origin) -> bool {
        self.origin.as_ref() == Some(origin)
    }

    pub fn tool_calls(&self) -> impl Iterator<Item = (&str, &str, &str)> {
        self.parts.iter().filter_map(|part| match part {
            Part::ToolCall {
                id, name, input, ..
            } => Some((&**id, &**name, &**input)),
            _ => None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Part {
    Text {
        text: Box<str>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        native: Option<Native>,
    },
    Reasoning {
        summary: Box<str>,
        native: Native,
    },
    ToolCall {
        id: Box<str>,
        name: Box<str>,
        input: Box<str>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        native: Option<Native>,
    },
    ToolResult {
        call_id: Box<str>,
        output: Box<str>,
        is_error: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<Image>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        trace: Vec<Trace>,
    },
    Image(Image),
    File {
        path: Box<str>,
        text: Box<str>,
    },
    Shell {
        command: Box<str>,
        output: Box<str>,
        code: Option<i32>,
    },
    Summary {
        text: Box<str>,
        tail: Id,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Trace {
    Call { kind: Kind, target: Box<str> },
    Failed { problem: Box<str> },
    Changed { diff: Diff },
    Loaded { path: Box<str> },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diff {
    pub added: u32,
    pub removed: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<DiffLine>,
    #[serde(default)]
    pub hidden: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffLine {
    pub sign: Sign,
    pub number: u32,
    pub text: Box<str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sign {
    Same,
    Added,
    Removed,
    Gap,
}

impl Part {
    pub fn text(text: &str) -> Self {
        Self::Text {
            text: text.into(),
            native: None,
        }
    }
}

const SHELL: &str = "\
The user ran this shell command themselves in the working directory. \
Its output is here for context, not as a request; wait for the user's next message.";

pub fn shell(command: &str, output: &str, code: Option<i32>) -> String {
    let mut text = format!("{SHELL}\n\n$ {command}\n{output}");
    if !text.ends_with('\n') {
        text.push('\n');
    }
    if let Some(code) = code {
        write!(text, "[exit {code}]").ok();
    }
    text
}

pub fn file(path: &str, text: &str) -> String {
    format!("The user attached {path} to their message:\n\n{text}")
}

#[derive(Debug, Clone)]
pub struct Native(Box<RawValue>);

impl Native {
    pub fn new(raw: Box<RawValue>) -> Self {
        Self(raw)
    }

    pub fn get(&self) -> &RawValue {
        &self.0
    }
}

impl Serialize for Native {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0.get())
    }
}

impl<'de> Deserialize<'de> for Native {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        RawValue::from_string(raw)
            .map(Self)
            .map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_with_native_data_round_trip_through_json() {
        let native = Native::new(
            RawValue::from_string(r#"{"type":"reasoning","encrypted_content":"x"}"#.to_owned())
                .unwrap(),
        );
        let parts = vec![Part::Reasoning {
            summary: "s".into(),
            native,
        }];
        let json = serde_json::to_string(&parts).unwrap();
        let back: Vec<Part> = serde_json::from_str(&json).unwrap();
        let Part::Reasoning { native, .. } = &back[0] else {
            panic!("expected reasoning");
        };
        assert_eq!(
            native.get().get(),
            r#"{"type":"reasoning","encrypted_content":"x"}"#
        );
    }
}
