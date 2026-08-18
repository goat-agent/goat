use std::collections::BTreeMap;

use crate::SkillError;
use crate::manifest::{Argument, ArgumentValue};

#[derive(Clone, Debug)]
pub enum Call {
    Raw(String),
    Named(BTreeMap<String, String>),
}

#[derive(Clone, Debug, Default)]
pub struct Resolved {
    raw: String,
    positional: Vec<String>,
    named: Vec<(String, String)>,
}

impl Resolved {
    #[must_use]
    pub fn apply(&self, body: &str) -> String {
        let body = replace_indexed(body, &self.positional);
        let body = replace_named(&body, &self.named);
        let body = replace_shorthand(&body, &self.positional);
        body.replace("$ARGUMENTS", &self.raw)
    }
}

pub fn resolve(declared: &[Argument], call: Option<&Call>) -> Result<Option<Resolved>, SkillError> {
    match call {
        None if declared.is_empty() => Ok(None),
        None => bind(declared, &[], String::new()).map(Some),
        Some(Call::Raw(raw)) => {
            let words = split_words(raw);
            if declared.is_empty() {
                return Ok(Some(Resolved {
                    raw: raw.clone(),
                    positional: words,
                    named: Vec::new(),
                }));
            }
            bind(declared, &words, raw.clone()).map(Some)
        }
        Some(Call::Named(map)) => {
            if let Some(unknown) = map
                .keys()
                .find(|key| !declared.iter().any(|a| &a.name == *key))
            {
                return Err(SkillError::InvalidArguments(if declared.is_empty() {
                    format!("`{unknown}` was passed but this skill takes no arguments")
                } else {
                    format!("unknown argument `{unknown}`; it takes {}", names(declared))
                }));
            }
            let values: Vec<String> = declared
                .iter()
                .map(|argument| map.get(&argument.name).cloned().unwrap_or_default())
                .collect();
            let raw = join(&values);
            bind_values(declared, values, raw).map(Some)
        }
    }
}

fn bind(declared: &[Argument], words: &[String], raw: String) -> Result<Resolved, SkillError> {
    let mut values = Vec::with_capacity(declared.len());
    for (index, argument) in declared.iter().enumerate() {
        let value = if argument.value == ArgumentValue::TextTail {
            words.get(index..).unwrap_or_default().join(" ")
        } else {
            words.get(index).cloned().unwrap_or_default()
        };
        values.push(value);
    }
    bind_values(declared, values, raw)
}

fn bind_values(
    declared: &[Argument],
    values: Vec<String>,
    raw: String,
) -> Result<Resolved, SkillError> {
    for (argument, value) in declared.iter().zip(&values) {
        let value = value.trim();
        if value.is_empty() {
            if argument.required {
                return Err(SkillError::InvalidArguments(format!(
                    "missing required argument `{}`; it takes {}",
                    argument.name,
                    names(declared)
                )));
            }
            continue;
        }
        match &argument.value {
            ArgumentValue::Integer if value.parse::<i64>().is_err() => {
                return Err(SkillError::InvalidArguments(format!(
                    "`{}` takes a whole number, not `{value}`",
                    argument.name
                )));
            }
            ArgumentValue::Choice(options)
                if !options.iter().any(|option| option.value == value) =>
            {
                return Err(SkillError::InvalidArguments(format!(
                    "`{}` takes one of {}, not `{value}`",
                    argument.name,
                    options
                        .iter()
                        .map(|option| option.value.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
            _ => {}
        }
    }
    let named = declared
        .iter()
        .zip(&values)
        .map(|(argument, value)| (argument.name.clone(), value.clone()))
        .collect();
    Ok(Resolved {
        raw,
        positional: values,
        named,
    })
}

fn names(declared: &[Argument]) -> String {
    declared
        .iter()
        .map(|argument| argument.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn join(values: &[String]) -> String {
    values
        .iter()
        .filter(|value| !value.is_empty())
        .map(|value| {
            if value.chars().any(char::is_whitespace) {
                format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
            } else {
                value.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn split_words(raw: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;

    for ch in raw.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        match quote {
            Some(open) => {
                if ch == open {
                    quote = None;
                } else {
                    current.push(ch);
                }
            }
            None => {
                if ch == '\'' || ch == '"' {
                    quote = Some(ch);
                } else if ch.is_whitespace() {
                    if !current.is_empty() {
                        words.push(std::mem::take(&mut current));
                    }
                } else {
                    current.push(ch);
                }
            }
        }
    }
    if escaped {
        current.push('\\');
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn replace_indexed(body: &str, positional: &[String]) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some(start) = rest.find("$ARGUMENTS[") {
        out.push_str(&rest[..start]);
        let after = &rest[start + "$ARGUMENTS[".len()..];
        let Some(end) = after.find(']') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let index = &after[..end];
        if !index.is_empty() && index.chars().all(|ch| ch.is_ascii_digit()) {
            let value = index
                .parse::<usize>()
                .ok()
                .and_then(|index| positional.get(index))
                .map_or("", String::as_str);
            out.push_str(value);
            rest = &after[end + 1..];
        } else {
            out.push_str("$ARGUMENTS[");
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

fn replace_named(body: &str, named: &[(String, String)]) -> String {
    if named.is_empty() {
        return body.to_owned();
    }
    let chars: Vec<char> = body.chars().collect();
    let mut out = String::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '$' && chars.get(index + 1).is_some_and(char::is_ascii_lowercase) {
            let mut end = index + 1;
            while chars.get(end).is_some_and(|ch| is_word_char(*ch)) {
                end += 1;
            }
            let word: String = chars[index + 1..end].iter().collect();
            if let Some((_, value)) = named.iter().find(|(name, _)| *name == word) {
                out.push_str(value);
                index = end;
                continue;
            }
        }
        out.push(chars[index]);
        index += 1;
    }
    out
}

fn replace_shorthand(body: &str, positional: &[String]) -> String {
    let chars: Vec<char> = body.chars().collect();
    let mut out = String::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '$' && chars.get(index + 1).is_some_and(char::is_ascii_digit) {
            let mut end = index + 1;
            while chars.get(end).is_some_and(char::is_ascii_digit) {
                end += 1;
            }
            if chars.get(end).is_none_or(|ch| !is_word_char(*ch)) {
                let digits: String = chars[index + 1..end].iter().collect();
                let value = digits
                    .parse::<usize>()
                    .ok()
                    .and_then(|digits| positional.get(digits))
                    .map_or("", String::as_str);
                out.push_str(value);
                index = end;
                continue;
            }
        }
        out.push(chars[index]);
        index += 1;
    }
    out
}

fn is_word_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

#[cfg(test)]
mod tests {
    use super::{Call, resolve};
    use crate::manifest::{Argument, ArgumentValue, Choice};
    use std::collections::BTreeMap;

    fn argument(name: &str, required: bool, value: ArgumentValue) -> Argument {
        Argument {
            name: name.to_owned(),
            description: format!("the {name}"),
            required,
            value,
        }
    }

    #[test]
    fn a_raw_line_and_a_named_map_resolve_alike() {
        let declared = vec![
            argument("task", true, ArgumentValue::Word),
            argument("when", false, ArgumentValue::Word),
        ];
        let body = "task=$task when=$when first=$0 raw=$ARGUMENTS";

        let raw = resolve(&declared, Some(&Call::Raw("deploy tomorrow".to_owned())))
            .expect("a raw line resolves")
            .expect("it binds");
        let named = resolve(
            &declared,
            Some(&Call::Named(BTreeMap::from([
                ("task".to_owned(), "deploy".to_owned()),
                ("when".to_owned(), "tomorrow".to_owned()),
            ]))),
        )
        .expect("a named map resolves")
        .expect("it binds");

        assert_eq!(
            raw.apply(body),
            "task=deploy when=tomorrow first=deploy raw=deploy tomorrow"
        );
        assert_eq!(raw.apply(body), named.apply(body));
    }

    #[test]
    fn every_substitution_form_is_honoured() {
        let declared = vec![
            argument("task", false, ArgumentValue::Word),
            argument("when", false, ArgumentValue::Word),
        ];
        let resolved = resolve(&declared, Some(&Call::Raw("ship now".to_owned())))
            .unwrap()
            .unwrap();
        assert_eq!(
            resolved
                .apply("raw=$ARGUMENTS first=$ARGUMENTS[0] second=$1 missing=$9 kept=$taskless"),
            "raw=ship now first=ship second=now missing= kept=$taskless"
        );
    }

    #[test]
    fn a_skill_declaring_nothing_indexes_the_words_themselves() {
        let resolved = resolve(&[], Some(&Call::Raw("one two".to_owned())))
            .unwrap()
            .unwrap();
        assert_eq!(resolved.apply("$0/$1/$ARGUMENTS"), "one/two/one two");
    }

    #[test]
    fn a_text_argument_takes_everything_left() {
        let declared = vec![
            argument("env", true, ArgumentValue::Word),
            argument("note", false, ArgumentValue::TextTail),
        ];
        let resolved = resolve(
            &declared,
            Some(&Call::Raw("prod rolling back the migration".to_owned())),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            resolved.apply("$env: $note"),
            "prod: rolling back the migration"
        );
    }

    #[test]
    fn a_missing_required_argument_is_refused() {
        let declared = vec![argument("task", true, ArgumentValue::Word)];
        assert!(resolve(&declared, None).is_err());
        assert!(resolve(&declared, Some(&Call::Raw(String::new()))).is_err());
        assert!(resolve(&declared, Some(&Call::Named(BTreeMap::new()))).is_err());
    }

    #[test]
    fn an_unknown_named_argument_is_refused() {
        let declared = vec![argument("task", false, ArgumentValue::Word)];
        let call = Call::Named(BTreeMap::from([("nope".to_owned(), "x".to_owned())]));
        assert!(resolve(&declared, Some(&call)).is_err());
        assert!(resolve(&[], Some(&call)).is_err());
    }

    #[test]
    fn a_declared_value_shape_is_enforced() {
        let count = vec![argument("count", true, ArgumentValue::Integer)];
        assert!(resolve(&count, Some(&Call::Raw("12".to_owned()))).is_ok());
        assert!(resolve(&count, Some(&Call::Raw("soon".to_owned()))).is_err());

        let env = vec![argument(
            "env",
            true,
            ArgumentValue::Choice(vec![Choice {
                value: "prod".to_owned(),
                description: None,
            }]),
        )];
        assert!(resolve(&env, Some(&Call::Raw("prod".to_owned()))).is_ok());
        assert!(resolve(&env, Some(&Call::Raw("staging".to_owned()))).is_err());
    }

    #[test]
    fn an_optional_argument_left_out_substitutes_to_nothing() {
        let declared = vec![argument("when", false, ArgumentValue::Word)];
        let resolved = resolve(&declared, None).unwrap().unwrap();
        assert_eq!(resolved.apply("when=[$when]"), "when=[]");
    }

    #[test]
    fn a_skill_with_no_arguments_and_no_call_substitutes_nothing() {
        assert!(resolve(&[], None).unwrap().is_none());
    }

    #[test]
    fn quotes_and_escapes_survive_a_raw_line() {
        let declared = vec![
            argument("first", true, ArgumentValue::Word),
            argument("second", true, ArgumentValue::Word),
        ];
        let resolved = resolve(
            &declared,
            Some(&Call::Raw(r#""two words" three\ four"#.to_owned())),
        )
        .unwrap()
        .unwrap();
        assert_eq!(resolved.apply("$first|$second"), "two words|three four");
    }
}
