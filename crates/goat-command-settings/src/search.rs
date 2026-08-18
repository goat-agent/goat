use std::fmt::Write as _;

use goat_auth::{Credential, CredentialKey, CredentialValue, SecretString};
use goat_client::AdminRequest;
use goat_command::{Command, CommandEffect, CommandInvocation, Session};
use goat_protocol::NotifyKind;

enum SearchOutcome {
    Notice(String),
    Edited(String, Vec<AdminRequest>),
    Error(String),
}
use goat_config::Config;

pub struct Search;

impl Command for Search {
    fn name(&self) -> &'static str {
        "search"
    }

    fn description(&self) -> &'static str {
        "configure web search providers (Tavily is free: 1000/month, no credit card)"
    }

    fn run(&self, invocation: CommandInvocation, session: &mut dyn Session) -> CommandEffect {
        let (kind, message, requests) = match run_search(invocation.raw_args.trim()) {
            SearchOutcome::Notice(message) => (NotifyKind::Info, message, Vec::new()),
            SearchOutcome::Edited(message, requests) => (NotifyKind::Info, message, requests),
            SearchOutcome::Error(message) => (NotifyKind::Error, message, Vec::new()),
        };
        session.notify(kind, message);
        if requests.is_empty() {
            return CommandEffect::Noop;
        }
        CommandEffect::Admin(requests)
    }
}

fn run_search(args: &str) -> SearchOutcome {
    let mut parts = args.split_whitespace();
    let Some(sub) = parts.next() else {
        return list();
    };
    match sub {
        "list" => list(),
        "tavily" | "brave" => match parts.next() {
            Some(key) => add_api_key(sub, key),
            None => SearchOutcome::Error(format!("usage: /search {sub} <api-key>")),
        },
        "searxng" => match parts.next() {
            Some(url) => add_searxng(url),
            None => SearchOutcome::Error("usage: /search searxng <instance-url>".to_owned()),
        },
        "default" => match parts.next() {
            Some(target) => set_default(target),
            None => SearchOutcome::Error("usage: /search default <provider/account>".to_owned()),
        },
        "remove" => match parts.next() {
            Some(target) => remove(target),
            None => SearchOutcome::Error("usage: /search remove <provider/account>".to_owned()),
        },
        other => SearchOutcome::Error(format!(
            "unknown /search subcommand: {other} (try list, tavily, brave, searxng, default, remove)"
        )),
    }
}

fn add_api_key(provider: &str, key: &str) -> SearchOutcome {
    let account = "default";
    let stored = AdminRequest::CredentialSet {
        key: CredentialKey::search(provider, account),
        value: CredentialValue::from(Credential::ApiKey(SecretString::from(key.to_owned()))),
    };
    match upsert_account(provider, account, None) {
        Ok((target, mut requests)) => {
            requests.insert(0, stored);
            SearchOutcome::Edited(
                format!("web search: configured {target} and set it as the default"),
                requests,
            )
        }
        Err(err) => SearchOutcome::Error(err),
    }
}

fn add_searxng(url: &str) -> SearchOutcome {
    match upsert_account("searxng", "home", Some(url)) {
        Ok((target, edits)) => SearchOutcome::Edited(
            format!("web search: configured {target} ({url}) and set it as the default"),
            edits,
        ),
        Err(err) => SearchOutcome::Error(err),
    }
}

fn upsert_account(
    provider: &str,
    account: &str,
    endpoint: Option<&str>,
) -> Result<(String, Vec<AdminRequest>), String> {
    let config_account =
        goat_search_providers::build_search_account_config(provider, account, endpoint, None)?;
    let target = config_account.target();
    let mut edits = vec![goat_api::ConfigEdit::SearchAccountSet {
        account: serde_json::to_value(&config_account).map_err(|err| err.to_string())?,
    }];
    if should_take_default(Config::load().search.default_target.as_deref()) {
        edits.push(goat_api::ConfigEdit::SearchDefaultSet {
            target: Some(target.clone()),
        });
    }
    Ok((target, vec![AdminRequest::ConfigEdit(edits)]))
}

fn should_take_default(current: Option<&str>) -> bool {
    match current {
        None => true,
        Some(target) => target.starts_with("browser/") || target.starts_with("duckduckgo/"),
    }
}

fn set_default(target: &str) -> SearchOutcome {
    let config = Config::load();
    if !config
        .search
        .accounts
        .iter()
        .any(|account| account.target() == target)
        && !goat_search_providers::is_builtin_search_target(target)
    {
        return SearchOutcome::Error(format!(
            "no configured or built-in search target named {target}"
        ));
    }
    SearchOutcome::Edited(
        format!("web search: default is now {target}"),
        vec![AdminRequest::ConfigEdit(vec![
            goat_api::ConfigEdit::SearchDefaultSet {
                target: Some(target.to_owned()),
            },
        ])],
    )
}

fn remove(target: &str) -> SearchOutcome {
    let config = Config::load();
    if !config
        .search
        .accounts
        .iter()
        .any(|account| account.target() == target)
    {
        return SearchOutcome::Error(format!("no configured search account named {target}"));
    }
    let mut requests = Vec::new();
    if let Some((provider, account)) = target.split_once('/') {
        requests.push(AdminRequest::CredentialRemove {
            key: CredentialKey::search(provider, account),
        });
    }
    requests.push(AdminRequest::ConfigEdit(vec![
        goat_api::ConfigEdit::SearchAccountRemove {
            target: target.to_owned(),
        },
    ]));
    SearchOutcome::Edited(format!("web search: removed {target}"), requests)
}

fn list() -> SearchOutcome {
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
    SearchOutcome::Notice(out)
}

#[cfg(test)]
mod tests {
    use super::{SearchOutcome, run_search, should_take_default};

    #[test]
    fn bare_lists() {
        assert!(matches!(run_search(""), SearchOutcome::Notice(_)));
    }

    #[test]
    fn missing_key_errors() {
        assert!(matches!(run_search("tavily"), SearchOutcome::Error(_)));
        assert!(matches!(run_search("searxng"), SearchOutcome::Error(_)));
    }

    #[test]
    fn unknown_subcommand_errors() {
        assert!(matches!(run_search("wat"), SearchOutcome::Error(_)));
    }

    #[test]
    fn default_takes_over_unreliable_targets() {
        assert!(should_take_default(None));
        assert!(should_take_default(Some("browser/duckduckgo")));
        assert!(should_take_default(Some("duckduckgo/html")));
        assert!(!should_take_default(Some("tavily/default")));
    }
}
