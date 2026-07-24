pub struct CommandSpec {
    pub name: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub shape: CommandShape,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandShape {
    Empty,
    Parameters(Vec<ParameterSpec>),
    Branches(Vec<BranchSpec>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchSpec {
    pub name: String,
    pub description: String,
    pub parameters: Vec<ParameterSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParameterSpec {
    pub name: String,
    pub description: String,
    pub required: bool,
    pub value: ParameterValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParameterValue {
    Word,
    Integer,
    Choice(Vec<ChoiceSpec>),
    TextTail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceSpec {
    pub value: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandLine {
    pub name: String,
    pub args: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandInvocation {
    pub name: String,
    pub subcommand: Option<String>,
    pub raw: String,
    pub raw_args: String,
    pub parameters: Vec<ParsedParameter>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedParameter {
    pub name: String,
    pub value: ParsedValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedValue {
    Word(String),
    Integer(i64),
    Choice(String),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandParseError {
    Empty,
    MissingParameter {
        usage: String,
        name: String,
    },
    MissingBranch {
        usage: String,
    },
    UnknownBranch {
        usage: String,
        name: String,
    },
    InvalidInteger {
        usage: String,
        name: String,
        value: String,
    },
    InvalidChoice {
        usage: String,
        name: String,
        value: String,
    },
    UnexpectedArguments {
        usage: String,
        value: String,
    },
    InvalidShape(String),
}

impl CommandInvocation {
    pub fn text(&self, name: &str) -> Option<&str> {
        self.parameters.iter().find_map(|parameter| {
            (parameter.name == name).then_some(match &parameter.value {
                ParsedValue::Word(value)
                | ParsedValue::Choice(value)
                | ParsedValue::Text(value) => value.as_str(),
                ParsedValue::Integer(_) => return None,
            })
        })
    }

    pub fn integer(&self, name: &str) -> Option<i64> {
        self.parameters.iter().find_map(|parameter| {
            match (parameter.name.as_str() == name, &parameter.value) {
                (true, ParsedValue::Integer(value)) => Some(*value),
                _ => None,
            }
        })
    }

    pub fn choice(&self, name: &str) -> Option<&str> {
        self.parameters.iter().find_map(|parameter| {
            match (parameter.name.as_str() == name, &parameter.value) {
                (true, ParsedValue::Choice(value)) => Some(value.as_str()),
                _ => None,
            }
        })
    }
}

impl CommandParseError {
    pub fn message(&self) -> String {
        match self {
            Self::Empty => "empty command".to_owned(),
            Self::MissingParameter { usage, name } => format!("missing parameter {name}; {usage}"),
            Self::MissingBranch { usage } => format!("missing subcommand; {usage}"),
            Self::UnknownBranch { usage, name } => format!("unknown subcommand: {name}; {usage}"),
            Self::InvalidInteger { usage, name, value } => {
                format!("invalid integer for {name}: {value}; {usage}")
            }
            Self::InvalidChoice { usage, name, value } => {
                format!("invalid choice for {name}: {value}; {usage}")
            }
            Self::UnexpectedArguments { usage, value } => {
                format!("unexpected arguments: {value}; {usage}")
            }
            Self::InvalidShape(reason) => format!("invalid command shape: {reason}"),
        }
    }
}
