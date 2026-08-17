use std::time::Duration;

use anyhow::{Result, anyhow};
use clap::Subcommand;
use goat_auth::{Credential, CredentialKey, CredentialService, CredentialStore, SecretString};
use goat_config::{Config, GoatPaths};
use goat_integration::{Integration, IntegrationAuth, IntegrationFactory};
use serde_json::json;

use super::agent::{remove_section_config, resolve_agent, section_contains, section_entries};
use super::ui::{self, Footer, Palette, Table};

const VERIFY_TIMEOUT: Duration = Duration::from_secs(10);
const SECTION: &str = "integrations";
const DEFAULT_ACCOUNT: &str = "default";

#[derive(Subcommand, Debug)]
pub enum ConnectCmd {
    #[command(
        visible_alias = "new",
        about = "Connect an integration to goat (OAuth or key; shared by every agent)."
    )]
    Add {
        #[arg(help = "Integration kind (e.g. `linear`); prompted if omitted.")]
        kind: Option<String>,
    },
    #[command(visible_alias = "ls", about = "List connected integrations.")]
    List,
    #[command(
        visible_alias = "rm",
        aliases = ["del", "delete"],
        about = "Disconnect an integration (removes the stored credential)."
    )]
    Remove { kind: String },
}

pub async fn run_connect(cmd: ConnectCmd) -> Result<()> {
    let paths = GoatPaths::default_layout()?;
    match cmd {
        ConnectCmd::Add { kind } => {
            connect_add(&paths, kind).await?;
            super::apply::config_changed(None).await;
            Ok(())
        }
        ConnectCmd::List => connect_list(&paths),
        ConnectCmd::Remove { kind } => {
            connect_remove(&paths, &kind).await?;
            super::apply::config_changed(None).await;
            Ok(())
        }
    }
}

async fn connect_add(paths: &GoatPaths, kind: Option<String>) -> Result<()> {
    ui::cell_async("Integration Connect", || async move {
        let kind = pick_kind(kind)?;
        let identity = connect_flow(paths, &kind).await?;
        ui::pair("connected", &identity);
        Ok(Footer::Hint(
            "Connected",
            "goat agent integration add".into(),
        ))
    })
    .await
}

fn connect_list(paths: &GoatPaths) -> Result<()> {
    ui::cell("Integration Connections", || {
        let store = CredentialStore::new(paths.credentials_json.clone());
        let config = Config::load();
        let mut table = Table::new(["kind", "auth"]);
        let mut rows = 0;
        for (key, cred_kind) in store.entries() {
            if key.service != CredentialService::Integration {
                continue;
            }
            let label = display_name(&key.provider).unwrap_or_else(|| key.provider.clone());
            table.styled_row(vec![
                (label, Palette::Plain),
                (format!("{cred_kind:?}").to_lowercase(), Palette::Success),
            ]);
            rows += 1;
        }
        for factory in goat_integration::factories() {
            let kind = factory.id.as_str();
            if auth_kind(kind) != Some(IntegrationAuth::External)
                || !config.integrations.contains_key(kind)
            {
                continue;
            }
            table.styled_row(vec![
                (
                    display_name(kind).unwrap_or_else(|| kind.to_string()),
                    Palette::Plain,
                ),
                ("external".to_string(), Palette::Success),
            ]);
            rows += 1;
        }
        if rows == 0 {
            ui::line(&ui::dim("none yet"));
            return Ok(Footer::Hint("none", "goat integration add".into()));
        }
        table.render();
        Ok(Footer::None)
    })
}

async fn connect_remove(paths: &GoatPaths, kind: &str) -> Result<()> {
    ui::cell_async("Integration Disconnect", || async move {
        let kind = kind.trim();
        let store = CredentialStore::new(paths.credentials_json.clone());
        let config = Config::load();
        if !is_connected(kind, &store, &config) {
            return Err(anyhow!("no connection for `{kind}`"));
        }
        if !ui::confirm(&format!("disconnect {kind} for every agent?"), false)? {
            return Ok(Footer::Cancel);
        }
        store.remove(&CredentialKey::integration(kind, DEFAULT_ACCOUNT))?;
        for slot in [
            goat_integration_mcp::CLIENT_ID_SLOT,
            goat_integration_mcp::CLIENT_SECRET_SLOT,
        ] {
            let key = CredentialKey::integration_slot(kind, DEFAULT_ACCOUNT, slot);
            if store.get(&key).is_some() {
                store.remove(&key)?;
            }
        }
        if config.integrations.contains_key(kind) {
            write_config(vec![goat_api::ConfigEdit::IntegrationRemove {
                kind: kind.to_string(),
            }])
            .await?;
        }
        ui::line(&ui::dim(
            "agent bindings are kept; remove them with `goat agent integration rm`",
        ));
        Ok(Footer::Ok("Disconnected"))
    })
    .await
}

fn store_oauth_client(store: &CredentialStore, kind: &str) -> Result<()> {
    let id_key = CredentialKey::integration_slot(
        kind,
        DEFAULT_ACCOUNT,
        goat_integration_mcp::CLIENT_ID_SLOT,
    );
    if store.get(&id_key).is_some() {
        return Ok(());
    }
    let Some(client_id) = ui::secret(&format!("{kind} OAuth client id"))? else {
        return Err(anyhow!("cancelled"));
    };
    store.store(
        &id_key,
        Credential::ApiKey(SecretString::from(client_id.as_str())),
    )?;
    let secret = ui::secret(&format!("{kind} OAuth client secret (blank if none)"))?;
    if let Some(secret) = secret.filter(|s| !s.trim().is_empty()) {
        store.store(
            &CredentialKey::integration_slot(
                kind,
                DEFAULT_ACCOUNT,
                goat_integration_mcp::CLIENT_SECRET_SLOT,
            ),
            Credential::ApiKey(SecretString::from(secret.as_str())),
        )?;
    }
    Ok(())
}

async fn connect_flow(paths: &GoatPaths, kind: &str) -> Result<String> {
    let (integration, metadata) = instantiate(kind)?;
    ui::line(&ui::dim(metadata.setup));
    let store = CredentialStore::new(paths.credentials_json.clone());
    match metadata.auth {
        IntegrationAuth::Secret => {
            let Some(secret) = ui::secret(metadata.secret_label)? else {
                return Err(anyhow!("cancelled"));
            };
            store.store(
                &CredentialKey::integration(kind, DEFAULT_ACCOUNT),
                Credential::ApiKey(SecretString::from(secret.as_str())),
            )?;
        }
        IntegrationAuth::OAuth => {
            if metadata.preregistered {
                store_oauth_client(&store, kind)?;
            }
            let patch = integration
                .oauth_login(&store, DEFAULT_ACCOUNT, &|url: &str| {
                    ui::pair("approve in browser", url);
                    let _ = open::that(url);
                })
                .await?;
            let mut entry = Config::load()
                .integrations
                .get(kind)
                .cloned()
                .unwrap_or_else(|| json!({}));
            if let (Some(entry), Some(patch)) = (entry.as_object_mut(), patch.as_object()) {
                entry.extend(patch.clone());
            }
            write_config(vec![goat_api::ConfigEdit::IntegrationSet {
                kind: kind.to_string(),
                config: entry,
            }])
            .await?;
        }
        IntegrationAuth::External => {}
    }
    let identity = verify_connection(&integration, kind, &store).await?;
    if metadata.auth == IntegrationAuth::External && !Config::load().integrations.contains_key(kind)
    {
        write_config(vec![goat_api::ConfigEdit::IntegrationSet {
            kind: kind.to_string(),
            config: json!({}),
        }])
        .await?;
    }
    Ok(identity)
}

fn auth_kind(kind: &str) -> Option<IntegrationAuth> {
    integration_factory(kind).map(|factory| (factory.ctor)().metadata().auth)
}

fn is_connected(kind: &str, store: &CredentialStore, config: &Config) -> bool {
    if auth_kind(kind) == Some(IntegrationAuth::External) {
        return config.integrations.contains_key(kind);
    }
    store
        .get(&CredentialKey::integration(kind, DEFAULT_ACCOUNT))
        .is_some()
}

async fn verify_connection(
    integration: &std::sync::Arc<dyn Integration>,
    kind: &str,
    store: &CredentialStore,
) -> Result<String> {
    let connection = Config::load()
        .integrations
        .get(kind)
        .cloned()
        .unwrap_or_else(|| json!({}));
    match tokio::time::timeout(VERIFY_TIMEOUT, integration.verify(&connection, store)).await {
        Ok(Ok(identity)) => Ok(identity),
        Ok(Err(e)) => Err(anyhow!("{e}")),
        Err(_) => Err(anyhow!(
            "verification timed out after {}s",
            VERIFY_TIMEOUT.as_secs()
        )),
    }
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    #[command(
        visible_alias = "new",
        about = "Bind an integration to an agent (connects it first if needed)."
    )]
    Add {
        #[arg(help = "Integration kind (e.g. `linear`); prompted if omitted.")]
        kind: Option<String>,
        #[arg(
            short = 'a',
            long = "agent",
            help = "Target agent; resolved automatically when omitted."
        )]
        agent: Option<String>,
        #[arg(long, help = "Skip the live connection check.")]
        no_verify: bool,
    },
    #[command(visible_alias = "ls", about = "List an agent's integration bindings.")]
    List {
        #[arg(short = 'a', long = "agent")]
        agent: Option<String>,
    },
    #[command(
        visible_alias = "rm",
        aliases = ["del", "delete"],
        about = "Remove an integration binding (the connection stays)."
    )]
    Remove {
        kind: String,
        #[arg(short = 'a', long = "agent")]
        agent: Option<String>,
    },
}

pub async fn run(cmd: Cmd) -> Result<()> {
    let paths = GoatPaths::default_layout()?;
    match cmd {
        Cmd::Add {
            kind,
            agent,
            no_verify,
        } => {
            let slug = agent.clone();
            bind_add(&paths, kind, agent, no_verify).await?;
            super::apply::config_changed(slug.as_deref()).await;
            Ok(())
        }
        Cmd::List { agent } => bind_list(&paths, agent.as_deref()),
        Cmd::Remove { kind, agent } => {
            bind_remove(&paths, &kind, agent.as_deref())?;
            super::apply::config_changed(agent.as_deref()).await;
            Ok(())
        }
    }
}

async fn bind_add(
    paths: &GoatPaths,
    kind: Option<String>,
    agent: Option<String>,
    no_verify: bool,
) -> Result<()> {
    ui::cell_async("Integration Add", || async move {
        let slug = resolve_agent(paths, agent.as_deref())?;
        ui::pair("agent", &slug);
        let dir = paths.agents_dir.join(&slug);
        let kind = pick_kind(kind)?;

        if section_contains(&dir, SECTION, &kind)? {
            ui::line(&ui::dim("already bound"));
            return Ok(Footer::None);
        }

        let store = CredentialStore::new(paths.credentials_json.clone());
        let connected = is_connected(&kind, &store, &Config::load());
        if connected {
            if !no_verify {
                let (integration, _) = instantiate(&kind)?;
                let identity = verify_connection(&integration, &kind, &store).await?;
                ui::pair("verified", &identity);
            }
        } else {
            let identity = connect_flow(paths, &kind).await?;
            ui::pair("connected", &identity);
        }

        super::agent::upsert_section_config(&dir, SECTION, &kind, json!({}))?;
        ui::pair("file", &dir.join("config.json").display().to_string());
        Ok(Footer::Ok("Bound"))
    })
    .await
}

fn bind_list(paths: &GoatPaths, agent: Option<&str>) -> Result<()> {
    ui::cell("Integrations", || {
        let slug = resolve_agent(paths, agent)?;
        ui::pair("agent", &slug);
        let dir = paths.agents_dir.join(&slug);
        let store = CredentialStore::new(paths.credentials_json.clone());
        let config = Config::load();
        let mut table = Table::new(["kind", "status"]);
        let mut rows = 0;
        for (kind, _) in section_entries(&dir, SECTION)? {
            let connected = is_connected(&kind, &store, &config);
            let (badge, style) = if connected {
                ("connected".to_string(), Palette::Success)
            } else {
                (
                    "not connected — run `goat integration add`".to_string(),
                    Palette::Warning,
                )
            };
            table.styled_row(vec![(kind, Palette::Plain), (badge, style)]);
            rows += 1;
        }
        if rows == 0 {
            ui::line(&ui::dim("none yet"));
            return Ok(Footer::Hint("none", "goat agent integration add".into()));
        }
        table.render();
        Ok(Footer::None)
    })
}

fn bind_remove(paths: &GoatPaths, kind: &str, agent: Option<&str>) -> Result<()> {
    ui::cell("Integration Remove", || {
        let slug = resolve_agent(paths, agent)?;
        ui::pair("agent", &slug);
        let kind = kind.trim();
        let dir = paths.agents_dir.join(&slug);
        if !section_contains(&dir, SECTION, kind)? {
            return Err(anyhow!("no binding for {slug}/{kind}"));
        }
        if !ui::confirm(&format!("delete config.json.{SECTION}.{kind}?"), false)? {
            return Ok(Footer::Cancel);
        }
        remove_section_config(&dir, SECTION, kind)?;
        ui::line(&ui::dim(
            "connection kept; disconnect with `goat integration rm`",
        ));
        Ok(Footer::Ok("Removed"))
    })
}

fn display_name(kind: &str) -> Option<String> {
    let factory = goat_integration::factory_for(kind)?;
    Some((factory.ctor)().metadata().display.to_string())
}

fn pick_kind(kind: Option<String>) -> Result<String> {
    if let Some(k) = kind {
        let k = k.trim().to_string();
        if integration_factory(&k).is_none() {
            return Err(anyhow!("unknown integration `{k}`"));
        }
        return Ok(k);
    }
    let mut items: Vec<(String, String)> = goat_integration::factories()
        .into_iter()
        .map(|f| {
            let id = f.id.to_string();
            let label = display_name(&id).unwrap_or_else(|| id.clone());
            (id, label)
        })
        .collect();
    items.sort_by(|a, b| a.1.cmp(&b.1));
    ui::pick("integration", &items).map_err(Into::into)
}

fn instantiate(
    kind: &str,
) -> Result<(
    std::sync::Arc<dyn Integration>,
    goat_integration::IntegrationMetadata,
)> {
    let factory = integration_factory(kind).ok_or_else(|| anyhow!("unknown integration"))?;
    let integration = (factory.ctor)();
    let metadata = integration.metadata();
    Ok((integration, metadata))
}

fn integration_factory(slug: &str) -> Option<&'static IntegrationFactory> {
    goat_integration::factory_for(slug)
}

async fn write_config(edits: Vec<goat_api::ConfigEdit>) -> Result<()> {
    let socket_path =
        goat_config::socket_path().ok_or_else(|| anyhow!(goat_config::HOME_NOT_FOUND))?;
    let daemon_exe = std::env::current_exe()?;
    let link = goat_client::Link::local(socket_path, daemon_exe);
    goat_client::edit_config(&link, edits)
        .await
        .map_err(|err| anyhow!("could not write the daemon config: {err}"))?;
    Ok(())
}
