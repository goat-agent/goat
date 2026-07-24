use std::{
    str::FromStr,
    sync::{Mutex, OnceLock},
};

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use syntect::{
    easy::HighlightLines,
    highlighting::{
        FontStyle, ScopeSelectors, StyleModifier, Theme as SyntectTheme, ThemeItem, ThemeSettings,
    },
    parsing::SyntaxSet,
    util::LinesWithEndings,
};

use crate::theme::{CodePalette, Theme};

pub trait Highlighter: Send + Sync {
    fn highlight(&self, lang: &str, code: &str, theme: Theme) -> Vec<Line<'static>>;
}

pub struct PlainHighlighter;

impl Highlighter for PlainHighlighter {
    fn highlight(&self, _lang: &str, code: &str, theme: Theme) -> Vec<Line<'static>> {
        code.lines()
            .map(|line| Line::from(Span::styled(line.to_owned(), theme.code_plain())))
            .collect()
    }
}

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();

fn syntax_set() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

pub struct SyntectHighlighter {
    cached_syntect_theme: Mutex<Option<(u8, SyntectTheme)>>,
}

impl SyntectHighlighter {
    pub fn new() -> Self {
        Self {
            cached_syntect_theme: Mutex::new(None),
        }
    }

    fn get_or_build_syntect_theme(&self, theme: Theme) -> SyntectTheme {
        let id = theme.id();
        let mut guard = self.cached_syntect_theme.lock().unwrap();
        if let Some((cached_id, ref built)) = *guard
            && cached_id == id
        {
            return built.clone();
        }
        let code = theme.code();
        let built = palette_to_syntect_theme(&code, theme.fg_color(), code.bg);
        *guard = Some((id, built.clone()));
        built
    }
}

impl Default for SyntectHighlighter {
    fn default() -> Self {
        Self::new()
    }
}

fn to_syntect(color: Color) -> syntect::highlighting::Color {
    match color {
        Color::Rgb(r, g, b) => syntect::highlighting::Color { r, g, b, a: 0xff },
        _ => syntect::highlighting::Color {
            r: 0xcc,
            g: 0xcc,
            b: 0xcc,
            a: 0xff,
        },
    }
}

fn make_scope_item(selectors: &str, fg: Color) -> ThemeItem {
    ThemeItem {
        scope: ScopeSelectors::from_str(selectors).unwrap_or_default(),
        style: StyleModifier {
            foreground: Some(to_syntect(fg)),
            background: None,
            font_style: None,
        },
    }
}

fn palette_to_syntect_theme(code: &CodePalette, fg: Color, bg: Color) -> SyntectTheme {
    SyntectTheme {
        name: None,
        author: None,
        settings: ThemeSettings {
            foreground: Some(to_syntect(fg)),
            background: Some(to_syntect(bg)),
            ..ThemeSettings::default()
        },
        scopes: vec![
            make_scope_item(
                "keyword, keyword.control, keyword.operator, storage.modifier",
                code.keyword,
            ),
            make_scope_item("string, string.quoted, constant.character", code.string),
            make_scope_item("comment, comment.line, comment.block", code.comment),
            make_scope_item("constant.numeric, constant.other.color", code.number),
            make_scope_item(
                "entity.name.type, support.type, storage.type, support.class, entity.name.class",
                code.type_,
            ),
            make_scope_item(
                "entity.name.function, support.function, variable.function, meta.function-call",
                code.function,
            ),
            make_scope_item(
                "variable, variable.other, variable.parameter, meta.definition.variable",
                code.variable,
            ),
            make_scope_item(
                "keyword.operator.arithmetic, keyword.operator.logical, keyword.operator.assignment, punctuation.separator, punctuation.terminator",
                code.operator,
            ),
            make_scope_item(
                "entity.name.macro, support.macro, meta.macro, variable.annotation, meta.annotation",
                code.macro_,
            ),
            make_scope_item(
                "variable.other.property, variable.other.member, entity.other.attribute-name, support.type.property-name, meta.attribute",
                code.property,
            ),
        ],
    }
}

impl Highlighter for SyntectHighlighter {
    fn highlight(&self, lang: &str, code: &str, theme: Theme) -> Vec<Line<'static>> {
        let ss = syntax_set();
        let syntax = ss
            .find_syntax_by_token(lang)
            .or_else(|| ss.find_syntax_by_extension(lang))
            .unwrap_or_else(|| ss.find_syntax_plain_text());

        let syntect_theme = self.get_or_build_syntect_theme(theme);
        let mut hl = HighlightLines::new(syntax, &syntect_theme);
        let mut result = Vec::new();

        for raw_line in LinesWithEndings::from(code) {
            let ranges = hl.highlight_line(raw_line, ss).unwrap_or_default();

            let spans: Vec<Span<'static>> = ranges
                .into_iter()
                .filter_map(|(style, text)| {
                    let text = text.trim_end_matches('\n');
                    if text.is_empty() {
                        return None;
                    }
                    let fg = Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b);
                    let mut s = Style::new().fg(fg);
                    if style.font_style.contains(FontStyle::BOLD) {
                        s = s.add_modifier(Modifier::BOLD);
                    }
                    if style.font_style.contains(FontStyle::ITALIC) {
                        s = s.add_modifier(Modifier::ITALIC);
                    }
                    if style.font_style.contains(FontStyle::UNDERLINE) {
                        s = s.add_modifier(Modifier::UNDERLINED);
                    }
                    Some(Span::styled(text.to_owned(), s))
                })
                .collect();

            result.push(Line::from(spans));
        }

        if result.is_empty() {
            result.push(Line::default());
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    #[test]
    fn plain_returns_one_line_per_input_line() {
        let hl = PlainHighlighter;
        let lines = hl.highlight("rust", "let x = 1;\nlet y = 2;", Theme::dark());
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn plain_empty_code_returns_empty() {
        let hl = PlainHighlighter;
        let lines = hl.highlight("rust", "", Theme::dark());
        assert!(lines.is_empty());
    }

    #[test]
    fn syntect_does_not_panic_on_unknown_lang() {
        let hl = SyntectHighlighter::new();
        let lines = hl.highlight("notareallang", "hello world", Theme::dark());
        assert!(!lines.is_empty());
    }

    #[test]
    fn syntect_rust_produces_spans() {
        let hl = SyntectHighlighter::new();
        let lines = hl.highlight("rust", "fn main() {}", Theme::dark());
        assert!(!lines.is_empty());
    }

    #[test]
    fn syntect_light_theme_picks_light() {
        let hl = SyntectHighlighter::new();
        let lines = hl.highlight("rs", "let x = 1;", Theme::light());
        assert!(!lines.is_empty());
    }
}
