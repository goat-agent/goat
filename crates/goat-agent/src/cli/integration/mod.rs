pub mod usage;

use std::io::{IsTerminal, Read};

use anyhow::{Result, anyhow};
use clap::{Args, Subcommand};
use goat_api::{
    AdminIntegrationConnectOutput, AdminIntegrationConnectParams, AdminIntegrationStatusOutput,
    IntegrationClient, IntegrationConnectionStatus, IntegrationRemoveScope, IntegrationState,
};
use goat_auth::{Credential, CredentialStore, CredentialValue, SecretString};
use goat_integration::{
    CLIENT_ID_SLOT, CLIENT_SECRET_SLOT, Connection, Integration, IntegrationAuth,
    IntegrationBinding, IntegrationMetadata, OAuthClient,
};
use serde_json::{Map, Value, json};

use super::ui::{self, Footer, Palette, Settled, Table};

#[derive(Args, Debug)]
pub struct IntegrationArgs {
    #[command(subcommand)]
    pub command: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    #[command(
        visible_alias = "ls",
        about = "List every integration and its connections",
        after_help = "Run `goat integration info <integration>` for what one offers and how to set it up."
    )]
    List,
    #[command(about = "Show what an integration offers and how its connections are set up")]
    Info {
        #[arg(help = "Integration (e.g. `linear`) or connection name")]
        name: String,
    },
    #[command(
        about = "Connect an integration",
        after_help = "Examples:
  goat integration add linear
  goat integration add linear --name linear-work
  goat integration add slack --key-stdin < token.txt
  goat integration add datadog --host https://api.datadoghq.eu"
    )]
    Add {
        #[arg(help = "Integration to connect; prompted if omitted")]
        kind: Option<String>,
        #[arg(
            long,
            short,
            help = "Connection name; defaults to the integration, e.g. `linear`"
        )]
        name: Option<String>,
        #[command(flatten)]
        credential: CredentialArgs,
    },
    #[command(about = "Log a connection in again, keeping its settings")]
    Login {
        #[arg(help = "Connection name")]
        name: String,
        #[command(flatten)]
        credential: CredentialArgs,
    },
    #[command(about = "Forget a connection's credential, keeping its settings")]
    Logout {
        #[arg(help = "Connection name")]
        name: String,
        #[arg(long, short, help = "Skip the confirmation")]
        yes: bool,
    },
    #[command(visible_alias = "rm", about = "Delete a connection and its credential")]
    Remove {
        #[arg(help = "Connection name")]
        name: String,
        #[arg(long, short, help = "Skip the confirmation")]
        yes: bool,
    },
    #[command(about = "Check connections against their services")]
    Verify {
        #[arg(help = "Connection name; every connection when omitted")]
        name: Option<String>,
    },
}

#[derive(Args, Debug, Default)]
pub struct CredentialArgs {
    #[arg(
        long,
        conflicts_with = "key_stdin",
        help = "API key or token; skips the browser for OAuth integrations"
    )]
    key: Option<String>,
    #[arg(long, help = "Read the API key or token from stdin")]
    key_stdin: bool,
    #[arg(
        long,
        value_name = "URL",
        help = "Base URL of a self-hosted or regional instance"
    )]
    host: Option<String>,
    #[arg(long, help = "OAuth client id, for integrations that need their own")]
    client_id: Option<String>,
    #[arg(long, requires = "client_id", help = "OAuth client secret")]
    client_secret: Option<String>,
    #[arg(long, help = "Store without checking against the service")]
    no_verify: bool,
}

pub async fn run(args: IntegrationArgs) -> Result<()> {
    match args.command.unwrap_or(Cmd::List) {
        Cmd::List => list().await,
        Cmd::Info { name } => info(&name).await,
        Cmd::Add {
            kind,
            name,
            credential,
        } => applied(add(kind, name, credential).await?).await,
        Cmd::Login { name, credential } => applied(login(&name, credential).await?).await,
        Cmd::Logout { name, yes } => {
            applied(remove(&name, IntegrationRemoveScope::Credential, yes).await?).await
        }
        Cmd::Remove { name, yes } => {
            applied(remove(&name, IntegrationRemoveScope::Connection, yes).await?).await
        }
        Cmd::Verify { name } => verify(name).await,
    }
}

pub async fn setup(agent: Option<&str>) -> Result<()> {
    let mut question = "Connect a service like Linear or Sentry now?";
    while ui::confirm(question, false)? {
        question = "Connect another service?";
        let kind = pick_kind(None)?;
        let Ok(Settled::Done) = add(Some(kind.clone()), None, CredentialArgs::default()).await
        else {
            continue;
        };
        if let Some(slug) = agent
            && ui::confirm(&format!("Let {slug} use {kind}?"), true)?
        {
            let _ = usage::run_agent(usage::AgentCmd::Add {
                name: Some(kind),
                agent: Some(slug.to_owned()),
                set: Vec::new(),
            })
            .await;
        }
    }
    super::apply::config_changed(None).await;
    Ok(())
}

async fn applied(settled: Settled) -> Result<()> {
    if settled == Settled::Done {
        super::apply::config_changed(None).await;
    }
    Ok(())
}

async fn list() -> Result<()> {
    ui::cell_async("Integrations", || async move {
        let status = status(None, false).await?;
        let mut rows: Vec<(IntegrationMetadata, Vec<&IntegrationConnectionStatus>)> = catalog()
            .into_iter()
            .map(|metadata| {
                let connections = status
                    .connections
                    .iter()
                    .filter(|c| c.kind == metadata.id)
                    .collect();
                (metadata, connections)
            })
            .collect();
        rows.sort_by_key(|(metadata, connections)| (connections.is_empty(), metadata.id));
        let mut table = Table::new(["", "integration", "connections", "offers", "summary"]);
        for (metadata, connections) in &rows {
            let (icon, palette) = kind_badge(connections);
            table.styled_row(vec![
                (icon.to_owned(), palette),
                (metadata.id.to_owned(), Palette::Provider),
                (connection_names(connections), palette),
                (offers(metadata), Palette::Muted),
                (metadata.summary.to_owned(), Palette::Muted),
            ]);
        }
        table.render();
        invalid_lines(&status);
        Ok(Footer::Hint(
            "",
            "goat integration add <integration>".into(),
        ))
    })
    .await?;
    Ok(())
}

async fn info(name: &str) -> Result<()> {
    ui::cell_async(&format!("Integration {name}"), || async move {
        let status = status(None, false).await?;
        let kind = status
            .connections
            .iter()
            .find(|c| c.name == name)
            .map_or_else(|| name.to_owned(), |c| c.kind.clone());
        let (integration, metadata) = instantiate(&kind)?;
        ui::pair("name", metadata.display);
        ui::pair("summary", metadata.summary);
        ui::pair("auth", auth_label(&metadata));
        if let Some(var) = metadata.env_var {
            ui::pair(
                "env",
                &format!("{var} (for the `{}` connection)", metadata.id),
            );
        }
        ui::pair("offers", &offers(&metadata));
        ui::blank();
        for line in metadata.setup.lines() {
            ui::line(&ui::dim(line.trim()));
        }
        keys_section(
            "connection settings  (goat integration add --host …)",
            metadata.connection_keys.iter().map(|k| (k.name, k.about)),
        );
        let common: &[(&str, &str)] = if metadata.tools {
            &[
                ("deny_prefixes", "hide tools whose names start with these"),
                ("deny_suffixes", "hide tools whose names end with these"),
            ]
        } else {
            &[]
        };
        keys_section(
            "usage settings  (goat agent integration set … --set key=value)",
            metadata
                .binding_keys
                .iter()
                .map(|k| (k.name, k.about))
                .chain(common.iter().copied()),
        );
        let defaults = integration.default_watch(&IntegrationBinding::from_config(json!({})));
        if !defaults.is_empty() {
            ui::blank();
            ui::section("default watch");
            for spec in defaults {
                ui::pair(&spec.stream, &spec.query);
            }
        }
        let connections: Vec<&IntegrationConnectionStatus> = status
            .connections
            .iter()
            .filter(|c| c.kind == metadata.id)
            .collect();
        ui::blank();
        if connections.is_empty() {
            ui::line(&ui::dim("not connected"));
            return Ok(Footer::Hint(
                "",
                format!("goat integration add {}", metadata.id),
            ));
        }
        connection_table(&connections);
        Ok(Footer::None)
    })
    .await?;
    Ok(())
}

async fn add(
    kind: Option<String>,
    name: Option<String>,
    credential: CredentialArgs,
) -> Result<Settled> {
    ui::cell_async("Integration Add", || async move {
        let kind = pick_kind(kind)?;
        let status = status(None, false).await?;
        let Some(name) = connection_name(&kind, name, &status)? else {
            return Ok(Footer::Cancel);
        };
        let existing = status.connections.iter().find(|c| c.name == name);
        connect(&kind, &name, existing, credential).await
    })
    .await
}

async fn login(name: &str, credential: CredentialArgs) -> Result<Settled> {
    ui::cell_async("Integration Login", || async move {
        let status = status(Some(name), false).await?;
        let existing = status
            .connections
            .first()
            .ok_or_else(|| missing_connection(name))?;
        connect(&existing.kind.clone(), name, Some(existing), credential).await
    })
    .await
}

async fn connect(
    kind: &str,
    name: &str,
    existing: Option<&IntegrationConnectionStatus>,
    args: CredentialArgs,
) -> Result<Footer> {
    let (integration, metadata) = instantiate(kind)?;
    ui::pair("connection", name);
    ui::blank();
    for line in metadata.setup.lines() {
        ui::line(&ui::dim(line.trim()));
    }
    ui::blank();

    let mut patch = Map::new();
    if let Some(host) = args.host.as_deref() {
        if !metadata.connection_keys.iter().any(|k| k.name == "host") {
            return Err(anyhow!("{} has no host setting", metadata.display));
        }
        patch.insert("host".to_owned(), Value::String(host.trim().to_owned()));
    }
    let mut config = existing
        .and_then(|c| c.config.as_object().cloned())
        .unwrap_or_default();
    config.extend(patch.clone());
    let connection = Connection::new(name, kind, Value::Object(config));

    let supplied_key = supplied_key(&args)?;
    let mut client = None;
    let credential = match (metadata.auth, supplied_key) {
        (IntegrationAuth::External, _) => None,
        (_, Some(key)) => Some(api_key(&key)),
        (IntegrationAuth::Secret, None) => {
            let Some(key) = ui::secret(metadata.secret_label)? else {
                return Ok(Footer::Cancel);
            };
            Some(api_key(&key))
        }
        (IntegrationAuth::OAuth, None) => {
            let supplied = oauth_client(&metadata, &connection, &args)?;
            let stored = stored_client(&connection);
            let login_client = supplied.clone().or(stored);
            let tokens = integration
                .oauth_login(&connection, login_client.as_ref(), &|url: &str| {
                    ui::pair("approve in browser", url);
                    let _ = open::that(url);
                })
                .await?;
            client = supplied.map(|c| IntegrationClient {
                id: c.id,
                secret: c.secret,
            });
            Some(CredentialValue::from(Credential::OAuth(tokens)))
        }
    };

    let outcome = goat_client::connect_integration(
        &link()?,
        AdminIntegrationConnectParams {
            name: name.to_owned(),
            kind: kind.to_owned(),
            config: Value::Object(patch),
            credential,
            client,
            verify: !args.no_verify,
        },
    )
    .await
    .map_err(daemon_error)?;
    match outcome {
        AdminIntegrationConnectOutput::Verified { identity } => ui::pair("connected", &identity),
        AdminIntegrationConnectOutput::Unverified { message } => {
            ui::pair_styled(
                "stored",
                &format!("not verified: {message}"),
                Palette::Warning,
            );
        }
    }
    Ok(Footer::Hint(
        "Connected",
        format!("goat agent integration add {name}"),
    ))
}

async fn remove(name: &str, scope: IntegrationRemoveScope, yes: bool) -> Result<Settled> {
    let title = match scope {
        IntegrationRemoveScope::Credential => "Integration Logout",
        IntegrationRemoveScope::Connection => "Integration Remove",
    };
    ui::cell_async(title, || async move {
        let status = status(Some(name), false).await?;
        let connection = status
            .connections
            .first()
            .ok_or_else(|| missing_connection(name))?;
        if !connection.used_by.is_empty() {
            ui::pair_styled("used by", &connection.used_by.join(", "), Palette::Warning);
        }
        let question = match scope {
            IntegrationRemoveScope::Credential => format!("log out of {name}?"),
            IntegrationRemoveScope::Connection => format!("delete the {name} connection?"),
        };
        if !confirmed(&question, yes)? {
            return Ok(Footer::Cancel);
        }
        goat_client::remove_integration(&link()?, name.to_owned(), scope)
            .await
            .map_err(daemon_error)?;
        Ok(match scope {
            IntegrationRemoveScope::Credential => {
                Footer::Hint("Logged out", format!("goat integration login {name}"))
            }
            IntegrationRemoveScope::Connection => Footer::Ok("Removed"),
        })
    })
    .await
}

async fn verify(name: Option<String>) -> Result<()> {
    ui::cell_async("Integration Verify", || async move {
        let status = status(name.as_deref(), true).await?;
        if let Some(name) = &name
            && status.connections.is_empty()
        {
            return Err(missing_connection(name));
        }
        if status.connections.is_empty() {
            ui::line(&ui::dim("no connections yet"));
            return Ok(Footer::Hint("", "goat integration add".into()));
        }
        let refs: Vec<&IntegrationConnectionStatus> = status.connections.iter().collect();
        connection_table(&refs);
        invalid_lines(&status);
        let failed = status
            .connections
            .iter()
            .filter(|c| !matches!(c.state, IntegrationState::Ready { .. }))
            .count();
        if failed > 0 || !status.invalid.is_empty() {
            return Err(anyhow!(
                "{} connection(s) need attention",
                failed + status.invalid.len()
            ));
        }
        Ok(Footer::Ok("All connections work"))
    })
    .await?;
    Ok(())
}

fn connection_name(
    kind: &str,
    explicit: Option<String>,
    status: &AdminIntegrationStatusOutput,
) -> Result<Option<String>> {
    if let Some(name) = explicit {
        let name = name.trim().to_owned();
        goat_integration::connection::validate_name(&name)?;
        if let Some(other) = status.connections.iter().find(|c| c.name == name)
            && other.kind != kind
        {
            return Err(anyhow!("`{name}` is already a {} connection", other.kind));
        }
        return Ok(Some(name));
    }
    let same_kind: Vec<&IntegrationConnectionStatus> = status
        .connections
        .iter()
        .filter(|c| c.kind == kind)
        .collect();
    let fresh = next_name(kind, status);
    if same_kind.is_empty() {
        return Ok(Some(fresh));
    }
    if !interactive() {
        return Err(ui::report_hint(
            format!("{kind} is already connected"),
            format!(
                "pass --name {fresh} to add another, or run `goat integration login {}`",
                same_kind[0].name
            ),
        ));
    }
    let mut choices: Vec<(Option<String>, String)> = same_kind
        .iter()
        .map(|c| (Some(c.name.clone()), format!("reconnect {}", c.name)))
        .collect();
    choices.push((None, format!("add another {kind} connection")));
    if let Some(existing) = ui::pick("connection", &choices)? {
        return Ok(Some(existing));
    }
    let Some(name) = ui::prompt("name", Some(&fresh))? else {
        return Ok(None);
    };
    let name = name.trim().to_owned();
    goat_integration::connection::validate_name(&name)?;
    if status.connections.iter().any(|c| c.name == name) {
        return Err(anyhow!("`{name}` already exists"));
    }
    Ok(Some(name))
}

fn next_name(kind: &str, status: &AdminIntegrationStatusOutput) -> String {
    let taken = |name: &str| status.connections.iter().any(|c| c.name == name);
    if !taken(kind) {
        return kind.to_owned();
    }
    (2..=status.connections.len() + 2)
        .map(|n| format!("{kind}-{n}"))
        .find(|name| !taken(name))
        .unwrap_or_else(|| kind.to_owned())
}

fn supplied_key(args: &CredentialArgs) -> Result<Option<String>> {
    if args.key_stdin {
        let mut raw = String::new();
        std::io::stdin().read_to_string(&mut raw)?;
        let key = raw.trim().to_owned();
        if key.is_empty() {
            return Err(anyhow!("stdin was empty"));
        }
        return Ok(Some(key));
    }
    Ok(args
        .key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_owned))
}

fn api_key(key: &str) -> CredentialValue {
    CredentialValue::from(Credential::ApiKey(SecretString::from(key)))
}

fn oauth_client(
    metadata: &IntegrationMetadata,
    connection: &Connection,
    args: &CredentialArgs,
) -> Result<Option<OAuthClient>> {
    if let Some(id) = &args.client_id {
        return Ok(Some(OAuthClient {
            id: id.trim().to_owned(),
            secret: args.client_secret.clone(),
        }));
    }
    if !metadata.preregistered || stored_client(connection).is_some() {
        return Ok(None);
    }
    let Some(id) = ui::prompt(&format!("{} OAuth client id", metadata.display), None)? else {
        return Err(anyhow!("cancelled"));
    };
    let secret = ui::secret("OAuth client secret (blank if none)")?
        .filter(|secret| !secret.trim().is_empty());
    Ok(Some(OAuthClient {
        id: id.trim().to_owned(),
        secret,
    }))
}

fn stored_client(connection: &Connection) -> Option<OAuthClient> {
    let store = CredentialStore::new(goat_config::auth_path()?);
    let read = |slot: &str| match store.get(&connection.slot_key(slot)) {
        Some(Credential::ApiKey(secret)) => Some(secret.expose().to_owned()),
        _ => None,
    };
    Some(OAuthClient {
        id: read(CLIENT_ID_SLOT)?,
        secret: read(CLIENT_SECRET_SLOT),
    })
}

fn confirmed(question: &str, yes: bool) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    if !interactive() {
        return Err(anyhow!("pass --yes to confirm without a terminal"));
    }
    Ok(ui::confirm(question, false)?)
}

fn interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

pub(crate) fn link() -> Result<goat_client::Link> {
    let socket = goat_config::socket_path().ok_or_else(|| anyhow!(goat_config::HOME_NOT_FOUND))?;
    Ok(goat_client::Link::local(socket, std::env::current_exe()?))
}

pub(crate) async fn status(
    name: Option<&str>,
    verify: bool,
) -> Result<AdminIntegrationStatusOutput> {
    goat_client::integration_status(&link()?, name.map(str::to_owned), verify)
        .await
        .map_err(daemon_error)
}

fn daemon_error(error: goat_client::ClientError) -> anyhow::Error {
    match error {
        goat_client::ClientError::Refused(message) => anyhow!(message),
        other => anyhow!("could not reach the daemon: {other}"),
    }
}

pub(crate) fn catalog() -> Vec<IntegrationMetadata> {
    let mut all: Vec<IntegrationMetadata> = goat_integration::factories()
        .into_iter()
        .map(|factory| (factory.ctor)().metadata())
        .collect();
    all.sort_by_key(|metadata| metadata.id);
    all
}

fn instantiate(kind: &str) -> Result<(std::sync::Arc<dyn Integration>, IntegrationMetadata)> {
    let factory = goat_integration::factory_for(kind).ok_or_else(|| unknown_kind(kind))?;
    let integration = (factory.ctor)();
    let metadata = integration.metadata();
    Ok((integration, metadata))
}

fn pick_kind(kind: Option<String>) -> Result<String> {
    if let Some(kind) = kind {
        let kind = kind.trim().to_owned();
        return goat_integration::factory_for(&kind)
            .map(|_| kind.clone())
            .ok_or_else(|| unknown_kind(&kind));
    }
    let items: Vec<(String, String)> = catalog()
        .into_iter()
        .map(|m| {
            (
                m.id.to_owned(),
                format!("{} — {} · {}", m.id, m.display, m.summary),
            )
        })
        .collect();
    Ok(ui::pick("integration", &items)?)
}

fn unknown_kind(kind: &str) -> anyhow::Error {
    let known: Vec<&str> = catalog().iter().map(|m| m.id).collect();
    let close: Vec<&str> = known
        .iter()
        .copied()
        .filter(|id| id.starts_with(kind.get(..2).unwrap_or(kind)) || id.contains(kind))
        .collect();
    let hint = if close.is_empty() {
        format!("known integrations: {}", known.join(", "))
    } else {
        format!("did you mean {}?", close.join(", "))
    };
    ui::report_hint(format!("unknown integration `{kind}`"), hint)
}

pub(crate) fn missing_connection(name: &str) -> anyhow::Error {
    ui::report_hint(
        format!("no connection named `{name}`"),
        "run `goat integration list` to see connections",
    )
}

fn auth_label(metadata: &IntegrationMetadata) -> &'static str {
    match metadata.auth {
        IntegrationAuth::OAuth if metadata.preregistered => {
            "OAuth in the browser, with your own OAuth client"
        }
        IntegrationAuth::OAuth => "OAuth in the browser (or --key with a token)",
        IntegrationAuth::Secret => "API key or token",
        IntegrationAuth::External => "a host tool holds the credential",
    }
}

fn offers(metadata: &IntegrationMetadata) -> String {
    let watch = (factory_watch(metadata.id)).then_some("watch");
    let tools = metadata.tools.then_some("tools");
    [tools, watch]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
}

fn factory_watch(kind: &str) -> bool {
    goat_integration::factory_for(kind).is_some_and(|f| (f.ctor)().watch_vocabulary().is_some())
}

fn kind_badge(connections: &[&IntegrationConnectionStatus]) -> (&'static str, Palette) {
    if connections.is_empty() {
        ("○", Palette::Muted)
    } else if connections
        .iter()
        .all(|c| matches!(c.state, IntegrationState::Ready { .. }))
    {
        ("●", Palette::Success)
    } else {
        ("●", Palette::Warning)
    }
}

fn connection_names(connections: &[&IntegrationConnectionStatus]) -> String {
    connections
        .iter()
        .map(|c| match c.state {
            IntegrationState::Ready { .. } => c.name.clone(),
            _ => format!("{} (login)", c.name),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn state_cell(state: &IntegrationState) -> (String, Palette) {
    match state {
        IntegrationState::Ready { source } if source == "stored" => {
            ("ready".to_owned(), Palette::Success)
        }
        IntegrationState::Ready { source } => (format!("ready ({source})"), Palette::Success),
        IntegrationState::NeedsLogin { reason } => {
            (format!("needs login: {reason}"), Palette::Warning)
        }
        IntegrationState::Failed { message } => (format!("failed: {message}"), Palette::Warning),
    }
}

fn connection_table(connections: &[&IntegrationConnectionStatus]) {
    let mut table = Table::new(["connection", "state", "identity", "used by"]);
    for connection in connections {
        let (state, palette) = state_cell(&connection.state);
        table.styled_row(vec![
            (connection.name.clone(), Palette::Provider),
            (state, palette),
            (
                connection.identity.clone().unwrap_or_default(),
                Palette::Value,
            ),
            (connection.used_by.join(", "), Palette::Muted),
        ]);
    }
    table.render();
}

fn keys_section<'a>(title: &str, keys: impl Iterator<Item = (&'a str, &'a str)>) {
    let keys: Vec<(&str, &str)> = keys.collect();
    if keys.is_empty() {
        return;
    }
    ui::blank();
    ui::section(title);
    let mut table = Table::new(["key", "meaning"]);
    for (name, about) in keys {
        table.styled_row(vec![
            (name.to_owned(), Palette::Value),
            (about.to_owned(), Palette::Muted),
        ]);
    }
    table.render();
}

fn invalid_lines(status: &AdminIntegrationStatusOutput) {
    for invalid in &status.invalid {
        ui::warning(&format!(
            "  invalid connection `{}`: {}",
            invalid.name, invalid.reason
        ));
    }
}
