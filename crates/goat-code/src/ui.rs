use std::io::IsTerminal;

use color_eyre::eyre::{Report, Result, eyre};
use goat_provider::AuthMethod;

pub use goat_console::{
    Cell, ColorMode, Palette, Table, blank, confirm, note, pair, pair_styled, prompt, raw, secret,
    section, select_index, select_indices, success, truncate_to_width, warning,
};

pub fn report(message: impl Into<String>) -> Report {
    eyre!(goat_console::format_failure(&message.into(), None))
}

pub fn report_hint(message: impl Into<String>, hint: impl Into<String>) -> Report {
    eyre!(goat_console::format_failure(
        &message.into(),
        Some(hint.into())
    ))
}

pub fn fail_hint(message: impl Into<String>, hint: impl Into<String>) -> Result<()> {
    Err(report_hint(message, hint))
}

pub fn worktree_entry(err: goat_worktree::WorktreeError) -> Report {
    match &err {
        goat_worktree::WorktreeError::Git(goat_git::GitError::Missing) => report_hint(
            "git was not found on PATH",
            "--worktree needs git installed",
        ),
        goat_worktree::WorktreeError::Git(goat_git::GitError::NotARepository) => report_hint(
            "not a git repository",
            "--worktree must run inside a git repository",
        ),
        _ => Report::from(err),
    }
}

pub enum AuthPick {
    OAuth,
    ApiKey,
}

pub fn pick_auth_method(provider: &str, method: AuthMethod) -> Result<Option<AuthPick>> {
    match method {
        AuthMethod::OAuth => Ok(Some(AuthPick::OAuth)),
        AuthMethod::ApiKey => Ok(Some(AuthPick::ApiKey)),
        AuthMethod::ApiKeyOrOAuth => {
            let labels = [
                "device code   (browser)".to_owned(),
                "api key       (secret)".to_owned(),
            ];
            let Some(index) = select_index(&format!("method for {provider}"), &labels)? else {
                return Ok(None);
            };
            Ok(Some(if index == 0 {
                AuthPick::OAuth
            } else {
                AuthPick::ApiKey
            }))
        }
        AuthMethod::None => Err(report(format!("{provider} requires no login"))),
    }
}

pub struct AccountResolution {
    pub name: String,
    pub replacing: bool,
}

pub fn resolve_account(
    service: &str,
    provider: &str,
    requested: Option<&str>,
    existing: &[String],
    default_name: &str,
) -> Result<Option<AccountResolution>> {
    let taken = |name: &str| existing.iter().any(|account| account == name);
    let interactive = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();

    if let Some(requested) = requested {
        let name = requested.trim();
        if name.is_empty() {
            return Err(report("account name must not be empty"));
        }
        return Ok(Some(AccountResolution {
            replacing: taken(name),
            name: name.to_owned(),
        }));
    }

    if existing.is_empty() {
        if !interactive {
            return Ok(Some(AccountResolution {
                name: default_name.to_owned(),
                replacing: false,
            }));
        }
        let Some(name) = prompt_account_name(provider, Some(default_name))? else {
            return Ok(None);
        };
        return Ok(Some(AccountResolution {
            replacing: taken(&name),
            name,
        }));
    }

    if !interactive {
        return Err(report_hint(
            format!(
                "{service} {provider} already has an account: {}",
                existing.join(", ")
            ),
            format!("pass `--account <name>` to choose or add an account for {provider}"),
        ));
    }

    let mut sorted = existing.to_vec();
    sorted.sort();
    let mut labels: Vec<String> = sorted.iter().map(|a| format!("{a}  (update)")).collect();
    labels.push("＋ new account".to_owned());
    let Some(index) = select_index(&format!("account for {provider}"), &labels)? else {
        return Ok(None);
    };
    if let Some(name) = sorted.get(index) {
        return Ok(Some(AccountResolution {
            replacing: true,
            name: name.clone(),
        }));
    }
    let Some(name) = prompt_account_name(provider, None)? else {
        return Ok(None);
    };
    if taken(&name)
        && !confirm(
            &format!("account `{name}` exists for {provider}; update it?"),
            false,
        )?
    {
        return Ok(None);
    }
    Ok(Some(AccountResolution {
        replacing: taken(&name),
        name,
    }))
}

fn prompt_account_name(provider: &str, default: Option<&str>) -> Result<Option<String>> {
    Ok(prompt(&format!("account for {provider}"), default)?)
}

pub fn prompt_api_key(provider: &str) -> Result<Option<String>> {
    Ok(secret(&format!("api key for {provider}"))?)
}

pub fn prompt_optional_api_key(provider: &str) -> Result<Option<String>> {
    Ok(secret(&format!("api key for {provider} (enter for none)"))?)
}

pub fn prompt_provider_name() -> Result<Option<String>> {
    Ok(prompt("provider name", None)?)
}

pub fn prompt_endpoint(default: Option<&str>) -> Result<Option<String>> {
    Ok(prompt("endpoint", default)?)
}

pub fn oauth_status(text: &str) {
    let color = ColorMode::detect();
    if let Some((visit, code)) = parse_device_code_message(text) {
        println!(
            "  {} {}",
            color.paint("visit", Palette::Muted),
            color.paint(visit, Palette::Info)
        );
        println!(
            "  {} {}",
            color.paint("code", Palette::Muted),
            color.paint(code, Palette::Value)
        );
        return;
    }
    if let Some(url) = parse_browser_url(text) {
        println!("  {}", color.paint(url, Palette::Info));
        return;
    }
    for line in text.lines() {
        if !line.trim().is_empty() {
            println!("  {}", color.paint(line, Palette::Info));
        }
    }
}

fn parse_device_code_message(text: &str) -> Option<(String, String)> {
    let rest = text.strip_prefix("open ")?;
    let (url, code) = rest.split_once(" and enter code: ")?;
    Some((url.trim().to_owned(), code.trim().to_owned()))
}

fn parse_browser_url(text: &str) -> Option<String> {
    let (_, url) = text.split_once(":\n")?;
    let url = url.trim();
    (!url.is_empty()).then(|| url.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{parse_browser_url, parse_device_code_message};

    #[test]
    fn parses_device_code_oauth_message() {
        let parsed = parse_device_code_message("open https://auth.example and enter code: ABCD");
        assert_eq!(
            parsed,
            Some(("https://auth.example".to_owned(), "ABCD".to_owned()))
        );
    }

    #[test]
    fn parses_browser_oauth_url() {
        let parsed = parse_browser_url(
            "opening browser to sign in… if it does not open, visit:\nhttps://example.com",
        );
        assert_eq!(parsed, Some("https://example.com".to_owned()));
    }
}
