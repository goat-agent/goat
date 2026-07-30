use std::io::IsTerminal;

use anyhow::{Result, anyhow};
use clap::Subcommand;
use goat_auth::CredentialStore;
use goat_config::GoatPaths;
use goat_model::{Model, ProviderId};
use goat_providers::Registry;
use serde_json::{Map, Value, json};

use super::ui::{self, Footer, Table};

#[derive(Subcommand, Debug)]
pub enum Cmd {
    #[command(visible_alias = "ls", about = "List agents")]
    List,
    #[command(visible_alias = "new", about = "Create an agent")]
    Add { slug: Option<String> },
    #[command(about = "Print an agent's definition and channel bindings")]
    Show { slug: String },
    #[command(visible_alias = "rm", aliases = ["del", "delete"], about = "Archive an agent")]
    Remove { slug: String },
    #[command(subcommand, about = "Manage an agent's channel bindings")]
    Channel(super::channel::Cmd),
    #[command(subcommand, about = "Manage an agent's integration bindings")]
    Integration(super::integration::Cmd),
    #[command(about = "Show agent and channel state")]
    Status,
    #[command(about = "Show recent actions the agent took")]
    Log {
        #[arg(short, long, default_value_t = 20)]
        limit: usize,
    },
}

pub async fn run(cmd: Cmd) -> Result<()> {
    let paths = GoatPaths::default_layout()?;
    match cmd {
        Cmd::List => list(&paths),
        Cmd::Add { slug } => add(&paths, slug),
        Cmd::Show { slug } => show(&paths, &slug),
        Cmd::Remove { slug } => remove(&paths, &slug),
        Cmd::Channel(c) => super::channel::run(c).await,
        Cmd::Integration(c) => super::integration::run(c).await,
        Cmd::Status => super::governance::status(),
        Cmd::Log { limit } => super::governance::log(limit).await,
    }
}

pub fn create_interactive(paths: &GoatPaths) -> Result<String> {
    ui::section("Agent");
    let slug = ui::prompt("slug", Some("dev"))?.ok_or_else(|| anyhow!("cancelled"))?;
    write_agent(paths, slug.trim())
}

fn write_agent(paths: &GoatPaths, slug: &str) -> Result<String> {
    let slug = slug.trim().to_string();
    if slug.is_empty() {
        return Err(anyhow!("empty slug"));
    }
    let dir = paths.agents_dir.join(&slug);
    if dir.join("agent.md").exists() {
        return Err(anyhow!("`{slug}` already exists at {}", dir.display()));
    }
    let model = pick_model(paths)?;
    std::fs::create_dir_all(&dir)?;
    let agent_md = dir.join("agent.md");
    std::fs::write(&agent_md, format!("You are {slug}.\n"))?;
    let config_json = dir.join("config.json");
    let body = serde_json::to_string_pretty(&json!({
        "display": slug,
        "model": model.to_string(),
        "tools": ["*"],
        "channels": {}
    }))?;
    std::fs::write(&config_json, format!("{body}\n"))?;
    ui::pair("file", &agent_md.display().to_string());
    ui::pair("config", &config_json.display().to_string());
    Ok(slug)
}

fn pick_model(paths: &GoatPaths) -> Result<Model> {
    let store = CredentialStore::new(paths.credentials_json.clone());
    let user = goat_config::UserProviders::at(paths.config_json.clone());
    let registry = Registry::new(&store, &user);
    let mut entries: Vec<(Option<(String, String)>, String)> = registry
        .all()
        .iter()
        .flat_map(|provider| {
            let id = provider.id().to_string();
            provider
                .list_models()
                .into_iter()
                .map(move |model| (Some((id.clone(), model.clone())), format!("{id}/{model}")))
        })
        .collect();
    entries.sort_by(|a, b| a.1.cmp(&b.1));
    entries.push((None, "custom…".into()));

    match ui::pick("model", &entries)? {
        Some((provider, model)) => Ok(Model::new(ProviderId::from(provider.as_str()), model)),
        None => pick_model_custom(&registry),
    }
}

fn pick_model_custom(registry: &Registry) -> Result<Model> {
    let mut items: Vec<(String, String)> = registry
        .all()
        .iter()
        .map(|p| {
            let id = p.id().to_string();
            (id.clone(), id)
        })
        .collect();
    items.sort_by(|a, b| a.1.cmp(&b.1));
    let provider = ui::pick("provider", &items)?;
    let id = ui::prompt("model id", Some("gpt-4o-mini"))?.ok_or_else(|| anyhow!("cancelled"))?;
    Ok(Model::new(ProviderId::from(provider.as_str()), id.trim()))
}

fn list(paths: &GoatPaths) -> Result<()> {
    ui::cell("Agents", || {
        if !paths.agents_dir.exists() {
            ui::line(&ui::dim("no agents dir"));
            return Ok(Footer::Hint("None", "goat setup".into()));
        }
        let mut table = Table::new(["slug", "display", "model", "channels"]);
        let mut rows = 0usize;
        for entry in std::fs::read_dir(&paths.agents_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let dir = entry.path();
            let slug = dir.file_name().and_then(|s| s.to_str()).unwrap_or("?");
            if !dir.join("agent.md").exists() {
                continue;
            }
            let cfg = match read_agent_config(&dir) {
                Ok(cfg) => cfg,
                Err(e) => {
                    table.row(vec![
                        slug.to_string(),
                        "config error".into(),
                        e.to_string(),
                        "—".into(),
                    ]);
                    rows += 1;
                    continue;
                }
            };
            let display = cfg
                .get("display")
                .and_then(Value::as_str)
                .map_or_else(|| slug.into(), String::from);
            let model = cfg
                .get("model")
                .and_then(Value::as_str)
                .map(String::from)
                .ok_or_else(|| {
                    anyhow!(
                        "missing or invalid model in {}",
                        dir.join("config.json").display()
                    )
                })?;
            let bindings = bindings_for(&dir)?;
            table.row(vec![
                slug.to_string(),
                display,
                model,
                if bindings.is_empty() {
                    "—".into()
                } else {
                    bindings.join(", ")
                },
            ]);
            rows += 1;
        }
        if rows == 0 {
            ui::line(&ui::dim("none yet"));
            return Ok(Footer::Hint("None", "goat agent add".into()));
        }
        table.render();
        Ok(Footer::None)
    })
}

fn bindings_for(dir: &std::path::Path) -> Result<Vec<String>> {
    let mut out = channels_from_config(dir)?;
    out.sort();
    Ok(out)
}

fn add(paths: &GoatPaths, slug: Option<String>) -> Result<()> {
    ui::cell("Agent Add", || {
        let slug = match slug {
            Some(s) => write_agent(paths, &s)?,
            None => create_interactive(paths)?,
        };
        let _ = slug;
        Ok(Footer::Hint("Created", "goat agent channel add".into()))
    })
}

fn show(paths: &GoatPaths, slug: &str) -> Result<()> {
    ui::cell(&format!("Agent {slug}"), || {
        let dir = paths.agents_dir.join(slug);
        let agent_md = dir.join("agent.md");
        if !agent_md.exists() {
            return Err(anyhow!("no agent at {}", agent_md.display()));
        }
        ui::line(&ui::dim(&agent_md.display().to_string()));
        ui::blank();
        for raw_line in std::fs::read_to_string(&agent_md)?.lines() {
            ui::line(raw_line);
        }
        let config_json = dir.join("config.json");
        if !config_json.exists() {
            return Err(anyhow!("missing {}", config_json.display()));
        }
        ui::blank();
        ui::line(&ui::dim(&config_json.display().to_string()));
        ui::blank();
        for raw_line in std::fs::read_to_string(&config_json)?.lines() {
            ui::line(raw_line);
        }
        Ok(Footer::None)
    })
}

fn remove(paths: &GoatPaths, slug: &str) -> Result<()> {
    ui::cell(&format!("Agent Remove {slug}"), || {
        let dir = paths.agents_dir.join(slug);
        if !dir.exists() {
            return Err(anyhow!("no agent at {}", dir.display()));
        }
        if !ui::confirm(&format!("delete {}?", dir.display()), false)? {
            return Ok(Footer::Cancel);
        }
        std::fs::remove_dir_all(&dir)?;
        Ok(Footer::Ok("Removed"))
    })
}

pub(crate) fn list_agents(paths: &GoatPaths) -> Result<Vec<String>> {
    let mut out = Vec::new();
    if !paths.agents_dir.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(&paths.agents_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let dir = entry.path();
        if dir.join("agent.md").exists()
            && let Some(slug) = dir.file_name().and_then(|s| s.to_str())
        {
            out.push(slug.to_string());
        }
    }
    out.sort();
    Ok(out)
}

pub(crate) fn agent_exists(paths: &GoatPaths, slug: &str) -> bool {
    paths.agents_dir.join(slug).join("agent.md").exists()
}

pub(crate) fn resolve_agent(paths: &GoatPaths, explicit: Option<&str>) -> Result<String> {
    if let Some(p) = explicit {
        let p = p.trim();
        if !agent_exists(paths, p) {
            return Err(anyhow!(
                "no agent `{p}` at {}",
                paths.agents_dir.join(p).display()
            ));
        }
        return Ok(p.to_string());
    }
    let mut agents = list_agents(paths)?;
    match agents.len() {
        0 => Err(anyhow!("no agents yet — run `goat agent add`")),
        1 => Ok(agents.pop().expect("len 1")),
        _ => {
            if std::io::stdin().is_terminal() && std::io::stderr().is_terminal() {
                let items: Vec<(String, String)> =
                    agents.iter().map(|s| (s.clone(), s.clone())).collect();
                ui::pick("agent", &items).map_err(Into::into)
            } else {
                Err(anyhow!("multiple agents — pass -a <agent>"))
            }
        }
    }
}

pub(crate) fn read_agent_config(dir: &std::path::Path) -> Result<Value> {
    let path = dir.join("config.json");
    if !path.exists() {
        return Err(anyhow!("missing {}", path.display()));
    }
    let cfg: Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    if !cfg.is_object() {
        return Err(anyhow!("{} must be a JSON object", path.display()));
    }
    Ok(cfg)
}

fn write_agent_config(dir: &std::path::Path, value: &Value) -> Result<()> {
    let body = serde_json::to_string_pretty(value)?;
    std::fs::write(dir.join("config.json"), format!("{body}\n"))?;
    Ok(())
}

fn config_object(value: &mut Value) -> Result<&mut Map<String, Value>> {
    if !value.is_object() {
        return Err(anyhow!("config.json must be a JSON object"));
    }
    Ok(value.as_object_mut().expect("object checked"))
}

fn section_object<'a>(value: &'a mut Value, section: &str) -> Result<&'a mut Map<String, Value>> {
    let obj = config_object(value)?;
    let entry = obj.entry(section).or_insert_with(|| json!({}));
    if !entry.is_object() {
        return Err(anyhow!("config.json {section} must be a JSON object"));
    }
    Ok(entry.as_object_mut().expect("object checked"))
}

fn section_keys(dir: &std::path::Path, section: &str) -> Result<Vec<String>> {
    Ok(section_entries(dir, section)?
        .into_iter()
        .map(|(kind, _)| kind)
        .collect())
}

pub(crate) fn section_entries(
    dir: &std::path::Path,
    section: &str,
) -> Result<Vec<(String, Value)>> {
    let cfg = read_agent_config(dir)?;
    let Some(entries) = cfg.get(section) else {
        return Ok(Vec::new());
    };
    let entries = entries
        .as_object()
        .ok_or_else(|| anyhow!("config.json {section} must be a JSON object"))?;
    let mut out = entries
        .iter()
        .map(|(kind, config)| (kind.clone(), config.clone()))
        .collect::<Vec<_>>();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

pub(crate) fn section_contains(dir: &std::path::Path, section: &str, kind: &str) -> Result<bool> {
    Ok(section_entries(dir, section)?
        .iter()
        .any(|(k, _)| k == kind))
}

pub(crate) fn upsert_section_config(
    dir: &std::path::Path,
    section: &str,
    kind: &str,
    value: Value,
) -> Result<()> {
    let mut cfg = read_agent_config(dir)?;
    let entries = section_object(&mut cfg, section)?;
    match (entries.get_mut(kind), value) {
        (Some(Value::Object(existing)), Value::Object(new)) => {
            existing.extend(new);
        }
        (_, value) => {
            entries.insert(kind.to_string(), value);
        }
    }
    write_agent_config(dir, &cfg)
}

pub(crate) fn remove_section_config(
    dir: &std::path::Path,
    section: &str,
    kind: &str,
) -> Result<()> {
    let mut cfg = read_agent_config(dir)?;
    section_object(&mut cfg, section)?.remove(kind);
    write_agent_config(dir, &cfg)
}

fn channels_from_config(dir: &std::path::Path) -> Result<Vec<String>> {
    section_keys(dir, "channels")
}

pub(crate) fn channels_from_config_with_values(
    dir: &std::path::Path,
) -> Result<Vec<(String, Value)>> {
    section_entries(dir, "channels")
}

pub(crate) fn channel_in_config(dir: &std::path::Path, kind: &str) -> Result<bool> {
    section_contains(dir, "channels", kind)
}

pub(crate) fn upsert_channel_config(
    dir: &std::path::Path,
    kind: &str,
    channel: Value,
) -> Result<()> {
    upsert_section_config(dir, "channels", kind, channel)
}

pub(crate) fn remove_channel_config(dir: &std::path::Path, kind: &str) -> Result<()> {
    remove_section_config(dir, "channels", kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_upsert_preserves_existing_channel_fields() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.json"),
            r#"{
              "channels": {
                "discord": {
                  "token": "old",
                  "allowed_user_ids": [123]
                }
              }
            }"#,
        )
        .unwrap();

        upsert_channel_config(dir.path(), "discord", json!({ "token": "new" })).unwrap();

        let cfg = read_agent_config(dir.path()).unwrap();
        let discord = &cfg["channels"]["discord"];
        assert_eq!(discord["token"], "new");
        assert_eq!(discord["allowed_user_ids"], json!([123]));
    }

    #[test]
    fn channel_remove_deletes_only_config_channel() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.json"),
            r#"{
              "channels": {
                "discord": {
                  "token": "new"
                }
              }
            }"#,
        )
        .unwrap();

        remove_channel_config(dir.path(), "discord").unwrap();

        let cfg = read_agent_config(dir.path()).unwrap();
        assert!(!cfg["channels"].as_object().unwrap().contains_key("discord"));
    }

    #[test]
    fn channel_helpers_error_when_config_missing() {
        let dir = tempfile::tempdir().unwrap();

        assert!(bindings_for(dir.path()).is_err());
        assert!(channel_in_config(dir.path(), "discord").is_err());
    }

    #[test]
    fn channel_helpers_error_when_channels_is_not_object() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.json"),
            r#"{
              "model": "openai/gpt-4o-mini",
              "channels": []
            }"#,
        )
        .unwrap();

        assert!(bindings_for(dir.path()).is_err());
        assert!(channel_in_config(dir.path(), "discord").is_err());
        assert!(upsert_channel_config(dir.path(), "discord", json!({ "token": "new" })).is_err());
    }

    #[test]
    fn agent_config_root_must_be_object() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.json"), "[]").unwrap();

        assert!(read_agent_config(dir.path()).is_err());
    }
}
