use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use clap::Subcommand;
use goat_api::{AdminIntegrationStatusOutput, IntegrationConnectionStatus, IntegrationState};
use goat_config::{GoatPaths, write_atomic};
use serde_json::{Map, Value};

use super::super::agent::{remove_section_config, resolve_agent, section_entries};
use super::super::ui::{self, Footer, Palette, Settled, Table};

const SECTION: &str = "integrations";
const LIST_KEYS: &[&str] = &["deny_prefixes", "deny_suffixes"];

#[derive(Subcommand, Debug)]
pub enum AgentCmd {
    #[command(
        visible_alias = "ls",
        about = "List connections and which ones the agent uses"
    )]
    List {
        #[arg(
            short = 'a',
            long = "agent",
            help = "Target agent; picked when omitted"
        )]
        agent: Option<String>,
    },
    #[command(
        about = "Let the agent use a connection",
        after_help = "Examples:
  goat agent integration add linear -a bot
  goat agent integration add sentry -a bot --set organization_slug=acme"
    )]
    Add {
        #[arg(help = "Connection name; picked when omitted")]
        name: Option<String>,
        #[arg(
            short = 'a',
            long = "agent",
            help = "Target agent; picked when omitted"
        )]
        agent: Option<String>,
        #[arg(
            long = "set",
            value_name = "KEY=VALUE",
            help = "Usage setting; repeatable"
        )]
        set: Vec<String>,
    },
    #[command(about = "Change the agent's settings for a connection")]
    Set {
        #[arg(help = "Connection name")]
        name: String,
        #[arg(
            short = 'a',
            long = "agent",
            help = "Target agent; picked when omitted"
        )]
        agent: Option<String>,
        #[arg(
            long = "set",
            value_name = "KEY=VALUE",
            help = "Usage setting; repeatable"
        )]
        set: Vec<String>,
        #[arg(
            long = "unset",
            value_name = "KEY",
            help = "Setting to clear; repeatable"
        )]
        unset: Vec<String>,
    },
    #[command(visible_alias = "rm", about = "Stop the agent using a connection")]
    Remove {
        #[arg(help = "Connection name")]
        name: String,
        #[arg(
            short = 'a',
            long = "agent",
            help = "Target agent; picked when omitted"
        )]
        agent: Option<String>,
        #[arg(long, short, help = "Skip the confirmation")]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum CodeCmd {
    #[command(
        visible_alias = "ls",
        about = "List connections and which ones this project's code sessions use"
    )]
    List,
    #[command(
        about = "Use only chosen connections in this project",
        after_help = "Without .goat/integrations.json every connection is available.
Adding the first one starts an explicit list.

Examples:
  goat code integration add linear-work
  goat code integration add sentry --set organization_slug=acme"
    )]
    Add {
        #[arg(help = "Connection name; picked when omitted")]
        name: Option<String>,
        #[arg(
            long = "set",
            value_name = "KEY=VALUE",
            help = "Usage setting; repeatable"
        )]
        set: Vec<String>,
    },
    #[command(about = "Change this project's settings for a connection")]
    Set {
        #[arg(help = "Connection name")]
        name: String,
        #[arg(
            long = "set",
            value_name = "KEY=VALUE",
            help = "Usage setting; repeatable"
        )]
        set: Vec<String>,
        #[arg(
            long = "unset",
            value_name = "KEY",
            help = "Setting to clear; repeatable"
        )]
        unset: Vec<String>,
    },
    #[command(
        visible_alias = "rm",
        about = "Stop this project's sessions using a connection"
    )]
    Remove {
        #[arg(help = "Connection name")]
        name: String,
        #[arg(long, short, help = "Skip the confirmation")]
        yes: bool,
    },
    #[command(about = "Forget this project's list, so every connection is available again")]
    Reset {
        #[arg(long, short, help = "Skip the confirmation")]
        yes: bool,
    },
}

enum Target {
    Agent { slug: String, dir: PathBuf },
    Project { root: PathBuf },
}

impl Target {
    fn agent(paths: &GoatPaths, agent: Option<&str>) -> Result<Self> {
        let slug = resolve_agent(paths, agent)?;
        let dir = paths.agents_dir.join(&slug);
        Ok(Self::Agent { slug, dir })
    }

    fn project() -> Result<Self> {
        let cwd = std::env::current_dir()?;
        let root = goat_worktree::workspace(&cwd).map_or(cwd, |workspace| workspace.repo_root);
        Ok(Self::Project { root })
    }

    fn show(&self) {
        match self {
            Self::Agent { slug, .. } => ui::pair("agent", slug),
            Self::Project { root } => ui::pair("project", &root.display().to_string()),
        }
    }

    fn usages(&self) -> Result<Option<BTreeMap<String, Value>>> {
        match self {
            Self::Agent { dir, .. } => {
                Ok(Some(section_entries(dir, SECTION)?.into_iter().collect()))
            }
            Self::Project { root } => Ok(goat_integration::load_project_usage(root)?),
        }
    }

    fn write(&self, usages: &BTreeMap<String, Value>, changed: &str) -> Result<()> {
        match self {
            Self::Agent { dir, .. } => {
                remove_section_config(dir, SECTION, changed)?;
                if let Some(value) = usages.get(changed) {
                    super::super::agent::upsert_section_config(
                        dir,
                        SECTION,
                        changed,
                        value.clone(),
                    )?;
                }
                Ok(())
            }
            Self::Project { root } => {
                let path = goat_integration::project_usage_path(root);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let body = serde_json::to_string_pretty(usages)?;
                write_atomic(&path, format!("{body}\n").as_bytes())?;
                Ok(())
            }
        }
    }

    fn apply_hint(&self) -> Option<&str> {
        match self {
            Self::Agent { slug, .. } => Some(slug),
            Self::Project { .. } => None,
        }
    }
}

pub async fn run_agent(cmd: AgentCmd) -> Result<()> {
    let paths = GoatPaths::default_layout()?;
    let target = |agent: Option<String>| {
        ui::within("Integration Usage", Target::agent(&paths, agent.as_deref()))
    };
    match cmd {
        AgentCmd::List { agent } => list(&target(agent)?).await,
        AgentCmd::Add { name, agent, set } => {
            let target = target(agent)?;
            applied(&target, add(&target, name, &set).await?).await
        }
        AgentCmd::Set {
            name,
            agent,
            set,
            unset,
        } => {
            let target = target(agent)?;
            applied(&target, change(&target, &name, &set, &unset).await?).await
        }
        AgentCmd::Remove { name, agent, yes } => {
            let target = target(agent)?;
            applied(&target, remove(&target, &name, yes).await?).await
        }
    }
}

pub async fn run_code(cmd: CodeCmd) -> Result<()> {
    let target = ui::within("Integration Usage", Target::project())?;
    match cmd {
        CodeCmd::List => list(&target).await,
        CodeCmd::Add { name, set } => add(&target, name, &set).await.map(drop),
        CodeCmd::Set { name, set, unset } => change(&target, &name, &set, &unset).await.map(drop),
        CodeCmd::Remove { name, yes } => remove(&target, &name, yes).await.map(drop),
        CodeCmd::Reset { yes } => reset(&target, yes).map(drop),
    }
}

async fn applied(target: &Target, settled: Settled) -> Result<()> {
    if settled == Settled::Done {
        super::super::apply::config_changed(target.apply_hint()).await;
    }
    Ok(())
}

async fn list(target: &Target) -> Result<()> {
    let status = ui::within("Integration Usage", super::status(None, false).await)?;
    let usages = ui::within("Integration Usage", target.usages())?;
    ui::cell("Integration Usage", || {
        target.show();
        if usages.is_none() {
            ui::line(&ui::dim(
                "no .goat/integrations.json here, so every connection is available",
            ));
        }
        ui::blank();
        if status.connections.is_empty() {
            ui::line(&ui::dim("no connections yet"));
            return Ok(Footer::Hint("", "goat integration add".into()));
        }
        let mut table = Table::new(["", "connection", "integration", "state", "settings"]);
        for connection in &status.connections {
            let usage = match &usages {
                Some(usages) => usages.get(&connection.name),
                None => Some(&Value::Null),
            };
            let (state, palette) = super::state_cell(&connection.state);
            let (mark, mark_palette) = if usage.is_some() {
                ("✓", Palette::Success)
            } else {
                ("", Palette::Muted)
            };
            table.styled_row(vec![
                (mark.to_owned(), mark_palette),
                (connection.name.clone(), Palette::Provider),
                (connection.kind.clone(), Palette::Muted),
                (state, palette),
                (usage.map(settings_line).unwrap_or_default(), Palette::Value),
            ]);
        }
        table.render();
        Ok(Footer::None)
    })?;
    Ok(())
}

async fn add(target: &Target, name: Option<String>, set: &[String]) -> Result<Settled> {
    let status = ui::within("Integration Use", super::status(None, false).await)?;
    let usages = ui::within("Integration Use", target.usages())?;
    let name = match name {
        Some(name) => name.trim().to_owned(),
        None => match pick_unused(&status, usages.as_ref())? {
            Some(name) => name,
            None => return Ok(Settled::Cancelled),
        },
    };
    let status = if status.connections.iter().any(|c| c.name == name)
        || goat_integration::factory_for(&name).is_none()
    {
        status
    } else {
        if super::add(Some(name.clone()), None, super::CredentialArgs::default()).await?
            != Settled::Done
        {
            return Ok(Settled::Cancelled);
        }
        super::status(None, false).await?
    };
    ui::cell_async("Integration Use", || async move {
        target.show();
        let connection = find(&status, &name)?;
        let mut usages = usages.unwrap_or_default();
        let mut usage = usages
            .get(&name)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let already = usages.contains_key(&name);
        if already && set.is_empty() {
            ui::line(&ui::dim(&format!("{name} is already in use here")));
            return Ok(Footer::None);
        }
        apply_settings(&mut usage, set, &[])?;
        validate(connection, &usage)?;
        usages.insert(name.clone(), Value::Object(usage));
        target.write(&usages, &name)?;
        login_warning(connection);
        Ok(match target {
            Target::Agent { slug, .. } if watches(&connection.kind) => Footer::Hint(
                if already { "Updated" } else { "Added" },
                format!("goat agent watch list -a {slug}"),
            ),
            Target::Agent { .. } => Footer::Ok(if already { "Updated" } else { "Added" }),
            Target::Project { .. } => Footer::Hint(
                if already { "Updated" } else { "Added" },
                "applies to new goat code sessions".into(),
            ),
        })
    })
    .await
}

async fn change(target: &Target, name: &str, set: &[String], unset: &[String]) -> Result<Settled> {
    let status = ui::within("Integration Settings", super::status(None, false).await)?;
    ui::cell_async("Integration Settings", || async move {
        target.show();
        if set.is_empty() && unset.is_empty() {
            return Err(anyhow!("pass --set key=value or --unset key"));
        }
        let connection = find(&status, name)?;
        let mut usages = target.usages()?.unwrap_or_default();
        let Some(existing) = usages.get(name) else {
            return Err(not_used(target, name));
        };
        let mut usage = existing.as_object().cloned().unwrap_or_default();
        apply_settings(&mut usage, set, unset)?;
        validate(connection, &usage)?;
        ui::pair("settings", &settings_line(&Value::Object(usage.clone())));
        usages.insert(name.to_owned(), Value::Object(usage));
        target.write(&usages, name)?;
        Ok(Footer::Ok("Updated"))
    })
    .await
}

async fn remove(target: &Target, name: &str, yes: bool) -> Result<Settled> {
    let status = ui::within("Integration Unuse", super::status(None, false).await)?;
    ui::cell_async("Integration Unuse", || async move {
        target.show();
        let mut usages = if let Some(usages) = target.usages()? {
            usages
        } else {
            ui::line(&ui::dim(
                "this project will list its connections explicitly from now on",
            ));
            status
                .connections
                .iter()
                .map(|c| (c.name.clone(), Value::Object(Map::new())))
                .collect()
        };
        if !usages.contains_key(name) {
            return Err(not_used(target, name));
        }
        if !super::confirmed(&format!("stop using {name} here?"), yes)? {
            return Ok(Footer::Cancel);
        }
        usages.remove(name);
        target.write(&usages, name)?;
        Ok(Footer::Ok("Removed"))
    })
    .await
}

fn reset(target: &Target, yes: bool) -> Result<Settled> {
    let Target::Project { root } = target else {
        return Ok(Settled::Cancelled);
    };
    let path = goat_integration::project_usage_path(root);
    ui::cell("Integration Reset", || {
        target.show();
        if !path.exists() {
            ui::line(&ui::dim("every connection is already available here"));
            return Ok(Footer::None);
        }
        if !super::confirmed(&format!("delete {}?", path.display()), yes)? {
            return Ok(Footer::Cancel);
        }
        std::fs::remove_file(&path)?;
        Ok(Footer::Ok("Every connection is available again"))
    })
}

fn pick_unused(
    status: &AdminIntegrationStatusOutput,
    usages: Option<&BTreeMap<String, Value>>,
) -> Result<Option<String>> {
    let mut items: Vec<(String, String)> = status
        .connections
        .iter()
        .filter(|c| usages.is_none_or(|usages| !usages.contains_key(&c.name)))
        .map(|c| {
            let (state, _) = super::state_cell(&c.state);
            (c.name.clone(), format!("{} — {} ({state})", c.name, c.kind))
        })
        .collect();
    let connected: std::collections::HashSet<&str> =
        status.connections.iter().map(|c| c.kind.as_str()).collect();
    items.extend(
        super::catalog()
            .into_iter()
            .filter(|m| !connected.contains(m.id))
            .map(|m| {
                (
                    m.id.to_owned(),
                    format!("{} — connect {} first · {}", m.id, m.display, m.summary),
                )
            }),
    );
    if items.is_empty() {
        ui::line(&ui::dim("every connection is already in use here"));
        return Ok(None);
    }
    Ok(Some(ui::pick("connection", &items)?))
}

fn find<'a>(
    status: &'a AdminIntegrationStatusOutput,
    name: &str,
) -> Result<&'a IntegrationConnectionStatus> {
    status
        .connections
        .iter()
        .find(|c| c.name == name)
        .ok_or_else(|| super::missing_connection(name))
}

fn not_used(target: &Target, name: &str) -> anyhow::Error {
    let hint = match target {
        Target::Agent { slug, .. } => format!("goat agent integration add {name} -a {slug}"),
        Target::Project { .. } => format!("goat code integration add {name}"),
    };
    ui::report_hint(format!("{name} is not in use here"), hint)
}

fn apply_settings(usage: &mut Map<String, Value>, set: &[String], unset: &[String]) -> Result<()> {
    for pair in set {
        let (key, value) = pair
            .split_once('=')
            .ok_or_else(|| anyhow!("`{pair}` is not KEY=VALUE"))?;
        let key = key.trim();
        let value = if LIST_KEYS.contains(&key) {
            Value::Array(
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(|item| Value::String(item.to_owned()))
                    .collect(),
            )
        } else {
            Value::String(value.trim().to_owned())
        };
        usage.insert(key.to_owned(), value);
    }
    for key in unset {
        usage.remove(key.trim());
    }
    Ok(())
}

fn validate(connection: &IntegrationConnectionStatus, usage: &Map<String, Value>) -> Result<()> {
    let usage = Value::Object(usage.clone());
    goat_integration::reject_connection_keys(&usage)?;
    let factory = goat_integration::factory_for(&connection.kind)
        .ok_or_else(|| anyhow!("unknown integration `{}`", connection.kind))?;
    (factory.validate_config)(&usage).map_err(|e| {
        ui::report_hint(
            e.to_string(),
            format!("see `goat integration info {}`", connection.kind),
        )
    })
}

fn login_warning(connection: &IntegrationConnectionStatus) {
    if !matches!(connection.state, IntegrationState::Ready { .. }) {
        ui::pair_styled(
            "note",
            &format!(
                "{} is not logged in; run `goat integration login {}`",
                connection.name, connection.name
            ),
            Palette::Warning,
        );
    }
}

fn watches(kind: &str) -> bool {
    goat_integration::factory_for(kind).is_some_and(|f| (f.ctor)().watch_vocabulary().is_some())
}

fn settings_line(usage: &Value) -> String {
    usage
        .as_object()
        .map(|object| {
            object
                .iter()
                .map(|(key, value)| match value {
                    Value::String(value) => format!("{key}={value}"),
                    Value::Array(items) => format!(
                        "{key}={}",
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(",")
                    ),
                    other => format!("{key}={other}"),
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn settings_parse_lists_and_unset() {
        let mut usage = Map::new();
        apply_settings(
            &mut usage,
            &[
                "organization_slug=acme".to_owned(),
                "deny_prefixes=delete, drop".to_owned(),
            ],
            &[],
        )
        .unwrap();
        assert_eq!(
            Value::Object(usage.clone()),
            json!({ "organization_slug": "acme", "deny_prefixes": ["delete", "drop"] })
        );
        apply_settings(&mut usage, &[], &["deny_prefixes".to_owned()]).unwrap();
        assert_eq!(
            settings_line(&Value::Object(usage)),
            "organization_slug=acme"
        );
        assert!(apply_settings(&mut Map::new(), &["nokey".to_owned()], &[]).is_err());
    }
}
