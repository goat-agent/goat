use std::time::Duration;

use anyhow::{Result, anyhow};
use clap::Subcommand;
use goat_auth::CredentialStore;
use goat_channel::{ChannelFactory, ChannelSecrets};
use goat_config::GoatPaths;
use serde_json::{Value, json};

use super::agent::{
    channel_in_config, channels_from_config_with_values, remove_channel_config, resolve_agent,
    upsert_channel_config,
};
use super::ui::{self, Footer, Palette, Table};

const VERIFY_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Subcommand, Debug)]
pub enum Cmd {
    #[command(
        visible_alias = "new",
        about = "Bind a channel to an agent (verifies the secrets before storing them)."
    )]
    Add {
        #[arg(help = "Channel kind (e.g. `discord`); prompted if omitted.")]
        kind: Option<String>,
        #[arg(
            short = 'a',
            long = "agent",
            help = "Target agent; resolved automatically when omitted."
        )]
        agent: Option<String>,
        #[arg(
            long,
            help = "Store the secrets without checking them against the API."
        )]
        no_verify: bool,
    },
    #[command(visible_alias = "ls", about = "List an agent's channel bindings.")]
    List {
        #[arg(short = 'a', long = "agent")]
        agent: Option<String>,
    },
    #[command(
        visible_alias = "rm",
        aliases = ["del", "delete"],
        about = "Remove a channel binding and its stored secrets."
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
            channel_add(&paths, kind, agent, no_verify).await?;
            super::apply::config_changed(slug.as_deref()).await;
            Ok(())
        }
        Cmd::List { agent } => channel_list(&paths, agent.as_deref()),
        Cmd::Remove { kind, agent } => {
            channel_remove(&paths, &kind, agent.as_deref())?;
            super::apply::config_changed(agent.as_deref()).await;
            Ok(())
        }
    }
}

async fn channel_add(
    paths: &GoatPaths,
    kind: Option<String>,
    agent: Option<String>,
    no_verify: bool,
) -> Result<()> {
    ui::cell_async("Channel Add", || async move {
        let slug = resolve_agent(paths, agent.as_deref())?;
        ui::pair("agent", &slug);
        let dir = paths.agents_dir.join(&slug);

        let factory = match kind {
            Some(kind) => resolve_factory(kind.trim())?,
            None => resolve_factory(&pick_channel()?)?,
        };
        let kind = factory.id.as_str();
        let metadata = (factory.metadata)();

        if channel_in_config(&dir, kind)?
            && !ui::confirm(&format!("replace the {kind} binding for {slug}?"), false)?
        {
            return Ok(Footer::Cancel);
        }

        ui::line(&ui::dim(metadata.setup));
        let Some(secrets) = prompt_secrets(&metadata)? else {
            return Ok(Footer::Cancel);
        };

        let config = json!({});
        if no_verify {
            ui::line(&ui::dim("skipped verification (--no-verify)"));
        } else {
            ui::pair(
                "verified",
                &verify_channel(factory, &config, &secrets).await?,
            );
        }

        let store = CredentialStore::new(paths.credentials_json.clone());
        goat_channel::save_secrets(&store, &factory.id, &slug, &secrets)?;
        upsert_channel_config(&dir, kind, config)?;
        ui::pair("secrets", &paths.credentials_json.display().to_string());
        ui::pair("binding", &dir.join("config.json").display().to_string());
        Ok(Footer::Ok("Saved"))
    })
    .await
}

fn channel_list(paths: &GoatPaths, agent: Option<&str>) -> Result<()> {
    ui::cell("Channels", || {
        let slug = resolve_agent(paths, agent)?;
        ui::pair("agent", &slug);
        let dir = paths.agents_dir.join(&slug);
        let store = CredentialStore::new(paths.credentials_json.clone());
        let mut table = Table::new(["kind", "status", "secrets"]);
        let mut rows = 0;
        for (kind, config) in channels_from_config_with_values(&dir)? {
            let (badge, style) = match binding_status(&kind, &config, &store, &slug) {
                Ok(()) => ("ok".to_string(), Palette::Success),
                Err(problem) => (problem, Palette::Warning),
            };
            let slots = goat_channel::secret_specs(&kind)
                .iter()
                .map(|spec| spec.slot)
                .collect::<Vec<_>>()
                .join(", ");
            table.styled_row(vec![
                (kind, Palette::Plain),
                (badge, style),
                (slots, Palette::Plain),
            ]);
            rows += 1;
        }
        if rows == 0 {
            ui::line(&ui::dim("none yet"));
            return Ok(Footer::Hint("none", "goat agent channel add".into()));
        }
        table.render();
        Ok(Footer::None)
    })
}

fn channel_remove(paths: &GoatPaths, kind: &str, agent: Option<&str>) -> Result<()> {
    ui::cell("Channel Remove", || {
        let slug = resolve_agent(paths, agent)?;
        ui::pair("agent", &slug);
        let kind = kind.trim();
        let dir = paths.agents_dir.join(&slug);
        if !channel_in_config(&dir, kind)? {
            return Err(anyhow!("no binding for {slug}/{kind}"));
        }
        if !ui::confirm(
            &format!("remove the {kind} binding and its secrets?"),
            false,
        )? {
            return Ok(Footer::Cancel);
        }
        remove_channel_config(&dir, kind)?;
        if let Some(factory) = goat_channel::factory_for(kind) {
            let store = CredentialStore::new(paths.credentials_json.clone());
            goat_channel::forget_secrets(&store, &factory.id, &slug, (factory.metadata)().secrets)?;
        }
        Ok(Footer::Ok("Removed"))
    })
}

fn pick_channel() -> Result<String> {
    let items: Vec<(String, String)> = goat_channel::registered_ids()
        .into_iter()
        .map(|id| {
            let label = goat_channel::metadata_for(id)
                .map_or_else(|| id.to_string(), |meta| format!("{id} — {}", meta.display));
            (id.to_string(), label)
        })
        .collect();
    Ok(ui::pick("channel", &items)?)
}

fn resolve_factory(kind: &str) -> Result<&'static ChannelFactory> {
    goat_channel::factory_for(kind).ok_or_else(|| {
        ui::report_hint(
            format!("unknown channel `{kind}`"),
            format!(
                "known channels: {}",
                goat_channel::registered_ids().join(", ")
            ),
        )
    })
}

fn prompt_secrets(metadata: &goat_channel::ChannelMetadata) -> Result<Option<ChannelSecrets>> {
    let mut secrets = ChannelSecrets::new();
    for spec in metadata.secrets {
        let Some(value) = ui::secret(spec.label)? else {
            return Ok(None);
        };
        secrets.insert(spec.slot, value);
    }
    Ok(Some(secrets))
}

async fn verify_channel(
    factory: &'static ChannelFactory,
    config: &Value,
    secrets: &ChannelSecrets,
) -> Result<String> {
    let channel = (factory.ctor)();
    match tokio::time::timeout(VERIFY_TIMEOUT, channel.verify(config, secrets)).await {
        Ok(Ok(identity)) => Ok(identity.handle),
        Ok(Err(e)) => Err(anyhow!("{e} — nothing was stored")),
        Err(_) => Err(anyhow!(
            "verification timed out after {}s — nothing was stored",
            VERIFY_TIMEOUT.as_secs()
        )),
    }
}

fn binding_status(
    kind: &str,
    config: &Value,
    store: &CredentialStore,
    slug: &str,
) -> Result<(), String> {
    let Some(factory) = goat_channel::factory_for(kind) else {
        return Err("warn: no compiled-in channel with this name".to_string());
    };
    (factory.validate_config)(config).map_err(|e| format!("warn: {e}"))?;
    let specs = (factory.metadata)().secrets;
    let missing = goat_channel::load_secrets(store, &factory.id, slug, specs).missing(specs);
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!("warn: missing {}", missing.join(", ")))
    }
}
