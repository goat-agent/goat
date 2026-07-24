use std::time::Duration;

use anyhow::{Result, anyhow};
use clap::Subcommand;
use goat_channel::ChannelFactory;
use goat_config::GoatPaths;
use serde_json::{Value, json};

use super::agent::{
    channel_in_config, channels_from_config_with_values, remove_channel_config, resolve_profile,
    upsert_channel_config,
};
use super::ui::{self, Footer, Palette, Table};

const VERIFY_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Subcommand, Debug)]
pub enum Cmd {
    #[command(
        visible_alias = "new",
        about = "Bind a channel to an agent (verifies the token before saving)."
    )]
    Add {
        #[arg(help = "Channel kind (e.g. `discord`, `telegram`); prompted if omitted.")]
        kind: Option<String>,
        #[arg(
            short = 'a',
            long = "agent",
            help = "Target agent; resolved automatically when omitted."
        )]
        profile: Option<String>,
        #[arg(
            long,
            help = "Skip the live credential check and save the token as-is."
        )]
        no_verify: bool,
    },
    #[command(visible_alias = "ls", about = "List an agent's channel bindings.")]
    List {
        #[arg(short = 'a', long = "agent")]
        profile: Option<String>,
    },
    #[command(
        visible_alias = "rm",
        aliases = ["del", "delete"],
        about = "Remove a channel binding."
    )]
    Remove {
        kind: String,
        #[arg(short = 'a', long = "agent")]
        profile: Option<String>,
    },
}

pub async fn run(cmd: Cmd) -> Result<()> {
    let paths = GoatPaths::default_layout()?;
    match cmd {
        Cmd::Add {
            kind,
            profile,
            no_verify,
        } => channel_add(&paths, kind, profile, no_verify).await,
        Cmd::List { profile } => channel_list(&paths, profile.as_deref()),
        Cmd::Remove { kind, profile } => channel_remove(&paths, &kind, profile.as_deref()),
    }
}

async fn channel_add(
    paths: &GoatPaths,
    kind: Option<String>,
    profile: Option<String>,
    no_verify: bool,
) -> Result<()> {
    ui::cell_async("Channel Add", || async move {
        let slug = resolve_profile(paths, profile.as_deref())?;
        ui::pair("profile", &slug);
        let dir = paths.agents_dir.join(&slug);

        let kind = if let Some(k) = kind {
            let k = k.trim().to_string();
            if !known_channel(&k) {
                return Err(anyhow!("unknown channel `{k}`"));
            }
            k
        } else {
            let mut items: Vec<(String, String)> = inventory::iter::<ChannelFactory>()
                .map(|f| (f.id.to_string(), f.id.to_string()))
                .collect();
            items.sort_by(|a, b| a.1.cmp(&b.1));
            ui::pick("channel", &items)?
        };

        if channel_in_config(&dir, &kind)?
            && !ui::confirm(&format!("overwrite config.json.channels.{kind}?"), false)?
        {
            return Ok(Footer::Cancel);
        }

        let Some(token) = ui::secret(&format!("{kind} token"))? else {
            return Ok(Footer::Cancel);
        };
        let config = json!({ "token": token });

        if no_verify {
            ui::line(&ui::dim("skipped verification (--no-verify)"));
        } else {
            let identity = verify_channel(&kind, &config).await?;
            ui::pair("verified", &identity);
        }

        upsert_channel_config(&dir, &kind, config)?;
        ui::pair("file", &dir.join("config.json").display().to_string());
        Ok(Footer::Ok("Saved"))
    })
    .await
}

async fn verify_channel(kind: &str, config: &Value) -> Result<String> {
    let factory = channel_factory(kind).ok_or_else(|| anyhow!("unknown channel `{kind}`"))?;
    let channel = (factory.ctor)();
    match tokio::time::timeout(VERIFY_TIMEOUT, channel.verify(config)).await {
        Ok(Ok(identity)) => Ok(identity.handle),
        Ok(Err(e)) => Err(anyhow!("{e} — not saved")),
        Err(_) => Err(anyhow!(
            "verification timed out after {}s — not saved",
            VERIFY_TIMEOUT.as_secs()
        )),
    }
}

fn channel_list(paths: &GoatPaths, profile: Option<&str>) -> Result<()> {
    ui::cell("Channels", || {
        let slug = resolve_profile(paths, profile)?;
        ui::pair("profile", &slug);
        let dir = paths.agents_dir.join(&slug);
        let mut table = Table::new(["kind", "status", "path"]);
        let mut rows = 0;
        for (kind, config) in channels_from_config_with_values(&dir)? {
            let path = dir.join("config.json");
            let (badge, style) = match validate_channel_config(&kind, &config) {
                Ok(()) => ("ok".to_string(), Palette::Success),
                Err(e) => (format!("warn: {e}"), Palette::Warning),
            };
            table.styled_row(vec![
                (kind, Palette::Plain),
                (badge, style),
                (path.display().to_string(), Palette::Plain),
            ]);
            rows += 1;
        }
        if rows == 0 {
            ui::line(&ui::dim("none yet"));
            return Ok(Footer::Hint("none", "goat channel add".into()));
        }
        table.render();
        Ok(Footer::None)
    })
}

fn channel_remove(paths: &GoatPaths, kind: &str, profile: Option<&str>) -> Result<()> {
    ui::cell("Channel Remove", || {
        let slug = resolve_profile(paths, profile)?;
        ui::pair("profile", &slug);
        let kind = kind.trim();
        let dir = paths.agents_dir.join(&slug);
        if !channel_in_config(&dir, kind)? {
            return Err(anyhow!("no binding for {slug}/{kind}"));
        }
        if !ui::confirm(&format!("delete config.json.channels.{kind}?"), false)? {
            return Ok(Footer::Cancel);
        }
        remove_channel_config(&dir, kind)?;
        Ok(Footer::Ok("Removed"))
    })
}

fn known_channel(slug: &str) -> bool {
    channel_factory(slug).is_some()
}

fn channel_factory(slug: &str) -> Option<&'static ChannelFactory> {
    inventory::iter::<ChannelFactory>().find(|f| f.id.as_str() == slug)
}

fn validate_channel_config(kind: &str, config: &Value) -> Result<()> {
    let factory = channel_factory(kind).ok_or_else(|| anyhow!("unknown channel"))?;
    (factory.validate_config)(config).map_err(Into::into)
}
