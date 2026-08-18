use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use clap::Args as ClapArgs;
use goat_auth::{CredentialKind, CredentialService, CredentialStore};
use goat_config::{GoatPaths, LoadedConfig};
use goat_providers::Registry;

use super::ui::{self, Footer, Palette, Table};
use super::verify::{self, VerifyOutcome};

#[derive(ClapArgs, Debug, Default)]
pub struct Args {
    #[arg(long)]
    pub check: bool,
}

pub async fn run(args: Args) -> Result<()> {
    let paths = GoatPaths::default_layout()?;
    let cfg = goat_config::load_from(paths.clone()).context("loading config")?;
    let store = CredentialStore::new(paths.credentials_json.clone());
    let user = goat_config::UserProviders::at(paths.config_json.clone());

    let probes = if args.check {
        Some(probe_all(&store, &user).await)
    } else {
        None
    };

    let daemon = daemon_line().await;

    let mut warnings = 0usize;
    let mut hint: Option<(&'static str, String)> = None;

    ui::cell("Doctor", || {
        ui::section("Paths");
        ui::pair("root", &paths.root.display().to_string());
        ui::pair("db", &paths.state_db.display().to_string());
        ui::pair("logs", &paths.logs_dir.display().to_string());
        ui::blank();

        ui::section("Providers");
        render_providers(&store, &user, &mut warnings, &mut hint);
        ui::blank();

        ui::section("Agents");
        render_agents(&paths, &cfg, &store, &user, &mut warnings, &mut hint)?;
        ui::blank();

        ui::section("Skills");
        render_skills(&paths, &mut warnings);

        ui::blank();
        ui::section("Coding");
        ui::pair("daemon", &daemon.0);
        if let Some(extra) = &daemon.1 {
            ui::pair("", extra);
            warnings += 1;
        }

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

async fn daemon_line() -> (String, Option<String>) {
    let Some(socket) = goat_config::socket_path() else {
        return ("not running".to_owned(), None);
    };
    match goat_client::greet(&socket).await {
        goat_client::Daemon::Absent => ("not running".to_owned(), None),
        goat_client::Daemon::Silent => (
            "not answering".to_owned(),
            Some("kill it with `pkill -f 'goat daemon serve'`".to_owned()),
        ),
        goat_client::Daemon::Reachable(them) => {
            let ours = goat_client::mine();
            let running = format!("running goat {} (pid {})", them.version, them.pid);
            if goat_client::is_current(&ours, &them) {
                (running, None)
            } else {
                (
                    running,
                    Some("a different build than this binary; run `goat daemon start`".to_owned()),
                )
            }
        }
    }
}

fn provider_ids(registry: &Registry) -> Vec<String> {
    let mut ids: Vec<String> = registry.all().iter().map(|p| p.id().to_string()).collect();
    ids.sort();
    ids
}

fn accounts_for(store: &CredentialStore, provider: &str) -> Vec<(String, CredentialKind)> {
    store
        .entries()
        .into_iter()
        .filter(|(key, _)| key.service == CredentialService::Model && key.provider == provider)
        .map(|(key, kind)| (key.account, kind))
        .collect()
}

fn credential_kind_label(kind: CredentialKind) -> &'static str {
    match kind {
        CredentialKind::ApiKey => "api key",
        CredentialKind::OAuth => "oauth",
    }
}

fn render_providers(
    store: &CredentialStore,
    user: &goat_config::UserProviders,
    warnings: &mut usize,
    hint: &mut Option<(&'static str, String)>,
) {
    let registry = Registry::new(store, user);
    let mut t = Table::new(["provider", "status", "entries", "summary"]);
    let mut any = false;
    for id in provider_ids(&registry) {
        let accounts = accounts_for(store, &id);
        let summary = if accounts.is_empty() {
            "—".into()
        } else {
            accounts
                .iter()
                .map(|(account, kind)| format!("{account} ({})", credential_kind_label(*kind)))
                .collect::<Vec<_>>()
                .join("  ·  ")
        };
        let (badge, style) = if accounts.is_empty() {
            ("missing", Palette::Muted)
        } else {
            any = true;
            ("ok", Palette::Success)
        };
        t.styled_row(vec![
            (id, Palette::Plain),
            (badge.to_string(), style),
            (accounts.len().to_string(), Palette::Plain),
            (summary, Palette::Plain),
        ]);
    }
    t.render();
    if !any {
        *warnings += 1;
        hint.get_or_insert(("none", "goat provider login".into()));
    }
}

fn known_models(
    store: &CredentialStore,
    user: &goat_config::UserProviders,
) -> HashSet<(String, String)> {
    Registry::new(store, user)
        .all()
        .iter()
        .flat_map(|provider| {
            let id = provider.id().to_string();
            provider
                .list_models()
                .into_iter()
                .map(move |model| (id.clone(), model))
        })
        .collect()
}

fn render_agents(
    paths: &GoatPaths,
    cfg: &LoadedConfig,
    store: &CredentialStore,
    user: &goat_config::UserProviders,
    warnings: &mut usize,
    hint: &mut Option<(&'static str, String)>,
) -> Result<()> {
    let catalog = known_models(store, user);
    let loaded: HashMap<&str, _> = cfg.agents.iter().map(|p| (p.slug.as_str(), p)).collect();
    let watch: HashMap<String, Vec<goat_runtime::WatchIssue>> =
        goat_runtime::validate_agents(cfg).into_iter().collect();

    if !paths.agents_dir.exists() {
        ui::line(&ui::dim("no agents dir"));
        *warnings += 1;
        hint.get_or_insert(("none", "goat agent add".into()));
        return Ok(());
    }

    let mut slugs: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&paths.agents_dir)
        .with_context(|| format!("reading {}", paths.agents_dir.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let dir = entry.path();
        if !dir.join("agent.md").exists() {
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
        hint.get_or_insert(("none", "goat agent add".into()));
        return Ok(());
    }

    let mut t = Table::new(["agent", "status", "model", "bindings", "watch"]);
    for slug in &slugs {
        if let Some(p) = loaded.get(slug.as_str()) {
            let issues = watch.get(slug).map_or(&[][..], Vec::as_slice);
            let (watch_cell, watch_style) = if issues.is_empty() {
                ("ok".to_string(), Palette::Success)
            } else {
                *warnings += issues.len();
                (format!("{} dropped", issues.len()), Palette::Warning)
            };
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
            let in_catalog = catalog.contains(&(
                p.default_model.provider.to_string(),
                p.default_model.id.clone(),
            ));
            let (badge, style) = if in_catalog {
                ("ok", Palette::Success)
            } else {
                *warnings += 1;
                ("warn", Palette::Warning)
            };
            t.styled_row(vec![
                (slug.clone(), Palette::Plain),
                (badge.to_string(), style),
                (model, Palette::Plain),
                (bindings, Palette::Plain),
                (watch_cell, watch_style),
            ]);
        } else {
            *warnings += 1;
            t.styled_row(vec![
                (slug.clone(), Palette::Plain),
                ("warn".into(), Palette::Warning),
                ("?".into(), Palette::Plain),
                ("?".into(), Palette::Plain),
                ("?".into(), Palette::Plain),
            ]);
        }
    }
    t.render();

    for slug in &slugs {
        for issue in watch.get(slug).map_or(&[][..], Vec::as_slice) {
            ui::line(&ui::dim(&format!("{slug}: {issue}")));
        }
    }
    Ok(())
}

fn render_skills(paths: &GoatPaths, warnings: &mut usize) {
    let survey = goat_skill::survey(&paths.root);
    let entries = survey.skills;
    let diagnostics = survey.diagnostics;

    if entries.is_empty() && diagnostics.is_empty() {
        ui::line(&ui::dim("none discovered"));
        return;
    }

    let mut t = Table::new(["skill", "scope", "status", "detail"]);
    for e in entries {
        t.styled_row(vec![
            (e.name.clone(), Palette::Plain),
            (e.scope.label().to_string(), Palette::Plain),
            ("ok".into(), Palette::Success),
            (e.description.clone(), Palette::Plain),
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
                Palette::Muted,
            ),
            (d.scope.label().to_string(), Palette::Plain),
            ("warn".into(), Palette::Warning),
            (d.message.clone(), Palette::Warning),
        ]);
    }
    t.render();
}

struct ProbeRow {
    label: String,
    outcome: VerifyOutcome,
}

async fn probe_all(store: &CredentialStore, user: &goat_config::UserProviders) -> Vec<ProbeRow> {
    let registry = Registry::new(store, user);
    let mut out = Vec::new();
    for provider in registry.all() {
        let id = provider.id().to_string();
        for account in verify::accounts_for(store, &id) {
            let outcome = verify::verify_credential(store, user, &id, &account).await;
            out.push(ProbeRow {
                label: verify::row_label(&id, &account),
                outcome,
            });
        }
    }
    out
}

fn render_check(rows: &[ProbeRow], warnings: &mut usize) {
    let mut t = Table::new(["provider", "status", "detail"]);
    for r in rows {
        if verify::is_warning(&r.outcome) {
            *warnings += 1;
        }
        let (status, style, detail) = verify::outcome_row(&r.outcome);
        t.styled_row(vec![
            (r.label.clone(), Palette::Plain),
            (status.to_owned(), style),
            (detail, Palette::Plain),
        ]);
    }
    t.render();
}
