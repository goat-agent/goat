use goat_auth::{Credential, CredentialKey, CredentialKind, CredentialStore, SecretString};
use goat_config::Config;
use goat_tool_search::{
    SearchCredentialMetadata, build_search_account_config, configured_search_provider,
    configured_search_target, default_search_target, is_builtin_search_target,
    search_builtin_targets, search_provider, search_providers,
};

use crate::ui::{ColorMode, Palette, pair, pair_styled};

use crate::cli::SearchCommand;
use crate::ui;

const PROVIDER_WIDTH: usize = 12;
const STATUS_WIDTH: usize = 10;
const ACCOUNT_WIDTH: usize = 18;

pub fn run(command: SearchCommand) -> color_eyre::Result<()> {
    match command {
        SearchCommand::List => {
            list();
            Ok(())
        }
        SearchCommand::Info { provider } => info(&provider),
        SearchCommand::Login {
            provider,
            account,
            endpoint,
            engine,
            key,
            default,
        } => login(
            &provider,
            account.as_deref(),
            endpoint.as_deref(),
            engine.as_deref(),
            key,
            default,
        ),
        SearchCommand::Logout { provider, account } => logout(&provider, &account),
        SearchCommand::Default { target } => set_default(&target),
    }
}

fn login(
    provider: &str,
    account: Option<&str>,
    endpoint: Option<&str>,
    engine: Option<&str>,
    key: Option<String>,
    make_default: bool,
) -> color_eyre::Result<()> {
    let metadata = search_provider(provider).ok_or_else(|| {
        ui::report_hint(
            format!("unknown search provider: {provider}"),
            "run `goat search list` to see available providers",
        )
    })?;
    let existing = existing_search_accounts(provider);
    let Some(resolution) = ui::resolve_account(
        "search provider",
        provider,
        account,
        &existing,
        metadata.default_account,
    )?
    else {
        ui::note("cancelled");
        return Ok(());
    };
    let account = resolution.name.as_str();
    let entry =
        build_search_account_config(provider, account, endpoint, engine).map_err(ui::report)?;
    store_search_key(provider, account, key, metadata.credential)?;
    let mut config = Config::load();
    let target = entry.target();
    config
        .search
        .accounts
        .retain(|existing| existing.target() != target);
    config.search.accounts.push(entry);
    if make_default || config.search.default_target.is_none() {
        config.search.default_target = Some(target.clone());
    }
    config.save()?;
    let verb = if resolution.replacing {
        "updated"
    } else {
        "connected"
    };
    if make_default {
        ui::success(&format!("{verb} {target} (default)"));
    } else {
        ui::success(&format!("{verb} {target}"));
    }
    Ok(())
}

fn existing_search_accounts(provider: &str) -> Vec<String> {
    let mut accounts = Config::load()
        .search
        .accounts
        .iter()
        .filter(|account| configured_search_provider(account) == provider)
        .map(|account| configured_search_target(account).account.to_owned())
        .collect::<Vec<_>>();
    for (key, _) in search_credentials() {
        if key.provider == provider && !accounts.contains(&key.account) {
            accounts.push(key.account);
        }
    }
    accounts
}

fn store_search_key(
    provider: &str,
    account: &str,
    key: Option<String>,
    credential: SearchCredentialMetadata,
) -> color_eyre::Result<()> {
    let SearchCredentialMetadata::EnvApiKey { env_var } = credential else {
        if key.is_some() {
            return ui::fail_hint(
                format!("--key is not supported for search provider {provider}"),
                "this provider does not use stored API keys",
            );
        }
        return Ok(());
    };
    if key.is_none() && std::env::var(env_var).is_ok_and(|value| !value.is_empty()) {
        return Ok(());
    }
    let secret = if let Some(key) = key {
        key
    } else {
        let Some(secret) = ui::prompt_api_key(provider)? else {
            ui::note("cancelled");
            return Ok(());
        };
        secret
    };
    let path = goat_config::auth_path().ok_or_else(|| ui::report(goat_config::HOME_NOT_FOUND))?;
    let store = CredentialStore::new(path);
    store
        .store(
            &CredentialKey::search(provider, account),
            Credential::ApiKey(SecretString::from(secret)),
        )
        .map_err(|err| {
            ui::report_hint(
                format!("could not update credential store: {err}"),
                "check permissions on ~/.goat",
            )
        })
}

fn set_default(target: &str) -> color_eyre::Result<()> {
    let mut config = Config::load();
    if !is_builtin_search_target(target)
        && !config
            .search
            .accounts
            .iter()
            .any(|account| account.target() == target)
    {
        return ui::fail_hint(
            format!("unknown search target: {target}"),
            "run `goat search list` to see configured targets",
        );
    }
    config.search.default_target = Some(target.to_owned());
    config.save()?;
    ui::success(&format!("default search target set to {target}"));
    Ok(())
}

fn list() {
    let config = Config::load();
    let default = default_target(&config);
    let credentials = search_credentials();
    let color = ColorMode::detect();
    println!();
    println!(
        "  {} {} {} {}",
        color.cell("provider", Palette::Muted, PROVIDER_WIDTH),
        color.cell("status", Palette::Muted, STATUS_WIDTH),
        color.cell("account", Palette::Muted, ACCOUNT_WIDTH),
        color.paint("target", Palette::Muted)
    );
    for provider in search_providers() {
        let mut printed = false;
        for target in builtin_targets_for(provider.id) {
            print_target(color, &target, &default, &credentials);
            printed = true;
        }
        for account in config
            .search
            .accounts
            .iter()
            .filter(|account| configured_search_provider(account) == provider.id)
        {
            print_target(color, &configured_target(account), &default, &credentials);
            printed = true;
        }
        if !printed {
            print_target(
                color,
                &available_target(provider.id, provider.default_account, provider.credential),
                &default,
                &credentials,
            );
        }
    }
}

fn info(provider: &str) -> color_eyre::Result<()> {
    let config = Config::load();
    let default = default_target(&config);
    let credentials = search_credentials();
    let mut targets = search_providers()
        .iter()
        .map(|provider| {
            available_target(provider.id, provider.default_account, provider.credential)
        })
        .collect::<Vec<_>>();
    targets.extend(builtin_targets());
    targets.extend(config.search.accounts.iter().map(configured_target));
    let matches = targets
        .into_iter()
        .filter(|target| target.provider == provider || target.target == provider)
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return ui::fail_hint(
            format!("unknown search provider: {provider}"),
            "run `goat search list` to see available providers",
        );
    }
    let color = ColorMode::detect();
    println!();
    println!("{}", color.paint(provider, Palette::Provider));
    for target in matches {
        println!();
        pair("target", &target.target);
        pair_styled(
            "status",
            target.status(&credentials),
            target.palette(&credentials),
        );
        pair("account", &target.account);
        pair("default", yes_no(target.target == default));
        pair("kind", target.kind);
        pair("setup", target.setup);
    }
    Ok(())
}

fn logout(provider: &str, account: &str) -> color_eyre::Result<()> {
    let target = format!("{provider}/{account}");
    if is_builtin_search_target(&target) {
        return ui::fail_hint(
            format!("cannot remove built-in search target: {target}"),
            "built-in targets are always available",
        );
    }
    let mut config = Config::load();
    let before = config.search.accounts.len();
    config
        .search
        .accounts
        .retain(|account| account.target() != target);
    if before == config.search.accounts.len() {
        return ui::fail_hint(
            format!("unknown search target: {target}"),
            "run `goat search list` to see configured targets",
        );
    }
    if config.search.default_target.as_deref() == Some(&target) {
        config.search.default_target = Some(default_search_target().to_owned());
    }
    config.save()?;
    if let Some(metadata) = search_provider(provider)
        && matches!(
            metadata.credential,
            SearchCredentialMetadata::EnvApiKey { .. }
        )
        && let Some(path) = goat_config::auth_path()
    {
        let store = CredentialStore::new(path);
        let _ = store.remove(&CredentialKey::search(provider, account));
    }
    ui::success(&format!("disconnected search target {target}"));
    Ok(())
}

fn print_target(
    color: ColorMode,
    target: &SearchTarget,
    default: &str,
    credentials: &[(CredentialKey, CredentialKind)],
) {
    let marker = if target.target == default {
        "●"
    } else {
        "○"
    };
    println!(
        "{} {} {} {} {}",
        color.paint(marker, target.palette(credentials)),
        color.cell(&target.provider, Palette::Provider, PROVIDER_WIDTH),
        color.cell(
            target.status(credentials),
            target.palette(credentials),
            STATUS_WIDTH
        ),
        color.cell(&target.account, Palette::Value, ACCOUNT_WIDTH),
        color.paint(&target.target, Palette::Value)
    );
}

fn default_target(config: &Config) -> String {
    config
        .search
        .default_target
        .clone()
        .unwrap_or_else(|| default_search_target().to_owned())
}

fn search_credentials() -> Vec<(CredentialKey, CredentialKind)> {
    goat_config::auth_path().map_or_else(Vec::new, |path| {
        CredentialStore::new(path)
            .entries()
            .into_iter()
            .filter(|(key, _)| key.service == goat_auth::CredentialService::Search)
            .collect()
    })
}

fn builtin_targets_for(provider: &str) -> Vec<SearchTarget> {
    search_builtin_targets()
        .into_iter()
        .filter(|target| target.provider == provider)
        .map(search_target_from_metadata)
        .collect()
}

fn builtin_targets() -> Vec<SearchTarget> {
    search_builtin_targets()
        .into_iter()
        .map(search_target_from_metadata)
        .collect()
}

fn search_target_from_metadata(target: goat_tool_search::SearchTargetMetadata<'_>) -> SearchTarget {
    SearchTarget {
        provider: target.provider.to_owned(),
        account: target.account.to_owned(),
        target: target.target.to_owned(),
        kind: target.kind,
        setup: target.setup,
        credential: target.credential,
    }
}

fn available_target(
    provider: &str,
    account: &str,
    credential: SearchCredentialMetadata,
) -> SearchTarget {
    SearchTarget {
        provider: provider.to_owned(),
        account: account.to_owned(),
        target: format!("{provider}/{account}"),
        kind: "available",
        setup: search_provider(provider).map_or("", |metadata| metadata.setup),
        credential,
    }
}

fn configured_target(account: &goat_config::SearchAccountConfig) -> SearchTarget {
    let metadata = configured_search_target(account);
    SearchTarget {
        provider: metadata.provider.to_owned(),
        account: metadata.account.to_owned(),
        target: account.target(),
        kind: metadata.kind,
        setup: metadata.setup,
        credential: metadata.credential,
    }
}

struct SearchTarget {
    provider: String,
    account: String,
    target: String,
    kind: &'static str,
    setup: &'static str,
    credential: SearchCredentialMetadata,
}

impl SearchTarget {
    fn status(&self, credentials: &[(CredentialKey, CredentialKind)]) -> &'static str {
        match self.credential {
            SearchCredentialMetadata::None => {
                if self.kind == "available" {
                    "available"
                } else {
                    "local"
                }
            }
            SearchCredentialMetadata::EnvApiKey { env_var } => {
                if std::env::var(env_var).is_ok_and(|value| !value.is_empty()) {
                    "env"
                } else if credentials
                    .iter()
                    .any(|(key, _)| key.provider == self.provider && key.account == self.account)
                {
                    "connected"
                } else {
                    "missing"
                }
            }
        }
    }

    fn palette(&self, credentials: &[(CredentialKey, CredentialKind)]) -> Palette {
        match self.status(credentials) {
            "connected" => Palette::Success,
            "env" => Palette::Info,
            "local" | "available" => Palette::Local,
            _ => Palette::Warning,
        }
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}
