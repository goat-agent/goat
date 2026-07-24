use std::fmt::Write as _;

use goat_auth::{Credential, CredentialKey, CredentialStore, SecretString};
use goat_command::{Command, CommandEffect, CommandInvocation};
use goat_config::Config;

pub struct Search;

impl Command for Search {
    fn name(&self) -> &'static str {
        "search"
    }

    fn description(&self) -> &'static str {
        "configure web search providers (Tavily is free: 1000/month, no credit card)"
    }

    fn run(&self, invocation: CommandInvocation) -> CommandEffect {
        run_search(invocation.raw_args.trim())
    }
}

fn run_search(args: &str) -> CommandEffect {
    let mut parts = args.split_whitespace();
    let Some(sub) = parts.next() else {
        return list();
    };
    match sub {
        "list" => list(),
        "tavily" | "brave" => match parts.next() {
            Some(key) => add_api_key(sub, key),
            None => CommandEffect::Error(format!("usage: /search {sub} <api-key>")),
        },
        "searxng" => match parts.next() {
            Some(url) => add_searxng(url),
            None => CommandEffect::Error("usage: /search searxng <instance-url>".to_owned()),
        },
        "default" => match parts.next() {
            Some(target) => set_default(target),
            None => CommandEffect::Error("usage: /search default <provider/account>".to_owned()),
        },
        "remove" => match parts.next() {
            Some(target) => remove(target),
            None => CommandEffect::Error("usage: /search remove <provider/account>".to_owned()),
        },
        other => CommandEffect::Error(format!(
            "unknown /search subcommand: {other} (try list, tavily, brave, searxng, default, remove)"
        )),
    }
}

fn credential_store() -> Option<CredentialStore> {
    goat_config::auth_path().map(CredentialStore::new)
}

fn add_api_key(provider: &str, key: &str) -> CommandEffect {
    let account = "default";
    let Some(store) = credential_store() else {
        return CommandEffect::Error(goat_config::HOME_NOT_FOUND.to_owned());
    };
    let credential = Credential::ApiKey(SecretString::from(key.to_owned()));
    if let Err(err) = store.store(&CredentialKey::search(provider, account), credential) {
        return CommandEffect::Error(format!("could not store {provider} credential: {err}"));
    }
    match upsert_account(provider, account, None) {
        Ok(target) => CommandEffect::Notice(format!(
            "web search: configured {target} and set it as the default"
        )),
        Err(err) => CommandEffect::Error(err),
    }
}

fn add_searxng(url: &str) -> CommandEffect {
    match upsert_account("searxng", "home", Some(url)) {
        Ok(target) => CommandEffect::Notice(format!(
            "web search: configured {target} ({url}) and set it as the default"
        )),
        Err(err) => CommandEffect::Error(err),
    }
}

fn upsert_account(provider: &str, account: &str, endpoint: Option<&str>) -> Result<String, String> {
    let config_account =
        goat_search_providers::build_search_account_config(provider, account, endpoint, None)?;
    let target = config_account.target();
    let mut config = Config::load();
    config
        .search
        .accounts
        .retain(|existing| existing.target() != target);
    config.search.accounts.push(config_account);
    if should_take_default(config.search.default_target.as_deref()) {
        config.search.default_target = Some(target.clone());
    }
    config.save().map_err(|err| err.to_string())?;
    Ok(target)
}

fn should_take_default(current: Option<&str>) -> bool {
    match current {
        None => true,
        Some(target) => target.starts_with("browser/") || target.starts_with("duckduckgo/"),
    }
}

fn set_default(target: &str) -> CommandEffect {
    let mut config = Config::load();
    if !config
        .search
        .accounts
        .iter()
        .any(|account| account.target() == target)
        && !goat_search_providers::is_builtin_search_target(target)
    {
        return CommandEffect::Error(format!(
            "no configured or built-in search target named {target}"
        ));
    }
    config.search.default_target = Some(target.to_owned());
    match config.save() {
        Ok(()) => CommandEffect::Notice(format!("web search: default is now {target}")),
        Err(err) => CommandEffect::Error(err.to_string()),
    }
}

fn remove(target: &str) -> CommandEffect {
    let mut config = Config::load();
    let before = config.search.accounts.len();
    config
        .search
        .accounts
        .retain(|account| account.target() != target);
    if config.search.accounts.len() == before {
        return CommandEffect::Error(format!("no configured search account named {target}"));
    }
    if config.search.default_target.as_deref() == Some(target) {
        config.search.default_target = None;
    }
    if let Some((provider, account)) = target.split_once('/')
        && let Some(store) = credential_store()
    {
        let _ = store.remove(&CredentialKey::search(provider, account));
    }
    match config.save() {
        Ok(()) => CommandEffect::Notice(format!("web search: removed {target}")),
        Err(err) => CommandEffect::Error(err.to_string()),
    }
}

fn list() -> CommandEffect {
    let config = Config::load();
    let mut out = String::new();
    out.push_str("web search providers\n");
    match &config.search.default_target {
        Some(target) => {
            let _ = writeln!(out, "default: {target}");
        }
        None => out.push_str("default: (none — DuckDuckGo is bot-blocked and unreliable)\n"),
    }
    if config.search.accounts.is_empty() {
        out.push_str("configured: (none)\n");
    } else {
        out.push_str("configured:\n");
        for account in &config.search.accounts {
            let _ = writeln!(out, "- {}", account.target());
        }
    }
    out.push_str("\nadd one: /search tavily <key> | /search brave <key> | /search searxng <url>\n");
    out.push_str("Tavily is free (1000 searches/month, no credit card): https://app.tavily.com\n");
    CommandEffect::Notice(out)
}

#[cfg(test)]
mod tests {
    use super::{run_search, should_take_default};
    use goat_command::CommandEffect;

    #[test]
    fn bare_lists() {
        assert!(matches!(run_search(""), CommandEffect::Notice(_)));
    }

    #[test]
    fn missing_key_errors() {
        assert!(matches!(run_search("tavily"), CommandEffect::Error(_)));
        assert!(matches!(run_search("searxng"), CommandEffect::Error(_)));
    }

    #[test]
    fn unknown_subcommand_errors() {
        assert!(matches!(run_search("wat"), CommandEffect::Error(_)));
    }

    #[test]
    fn default_takes_over_unreliable_targets() {
        assert!(should_take_default(None));
        assert!(should_take_default(Some("browser/duckduckgo")));
        assert!(should_take_default(Some("duckduckgo/html")));
        assert!(!should_take_default(Some("tavily/default")));
    }
}
