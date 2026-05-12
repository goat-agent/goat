use anyhow::{anyhow, Result};
use clap::Subcommand;
use goat_config::GoatPaths;
use goat_credentials::Credentials;
use goat_llm::{LlmProviderFactory, ProviderId};
use serde_json::{json, Value};

use super::ui::{self, Footer, Style, Table};
use super::{edit_credentials, mask_key};

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// List configured provider keys.
    #[command(visible_alias = "ls")]
    List,
    /// Add a provider key.
    #[command(visible_alias = "new")]
    Add { name: Option<String> },
    /// Remove a provider key.
    #[command(visible_alias = "rm", aliases = ["del", "delete"])]
    Remove {
        name: Option<String>,
        label: Option<String>,
    },
}

pub async fn run(cmd: Cmd) -> Result<()> {
    let paths = GoatPaths::default_layout()?;
    match cmd {
        Cmd::List => list(&paths),
        Cmd::Add { name } => add(&paths, name),
        Cmd::Remove { name, label } => remove(&paths, name, label),
    }
}

fn list(paths: &GoatPaths) -> Result<()> {
    ui::cell("Providers", || {
        let creds = Credentials::load(&paths.credentials_json)?;
        let mut configured = 0usize;
        let mut table = Table::new(["provider", "keys", "labels"]);

        for factory in inventory::iter::<LlmProviderFactory>() {
            let pid = factory.id.clone();
            let entries: &[goat_credentials::CredentialEntry] =
                creds.llm.get(&pid).map(|v| v.as_slice()).unwrap_or(&[]);
            if !entries.is_empty() {
                configured += 1;
            }
            let labels: Vec<String> = entries
                .iter()
                .map(|e| {
                    let lbl = e.label.as_deref().unwrap_or("-");
                    let masked = e
                        .api_key
                        .as_deref()
                        .map(mask_key)
                        .unwrap_or_else(|| "?".into());
                    format!("{lbl}  {masked}")
                })
                .collect();
            table.styled_row(vec![
                (pid.as_str().to_string(), Style::Plain),
                (entries.len().to_string(), Style::Plain),
                (
                    if labels.is_empty() {
                        "—".into()
                    } else {
                        labels.join("  ·  ")
                    },
                    Style::Plain,
                ),
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

fn add(paths: &GoatPaths, name: Option<String>) -> Result<()> {
    ui::cell("Provider Add", || {
        let provider = match name {
            Some(n) => lookup_provider(&n).ok_or_else(|| anyhow!("unknown provider `{n}`"))?,
            None => {
                let mut items: Vec<(ProviderId, String)> = inventory::iter::<LlmProviderFactory>()
                    .map(|f| (f.id.clone(), f.id.as_str().to_string()))
                    .collect();
                items.sort_by(|a, b| a.1.cmp(&b.1));
                ui::pick("provider", &items)?
            }
        };
        let api_key = ui::secret(&format!("{} key", provider.as_str()))?;
        let raw_label = ui::ask("label", Some(""))?;
        let label = if raw_label.trim().is_empty() {
            None
        } else {
            Some(raw_label.trim().to_string())
        };
        edit_credentials(&paths.credentials_json, |map| {
            let entry = json!({ "api_key": api_key, "label": label });
            let list = map
                .entry(provider.as_str().to_string())
                .or_insert_with(|| Value::Array(Vec::new()));
            if let Value::Array(arr) = list {
                arr.push(entry);
            }
        })?;
        ui::pair(provider.as_str(), "saved");
        Ok(Footer::Ok("Saved"))
    })
}

fn remove(paths: &GoatPaths, name: Option<String>, label: Option<String>) -> Result<()> {
    ui::cell("Provider Remove", || {
        let creds = Credentials::load(&paths.credentials_json)?;
        let with_keys: Vec<(ProviderId, usize)> = creds
            .llm
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(p, v)| (p.clone(), v.len()))
            .collect();

        if with_keys.is_empty() {
            ui::line(&ui::dim("no provider keys configured"));
            return Ok(Footer::Cancel);
        }

        let provider = match name {
            Some(n) => lookup_provider(&n).ok_or_else(|| anyhow!("unknown provider `{n}`"))?,
            None => {
                let items: Vec<(ProviderId, String)> = with_keys
                    .iter()
                    .map(|(p, n)| (p.clone(), format!("{} ({n} key)", p.as_str())))
                    .collect();
                ui::pick("provider", &items)?
            }
        };

        let entries = creds
            .llm
            .get(&provider)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| anyhow!("no keys for `{}`", provider.as_str()))?;

        let chosen_label = match label {
            Some(l) => Some(l),
            None => {
                let items: Vec<(Option<String>, String)> = entries
                    .iter()
                    .map(|e| {
                        let lbl = e.label.as_deref().unwrap_or("-");
                        let masked = e
                            .api_key
                            .as_deref()
                            .map(mask_key)
                            .unwrap_or_else(|| "?".into());
                        (e.label.clone(), format!("{lbl}  {masked}"))
                    })
                    .collect();
                ui::pick("key", &items)?
            }
        };

        let target = chosen_label.as_deref().unwrap_or("-");
        if !ui::confirm(
            &format!("remove {} key `{target}`?", provider.as_str()),
            false,
        )? {
            return Ok(Footer::Cancel);
        }

        let mut removed = 0usize;
        edit_credentials(&paths.credentials_json, |map| {
            if let Some(Value::Array(arr)) = map.get_mut(provider.as_str()) {
                let before = arr.len();
                arr.retain(|v| {
                    let lbl = v.get("label").and_then(|l| l.as_str());
                    match (&chosen_label, lbl) {
                        (Some(want), Some(got)) => got != want.as_str(),
                        (None, None) => false,
                        _ => true,
                    }
                });
                removed = before - arr.len();
                if arr.is_empty() {
                    map.remove(provider.as_str());
                }
            }
        })?;

        if removed == 0 {
            Ok(Footer::Warn("No matching key".into()))
        } else {
            ui::pair(provider.as_str(), &format!("{removed} removed"));
            Ok(Footer::Ok("Removed"))
        }
    })
}

fn lookup_provider(name: &str) -> Option<ProviderId> {
    inventory::iter::<LlmProviderFactory>()
        .find(|factory| factory.id.as_str() == name)
        .map(|factory| factory.id.clone())
}
