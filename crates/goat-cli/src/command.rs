use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Name {
    Model,
    Effort,
    Compact,
    Clear,
    Rewind,
    Resume,
    Status,
    Usage,
    Help,
    Exit,
}

pub(crate) struct Spec {
    pub(crate) name: Name,
    pub(crate) word: &'static str,
    pub(crate) about: &'static str,
}

pub(crate) const SPECS: &[Spec] = &[
    Spec {
        name: Name::Model,
        word: "model",
        about: "Switch the model",
    },
    Spec {
        name: Name::Effort,
        word: "effort",
        about: "Set the reasoning effort",
    },
    Spec {
        name: Name::Compact,
        word: "compact",
        about: "Summarize the conversation to free up context",
    },
    Spec {
        name: Name::Clear,
        word: "clear",
        about: "Start a new session and clear the screen",
    },
    Spec {
        name: Name::Rewind,
        word: "rewind",
        about: "Go back to an earlier prompt",
    },
    Spec {
        name: Name::Resume,
        word: "resume",
        about: "Continue an earlier session in this directory",
    },
    Spec {
        name: Name::Status,
        word: "status",
        about: "Show the session, model and instructions",
    },
    Spec {
        name: Name::Usage,
        word: "usage",
        about: "Show how full the context and the plan are",
    },
    Spec {
        name: Name::Help,
        word: "help",
        about: "Show commands and keys",
    },
    Spec {
        name: Name::Exit,
        word: "exit",
        about: "Quit",
    },
];

const ALIASES: &[(&str, Name)] = &[("quit", Name::Exit)];

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Invocation<'a> {
    pub(crate) name: Name,
    pub(crate) argument: &'a str,
    pub(crate) span: Range<usize>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Typing<'a> {
    Name(&'a str),
    Argument(Name, &'a str),
    Message,
}

pub(crate) fn parse(text: &str) -> Option<Invocation<'_>> {
    let start = text.len() - text.trim_start().len();
    let rest = text.trim().strip_prefix('/')?;
    let (word, argument) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    Some(Invocation {
        name: find(word)?,
        argument: argument.trim(),
        span: start..start + 1 + word.len(),
    })
}

pub(crate) fn typing(text: &str) -> Typing<'_> {
    let Some(rest) = text.trim_start().strip_prefix('/') else {
        return Typing::Message;
    };
    let Some((word, argument)) = rest.split_once(char::is_whitespace) else {
        return Typing::Name(rest);
    };
    match find(word) {
        Some(name) => Typing::Argument(name, argument.trim_start()),
        None => Typing::Message,
    }
}

pub(crate) fn words() -> impl Iterator<Item = &'static str> {
    SPECS
        .iter()
        .map(|spec| spec.word)
        .chain(ALIASES.iter().map(|(word, _)| *word))
}

fn find(word: &str) -> Option<Name> {
    SPECS
        .iter()
        .map(|spec| (spec.word, spec.name))
        .chain(ALIASES.iter().copied())
        .find(|(known, _)| *known == word)
        .map(|(_, name)| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_with_an_argument_is_parsed() {
        assert_eq!(
            parse("  /model gpt-6.2-sol "),
            Some(Invocation {
                name: Name::Model,
                argument: "gpt-6.2-sol",
                span: 2..8,
            })
        );
    }

    #[test]
    fn compact_takes_what_to_focus_on() {
        let run = parse("/compact keep the test plan").unwrap();
        assert_eq!(run.name, Name::Compact);
        assert_eq!(run.argument, "keep the test plan");
    }

    #[test]
    fn an_alias_runs_its_command() {
        assert_eq!(parse("/quit").map(|run| run.name), Some(Name::Exit));
    }

    #[test]
    fn unknown_commands_and_commands_inside_a_message_are_messages() {
        assert_eq!(parse("/modle gpt"), None);
        assert_eq!(parse("I ran /model earlier"), None);
    }

    #[test]
    fn typing_tells_a_name_from_an_argument() {
        assert_eq!(typing("/mo"), Typing::Name("mo"));
        assert_eq!(typing("/model gp"), Typing::Argument(Name::Model, "gp"));
        assert_eq!(typing("/modle gp"), Typing::Message);
        assert_eq!(typing("hello"), Typing::Message);
    }
}
