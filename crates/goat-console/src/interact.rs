use std::io::IsTerminal;

use dialoguer::{Confirm, Input, Password, Select};

use crate::color::{ColorMode, Palette};
use crate::error::{ConsoleResult, dialoguer_error, fail, report};
use crate::theme::goat_theme;

const BACK_HINT: &str = "enter to go back";

pub fn require_terminal() -> ConsoleResult<()> {
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        return fail("interactive commands require a terminal");
    }
    Ok(())
}

pub fn success(text: &str) {
    println!("{}", ColorMode::detect().paint(text, Palette::Success));
}

pub fn warning(text: &str) {
    println!("{}", ColorMode::detect().paint(text, Palette::Warning));
}

pub fn note(text: &str) {
    println!("{}", ColorMode::detect().paint(text, Palette::Muted));
}

pub fn select_index(prompt: &str, labels: &[String]) -> ConsoleResult<Option<usize>> {
    require_terminal()?;
    if labels.is_empty() {
        return Ok(None);
    }
    Select::with_theme(goat_theme())
        .with_prompt(prompt)
        .items(labels)
        .default(0)
        .report(false)
        .interact_opt()
        .map_err(dialoguer_error)
}

pub fn pick<T: Clone>(label: &str, items: &[(T, String)]) -> ConsoleResult<T> {
    let labels: Vec<String> = items.iter().map(|(_, l)| l.clone()).collect();
    let index = select_index(label, &labels)?.ok_or_else(|| report("cancelled"))?;
    Ok(items[index].0.clone())
}

pub fn confirm(prompt: &str, default: bool) -> ConsoleResult<bool> {
    require_terminal()?;
    Confirm::with_theme(goat_theme())
        .with_prompt(prompt)
        .default(default)
        .interact()
        .map_err(dialoguer_error)
}

pub fn prompt(label: &str, default: Option<&str>) -> ConsoleResult<Option<String>> {
    require_terminal()?;
    let mut input = Input::<String>::with_theme(goat_theme())
        .with_prompt(with_back_hint(label, default))
        .report(false);
    match default {
        Some(value) => input = input.default(value.to_owned()),
        None => input = input.allow_empty(true),
    }
    let value = input.interact_text().map_err(dialoguer_error)?;
    Ok(accept(value.trim(), default))
}

pub fn secret(label: &str) -> ConsoleResult<Option<String>> {
    require_terminal()?;
    let value = Password::with_theme(goat_theme())
        .with_prompt(with_back_hint(label, None))
        .allow_empty_password(true)
        .report(false)
        .interact()
        .map_err(dialoguer_error)?;
    Ok(accept(value.trim(), None))
}

fn with_back_hint(label: &str, default: Option<&str>) -> String {
    if default.is_some() {
        return label.to_owned();
    }
    let hint = ColorMode::detect_stderr().paint(format!("({BACK_HINT})"), Palette::Muted);
    format!("{label} {hint}")
}

fn accept(value: &str, default: Option<&str>) -> Option<String> {
    if value.is_empty() {
        return default.map(str::to_owned);
    }
    Some(value.to_owned())
}
