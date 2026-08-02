use std::fmt::Display;
use std::future::Future;

use unicode_width::UnicodeWidthStr;

use crate::color::{ColorMode, Palette};

const INDENT: &str = "  ";
const KEY_WIDTH: usize = 10;

fn color() -> ColorMode {
    ColorMode::detect()
}

pub enum Footer {
    None,
    Ok(&'static str),
    Cancel,
    Warn(String),
    Hint(&'static str, String),
}

pub fn cell<F, E>(title: &str, body: F) -> Result<(), E>
where
    F: FnOnce() -> Result<Footer, E>,
    E: Display,
{
    print_title(title);
    let footer = match body() {
        Ok(f) => f,
        Err(e) => Footer::Warn(e.to_string()),
    };
    close_cell(&footer);
    Ok(())
}

pub async fn cell_async<F, Fut, E>(title: &str, body: F) -> Result<(), E>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Footer, E>>,
    E: Display,
{
    print_title(title);
    let footer = match body().await {
        Ok(f) => f,
        Err(e) => Footer::Warn(e.to_string()),
    };
    close_cell(&footer);
    Ok(())
}

fn print_title(title: &str) {
    let c = color();
    println!();
    println!("{INDENT}{}", c.paint(title, Palette::Provider));
    println!();
}

fn close_cell(footer: &Footer) {
    if !matches!(footer, Footer::None) {
        println!();
        print_footer(footer);
    }
    println!();
}

pub fn line(text: &str) {
    println!("{INDENT}{text}");
}

pub fn raw(text: &str) {
    println!("{text}");
}

pub fn pair(key: &str, value: &str) {
    pair_styled(key, value, Palette::Value);
}

pub fn pair_styled(key: &str, value: &str, palette: Palette) {
    let c = color();
    let pad = " ".repeat(KEY_WIDTH.saturating_sub(key.width()));
    println!(
        "{INDENT}{}{pad}  {}",
        c.paint(key, Palette::Muted),
        c.paint(value, palette)
    );
}

pub fn section(name: &str) {
    println!("{INDENT}{}", color().paint(name, Palette::Muted));
}

pub fn blank() {
    println!();
}

pub fn dim(text: &str) -> String {
    color().paint(text, Palette::Muted)
}

fn print_footer(f: &Footer) {
    let c = color();
    let painted = match f {
        Footer::None => return,
        Footer::Ok(verb) => c.paint(verb, Palette::Success),
        Footer::Cancel => c.paint("cancelled", Palette::Muted),
        Footer::Warn(msg) => c.paint(msg, Palette::Warning),
        Footer::Hint(verb, next) => format!(
            "{}  {} {}",
            c.paint(verb, Palette::Success),
            c.paint("→", Palette::Muted),
            c.paint(next, Palette::Muted)
        ),
    };
    println!("{INDENT}{painted}");
}

pub type Cell = (String, Palette);

pub struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<Cell>>,
}

impl Table {
    pub fn new<I, S>(headers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            headers: headers.into_iter().map(Into::into).collect(),
            rows: Vec::new(),
        }
    }

    pub fn row<I, S>(&mut self, cells: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.rows.push(
            cells
                .into_iter()
                .map(|s| (s.into(), Palette::Plain))
                .collect(),
        );
    }

    pub fn styled_row(&mut self, cells: Vec<Cell>) {
        self.rows.push(cells);
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    fn column_widths(&self) -> Vec<usize> {
        let ncol = self
            .headers
            .len()
            .max(self.rows.iter().map(Vec::len).max().unwrap_or(0));
        let mut widths = vec![0usize; ncol];
        for (i, h) in self.headers.iter().enumerate() {
            widths[i] = visible_width(h);
        }
        for row in &self.rows {
            for (i, (text, _)) in row.iter().enumerate().take(ncol) {
                widths[i] = widths[i].max(visible_width(text));
            }
        }
        widths
    }

    fn format_row(c: ColorMode, row: &[Cell], widths: &[usize]) -> String {
        let mut line = String::new();
        for (i, (text, palette)) in row.iter().enumerate() {
            line.push_str(&c.paint(text, *palette));
            line.push_str(&" ".repeat(widths[i].saturating_sub(visible_width(text))));
            if i + 1 < row.len() {
                line.push_str("  ");
            }
        }
        line
    }

    pub fn render(&self) {
        let c = color();
        let widths = self.column_widths();
        if !self.headers.is_empty() {
            let mut hdr = String::from(INDENT);
            for (i, h) in self.headers.iter().enumerate() {
                hdr.push_str(&c.paint(h, Palette::Muted));
                hdr.push_str(&" ".repeat(widths[i].saturating_sub(visible_width(h))));
                if i + 1 < self.headers.len() {
                    hdr.push_str("  ");
                }
            }
            println!("{hdr}");
        }
        for row in &self.rows {
            println!("{INDENT}{}", Self::format_row(c, row, &widths));
        }
    }

    pub fn pick(&self, prompt: &str) -> crate::error::ConsoleResult<Option<usize>> {
        crate::interact::require_terminal()?;
        let c = ColorMode::detect_stderr();
        let widths = self.column_widths();
        let labels: Vec<String> = self
            .rows
            .iter()
            .map(|row| Self::format_row(c, row, &widths))
            .collect();
        crate::interact::select_index(prompt, &labels)
    }
}

fn visible_width(s: &str) -> usize {
    let mut width = 0usize;
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if c == 'm' {
                    break;
                }
            }
        } else {
            width += UnicodeWidthStr::width(ch.encode_utf8(&mut [0; 4]) as &str);
        }
    }
    width
}
