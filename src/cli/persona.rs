use anyhow::{anyhow, Result};
use clap::Subcommand;
use goat_channel::ChannelFactory;
use goat_config::GoatPaths;
use goat_llm::{LlmProviderSpec, Model, ModelInfo, ProviderId};
use serde_json::{json, Map, Value};

use super::ui::{self, Footer, Style, Table};

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// List every persona under `~/.goat/personas/`.
    #[command(visible_alias = "ls")]
    List,
    /// Create a new persona.
    #[command(visible_alias = "new")]
    Add {
        #[arg(long)]
        slug: Option<String>,
    },
    /// Print a persona's `persona.md` and channel bindings.
    Show { slug: String },
    /// Delete a persona folder.
    #[command(visible_alias = "rm", aliases = ["del", "delete"])]
    Remove { slug: String },
    /// Manage channel bindings for a persona.
    #[command(subcommand)]
    Channel(ChannelCmd),
}

#[derive(Subcommand, Debug)]
pub enum ChannelCmd {
    /// List channel bindings for a persona.
    #[command(visible_alias = "ls")]
    List { slug: String },
    /// Bind a channel to a persona.
    #[command(visible_alias = "new")]
    Add { slug: String, kind: Option<String> },
    /// Remove a channel binding.
    #[command(visible_alias = "rm", aliases = ["del", "delete"])]
    Remove { slug: String, kind: String },
}

pub async fn run(cmd: Cmd) -> Result<()> {
    let paths = GoatPaths::default_layout()?;
    match cmd {
        Cmd::List => list(&paths),
        Cmd::Add { slug } => add(&paths, slug),
        Cmd::Show { slug } => show(&paths, &slug),
        Cmd::Remove { slug } => remove(&paths, &slug),
        Cmd::Channel(c) => channel_run(&paths, c),
    }
}

fn channel_run(paths: &GoatPaths, cmd: ChannelCmd) -> Result<()> {
    match cmd {
        ChannelCmd::List { slug } => channel_list(paths, &slug),
        ChannelCmd::Add { slug, kind } => channel_add(paths, &slug, kind),
        ChannelCmd::Remove { slug, kind } => channel_remove(paths, &slug, &kind),
    }
}

pub fn create_interactive(paths: &GoatPaths) -> Result<String> {
    ui::section("Persona");
    let slug = ui::ask("slug", Some("dev"))?;
    write_persona(paths, slug.trim())
}

fn write_persona(paths: &GoatPaths, slug: &str) -> Result<String> {
    let slug = slug.trim().to_string();
    if slug.is_empty() {
        return Err(anyhow!("empty slug"));
    }
    let dir = paths.personas_dir.join(&slug);
    if dir.join("persona.md").exists() {
        return Err(anyhow!("`{slug}` already exists at {}", dir.display()));
    }
    let model = pick_model()?;
    std::fs::create_dir_all(&dir)?;
    let persona_md = dir.join("persona.md");
    std::fs::write(&persona_md, format!("You are {slug}.\n"))?;
    let config_json = dir.join("config.json");
    let body = serde_json::to_string_pretty(&json!({
        "display": slug,
        "model": model.to_string(),
        "tools": ["*"],
        "channels": {}
    }))?;
    std::fs::write(&config_json, format!("{body}\n"))?;
    ui::pair("file", &persona_md.display().to_string());
    ui::pair("config", &config_json.display().to_string());
    Ok(slug)
}

fn pick_model() -> Result<Model> {
    let mut entries: Vec<(Option<ModelInfo>, String)> = inventory::iter::<ModelInfo>()
        .map(|m| {
            (
                Some(m.clone()),
                format!("{}/{}  {}", m.provider, m.id, fmt_ctx(m.context)),
            )
        })
        .collect();
    entries.sort_by(|a, b| a.1.cmp(&b.1));
    entries.push((None, "custom…".into()));

    match ui::pick("model", &entries)? {
        Some(info) => Ok(Model::new(info.provider, info.id)),
        None => pick_model_custom(),
    }
}

fn pick_model_custom() -> Result<Model> {
    let mut items: Vec<(ProviderId, String)> = inventory::iter::<LlmProviderSpec>()
        .map(|f| (f.id.clone(), f.id.as_str().to_string()))
        .collect();
    items.sort_by(|a, b| a.1.cmp(&b.1));
    let provider = ui::pick("provider", &items)?;
    let id = ui::ask("model id", Some("gpt-4o-mini"))?;
    Ok(Model::new(provider, id.trim()))
}

fn fmt_ctx(n: u32) -> String {
    if n >= 1_000_000 {
        format!("{:.0}M ctx", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.0}k ctx", n as f64 / 1_000.0)
    } else {
        format!("{n} ctx")
    }
}

fn list(paths: &GoatPaths) -> Result<()> {
    ui::cell("Personas", || {
        if !paths.personas_dir.exists() {
            ui::line(&ui::dim("no personas dir"));
            return Ok(Footer::Hint("None", "goat setup".into()));
        }
        let mut table = Table::new(["slug", "display", "model", "channels"]);
        let mut rows = 0usize;
        for entry in std::fs::read_dir(&paths.personas_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let dir = entry.path();
            let slug = dir.file_name().and_then(|s| s.to_str()).unwrap_or("?");
            let persona_md = dir.join("persona.md");
            if !persona_md.exists() {
                continue;
            }
            let cfg = match read_persona_config(&dir) {
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
                .map(String::from)
                .unwrap_or_else(|| slug.into());
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
            return Ok(Footer::Hint("None", "goat persona add".into()));
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
    ui::cell("Persona Add", || {
        let slug = match slug {
            Some(s) => write_persona(paths, &s)?,
            None => create_interactive(paths)?,
        };
        let next = format!("goat persona channel add {slug}");
        Ok(Footer::Hint("Created", next))
    })
}

fn show(paths: &GoatPaths, slug: &str) -> Result<()> {
    ui::cell(&format!("Persona {slug}"), || {
        let dir = paths.personas_dir.join(slug);
        let persona_md = dir.join("persona.md");
        if !persona_md.exists() {
            return Err(anyhow!("no persona at {}", persona_md.display()));
        }
        ui::line(&ui::dim(&persona_md.display().to_string()));
        ui::blank();
        for raw_line in std::fs::read_to_string(&persona_md)?.lines() {
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
    ui::cell(&format!("Persona Remove {slug}"), || {
        let dir = paths.personas_dir.join(slug);
        if !dir.exists() {
            return Err(anyhow!("no persona at {}", dir.display()));
        }
        if !ui::confirm(&format!("delete {}?", dir.display()), false)? {
            return Ok(Footer::Cancel);
        }
        std::fs::remove_dir_all(&dir)?;
        Ok(Footer::Ok("Removed"))
    })
}

fn channel_list(paths: &GoatPaths, persona: &str) -> Result<()> {
    ui::cell(&format!("Channels {persona}"), || {
        let dir = paths.personas_dir.join(persona);
        if !dir.join("persona.md").exists() {
            return Err(anyhow!("no persona at {}", dir.display()));
        }
        let mut table = Table::new(["kind", "status", "path"]);
        let mut rows = 0;
        for (kind, config) in channels_from_config_with_values(&dir)? {
            let path = dir.join("config.json");
            let (badge, style) = match validate_channel_config(&kind, &config) {
                Ok(()) => ("ok".to_string(), Style::Ok),
                Err(e) => (format!("warn: {e}"), Style::Warn),
            };
            table.styled_row(vec![
                (kind, Style::Plain),
                (badge, style),
                (path.display().to_string(), Style::Plain),
            ]);
            rows += 1;
        }
        if rows == 0 {
            ui::line(&ui::dim("none yet"));
            return Ok(Footer::Hint(
                "none",
                format!("goat persona channel add {persona}"),
            ));
        }
        table.render();
        Ok(Footer::None)
    })
}

fn channel_add(paths: &GoatPaths, persona: &str, kind: Option<String>) -> Result<()> {
    ui::cell(&format!("Channel Add {persona}"), || {
        let dir = paths.personas_dir.join(persona);
        if !dir.join("persona.md").exists() {
            return Err(anyhow!("no persona at {}", dir.display()));
        }
        let kind = match kind {
            Some(k) => {
                let k = k.trim().to_string();
                if !known_channel(&k) {
                    return Err(anyhow!("unknown channel `{k}`"));
                }
                k
            }
            None => {
                let mut items: Vec<(String, String)> = inventory::iter::<ChannelFactory>()
                    .map(|f| (f.id.to_string(), f.id.to_string()))
                    .collect();
                items.sort_by(|a, b| a.1.cmp(&b.1));
                ui::pick("channel", &items)?
            }
        };
        if channel_in_config(&dir, &kind)?
            && !ui::confirm(&format!("overwrite config.json.channels.{kind}?"), false)?
        {
            return Ok(Footer::Cancel);
        }
        let token = ui::secret(&format!("{kind} token"))?;
        upsert_channel_config(&dir, &kind, json!({ "token": token }))?;
        let path = dir.join("config.json");
        ui::pair("file", &path.display().to_string());
        Ok(Footer::Ok("Saved"))
    })
}

fn channel_remove(paths: &GoatPaths, persona: &str, kind: &str) -> Result<()> {
    ui::cell(&format!("Channel Remove {persona}/{kind}"), || {
        let kind = kind.trim();
        let dir = paths.personas_dir.join(persona);
        if !channel_in_config(&dir, kind)? {
            return Err(anyhow!("no binding for {persona}/{kind}"));
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

fn read_persona_config(dir: &std::path::Path) -> Result<Value> {
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

fn write_persona_config(dir: &std::path::Path, value: &Value) -> Result<()> {
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

fn channels_object(value: &mut Value) -> Result<&mut Map<String, Value>> {
    let obj = config_object(value)?;
    let entry = obj.entry("channels").or_insert_with(|| json!({}));
    if !entry.is_object() {
        return Err(anyhow!("config.json channels must be a JSON object"));
    }
    Ok(entry.as_object_mut().expect("object checked"))
}

fn channels_from_config(dir: &std::path::Path) -> Result<Vec<String>> {
    let cfg = read_persona_config(dir)?;
    let Some(channels) = cfg.get("channels") else {
        return Ok(Vec::new());
    };
    let channels = channels
        .as_object()
        .ok_or_else(|| anyhow!("config.json channels must be a JSON object"))?;
    Ok(channels.keys().cloned().collect())
}

fn channels_from_config_with_values(dir: &std::path::Path) -> Result<Vec<(String, Value)>> {
    let cfg = read_persona_config(dir)?;
    let Some(channels) = cfg.get("channels") else {
        return Ok(Vec::new());
    };
    let channels = channels
        .as_object()
        .ok_or_else(|| anyhow!("config.json channels must be a JSON object"))?;
    let mut out = channels
        .iter()
        .map(|(kind, config)| (kind.clone(), config.clone()))
        .collect::<Vec<_>>();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn channel_in_config(dir: &std::path::Path, kind: &str) -> Result<bool> {
    let cfg = read_persona_config(dir)?;
    let Some(channels) = cfg.get("channels") else {
        return Ok(false);
    };
    let channels = channels
        .as_object()
        .ok_or_else(|| anyhow!("config.json channels must be a JSON object"))?;
    Ok(channels.contains_key(kind))
}

fn upsert_channel_config(dir: &std::path::Path, kind: &str, channel: Value) -> Result<()> {
    let mut cfg = read_persona_config(dir)?;
    let channels = channels_object(&mut cfg)?;
    match (channels.get_mut(kind), channel) {
        (Some(Value::Object(existing)), Value::Object(new)) => {
            existing.extend(new);
        }
        (_, channel) => {
            channels.insert(kind.to_string(), channel);
        }
    }
    write_persona_config(dir, &cfg)
}

fn remove_channel_config(dir: &std::path::Path, kind: &str) -> Result<()> {
    let mut cfg = read_persona_config(dir)?;
    channels_object(&mut cfg)?.remove(kind);
    write_persona_config(dir, &cfg)
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
                "telegram": {
                  "token": "old",
                  "allowed_user_ids": [123]
                }
              }
            }"#,
        )
        .unwrap();

        upsert_channel_config(dir.path(), "telegram", json!({ "token": "new" })).unwrap();

        let cfg = read_persona_config(dir.path()).unwrap();
        let telegram = &cfg["channels"]["telegram"];
        assert_eq!(telegram["token"], "new");
        assert_eq!(telegram["allowed_user_ids"], json!([123]));
    }

    #[test]
    fn channel_remove_deletes_only_config_channel() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.json"),
            r#"{
              "channels": {
                "telegram": {
                  "token": "new"
                }
              }
            }"#,
        )
        .unwrap();

        remove_channel_config(dir.path(), "telegram").unwrap();

        let cfg = read_persona_config(dir.path()).unwrap();
        assert!(!cfg["channels"]
            .as_object()
            .unwrap()
            .contains_key("telegram"));
    }

    #[test]
    fn channel_helpers_error_when_config_missing() {
        let dir = tempfile::tempdir().unwrap();

        assert!(bindings_for(dir.path()).is_err());
        assert!(channel_in_config(dir.path(), "telegram").is_err());
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
        assert!(channel_in_config(dir.path(), "telegram").is_err());
        assert!(upsert_channel_config(dir.path(), "telegram", json!({ "token": "new" })).is_err());
    }

    #[test]
    fn persona_config_root_must_be_object() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.json"), "[]").unwrap();

        assert!(read_persona_config(dir.path()).is_err());
    }
}
