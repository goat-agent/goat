use std::process::Command as Proc;

use anyhow::{anyhow, Result};
use clap::Subcommand;
use goat_channel::ChannelFactory;
use goat_config::GoatPaths;
use goat_llm::{LlmProviderFactory, Model, ModelInfo, ProviderId};
use serde_json::json;

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
    /// Open the persona's `persona.md` in `$EDITOR`.
    Edit { slug: String },
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
        Cmd::Edit { slug } => edit(&paths, &slug),
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
    let body = format!("---\ndisplay: {slug}\nmodel: {model}\n---\n\nYou are {slug}.\n");
    let path = dir.join("persona.md");
    std::fs::write(&path, body)?;
    ui::pair("file", &path.display().to_string());
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
    let mut items: Vec<(ProviderId, String)> = inventory::iter::<LlmProviderFactory>()
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
            let raw = std::fs::read_to_string(&persona_md).unwrap_or_default();
            let display = goat_config::front_field(&raw, "display").unwrap_or_else(|| slug.into());
            let model = goat_config::front_field(&raw, "model").unwrap_or_else(|| "?".into());
            let bindings = bindings_for(&dir);
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

fn bindings_for(dir: &std::path::Path) -> Vec<String> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    read.flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) == Some("json") {
                p.file_stem().and_then(|s| s.to_str()).map(String::from)
            } else {
                None
            }
        })
        .collect()
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
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) == Some("json") {
                ui::blank();
                ui::line(&ui::dim(&p.display().to_string()));
                ui::blank();
                for raw_line in std::fs::read_to_string(&p)?.lines() {
                    ui::line(raw_line);
                }
            }
        }
        Ok(Footer::None)
    })
}

fn edit(paths: &GoatPaths, slug: &str) -> Result<()> {
    ui::cell(&format!("Persona Edit {slug}"), || {
        let persona_md = paths.personas_dir.join(slug).join("persona.md");
        if !persona_md.exists() {
            return Err(anyhow!("no persona at {}", persona_md.display()));
        }
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".into());
        ui::pair("file", &persona_md.display().to_string());
        ui::pair("editor", &editor);
        let status = Proc::new(&editor).arg(&persona_md).status()?;
        if status.success() {
            Ok(Footer::Ok("Saved"))
        } else {
            Ok(Footer::Cancel)
        }
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
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let kind = p.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
            let parse_ok = std::fs::read_to_string(&p)
                .ok()
                .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                .is_some();
            let style = if parse_ok { Style::Ok } else { Style::Warn };
            let badge = if parse_ok { "ok" } else { "warn" };
            table.styled_row(vec![
                (kind.to_string(), Style::Plain),
                (badge.to_string(), style),
                (p.display().to_string(), Style::Plain),
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
        let path = dir.join(format!("{kind}.json"));
        if path.exists() && !ui::confirm(&format!("overwrite {}?", path.display()), false)? {
            return Ok(Footer::Cancel);
        }
        let token = ui::secret(&format!("{kind} token"))?;
        let body = serde_json::to_string_pretty(&json!({ "token": token }))?;
        std::fs::write(&path, format!("{body}\n"))?;
        ui::pair("file", &path.display().to_string());
        Ok(Footer::Ok("Saved"))
    })
}

fn channel_remove(paths: &GoatPaths, persona: &str, kind: &str) -> Result<()> {
    ui::cell(&format!("Channel Remove {persona}/{kind}"), || {
        let kind = kind.trim();
        let path = paths
            .personas_dir
            .join(persona)
            .join(format!("{kind}.json"));
        if !path.exists() {
            return Err(anyhow!("no binding at {}", path.display()));
        }
        if !ui::confirm(&format!("delete {}?", path.display()), false)? {
            return Ok(Footer::Cancel);
        }
        std::fs::remove_file(&path)?;
        Ok(Footer::Ok("Removed"))
    })
}

fn known_channel(slug: &str) -> bool {
    inventory::iter::<ChannelFactory>().any(|f| f.id.as_str() == slug)
}
