use std::io::IsTerminal;
use std::time::Duration;

use color_eyre::eyre::Result;
use goat_auth::{
    Credential, CredentialKey, CredentialKind, CredentialService, CredentialStore, SecretString,
};
use goat_provider::{AuthMethod, LoginEndpointMetadata, ProviderId, ProviderMetadata};
use goat_providers::Registry;
use tokio::sync::mpsc;

use crate::ui::{AuthPick, ColorMode, Palette, pair, pair_styled, truncate_to_width};

use crate::cli::ProviderCommand;
use crate::ui;

pub async fn run_setup() -> color_eyre::Result<()> {
    let paths = goat_config::GoatPaths::default_layout().map_err(|e| ui::report(e.to_string()))?;
    for dir in [
        &paths.root,
        &paths.agents_dir,
        &paths.skills_dir,
        &paths.logs_dir,
    ] {
        std::fs::create_dir_all(dir).map_err(|e| ui::report(e.to_string()))?;
    }
    let store = CredentialStore::new(paths.credentials_json.clone());
    let user = goat_config::UserProviders::at(paths.config_json.clone());

    ui::section("Providers");
    ui::note("Connect the model providers goat should use.");
    connect_providers(&store, &user).await?;

    if !has_model_credentials(&store) {
        ui::blank();
        ui::warning("No provider connected — `goat code` and agents need one. Rerun `goat setup`.");
        return Ok(());
    }

    ui::blank();
    let agent = setup_agent(&paths).await?;

    ui::blank();
    ui::section("Ready");
    ui::pair("code", "goat code");
    match agent {
        Some(slug) => ui::pair("agent", &format!("{slug} — runs while the daemon is up")),
        None => ui::pair("agent", "goat agent add"),
    }
    Ok(())
}

async fn connect_providers(
    store: &CredentialStore,
    user: &goat_config::UserProviders,
) -> color_eyre::Result<()> {
    loop {
        let (ids, rows) = provider_rows(store, user, true);
        let mut table = ui::Table::new(Vec::<String>::new());
        table.styled_row(vec![
            ("✓".to_owned(), ui::Palette::Muted),
            ("done".to_owned(), ui::Palette::Value),
            (String::new(), ui::Palette::Muted),
            (String::new(), ui::Palette::Muted),
        ]);
        for row in rows {
            table.styled_row(row);
        }

        println!();
        let Ok(Some(index)) = table.pick("provider") else {
            return Ok(());
        };
        if index == 0 {
            return Ok(());
        }
        let provider = ids[index - 1].clone();

        let existing = provider_accounts(&store.entries(), &provider)
            .into_iter()
            .map(|(account, _)| account)
            .collect::<Vec<_>>();
        let Some(resolution) = ui::resolve_account(
            "provider",
            &provider,
            None,
            &existing,
            goat_providers::DEFAULT_ACCOUNT,
        )?
        else {
            continue;
        };
        if let Err(e) = login(
            store,
            user,
            &provider,
            &resolution.name,
            None,
            None,
            resolution.replacing,
        )
        .await
        {
            eprintln!("skipped {provider}: {e}");
        }
    }
}

async fn setup_agent(paths: &goat_config::GoatPaths) -> color_eyre::Result<Option<String>> {
    if !ui::confirm("Set up an autonomous chat agent now?", false)? {
        ui::note("skip — add one anytime with `goat agent add`");
        return Ok(None);
    }
    let slug =
        goat_agent::cli::agent::create_interactive(paths).map_err(|e| ui::report(e.to_string()))?;
    if ui::confirm(&format!("Bind a chat channel to {slug} now?"), true)? {
        goat_agent::cli::channel::run(goat_agent::cli::channel::Cmd::Add {
            kind: None,
            agent: Some(slug.clone()),
            no_verify: false,
        })
        .await
        .map_err(|e| ui::report(e.to_string()))?;
    }
    Ok(Some(slug))
}

fn has_model_credentials(store: &CredentialStore) -> bool {
    store
        .entries()
        .iter()
        .any(|(key, _)| key.service == CredentialService::Model)
}

pub async fn run_provider(command: ProviderCommand) -> color_eyre::Result<()> {
    let path = goat_config::auth_path().ok_or_else(|| ui::report(goat_config::HOME_NOT_FOUND))?;
    let store = CredentialStore::new(path);
    let user = goat_config::UserProviders::detect();
    match command {
        ProviderCommand::Login {
            provider,
            account,
            key,
            endpoint,
        } => {
            let provider = match provider {
                Some(provider) => provider,
                None => pick_login_provider(&store, &user)?,
            };
            let existing = provider_accounts(&store.entries(), &provider)
                .into_iter()
                .map(|(account, _)| account)
                .collect::<Vec<_>>();
            let Some(resolution) = ui::resolve_account(
                "provider",
                &provider,
                account.as_deref(),
                &existing,
                goat_providers::DEFAULT_ACCOUNT,
            )?
            else {
                ui::note("cancelled");
                return Ok(());
            };
            if !login(
                &store,
                &user,
                &provider,
                &resolution.name,
                key,
                endpoint,
                resolution.replacing,
            )
            .await?
            {
                ui::note("cancelled");
            }
            Ok(())
        }
        ProviderCommand::Add {
            name,
            endpoint,
            key,
            account,
        } => add_custom(&store, &user, name, endpoint, key, account).await,
        ProviderCommand::Remove { name } => remove_custom(&store, &user, &name).await,
        ProviderCommand::List => {
            list_providers(&store, &user);
            Ok(())
        }
        ProviderCommand::Info { provider } => provider_info(&store, &user, &provider),
        ProviderCommand::Logout { provider, account } => {
            logout(&store, &provider, &account, CredentialService::Model)
        }
    }
}

async fn add_custom(
    store: &CredentialStore,
    user: &goat_config::UserProviders,
    name: Option<String>,
    endpoint: Option<String>,
    key: Option<String>,
    account: Option<String>,
) -> color_eyre::Result<()> {
    let name = if let Some(name) = name {
        name
    } else {
        let Some(name) = ui::prompt_provider_name()? else {
            ui::note("cancelled");
            return Ok(());
        };
        name
    };
    goat_providers::builtin::validate_id(&name).map_err(ui::report)?;
    let existing = user.load().get(&name).map(|config| config.endpoint.clone());
    if existing.is_none()
        && Registry::new(store, user)
            .get(&ProviderId::from(name.as_str()))
            .is_some()
    {
        return ui::fail_hint(
            format!("{name} is a built-in provider"),
            format!("use `goat provider login {name}`"),
        );
    }
    let endpoint = if let Some(endpoint) = endpoint {
        endpoint
    } else {
        let Some(endpoint) = ui::prompt_endpoint(existing.as_deref())? else {
            ui::note("cancelled");
            return Ok(());
        };
        endpoint
    };
    let endpoint =
        goat_providers::builtin::validate_user_endpoint(&endpoint).map_err(ui::report)?;
    let key = match key {
        Some(key) => Some(key),
        None if existing.is_none() && std::io::stdin().is_terminal() => {
            ui::prompt_optional_api_key(&name)?
        }
        None => None,
    };
    write_config(vec![goat_api::ConfigEdit::ProviderSet {
        name: name.clone(),
        endpoint,
    }])
    .await?;
    let account = account.unwrap_or_else(|| goat_providers::DEFAULT_ACCOUNT.to_owned());
    if let Some(key) = key.filter(|key| !key.trim().is_empty()) {
        store
            .store(
                &CredentialKey::model(name.as_str(), account.as_str()),
                Credential::ApiKey(SecretString::from(key)),
            )
            .map_err(storage_error)?;
    }
    let verb = if existing.is_some() {
        "updated"
    } else {
        "added"
    };
    ui::success(&format!("{verb} provider {name}"));
    verify(store, user, &name, &account).await;
    apply_to_daemon().await;
    Ok(())
}

async fn write_config(edits: Vec<goat_api::ConfigEdit>) -> color_eyre::Result<()> {
    let link = crate::remote::local()?;
    goat_client::edit_config(&link, edits)
        .await
        .map_err(|err| color_eyre::eyre::eyre!("could not write the daemon config: {err}"))?;
    Ok(())
}

async fn apply_to_daemon() {
    let Some(socket_path) = goat_config::socket_path() else {
        return;
    };
    let goat_client::Daemon::Reachable(them) = goat_client::greet(&socket_path).await else {
        return;
    };
    if !goat_client::is_current(&goat_client::mine(), &them) {
        return;
    }
    let Ok(link) = crate::remote::local() else {
        return;
    };
    if let Err(e) = goat_client::reload(&link, None).await {
        ui::note(&format!(
            "could not apply to the running daemon: {e}; run `goat reload`"
        ));
    }
}

async fn remove_custom(
    store: &CredentialStore,
    user: &goat_config::UserProviders,
    name: &str,
) -> color_eyre::Result<()> {
    if !user.load().contains_key(name) {
        return ui::fail_hint(
            format!("{name} is not a custom provider"),
            "run `goat provider list` to see providers",
        );
    }
    if !ui::confirm(
        &format!("Remove provider {name} and all of its credentials?"),
        false,
    )? {
        ui::note("cancelled");
        return Ok(());
    }
    write_config(vec![goat_api::ConfigEdit::ProviderRemove {
        name: name.to_owned(),
    }])
    .await?;
    for (key, _) in store.entries() {
        if key.service == CredentialService::Model && key.provider == name {
            let _ = store.remove(&key);
        }
    }
    ui::success(&format!("removed provider {name}"));
    apply_to_daemon().await;
    Ok(())
}

fn pick_login_provider(
    store: &CredentialStore,
    user: &goat_config::UserProviders,
) -> Result<String> {
    let (ids, rows) = provider_rows(store, user, true);
    if ids.is_empty() {
        return Err(ui::report("no login-capable providers available"));
    }
    let mut table = ui::Table::new(Vec::<String>::new());
    for row in rows {
        table.styled_row(row);
    }
    let index = table
        .pick("provider")?
        .ok_or_else(|| ui::report("provider login cancelled"))?;
    Ok(ids[index].clone())
}

async fn login(
    store: &CredentialStore,
    user: &goat_config::UserProviders,
    provider: &str,
    account: &str,
    key: Option<String>,
    endpoint: Option<String>,
    replacing: bool,
) -> color_eyre::Result<bool> {
    let registry = Registry::new(store, user);
    let provider_handle = registry
        .all()
        .iter()
        .find(|p| p.id().to_string() == provider)
        .cloned()
        .ok_or_else(|| unknown_provider_error(provider, &registry))?;
    let method = provider_handle.capabilities().auth;
    let metadata = provider_handle.metadata();

    if goat_providers::builtin::is_custom(provider_handle.as_ref()) {
        if endpoint.is_some() {
            return ui::fail_hint(
                format!("--endpoint is not supported by `login` for custom provider {provider}"),
                format!("change the endpoint with `goat provider add {provider} --endpoint <url>`"),
            )
            .map(|()| false);
        }
        let secret = if let Some(key) = key {
            key
        } else {
            let Some(secret) = ui::prompt_api_key(provider)? else {
                return Ok(false);
            };
            secret
        };
        store
            .store(
                &CredentialKey::model(provider, account),
                Credential::ApiKey(SecretString::from(secret)),
            )
            .map_err(storage_error)?;
        let verb = if replacing { "updated" } else { "stored" };
        ui::success(&format!("{verb} credential for {provider} ({account})"));
        verify(store, user, provider, account).await;
        return Ok(true);
    }

    if endpoint.is_some() && metadata.login_endpoint.is_none() {
        return ui::fail_hint(
            format!("--endpoint is not supported for provider {provider}"),
            "omit --endpoint for this provider",
        )
        .map(|()| false);
    }
    if key.is_some() && matches!(method, AuthMethod::OAuth) {
        return ui::fail_hint(
            format!("--key is not supported for OAuth-only provider {provider}"),
            "run without --key to start device-code login",
        )
        .map(|()| false);
    }
    if endpoint.is_some() && matches!(method, AuthMethod::OAuth) {
        return ui::fail_hint(
            format!("--endpoint is not supported for OAuth-only provider {provider}"),
            "omit --endpoint for this provider",
        )
        .map(|()| false);
    }

    let credential_key = CredentialKey::model(provider, account);

    if matches!(method, AuthMethod::None) {
        return Ok(true);
    }

    let auth_pick = if key.is_some() {
        AuthPick::ApiKey
    } else {
        match ui::pick_auth_method(provider, method)? {
            Some(pick) => pick,
            None => return Ok(false),
        }
    };

    match auth_pick {
        AuthPick::OAuth => {
            let (status, mut lines) = mpsc::channel::<String>(16);
            let printer = tokio::spawn(async move {
                while let Some(line) = lines.recv().await {
                    ui::oauth_status(&line);
                }
            });
            let tokens = registry.login(provider, status).await.map_err(ui::report)?;
            let _ = printer.await;
            store
                .store(&credential_key, Credential::OAuth(tokens))
                .map_err(storage_error)?;
        }
        AuthPick::ApiKey => {
            let secret = if let Some(key) = key {
                key
            } else {
                let Some(secret) = ui::prompt_api_key(provider)? else {
                    return Ok(false);
                };
                secret
            };
            let Some(endpoint) = resolve_login_endpoint(endpoint, metadata.login_endpoint)? else {
                return Ok(false);
            };
            let credential = api_key_credential(secret, Some(endpoint), metadata)?;
            store
                .store(&credential_key, credential)
                .map_err(storage_error)?;
        }
    }

    let verb = if replacing { "updated" } else { "stored" };
    ui::success(&format!("{verb} credential for {provider} ({account})"));
    verify(store, user, provider, account).await;
    Ok(true)
}

fn resolve_login_endpoint(
    endpoint: Option<String>,
    login_endpoint: Option<LoginEndpointMetadata>,
) -> color_eyre::Result<Option<String>> {
    let Some(endpoint_metadata) = login_endpoint else {
        return Ok(Some(String::new()));
    };
    let endpoint = endpoint
        .or_else(|| {
            endpoint_metadata
                .env_var
                .and_then(|env_var| std::env::var(env_var).ok())
        })
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    let endpoint = match endpoint {
        Some(endpoint) => endpoint,
        None if std::io::stdin().is_terminal() && std::io::stderr().is_terminal() => {
            match ui::prompt_endpoint(endpoint_metadata.default)? {
                Some(endpoint) => endpoint,
                None => return Ok(None),
            }
        }
        None => endpoint_metadata
            .default
            .map(str::to_owned)
            .ok_or_else(|| {
                ui::report_hint(
                    "endpoint is required for this provider",
                    "pass --endpoint or set the provider env var",
                )
            })?,
    };
    if endpoint.is_empty() {
        return Err(ui::report_hint(
            "endpoint is required for this provider",
            "pass --endpoint or set the provider env var",
        ));
    }
    if let Some(validate) = endpoint_metadata.validate {
        Ok(Some(validate(&endpoint).map_err(ui::report)?))
    } else {
        Ok(Some(endpoint))
    }
}

fn api_key_credential(
    secret: String,
    endpoint: Option<String>,
    metadata: ProviderMetadata,
) -> color_eyre::Result<Credential> {
    let Some(endpoint_metadata) = metadata.login_endpoint else {
        return Ok(Credential::ApiKey(SecretString::from(secret)));
    };
    let endpoint = endpoint.filter(|value| !value.is_empty()).ok_or_else(|| {
        ui::report_hint(
            "endpoint is required for this provider",
            "pass --endpoint or set the provider env var",
        )
    })?;
    let endpoint = if let Some(validate) = endpoint_metadata.validate {
        validate(&endpoint).map_err(ui::report)?
    } else {
        endpoint
    };
    Ok(Credential::ApiKeyWithEndpoint {
        secret: SecretString::from(secret),
        endpoint,
    })
}

async fn verify(
    store: &CredentialStore,
    user: &goat_config::UserProviders,
    provider: &str,
    account: &str,
) {
    let registry = Registry::load(store, user, account);
    let Some(provider) = registry.get(&ProviderId::from(provider)) else {
        return;
    };
    let (tx, mut rx) = mpsc::channel(32);
    let handle = provider.discover(tx);
    let mut count = 0usize;
    let collect = async {
        while rx.recv().await.is_some() {
            count += 1;
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(5), collect).await;
    handle.abort();
    if count > 0 {
        ui::success(&format!("verified: {count} models"));
    } else if provider.verifies_credentials() {
        ui::warning("could not verify credential");
    }
}

const ACCOUNT_WIDTH: usize = 22;

fn provider_rows(
    store: &CredentialStore,
    user: &goat_config::UserProviders,
    login_only: bool,
) -> (Vec<String>, Vec<Vec<ui::Cell>>) {
    let registry = Registry::new(store, user);
    let stored = store.entries();
    let mut ids = Vec::new();
    let mut rows = Vec::new();
    for provider in registry.all() {
        let caps = provider.capabilities();
        if login_only && matches!(caps.auth, AuthMethod::None) {
            continue;
        }
        let id = provider.id().to_string();
        let accounts = provider_accounts(&stored, &id);
        let status = connection_status(caps.auth, provider.metadata().env_var, &accounts);
        rows.push(vec![
            (status.icon().to_owned(), status.palette()),
            (id.clone(), ui::Palette::Provider),
            (status.compact_label().to_owned(), status.palette()),
            (account_preview(&accounts), ui::Palette::Muted),
        ]);
        ids.push(id);
    }
    (ids, rows)
}

fn list_providers(store: &CredentialStore, user: &goat_config::UserProviders) {
    let (_, rows) = provider_rows(store, user, false);
    let mut table = ui::Table::new(["", "provider", "status", "account"]);
    for row in rows {
        table.styled_row(row);
    }
    println!();
    table.render();
}

fn provider_info(
    store: &CredentialStore,
    user: &goat_config::UserProviders,
    provider: &str,
) -> color_eyre::Result<()> {
    let registry = Registry::new(store, user);
    let target = registry
        .all()
        .iter()
        .find(|candidate| candidate.id().to_string() == provider)
        .ok_or_else(|| unknown_provider_error(provider, &registry))?;
    let stored = store.entries();
    let id = target.id().to_string();
    let caps = target.capabilities();
    let metadata = target.metadata();
    let accounts = provider_accounts(&stored, &id);
    let status = connection_status(caps.auth, metadata.env_var, &accounts);
    let color = ColorMode::detect();
    println!("{}", color.paint(&id, Palette::Provider));
    pair_styled("status", status.label(), status.palette());
    pair("auth", auth_label(caps.auth));
    pair("accounts", &provider_account_details(&accounts));
    pair("env", metadata.env_var.unwrap_or("-"));
    let custom_endpoint = user.load().get(&id).map(|config| config.endpoint.clone());
    pair(
        "endpoint",
        custom_endpoint
            .as_deref()
            .or(metadata.endpoint)
            .unwrap_or("fixed"),
    );
    pair("validation", metadata.validation);
    let oauth = metadata.oauth.unwrap_or(match caps.auth {
        AuthMethod::OAuth | AuthMethod::ApiKeyOrOAuth => "device code",
        AuthMethod::ApiKey | AuthMethod::None => "-",
    });
    pair("oauth", oauth);
    pair("models", &model_preview(&target.list_models()));
    println!();
    println!("{}", color.paint("setup", Palette::Muted));
    for line in provider_setup_lines(&id, caps.auth, metadata) {
        println!("  {}", color.paint(line, Palette::Value));
    }
    Ok(())
}

fn provider_accounts(
    stored: &[(CredentialKey, CredentialKind)],
    provider: &str,
) -> Vec<(String, CredentialKind)> {
    stored
        .iter()
        .filter(|(key, _)| key.service == CredentialService::Model && key.provider == provider)
        .map(|(key, kind)| (key.account.clone(), *kind))
        .collect()
}

fn account_preview(accounts: &[(String, CredentialKind)]) -> String {
    match accounts {
        [] => String::new(),
        [(account, _)] => truncate_label(account, ACCOUNT_WIDTH),
        [(first, _), (second, _)] => truncate_label(&format!("{first}, {second}"), ACCOUNT_WIDTH),
        [(first, _), (second, _), rest @ ..] => {
            truncate_label(&format!("{first}, {second} +{}", rest.len()), ACCOUNT_WIDTH)
        }
    }
}

fn provider_account_details(accounts: &[(String, CredentialKind)]) -> String {
    if accounts.is_empty() {
        return "none".to_owned();
    }
    accounts
        .iter()
        .map(|(account, kind)| format!("{account} ({})", credential_kind_label(*kind)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn truncate_label(label: &str, width: usize) -> String {
    truncate_to_width(label, width)
}

struct ConnectionStatus {
    kind: ConnectionKind,
    label: String,
}

impl ConnectionStatus {
    fn icon(&self) -> &'static str {
        match self.kind {
            ConnectionKind::Connected | ConnectionKind::Env => "●",
            ConnectionKind::Local => "◆",
            ConnectionKind::Disconnected => "○",
        }
    }

    fn palette(&self) -> Palette {
        match self.kind {
            ConnectionKind::Connected => Palette::Success,
            ConnectionKind::Env => Palette::Info,
            ConnectionKind::Local => Palette::Local,
            ConnectionKind::Disconnected => Palette::Warning,
        }
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn compact_label(&self) -> &str {
        match self.kind {
            ConnectionKind::Connected => "connected",
            ConnectionKind::Env => "env",
            ConnectionKind::Local => "local",
            ConnectionKind::Disconnected => "missing",
        }
    }
}

enum ConnectionKind {
    Connected,
    Env,
    Local,
    Disconnected,
}

fn connection_status(
    auth: AuthMethod,
    env_var: Option<&str>,
    accounts: &[(String, CredentialKind)],
) -> ConnectionStatus {
    if !accounts.is_empty() {
        let label = if accounts.len() == 1 {
            format!("connected: {}", accounts[0].0)
        } else {
            format!("connected: {} accounts", accounts.len())
        };
        return ConnectionStatus {
            kind: ConnectionKind::Connected,
            label,
        };
    }
    if let Some(var) = env_var
        && std::env::var(var).is_ok_and(|value| !value.is_empty())
    {
        return ConnectionStatus {
            kind: ConnectionKind::Env,
            label: format!("env: {var}"),
        };
    }
    if matches!(auth, AuthMethod::None) {
        ConnectionStatus {
            kind: ConnectionKind::Local,
            label: "local".to_owned(),
        }
    } else {
        ConnectionStatus {
            kind: ConnectionKind::Disconnected,
            label: "not connected".to_owned(),
        }
    }
}

fn provider_setup_lines(id: &str, auth: AuthMethod, metadata: ProviderMetadata) -> Vec<String> {
    if !metadata.setup.is_empty() {
        return metadata.setup.iter().map(ToString::to_string).collect();
    }
    match auth {
        AuthMethod::None => {
            vec!["No login required. Make sure the local server is running.".to_owned()]
        }
        AuthMethod::OAuth => vec![format!(
            "Run `goat provider login {id}` for device-code login."
        )],
        AuthMethod::ApiKey => vec![metadata.env_var.map_or_else(
            || format!("Run `goat provider login {id} --key sk-...`."),
            |var| format!("Set `{var}` or run `goat provider login {id} --key sk-...`."),
        )],
        AuthMethod::ApiKeyOrOAuth => vec![
            format!("Run `goat provider login {id}` for OAuth device-code login."),
            format!("Run `goat provider login {id} --key sk-...` to store an API key."),
        ],
    }
}

fn model_preview(models: &[String]) -> String {
    if models.is_empty() {
        return "discovered live".to_owned();
    }
    let shown = models
        .iter()
        .take(3)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    if models.len() > 3 {
        format!("{shown}, …")
    } else {
        shown
    }
}

fn credential_kind_label(kind: CredentialKind) -> &'static str {
    match kind {
        CredentialKind::ApiKey => "api key",
        CredentialKind::OAuth => "oauth",
    }
}

fn auth_label(auth: AuthMethod) -> &'static str {
    match auth {
        AuthMethod::None => "none",
        AuthMethod::ApiKey => "api key",
        AuthMethod::OAuth => "device code",
        AuthMethod::ApiKeyOrOAuth => "api key or device code",
    }
}

fn unknown_provider_error(provider: &str, registry: &Registry) -> color_eyre::Report {
    let mut ids: Vec<String> = registry.all().iter().map(|p| p.id().to_string()).collect();
    ids.sort();
    let suggestions = closest_provider_ids(provider, &ids);
    let message = if suggestions.is_empty() {
        format!("unknown provider: {provider}")
    } else {
        format!(
            "unknown provider: {provider} · did you mean {}",
            suggestions.join(", ")
        )
    };
    ui::report_hint(
        message,
        "run `goat provider list`, or `goat provider add <name> --endpoint <url>` for a custom endpoint",
    )
}

fn storage_error(err: impl std::fmt::Display) -> color_eyre::Report {
    ui::report_hint(
        format!("could not update credential store: {err}"),
        "check permissions on ~/.goat",
    )
}

fn closest_provider_ids(provider: &str, ids: &[String]) -> Vec<String> {
    ids.iter()
        .filter(|id| {
            id.contains(provider)
                || provider.contains(id.as_str())
                || id.chars().next() == provider.chars().next()
        })
        .take(3)
        .cloned()
        .collect()
}

fn logout(
    store: &CredentialStore,
    provider: &str,
    account: &str,
    service: CredentialService,
) -> color_eyre::Result<()> {
    let key = match service {
        CredentialService::Model => CredentialKey::model(provider, account),
        CredentialService::Search => CredentialKey::search(provider, account),
        CredentialService::Integration => CredentialKey::integration(provider, account),
        CredentialService::Channel => {
            return Err(color_eyre::eyre::eyre!(
                "channel secrets are scoped to one agent and one slot; remove them with `goat agent channel rm {provider}`"
            ));
        }
        CredentialService::Remote => {
            return Err(color_eyre::eyre::eyre!(
                "device key material belongs to a remote; remove it with `goat remote rm {provider}`"
            ));
        }
        CredentialService::Mcp => {
            return Err(color_eyre::eyre::eyre!(
                "MCP credentials are scoped to one server; remove them with `goat mcp logout {provider}`"
            ));
        }
    };
    if store.remove(&key).map_err(storage_error)? {
        ui::success(&format!("disconnected {provider} ({account})"));
    } else if service == CredentialService::Model {
        ui::warning(&format!("no stored account for {provider} ({account})"));
    } else {
        ui::warning(&format!("no credential found for {provider} ({account})"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn provider_list_output_discovers_providers() {
        let store = goat_auth::CredentialStore::new(
            std::env::temp_dir().join("goat-code-provider-list-test.json"),
        );
        let no_user = goat_config::UserProviders::at(
            std::env::temp_dir().join("goat-code-provider-no-user.json"),
        );
        let (ids, rows) = super::provider_rows(&store, &no_user, false);
        assert_eq!(ids.len(), rows.len());
        assert!(ids.iter().any(|id| id == "openrouter"));
        assert!(ids.iter().any(|id| id == "kimi-code"));
        assert!(ids.iter().any(|id| id == "zai-coding"));
    }

    #[test]
    fn provider_accounts_output_data_is_grouped() {
        let rows = super::provider_accounts(
            &[(
                goat_auth::CredentialKey::model("kimi-code", "default"),
                goat_auth::CredentialKind::OAuth,
            )],
            "kimi-code",
        );
        assert_eq!(
            rows,
            vec![("default".to_owned(), goat_auth::CredentialKind::OAuth)]
        );
    }

    #[test]
    fn provider_info_unknown_suggests_list() {
        let store = goat_auth::CredentialStore::new(
            std::env::temp_dir().join("goat-code-provider-info-test.json"),
        );
        let no_user = goat_config::UserProviders::at(
            std::env::temp_dir().join("goat-code-provider-no-user.json"),
        );
        let error = super::provider_info(&store, &no_user, "kim-code")
            .unwrap_err()
            .to_string();
        assert!(error.contains("goat provider list"));
    }

    #[test]
    fn unknown_provider_suggests_list() {
        let store = goat_auth::CredentialStore::new(
            std::env::temp_dir().join("goat-code-provider-unknown-test.json"),
        );
        let no_user = goat_config::UserProviders::at(
            std::env::temp_dir().join("goat-code-provider-no-user.json"),
        );
        let registry = goat_providers::Registry::new(&store, &no_user);
        let error = super::unknown_provider_error("openruter", &registry).to_string();
        assert!(error.contains("goat provider list"));
    }
}
