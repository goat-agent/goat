use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use clap::Args as ClapArgs;
use goat_config::{GoatPaths, LoadedConfig};
use goat_llm::{LlmProviderFactory, ModelInfo, ProviderId};
use goat_skills::SkillIndex;

use super::ui::{self, Footer, Style, Table};

#[derive(ClapArgs, Debug, Default)]
pub struct Args {
    /// Probe each provider with one cheap request to confirm the key works.
    #[arg(long)]
    pub check: bool,
}

pub async fn run(args: Args) -> Result<()> {
    let paths = GoatPaths::default_layout()?;
    let cfg = goat_config::load_from(paths.clone())
        .await
        .context("loading config")?;

    let providers_with_keys = list_providers_with_keys(&cfg);
    let probes = if args.check {
        Some(probe_all(&cfg, &providers_with_keys).await)
    } else {
        None
    };

    let mut warnings = 0usize;
    let mut hint: Option<(&'static str, String)> = None;

    ui::cell("Doctor", || {
        ui::section("Paths");
        ui::pair("root", &paths.root.display().to_string());
        ui::pair("db", &paths.state_db.display().to_string());
        ui::pair("logs", &paths.logs_dir.display().to_string());
        ui::blank();

        ui::section("Providers");
        render_providers(&cfg, &mut warnings, &mut hint);
        ui::blank();

        ui::section("Personas");
        render_personas(&paths, &cfg, &mut warnings, &mut hint)?;
        ui::blank();

        ui::section("Skills");
        render_skills(&paths, &mut warnings);

        if let Some(rows) = &probes {
            ui::blank();
            ui::section("Check");
            render_check(rows, &mut warnings);
        }

        let footer = if warnings == 0 {
            Footer::None
        } else if let Some((verb, next)) = hint.take() {
            Footer::Hint(verb, next)
        } else {
            Footer::Warn(format!(
                "{warnings} warning{}",
                if warnings == 1 { "" } else { "s" }
            ))
        };
        Ok(footer)
    })?;
    Ok(())
}

fn list_providers_with_keys(cfg: &LoadedConfig) -> Vec<ProviderId> {
    let mut out = Vec::new();
    for factory in inventory::iter::<LlmProviderFactory>() {
        let pid = factory.id.clone();
        if cfg
            .credentials
            .llm
            .get(&pid)
            .map(|v| !v.is_empty())
            .unwrap_or(false)
        {
            out.push(pid);
        }
    }
    out
}

fn render_providers(
    cfg: &LoadedConfig,
    warnings: &mut usize,
    hint: &mut Option<(&'static str, String)>,
) {
    let mut t = Table::new(["provider", "status", "keys", "labels"]);
    let mut any_keys = false;
    for factory in inventory::iter::<LlmProviderFactory>() {
        let pid = factory.id.clone();
        let entries: &[goat_credentials::CredentialEntry] = cfg
            .credentials
            .llm
            .get(&pid)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        let labels = entries
            .iter()
            .map(|e| e.label.clone().unwrap_or_else(|| "-".into()))
            .collect::<Vec<_>>()
            .join(", ");
        let (badge, style) = if entries.is_empty() {
            ("missing", Style::Dim)
        } else {
            any_keys = true;
            ("ok", Style::Ok)
        };
        t.styled_row(vec![
            (pid.as_str().to_string(), Style::Plain),
            (badge.to_string(), style),
            (entries.len().to_string(), Style::Plain),
            (
                if labels.is_empty() {
                    "—".into()
                } else {
                    labels
                },
                Style::Plain,
            ),
        ]);
    }
    t.render();
    if !any_keys {
        *warnings += 1;
        hint.get_or_insert(("none", "goat provider add".into()));
    }
}

fn render_personas(
    paths: &GoatPaths,
    cfg: &LoadedConfig,
    warnings: &mut usize,
    hint: &mut Option<(&'static str, String)>,
) -> Result<()> {
    let known_models: HashSet<(ProviderId, &'static str)> = inventory::iter::<ModelInfo>()
        .map(|m| (m.provider.clone(), m.id))
        .collect();
    let loaded: HashMap<&str, _> = cfg.personas.iter().map(|p| (p.slug.as_str(), p)).collect();

    if !paths.personas_dir.exists() {
        ui::line(&ui::dim("no personas dir"));
        *warnings += 1;
        hint.get_or_insert(("none", "goat persona add".into()));
        return Ok(());
    }

    let mut slugs: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&paths.personas_dir)
        .with_context(|| format!("reading {}", paths.personas_dir.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let dir = entry.path();
        if !dir.join("persona.md").exists() {
            continue;
        }
        if let Some(slug) = dir.file_name().and_then(|s| s.to_str()) {
            slugs.push(slug.to_string());
        }
    }
    slugs.sort();

    if slugs.is_empty() {
        ui::line(&ui::dim("none yet"));
        *warnings += 1;
        hint.get_or_insert(("none", "goat persona add".into()));
        return Ok(());
    }

    let mut t = Table::new(["persona", "status", "model", "bindings"]);
    for slug in &slugs {
        match loaded.get(slug.as_str()) {
            Some(p) => {
                let model = p.default_model.to_string();
                let bindings = if p.bindings.is_empty() {
                    "—".into()
                } else {
                    p.bindings
                        .iter()
                        .map(|b| b.name.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let in_catalog = known_models.contains(&(
                    p.default_model.provider.clone(),
                    p.default_model.id.as_str(),
                ));
                let (badge, style) = if in_catalog {
                    ("ok", Style::Ok)
                } else {
                    *warnings += 1;
                    ("warn", Style::Warn)
                };
                t.styled_row(vec![
                    (slug.clone(), Style::Plain),
                    (badge.to_string(), style),
                    (model, Style::Plain),
                    (bindings, Style::Plain),
                ]);
            }
            None => {
                *warnings += 1;
                t.styled_row(vec![
                    (slug.clone(), Style::Plain),
                    ("warn".into(), Style::Warn),
                    ("?".into(), Style::Plain),
                    ("?".into(), Style::Plain),
                ]);
            }
        }
    }
    t.render();
    Ok(())
}

fn render_skills(paths: &GoatPaths, warnings: &mut usize) {
    let idx = SkillIndex::discover_root(&paths.root);
    let entries = idx.all_entries();
    let diagnostics = idx.diagnostics();

    if entries.is_empty() && diagnostics.is_empty() {
        ui::line(&ui::dim("none discovered"));
        return;
    }

    let mut t = Table::new(["skill", "scope", "status", "detail"]);
    for e in entries {
        t.styled_row(vec![
            (e.name.clone(), Style::Plain),
            (e.scope.label().to_string(), Style::Plain),
            ("ok".into(), Style::Ok),
            (e.description.clone(), Style::Plain),
        ]);
    }
    for d in diagnostics {
        *warnings += 1;
        t.styled_row(vec![
            (
                d.path
                    .parent()
                    .and_then(|p| p.file_name())
                    .and_then(|s| s.to_str())
                    .unwrap_or("?")
                    .to_string(),
                Style::Dim,
            ),
            (d.scope.label().to_string(), Style::Plain),
            ("warn".into(), Style::Warn),
            (d.message.clone(), Style::Warn),
        ]);
    }
    t.render();
}

struct ProbeRow {
    provider: ProviderId,
    result: std::result::Result<(), String>,
}

async fn probe_all(cfg: &LoadedConfig, providers: &[ProviderId]) -> Vec<ProbeRow> {
    let mut out = Vec::new();
    for pid in providers {
        let result = match cfg
            .credentials
            .llm
            .get(pid)
            .and_then(|entries| entries.first())
            .and_then(|e| e.api_key.as_deref())
        {
            Some(key) => match inventory::iter::<LlmProviderFactory>()
                .find(|factory| factory.id == *pid)
                .and_then(|factory| factory.probe)
            {
                Some(probe) => probe(key.to_string()).await,
                None => Err("probe unavailable".into()),
            },
            None => Err("no api_key field".into()),
        };
        out.push(ProbeRow {
            provider: pid.clone(),
            result,
        });
    }
    out
}

fn render_check(rows: &[ProbeRow], warnings: &mut usize) {
    let mut t = Table::new(["provider", "status", "detail"]);
    for r in rows {
        match &r.result {
            Ok(()) => t.styled_row(vec![
                (r.provider.as_str().to_string(), Style::Plain),
                ("ok".into(), Style::Ok),
                ("reachable".into(), Style::Plain),
            ]),
            Err(msg) => {
                *warnings += 1;
                t.styled_row(vec![
                    (r.provider.as_str().to_string(), Style::Plain),
                    ("warn".into(), Style::Warn),
                    (msg.clone(), Style::Plain),
                ]);
            }
        }
    }
    t.render();
}
