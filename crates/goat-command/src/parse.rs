use std::collections::BTreeSet;

use crate::{
    BranchSpec, CommandInvocation, CommandLine, CommandParseError, CommandShape, CommandSpec,
    ParameterSpec, ParameterValue, ParsedParameter, ParsedValue,
};

impl CommandSpec {
    pub fn usage(&self) -> String {
        match &self.shape {
            CommandShape::Empty => format!("usage: /{}", self.name),
            CommandShape::Parameters(parameters) => {
                format!("usage: /{}{}", self.name, parameter_usage(parameters))
            }
            CommandShape::Branches(branches) => {
                let names = branches
                    .iter()
                    .map(|branch| branch.name.as_str())
                    .collect::<Vec<_>>()
                    .join("|");
                format!("usage: /{} <{}>", self.name, names)
            }
        }
    }

    pub fn branch_usage(&self, branch: &BranchSpec) -> String {
        format!(
            "usage: /{} {}{}",
            self.name,
            branch.name,
            parameter_usage(&branch.parameters)
        )
    }

    pub fn validate(&self) -> Result<(), CommandParseError> {
        validate_name(&self.name, "command")?;
        validate_unique(self.aliases.iter().map(String::as_str), "alias")?;
        for alias in &self.aliases {
            validate_name(alias, "alias")?;
        }
        match &self.shape {
            CommandShape::Empty => Ok(()),
            CommandShape::Parameters(parameters) => validate_parameters(parameters),
            CommandShape::Branches(branches) => {
                if branches.is_empty() {
                    return Err(CommandParseError::InvalidShape(
                        "branches cannot be empty".to_owned(),
                    ));
                }
                validate_unique(branches.iter().map(|branch| branch.name.as_str()), "branch")?;
                for branch in branches {
                    validate_name(&branch.name, "branch")?;
                    if branch.description.trim().is_empty() {
                        return Err(CommandParseError::InvalidShape(format!(
                            "branch {} description cannot be empty",
                            branch.name
                        )));
                    }
                    validate_parameters(&branch.parameters)?;
                }
                Ok(())
            }
        }
    }

    pub fn parse(&self, raw: &str, args: &str) -> Result<CommandInvocation, CommandParseError> {
        self.validate()?;
        match &self.shape {
            CommandShape::Empty => {
                let trimmed = args.trim();
                if trimmed.is_empty() {
                    Ok(invocation(&self.name, None, raw, args, Vec::new()))
                } else {
                    Err(CommandParseError::UnexpectedArguments {
                        usage: self.usage(),
                        value: trimmed.to_owned(),
                    })
                }
            }
            CommandShape::Parameters(parameters) => {
                let parsed = parse_parameters(parameters, args, &self.usage())?;
                Ok(invocation(&self.name, None, raw, args, parsed))
            }
            CommandShape::Branches(branches) => {
                let tokens = tokens(args);
                let Some(first) = tokens.first() else {
                    return Err(CommandParseError::MissingBranch {
                        usage: self.usage(),
                    });
                };
                let branch_name = &args[first.start..first.end];
                let Some(branch) = branches.iter().find(|branch| branch.name == branch_name) else {
                    return Err(CommandParseError::UnknownBranch {
                        usage: self.usage(),
                        name: branch_name.to_owned(),
                    });
                };
                let rest = args[first.end..].trim_start();
                let usage = self.branch_usage(branch);
                let parsed = parse_parameters(&branch.parameters, rest, &usage)?;
                Ok(invocation(
                    &self.name,
                    Some(branch.name.clone()),
                    raw,
                    args,
                    parsed,
                ))
            }
        }
    }
}

pub fn parse_line(raw: &str) -> Result<CommandLine, CommandParseError> {
    let trimmed = raw.trim();
    let Some(body) = trimmed.strip_prefix('/') else {
        return Err(CommandParseError::Empty);
    };
    let body = body.trim_start();
    if body.is_empty() {
        return Err(CommandParseError::Empty);
    }
    let name_end = body.find(char::is_whitespace).unwrap_or(body.len());
    let name = body[..name_end].to_owned();
    let args = body[name_end..].trim_start().to_owned();
    Ok(CommandLine { name, args })
}

fn invocation(
    name: &str,
    subcommand: Option<String>,
    raw: &str,
    raw_args: &str,
    parameters: Vec<ParsedParameter>,
) -> CommandInvocation {
    CommandInvocation {
        name: name.to_owned(),
        subcommand,
        raw: raw.to_owned(),
        raw_args: raw_args.to_owned(),
        parameters,
    }
}

fn parse_parameters(
    parameters: &[ParameterSpec],
    args: &str,
    usage: &str,
) -> Result<Vec<ParsedParameter>, CommandParseError> {
    let token_list = tokens(args);
    let mut index = 0usize;
    let mut parsed = Vec::new();
    for parameter in parameters {
        match &parameter.value {
            ParameterValue::TextTail => {
                let value = if let Some(token) = token_list.get(index) {
                    args[token.start..].trim().to_owned()
                } else {
                    String::new()
                };
                if value.is_empty() {
                    if parameter.required {
                        return Err(CommandParseError::MissingParameter {
                            usage: usage.to_owned(),
                            name: parameter.name.clone(),
                        });
                    }
                } else {
                    parsed.push(ParsedParameter {
                        name: parameter.name.clone(),
                        value: ParsedValue::Text(value),
                    });
                }
                index = token_list.len();
                break;
            }
            ParameterValue::Word => {
                if let Some(token) = next_token(&token_list, index) {
                    let value = &args[token.start..token.end];
                    parsed.push(ParsedParameter {
                        name: parameter.name.clone(),
                        value: ParsedValue::Word(value.to_owned()),
                    });
                    index += 1;
                } else if parameter.required {
                    return Err(missing(usage, parameter));
                }
            }
            ParameterValue::Integer => {
                if let Some(token) = next_token(&token_list, index) {
                    let value = &args[token.start..token.end];
                    let Ok(integer) = value.parse::<i64>() else {
                        return Err(CommandParseError::InvalidInteger {
                            usage: usage.to_owned(),
                            name: parameter.name.clone(),
                            value: value.to_owned(),
                        });
                    };
                    parsed.push(ParsedParameter {
                        name: parameter.name.clone(),
                        value: ParsedValue::Integer(integer),
                    });
                    index += 1;
                } else if parameter.required {
                    return Err(missing(usage, parameter));
                }
            }
            ParameterValue::Choice(choices) => {
                if let Some(token) = next_token(&token_list, index) {
                    let value = &args[token.start..token.end];
                    if choices.iter().any(|choice| choice.value == value) {
                        parsed.push(ParsedParameter {
                            name: parameter.name.clone(),
                            value: ParsedValue::Choice(value.to_owned()),
                        });
                        index += 1;
                    } else {
                        return Err(CommandParseError::InvalidChoice {
                            usage: usage.to_owned(),
                            name: parameter.name.clone(),
                            value: value.to_owned(),
                        });
                    }
                } else if parameter.required {
                    return Err(missing(usage, parameter));
                }
            }
        }
    }
    if let Some(token) = token_list.get(index) {
        return Err(CommandParseError::UnexpectedArguments {
            usage: usage.to_owned(),
            value: args[token.start..].trim().to_owned(),
        });
    }
    Ok(parsed)
}

fn next_token(token_list: &[Token], index: usize) -> Option<&Token> {
    token_list.get(index)
}

fn missing(usage: &str, parameter: &ParameterSpec) -> CommandParseError {
    CommandParseError::MissingParameter {
        usage: usage.to_owned(),
        name: parameter.name.clone(),
    }
}

#[derive(Clone, Copy)]
struct Token {
    start: usize,
    end: usize,
}

fn tokens(input: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut start = None;
    for (index, ch) in input.char_indices() {
        if ch.is_whitespace() {
            if let Some(token_start) = start.take() {
                tokens.push(Token {
                    start: token_start,
                    end: index,
                });
            }
        } else if start.is_none() {
            start = Some(index);
        }
    }
    if let Some(token_start) = start {
        tokens.push(Token {
            start: token_start,
            end: input.len(),
        });
    }
    tokens
}

fn parameter_usage(parameters: &[ParameterSpec]) -> String {
    let mut usage = String::new();
    for parameter in parameters {
        usage.push(' ');
        usage.push_str(&parameter_usage_part(parameter));
    }
    usage
}

fn parameter_usage_part(parameter: &ParameterSpec) -> String {
    let body = match &parameter.value {
        ParameterValue::Word | ParameterValue::Integer => parameter.name.clone(),
        ParameterValue::Choice(choices) => choices
            .iter()
            .map(|choice| choice.value.as_str())
            .collect::<Vec<_>>()
            .join("|"),
        ParameterValue::TextTail => format!("{}...", parameter.name),
    };
    if parameter.required {
        format!("<{body}>")
    } else {
        format!("[{body}]")
    }
}

fn validate_parameters(parameters: &[ParameterSpec]) -> Result<(), CommandParseError> {
    validate_unique(
        parameters.iter().map(|parameter| parameter.name.as_str()),
        "parameter",
    )?;
    let mut optional_seen = false;
    let mut text_tail_seen = false;
    for (index, parameter) in parameters.iter().enumerate() {
        validate_name(&parameter.name, "parameter")?;
        if parameter.description.trim().is_empty() {
            return Err(CommandParseError::InvalidShape(format!(
                "parameter {} description cannot be empty",
                parameter.name
            )));
        }
        if !parameter.required {
            optional_seen = true;
        } else if optional_seen {
            return Err(CommandParseError::InvalidShape(
                "required parameter cannot follow optional parameter".to_owned(),
            ));
        }
        if matches!(parameter.value, ParameterValue::TextTail) {
            if text_tail_seen {
                return Err(CommandParseError::InvalidShape(
                    "text tail cannot appear more than once".to_owned(),
                ));
            }
            text_tail_seen = true;
            if index + 1 != parameters.len() {
                return Err(CommandParseError::InvalidShape(
                    "text tail must be last".to_owned(),
                ));
            }
        }
        if let ParameterValue::Choice(choices) = &parameter.value {
            if choices.is_empty() {
                return Err(CommandParseError::InvalidShape(format!(
                    "choice parameter {} cannot be empty",
                    parameter.name
                )));
            }
            validate_unique(choices.iter().map(|choice| choice.value.as_str()), "choice")?;
            for choice in choices {
                if choice.value.trim().is_empty() {
                    return Err(CommandParseError::InvalidShape(
                        "choice value cannot be empty".to_owned(),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn validate_name(name: &str, kind: &str) -> Result<(), CommandParseError> {
    if name.is_empty() {
        return Err(CommandParseError::InvalidShape(format!(
            "{kind} name cannot be empty"
        )));
    }
    if !name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(CommandParseError::InvalidShape(format!(
            "{kind} name is not slash-safe: {name}"
        )));
    }
    Ok(())
}

fn validate_unique<'a>(
    values: impl Iterator<Item = &'a str>,
    kind: &str,
) -> Result<(), CommandParseError> {
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            return Err(CommandParseError::InvalidShape(format!(
                "duplicate {kind}: {value}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{
        BranchSpec, ChoiceSpec, CommandParseError, CommandShape, CommandSpec, ParameterSpec,
        ParameterValue, ParsedValue,
    };

    fn spec(parameters: Vec<ParameterSpec>) -> CommandSpec {
        CommandSpec {
            name: "test".to_owned(),
            description: "test".to_owned(),
            aliases: Vec::new(),
            shape: CommandShape::Parameters(parameters),
        }
    }

    fn parameter(name: &str, required: bool, value: ParameterValue) -> ParameterSpec {
        ParameterSpec {
            name: name.to_owned(),
            description: name.to_owned(),
            required,
            value,
        }
    }

    #[test]
    fn parses_text_tail() {
        let spec = spec(vec![parameter("title", true, ParameterValue::TextTail)]);
        let parsed = spec.parse("/test hello world", "hello world").unwrap();
        assert_eq!(parsed.text("title"), Some("hello world"));
    }

    #[test]
    fn rejects_required_after_optional() {
        let spec = spec(vec![
            parameter("a", false, ParameterValue::Word),
            parameter("b", true, ParameterValue::Word),
        ]);
        assert!(spec.validate().is_err());
    }

    #[test]
    fn rejects_text_tail_before_last() {
        let spec = spec(vec![
            parameter("a", true, ParameterValue::TextTail),
            parameter("b", true, ParameterValue::Word),
        ]);
        assert!(spec.validate().is_err());
    }

    #[test]
    fn parses_integer() {
        let spec = spec(vec![parameter("n", true, ParameterValue::Integer)]);
        let parsed = spec.parse("/test 42", "42").unwrap();
        assert_eq!(parsed.integer("n"), Some(42));
    }

    #[test]
    fn rejects_invalid_integer() {
        let spec = spec(vec![parameter("n", true, ParameterValue::Integer)]);
        assert!(matches!(
            spec.parse("/test no", "no"),
            Err(CommandParseError::InvalidInteger { .. })
        ));
    }

    #[test]
    fn parses_choice() {
        let spec = spec(vec![parameter(
            "level",
            true,
            ParameterValue::Choice(vec![ChoiceSpec {
                value: "high".to_owned(),
                description: None,
            }]),
        )]);
        let parsed = spec.parse("/test high", "high").unwrap();
        assert_eq!(parsed.choice("level"), Some("high"));
    }

    #[test]
    fn rejects_missing_required() {
        let spec = spec(vec![parameter("name", true, ParameterValue::Word)]);
        assert!(matches!(
            spec.parse("/test", ""),
            Err(CommandParseError::MissingParameter { .. })
        ));
    }

    #[test]
    fn parses_branch() {
        let spec = CommandSpec {
            name: "review".to_owned(),
            description: "review".to_owned(),
            aliases: Vec::new(),
            shape: CommandShape::Branches(vec![BranchSpec {
                name: "security".to_owned(),
                description: "security".to_owned(),
                parameters: vec![parameter("target", true, ParameterValue::Word)],
            }]),
        };
        let parsed = spec.parse("/review security src", "security src").unwrap();
        assert_eq!(parsed.subcommand.as_deref(), Some("security"));
        assert_eq!(parsed.text("target"), Some("src"));
    }

    #[test]
    fn renders_usage() {
        let spec = spec(vec![parameter("title", true, ParameterValue::TextTail)]);
        assert_eq!(spec.usage(), "usage: /test <title...>");
    }

    #[test]
    fn text_accessor_reads_words_and_text() {
        let value = ParsedValue::Word("demo".to_owned());
        assert_eq!(value, ParsedValue::Word("demo".to_owned()));
    }
}
