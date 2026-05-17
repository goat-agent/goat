use std::sync::Arc;

use anyhow::{anyhow, Result};
use clap::Subcommand;
use goat_config::GoatPaths;
use goat_credentials::JsonFileStore;
use goat_llm::{CredentialStore, LlmProviderSpec, SetupCtx, UserPrompt};

use super::ui::{self, Footer, Style, Table};
use super::CliPrompt;

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// List configured provider credentials.
    #[command(visible_alias = "ls")]
    List,
    /// Add a provider credential.
    #[command(visible_alias = "new")]
    Add { name: Option<String> },
    /// Remove a provider credential.
    #[command(visible_alias = "rm", aliases = ["del", "delete"])]
    Remove {
        name: Option<String>,
        label: Option<String>,
    },
}

pub async fn run(cmd: Cmd) -> Result<()> {
    let paths = GoatPaths::default_layout()?;
    let store: Arc<dyn CredentialStore> =
        Arc::new(JsonFileStore::open(paths.credentials_json.clone())?);
    match cmd {
        Cmd::List => list(&store),
        Cmd::Add { name } => add(&store, name).await,
        Cmd::Remove { name, label } => remove(&store, name, label),
    }
}

fn list(store: &Arc<dyn CredentialStore>) -> Result<()> {
    ui::cell("Providers", || {
        let mut configured = 0usize;
        let mut table = Table::new(["provider", "entries", "summary"]);

        for spec in inventory::iter::<LlmProviderSpec>() {
            let entries = store.list(spec.id.clone());
            if !entries.is_empty() {
                configured += 1;
            }
            let summary = if entries.is_empty() {
                "—".to_string()
            } else {
                entries
                    .iter()
                    .map(|e| (spec.summarize)(&e.raw))
                    .collect::<Vec<_>>()
                    .join("  ·  ")
            };
            table.styled_row(vec![
                (spec.id.as_str().to_string(), Style::Plain),
                (entries.len().to_string(), Style::Plain),
                (summary, Style::Plain),
            ]);
        }

        if configured == 0 {
            ui::line(&ui::dim("none configured"));
            return Ok(Footer::Hint("None", "goat provider add".into()));
        }
        table.render();
        Ok(Footer::None)
    })
}

async fn add(store: &Arc<dyn CredentialStore>, name: Option<String>) -> Result<()> {
    let spec = match name {
        Some(n) => find_spec(&n).ok_or_else(|| anyhow!("unknown provider `{n}`"))?,
        None => pick_spec()?,
    };

    let prompt: Arc<dyn UserPrompt> = Arc::new(CliPrompt);
    let raw_label = ui::ask("label", Some(""))?;
    let label = if raw_label.trim().is_empty() {
        None
    } else {
        Some(raw_label.trim().to_string())
    };

    let ctx = SetupCtx {
        provider: spec.id.clone(),
        label: label.clone(),
        prompt,
    };

    let value = spec.setup.run(ctx).await.map_err(anyhow::Error::from)?;
    store.write(spec.id.clone(), label.as_deref(), value.clone())?;
    ui::pair(spec.id.as_str(), &(spec.summarize)(&value));
    Ok(())
}

fn remove(
    store: &Arc<dyn CredentialStore>,
    name: Option<String>,
    label: Option<String>,
) -> Result<()> {
    ui::cell("Provider Remove", || {
        let configured: Vec<(&'static LlmProviderSpec, usize)> =
            inventory::iter::<LlmProviderSpec>()
                .map(|s| (s, store.list(s.id.clone()).len()))
                .filter(|(_, n)| *n > 0)
                .collect();

        if configured.is_empty() {
            ui::line(&ui::dim("no provider credentials configured"));
            return Ok(Footer::Cancel);
        }

        let spec = match name {
            Some(n) => find_spec(&n).ok_or_else(|| anyhow!("unknown provider `{n}`"))?,
            None => {
                let items: Vec<(&LlmProviderSpec, String)> = configured
                    .iter()
                    .map(|(s, n)| (*s, format!("{} ({n})", s.id.as_str())))
                    .collect();
                ui::pick("provider", &items)?
            }
        };

        let entries = store.list(spec.id.clone());
        if entries.is_empty() {
            return Err(anyhow!("no credentials for `{}`", spec.id.as_str()));
        }

        let chosen_label = match label {
            Some(l) => Some(l),
            None => {
                let items: Vec<(Option<String>, String)> = entries
                    .iter()
                    .map(|e| {
                        let l = e.label.as_deref().unwrap_or("-");
                        (
                            e.label.clone(),
                            format!("{l}  {}", (spec.summarize)(&e.raw)),
                        )
                    })
                    .collect();
                ui::pick("credential", &items)?
            }
        };

        let target = chosen_label.as_deref().unwrap_or("-");
        if !ui::confirm(
            &format!("remove {} credential `{target}`?", spec.id.as_str()),
            false,
        )? {
            return Ok(Footer::Cancel);
        }

        store.remove(spec.id.clone(), chosen_label.as_deref())?;
        ui::pair(spec.id.as_str(), "removed");
        Ok(Footer::Ok("Removed"))
    })
}

fn find_spec(name: &str) -> Option<&'static LlmProviderSpec> {
    inventory::iter::<LlmProviderSpec>().find(|s| s.id.as_str() == name)
}

fn pick_spec() -> Result<&'static LlmProviderSpec> {
    let mut items: Vec<(&LlmProviderSpec, String)> = inventory::iter::<LlmProviderSpec>()
        .map(|s| (s, s.id.as_str().to_string()))
        .collect();
    items.sort_by(|a, b| a.1.cmp(&b.1));
    ui::pick("provider", &items)
}
