use std::collections::BTreeSet;
use std::path::Path;

use serde::Deserialize;

use crate::SkillError;

const MAX_NAME_LEN: usize = 64;
const MAX_DESCRIPTION_LEN: usize = 1024;
const MAX_ARGUMENT_NAME_LEN: usize = 32;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Argument {
    pub name: String,
    pub description: String,
    pub required: bool,
    pub value: ArgumentValue,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ArgumentValue {
    Word,
    Integer,
    TextTail,
    Choice(Vec<Choice>),
}

impl ArgumentValue {
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Word => "word",
            Self::Integer => "integer",
            Self::TextTail => "text",
            Self::Choice(_) => "choice",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Choice {
    pub value: String,
    pub description: Option<String>,
}

#[derive(Debug)]
pub(crate) struct Parsed {
    pub name: String,
    pub description: String,
    pub arguments: Vec<Argument>,
    pub body: String,
}

#[derive(Deserialize)]
struct FrontMatter {
    #[serde(default)]
    name: Option<String>,
    description: String,
    #[serde(default)]
    arguments: Vec<RawArgument>,
}

#[derive(Deserialize)]
struct RawArgument {
    name: String,
    description: String,
    #[serde(default)]
    required: bool,
    value: RawValue,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawValue {
    Kind(RawKind),
    Choice { choice: Vec<RawChoice> },
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawKind {
    Word,
    Integer,
    TextTail,
}

#[derive(Deserialize)]
struct RawChoice {
    value: String,
    #[serde(default)]
    description: Option<String>,
}

pub(crate) fn parse(text: &str, path: &Path, dir_name: &str) -> Result<Parsed, SkillError> {
    let (front, body) =
        split(text).ok_or_else(|| SkillError::MissingFrontMatter(path.to_path_buf()))?;
    let front: FrontMatter = serde_yaml::from_str(front).map_err(|source| SkillError::Yaml {
        path: path.to_path_buf(),
        source,
    })?;

    let name = match front.name.map(|name| name.trim().to_owned()) {
        Some(name) if !name.is_empty() => {
            if name != dir_name {
                return Err(SkillError::NameMismatch {
                    name,
                    dir: dir_name.to_owned(),
                });
            }
            name
        }
        _ => dir_name.to_owned(),
    };
    validate_name(&name, path)?;

    let description = front.description.trim().to_owned();
    if description.is_empty() {
        return Err(invalid(path, "description", "cannot be empty"));
    }
    if description.chars().count() > MAX_DESCRIPTION_LEN {
        return Err(invalid(
            path,
            "description",
            format!("exceeds {MAX_DESCRIPTION_LEN} characters"),
        ));
    }

    let arguments = arguments(front.arguments, path)?;

    Ok(Parsed {
        name,
        description,
        arguments,
        body: body.trim().to_owned(),
    })
}

fn split(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start_matches('\u{feff}');
    let rest = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))?;
    let end = rest.find("\n---")?;
    let (front, after) = rest.split_at(end);
    let body = after
        .trim_start_matches("\n---")
        .trim_start_matches(['\r', '\n']);
    Some((front, body))
}

fn arguments(raw: Vec<RawArgument>, path: &Path) -> Result<Vec<Argument>, SkillError> {
    let count = raw.len();
    let mut names = BTreeSet::new();
    let mut optional_seen = false;
    let mut tail_seen = false;
    let mut out = Vec::with_capacity(count);
    for (index, argument) in raw.into_iter().enumerate() {
        validate_argument_name(&argument.name, path)?;
        if !names.insert(argument.name.clone()) {
            return Err(invalid(
                path,
                "arguments",
                format!("`{}` is declared twice", argument.name),
            ));
        }
        let description = argument.description.trim().to_owned();
        if description.is_empty() {
            return Err(invalid(
                path,
                "arguments",
                format!("`{}` has no description", argument.name),
            ));
        }
        if description.chars().count() > MAX_DESCRIPTION_LEN {
            return Err(invalid(
                path,
                "arguments",
                format!(
                    "`{}` description exceeds {MAX_DESCRIPTION_LEN} characters",
                    argument.name
                ),
            ));
        }
        if argument.required {
            if optional_seen {
                return Err(invalid(
                    path,
                    "arguments",
                    format!("required `{}` follows an optional one", argument.name),
                ));
            }
        } else {
            optional_seen = true;
        }
        let value = value(argument.value, &argument.name, path)?;
        if value == ArgumentValue::TextTail {
            if tail_seen {
                return Err(invalid(path, "arguments", "only one may take text"));
            }
            tail_seen = true;
            if index + 1 != count {
                return Err(invalid(
                    path,
                    "arguments",
                    format!("`{}` takes text so it must come last", argument.name),
                ));
            }
        }
        out.push(Argument {
            name: argument.name,
            description,
            required: argument.required,
            value,
        });
    }
    Ok(out)
}

fn value(raw: RawValue, argument: &str, path: &Path) -> Result<ArgumentValue, SkillError> {
    let choices = match raw {
        RawValue::Kind(RawKind::Word) => return Ok(ArgumentValue::Word),
        RawValue::Kind(RawKind::Integer) => return Ok(ArgumentValue::Integer),
        RawValue::Kind(RawKind::TextTail) => return Ok(ArgumentValue::TextTail),
        RawValue::Choice { choice } => choice,
    };
    if choices.is_empty() {
        return Err(invalid(
            path,
            "arguments",
            format!("`{argument}` offers no choices"),
        ));
    }
    let mut values = BTreeSet::new();
    let mut out = Vec::with_capacity(choices.len());
    for choice in choices {
        let value = choice.value.trim().to_owned();
        if value.is_empty() {
            return Err(invalid(
                path,
                "arguments",
                format!("`{argument}` has an empty choice"),
            ));
        }
        if !values.insert(value.clone()) {
            return Err(invalid(
                path,
                "arguments",
                format!("`{argument}` offers `{value}` twice"),
            ));
        }
        out.push(Choice {
            value,
            description: choice.description,
        });
    }
    Ok(ArgumentValue::Choice(out))
}

fn validate_name(name: &str, path: &Path) -> Result<(), SkillError> {
    if name.is_empty() {
        return Err(invalid(path, "name", "cannot be empty"));
    }
    if name.len() > MAX_NAME_LEN {
        return Err(invalid(
            path,
            "name",
            format!("exceeds {MAX_NAME_LEN} characters"),
        ));
    }
    if !name
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(invalid(
            path,
            "name",
            "may only use lowercase letters, digits and hyphens",
        ));
    }
    if name.starts_with('-') || name.ends_with('-') || name.contains("--") {
        return Err(invalid(path, "name", "has a stray hyphen"));
    }
    Ok(())
}

fn validate_argument_name(name: &str, path: &Path) -> Result<(), SkillError> {
    if name.is_empty() {
        return Err(invalid(path, "arguments", "an argument has no name"));
    }
    if name.len() > MAX_ARGUMENT_NAME_LEN {
        return Err(invalid(
            path,
            "arguments",
            format!("`{name}` exceeds {MAX_ARGUMENT_NAME_LEN} characters"),
        ));
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap_or_default();
    if !first.is_ascii_lowercase() {
        return Err(invalid(
            path,
            "arguments",
            format!("`{name}` must start with a lowercase letter"),
        ));
    }
    if !chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_') {
        return Err(invalid(
            path,
            "arguments",
            format!("`{name}` may only use lowercase letters, digits and underscores"),
        ));
    }
    Ok(())
}

fn invalid(path: &Path, field: &'static str, reason: impl Into<String>) -> SkillError {
    SkillError::Validation {
        path: path.to_path_buf(),
        field,
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::{ArgumentValue, parse};
    use std::path::Path;

    fn at(text: &str, dir: &str) -> Result<super::Parsed, crate::SkillError> {
        parse(text, Path::new("/skills/x/SKILL.md"), dir)
    }

    #[test]
    fn front_matter_and_body_are_separated() {
        let parsed = at(
            "---\nname: greet\ndescription: says hello\n---\n\nBody line.\n",
            "greet",
        )
        .expect("a canonical skill parses");
        assert_eq!(parsed.name, "greet");
        assert_eq!(parsed.description, "says hello");
        assert_eq!(parsed.body, "Body line.");
    }

    #[test]
    fn a_name_may_be_omitted_and_falls_back_to_the_directory() {
        let parsed = at("---\ndescription: d\n---\nbody", "review-pr").expect("a skill parses");
        assert_eq!(parsed.name, "review-pr");
    }

    #[test]
    fn a_name_that_disagrees_with_its_directory_is_rejected() {
        let err = at("---\nname: greet\ndescription: d\n---\nbody", "greet-dir")
            .expect_err("the pair must agree");
        assert!(
            matches!(err, crate::SkillError::NameMismatch { .. }),
            "{err}"
        );
    }

    #[test]
    fn names_are_lowercase_kebab() {
        for name in ["Greet", "greet_pr", "-greet", "greet-", "gr--eet"] {
            assert!(
                at("---\ndescription: d\n---\nbody", name).is_err(),
                "`{name}` must be rejected"
            );
        }
        assert!(at("---\ndescription: d\n---\nbody", "greet-pr-2").is_ok());
    }

    #[test]
    fn a_missing_description_or_front_matter_is_an_error() {
        assert!(at("---\nname: greet\n---\nbody", "greet").is_err());
        assert!(at("---\ndescription: '  '\n---\nbody", "greet").is_err());
        assert!(at("no front matter here", "greet").is_err());
    }

    #[test]
    fn arguments_carry_their_value_shape() {
        let parsed = at(
            "---\nname: deploy\ndescription: d\narguments:\n  - name: env\n    description: target\n    required: true\n    value:\n      choice:\n        - value: prod\n          description: production\n        - value: staging\n  - name: note\n    description: why\n    value: text_tail\n---\nbody",
            "deploy",
        )
        .expect("an argument list parses");
        assert_eq!(parsed.arguments.len(), 2);
        assert!(parsed.arguments[0].required);
        let ArgumentValue::Choice(options) = &parsed.arguments[0].value else {
            panic!("env is a choice")
        };
        assert_eq!(options.len(), 2);
        assert_eq!(options[1].description, None);
        assert_eq!(parsed.arguments[1].value, ArgumentValue::TextTail);
    }

    #[test]
    fn argument_ordering_rules_are_enforced() {
        let text = "---\ndescription: d\narguments:\n  - name: a\n    description: a\n    value: word\n  - name: b\n    description: b\n    required: true\n    value: word\n---\nbody";
        assert!(at(text, "x").is_err(), "required cannot follow optional");

        let text = "---\ndescription: d\narguments:\n  - name: a\n    description: a\n    value: text_tail\n  - name: b\n    description: b\n    value: word\n---\nbody";
        assert!(at(text, "x").is_err(), "text must come last");

        let text = "---\ndescription: d\narguments:\n  - name: a\n    description: a\n    value: word\n  - name: a\n    description: a\n    value: word\n---\nbody";
        assert!(at(text, "x").is_err(), "names must be unique");
    }

    #[test]
    fn invalid_argument_names_are_rejected() {
        for name in ["Env", "1env", "en v", "env-name"] {
            let text = format!(
                "---\ndescription: d\narguments:\n  - name: {name}\n    description: a\n    value: word\n---\nbody"
            );
            assert!(at(&text, "x").is_err(), "`{name}` must be rejected");
        }
    }

    #[test]
    fn unknown_front_matter_keys_are_ignored() {
        let parsed = at(
            "---\ndescription: d\nlicense: MIT\nallowed-tools: [Bash]\n---\nbody",
            "x",
        )
        .expect("real skills carry keys goat does not read");
        assert_eq!(parsed.description, "d");
    }
}
