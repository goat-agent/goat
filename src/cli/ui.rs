use anyhow::Result;
use dialoguer::theme::SimpleTheme;
use dialoguer::{Confirm, Input, Password, Select};
use owo_colors::OwoColorize;

const INDENT: &str = "  ";
const ACCENT: (u8, u8, u8) = (0x8B, 0x5C, 0xF6);

pub enum Footer {
    None,
    Ok(&'static str),
    Cancel,
    Warn(String),
    Hint(&'static str, String),
}

pub fn cell<F>(title: &str, body: F) -> Result<()>
where
    F: FnOnce() -> Result<Footer>,
{
    println!();
    println!(
        "{INDENT}{}",
        title.bold().truecolor(ACCENT.0, ACCENT.1, ACCENT.2)
    );
    println!();
    let footer = match body() {
        Ok(f) => f,
        Err(e) => Footer::Warn(e.to_string()),
    };
    if !matches!(footer, Footer::None) {
        println!();
        print_footer(&footer);
    }
    println!();
    Ok(())
}

pub fn line(text: &str) {
    println!("{INDENT}{text}");
}

pub fn pair(key: &str, value: &str) {
    println!("{INDENT}{}  {value}", key.bright_black());
}

pub fn section(name: &str) {
    println!("{INDENT}{}", name.bold());
}

pub fn blank() {
    println!();
}

pub fn dim(text: &str) -> String {
    text.bright_black().to_string()
}

fn print_footer(f: &Footer) {
    let painted = match f {
        Footer::None => return,
        Footer::Ok(verb) => verb.green().bold().to_string(),
        Footer::Cancel => "cancelled".bright_black().to_string(),
        Footer::Warn(msg) => msg.yellow().to_string(),
        Footer::Hint(verb, next) => format!(
            "{}  {} {}",
            verb.green().bold(),
            "→".bright_black(),
            next.bright_black()
        ),
    };
    println!("{INDENT}{painted}");
}

pub enum Style {
    Plain,
    Ok,
    Warn,
    Dim,
}

pub struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<(String, Style)>>,
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
                .map(|s| (s.into(), Style::Plain))
                .collect(),
        );
    }

    pub fn styled_row(&mut self, cells: Vec<(String, Style)>) {
        self.rows.push(cells);
    }

    pub fn render(&self) {
        let ncol = self.headers.len();
        let mut widths = vec![0usize; ncol];
        for (i, h) in self.headers.iter().enumerate() {
            widths[i] = visible_width(h);
        }
        for row in &self.rows {
            for (i, (text, _)) in row.iter().enumerate().take(ncol) {
                widths[i] = widths[i].max(visible_width(text));
            }
        }

        let mut hdr = String::from(INDENT);
        for (i, h) in self.headers.iter().enumerate() {
            hdr.push_str(&h.bold().truecolor(ACCENT.0, ACCENT.1, ACCENT.2).to_string());
            hdr.push_str(&" ".repeat(widths[i] - visible_width(h)));
            if i + 1 < ncol {
                hdr.push_str("  ");
            }
        }
        println!("{hdr}");

        for row in &self.rows {
            let mut line = String::from(INDENT);
            for (i, (text, style)) in row.iter().enumerate().take(ncol) {
                line.push_str(&paint(text, style));
                line.push_str(&" ".repeat(widths[i].saturating_sub(visible_width(text))));
                if i + 1 < ncol {
                    line.push_str("  ");
                }
            }
            println!("{line}");
        }
    }
}

fn paint(text: &str, style: &Style) -> String {
    match style {
        Style::Plain => text.to_string(),
        Style::Ok => text.green().bold().to_string(),
        Style::Warn => text.yellow().to_string(),
        Style::Dim => text.bright_black().to_string(),
    }
}

fn visible_width(s: &str) -> usize {
    let mut w = 0usize;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            i += 2;
            while i < bytes.len() && bytes[i] != b'm' {
                i += 1;
            }
            i += 1;
        } else if bytes[i] < 0x80 {
            w += 1;
            i += 1;
        } else {
            w += 1;
            i += utf8_len(bytes[i]).max(1);
        }
    }
    w
}

fn utf8_len(first: u8) -> usize {
    if first & 0x80 == 0 {
        1
    } else if first & 0xE0 == 0xC0 {
        2
    } else if first & 0xF0 == 0xE0 {
        3
    } else if first & 0xF8 == 0xF0 {
        4
    } else {
        1
    }
}

pub fn ask(label: &str, default: Option<&str>) -> Result<String> {
    let mut p = Input::<String>::with_theme(&SimpleTheme).with_prompt(format!("{INDENT}{label}"));
    if let Some(d) = default {
        p = p.default(d.to_string());
    } else {
        p = p.allow_empty(true);
    }
    Ok(p.interact_text()?)
}

pub fn pick<T: Clone>(label: &str, items: &[(T, String)]) -> Result<T> {
    let labels: Vec<&str> = items.iter().map(|(_, l)| l.as_str()).collect();
    let idx = Select::with_theme(&SimpleTheme)
        .with_prompt(format!("{INDENT}{label}"))
        .items(&labels)
        .default(0)
        .interact()?;
    Ok(items[idx].0.clone())
}

pub fn confirm(prompt: &str, default: bool) -> Result<bool> {
    Ok(Confirm::with_theme(&SimpleTheme)
        .with_prompt(format!("{INDENT}{prompt}"))
        .default(default)
        .interact()?)
}

pub fn secret(label: &str) -> Result<String> {
    Ok(Password::with_theme(&SimpleTheme)
        .with_prompt(format!("{INDENT}{label}"))
        .interact()?)
}
