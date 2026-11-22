use std::sync::LazyLock;

use crossterm::style::{Color, Stylize};
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxSet};

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static PAINTS: LazyLock<Vec<(Scope, Paint)>> = LazyLock::new(|| {
    PALETTE
        .iter()
        .filter_map(|(name, paint)| Some((Scope::new(name).ok()?, *paint)))
        .collect()
});

const ALIASES: [(&str, &str); 4] = [
    ("ts", "js"),
    ("typescript", "js"),
    ("tsx", "js"),
    ("jsx", "js"),
];
const PALETTE: [(&str, Paint); 15] = [
    ("comment", Paint::Dim),
    ("string", Paint::Color(Color::Green)),
    ("constant.numeric", Paint::Color(Color::Yellow)),
    ("constant.character", Paint::Color(Color::Yellow)),
    ("constant.language", Paint::Color(Color::Yellow)),
    ("keyword", Paint::Color(Color::Magenta)),
    ("storage", Paint::Color(Color::Magenta)),
    ("entity.name.function", Paint::Color(Color::Blue)),
    ("support.function", Paint::Color(Color::Blue)),
    ("variable.function", Paint::Color(Color::Blue)),
    ("entity.name.type", Paint::Color(Color::Cyan)),
    ("entity.name.class", Paint::Color(Color::Cyan)),
    ("entity.name.struct", Paint::Color(Color::Cyan)),
    ("support.type", Paint::Color(Color::Cyan)),
    ("support.class", Paint::Color(Color::Cyan)),
];

#[derive(Clone, Copy)]
enum Paint {
    Dim,
    Color(Color),
}

pub(crate) struct Block {
    state: Option<(ParseState, ScopeStack)>,
}

impl Block {
    pub(crate) fn new(language: &str) -> Self {
        let language = language.to_lowercase();
        let token = ALIASES
            .iter()
            .find(|(alias, _)| *alias == language)
            .map_or(language.as_str(), |(_, token)| token);
        let state = (!token.is_empty())
            .then(|| SYNTAXES.find_syntax_by_token(token))
            .flatten()
            .map(|syntax| (ParseState::new(syntax), ScopeStack::new()));
        Self { state }
    }

    pub(crate) fn paint(&mut self, line: &str) -> String {
        let Some((state, stack)) = &mut self.state else {
            return line.to_owned();
        };
        let fed = format!("{line}\n");
        let Ok(ops) = state.parse_line(&fed, &SYNTAXES) else {
            self.state = None;
            return line.to_owned();
        };
        let mut out = String::with_capacity(line.len() * 2);
        let mut at = 0;
        for (end, op) in ops {
            let end = end.min(line.len());
            if end > at {
                push(&mut out, &line[at..end], stack);
                at = end;
            }
            if stack.apply(&op).is_err() {
                self.state = None;
                out.push_str(&line[at..]);
                return out;
            }
        }
        if at < line.len() {
            push(&mut out, &line[at..], stack);
        }
        out
    }
}

fn push(out: &mut String, text: &str, stack: &ScopeStack) {
    let paint = stack.as_slice().iter().rev().find_map(|scope| {
        PAINTS
            .iter()
            .find(|(prefix, _)| prefix.is_prefix_of(*scope))
            .map(|(_, paint)| *paint)
    });
    match paint {
        Some(Paint::Dim) => out.push_str(&text.dim().to_string()),
        Some(Paint::Color(color)) => out.push_str(&text.with(color).to_string()),
        None => out.push_str(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_gets_keyword_number_and_comment_colors() {
        let painted = Block::new("rust").paint("let x = 42; // why");

        assert!(painted.contains(&"let".with(Color::Magenta).to_string()));
        assert!(painted.contains(&"42".with(Color::Yellow).to_string()));
        assert!(
            painted.ends_with(&format!("{}{}", "//".dim(), " why".dim())),
            "{painted:?}"
        );
    }

    #[test]
    fn a_string_keeps_its_color_across_lines() {
        let mut block = Block::new("python");
        block.paint("text = \"\"\"start");

        let painted = block.paint("still inside");

        assert_eq!(painted, "still inside".with(Color::Green).to_string());
    }

    #[test]
    fn typescript_borrows_javascript() {
        let painted = Block::new("ts").paint("const a = 1");

        assert!(painted.contains(&"const".with(Color::Magenta).to_string()));
    }

    #[test]
    fn an_unknown_language_stays_plain() {
        assert_eq!(Block::new("nothing").paint("a + b"), "a + b");
        assert_eq!(Block::new("").paint("a + b"), "a + b");
    }
}
