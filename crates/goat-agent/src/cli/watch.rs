use std::collections::BTreeMap;

use anyhow::{Result, anyhow};
use clap::Subcommand;
use goat_agent_config::{AgentConfig, WatchSourceEntry, WatchWorkflow};
use goat_config::GoatPaths;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::agent::{read_agent_config, resolve_agent, write_agent_config};
use super::ui::{self, Footer, Palette, Settled, Table};

const SECTION: &str = "watch";

#[derive(Subcommand, Debug)]
pub enum Cmd {
    #[command(
        visible_alias = "ls",
        about = "Show the workflows the agent watches, defaults included"
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
        about = "Watch a query on a connection and brief the agent on changes",
        after_help = "Examples:
  goat agent watch add triage --source linear --query \"assignee:@me is:open\" -a bot
  goat agent watch add reviews --source github --query \"is:open is:pr review-requested:@me\"

See `goat integration info <integration>` for each query vocabulary."
    )]
    Add {
        #[arg(help = "Workflow name; sources in one workflow are briefed together")]
        workflow: String,
        #[arg(long, help = "Connection to poll")]
        source: String,
        #[arg(long, help = "Query in the connection's watch vocabulary")]
        query: String,
        #[arg(long, help = "Stream name; defaults to the workflow name")]
        stream: Option<String>,
        #[arg(long, help = "Stable id, so editing the query keeps its history")]
        id: Option<String>,
        #[arg(
            short = 'a',
            long = "agent",
            help = "Target agent; picked when omitted"
        )]
        agent: Option<String>,
    },
    #[command(
        visible_alias = "rm",
        about = "Stop watching a workflow, or one of its sources"
    )]
    Remove {
        #[arg(help = "Workflow name")]
        workflow: String,
        #[arg(long, help = "Remove only this connection's sources")]
        source: Option<String>,
        #[arg(
            short = 'a',
            long = "agent",
            help = "Target agent; picked when omitted"
        )]
        agent: Option<String>,
    },
    #[command(about = "Forget the agent's workflows and go back to the integrations' defaults")]
    Reset {
        #[arg(
            short = 'a',
            long = "agent",
            help = "Target agent; picked when omitted"
        )]
        agent: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Entry {
    source: String,
    query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stream: Option<String>,
}

type Workflows = BTreeMap<String, Vec<Entry>>;

pub async fn run(cmd: Cmd) -> Result<()> {
    let paths = GoatPaths::default_layout()?;
    let (slug, settled) = match cmd {
        Cmd::List { agent } => {
            let slug = ui::within("Watch", resolve_agent(&paths, agent.as_deref()))?;
            return list(&paths, &slug);
        }
        Cmd::Add {
            workflow,
            source,
            query,
            stream,
            id,
            agent,
        } => {
            let slug = ui::within("Watch", resolve_agent(&paths, agent.as_deref()))?;
            let entry = Entry {
                source,
                query,
                id,
                stream,
            };
            let settled = edit(&paths, &slug, "Watch Add", |workflows| {
                workflows.entry(workflow).or_default().push(entry);
                Ok(())
            })?;
            (slug, settled)
        }
        Cmd::Remove {
            workflow,
            source,
            agent,
        } => {
            let slug = ui::within("Watch", resolve_agent(&paths, agent.as_deref()))?;
            let settled = edit(&paths, &slug, "Watch Remove", |workflows| {
                let entries = workflows
                    .get_mut(&workflow)
                    .ok_or_else(|| anyhow!("no workflow named `{workflow}`"))?;
                match &source {
                    Some(source) => {
                        let before = entries.len();
                        entries.retain(|entry| &entry.source != source);
                        if entries.len() == before {
                            return Err(anyhow!("`{workflow}` has no `{source}` source"));
                        }
                        if entries.is_empty() {
                            workflows.remove(&workflow);
                        }
                    }
                    None => {
                        workflows.remove(&workflow);
                    }
                }
                Ok(())
            })?;
            (slug, settled)
        }
        Cmd::Reset { agent } => {
            let slug = ui::within("Watch", resolve_agent(&paths, agent.as_deref()))?;
            let settled = reset(&paths, &slug)?;
            (slug, settled)
        }
    };
    if settled == Settled::Done {
        super::apply::config_changed(Some(&slug)).await;
    }
    Ok(())
}

fn list(paths: &GoatPaths, slug: &str) -> Result<()> {
    ui::cell("Watch", || {
        ui::pair("agent", slug);
        let agent = load_agent(paths, slug)?;
        let connections = goat_runtime::load_integration_connections(&paths.config_toml);
        let effective = goat_runtime::effective_watch(&agent, &connections);
        if effective.defaulted {
            ui::line(&ui::dim(
                "no watch section, so each integration's defaults run",
            ));
        }
        ui::blank();
        if effective.workflows.is_empty() {
            ui::line(&ui::dim("nothing is watched"));
            return Ok(Footer::Hint(
                "",
                format!(
                    "goat agent watch add <workflow> --source <connection> --query <query> -a {slug}"
                ),
            ));
        }
        let mut table = Table::new(["workflow", "source", "query", "stream"]);
        for (workflow, sources) in &effective.workflows {
            for (source, spec) in sources {
                table.styled_row(vec![
                    (workflow.clone(), Palette::Provider),
                    (source.clone(), Palette::Value),
                    (spec.query.clone(), Palette::Plain),
                    (
                        if &spec.stream == workflow {
                            String::new()
                        } else {
                            spec.stream.clone()
                        },
                        Palette::Muted,
                    ),
                ]);
            }
        }
        table.render();
        for issue in &effective.issues {
            ui::warning(&format!("  {issue}"));
        }
        Ok(Footer::None)
    })?;
    Ok(())
}

fn edit(
    paths: &GoatPaths,
    slug: &str,
    title: &str,
    change: impl FnOnce(&mut Workflows) -> Result<()>,
) -> Result<Settled> {
    ui::cell(title, || {
        ui::pair("agent", slug);
        let agent = load_agent(paths, slug)?;
        let connections = goat_runtime::load_integration_connections(&paths.config_toml);
        let mut workflows = if let Some(workflows) = declared(paths, slug)? {
            workflows
        } else {
            ui::line(&ui::dim(
                "the defaults become explicit workflows you can edit",
            ));
            defaults(&agent, &connections)
        };
        change(&mut workflows)?;
        let mut candidate = agent.clone();
        candidate.watch = Some(to_config(&workflows));
        let issues = goat_runtime::effective_watch(&candidate, &connections).issues;
        if let Some(issue) = issues.first() {
            return Err(anyhow!("{issue}"));
        }
        write(paths, slug, Some(&workflows))?;
        if workflows.is_empty() {
            ui::line(&ui::dim(&format!(
                "nothing is watched now; `goat agent watch reset -a {slug}` restores the defaults"
            )));
        }
        Ok(Footer::Hint(
            "Saved",
            format!("goat agent watch list -a {slug}"),
        ))
    })
}

fn reset(paths: &GoatPaths, slug: &str) -> Result<Settled> {
    ui::cell("Watch Reset", || {
        ui::pair("agent", slug);
        if declared(paths, slug)?.is_none() {
            ui::line(&ui::dim("the defaults already run"));
            return Ok(Footer::None);
        }
        write(paths, slug, None)?;
        Ok(Footer::Hint(
            "Reset",
            format!("goat agent watch list -a {slug}"),
        ))
    })
}

fn load_agent(paths: &GoatPaths, slug: &str) -> Result<AgentConfig> {
    let loaded = goat_config::load_from(paths.clone())?;
    loaded
        .agents
        .into_iter()
        .find(|agent| agent.slug == slug)
        .ok_or_else(|| anyhow!("agent `{slug}` did not load; run `goat doctor`"))
}

fn declared(paths: &GoatPaths, slug: &str) -> Result<Option<Workflows>> {
    let config = read_agent_config(&paths.agents_dir.join(slug))?;
    match config.get(SECTION) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => Ok(Some(serde_json::from_value(value.clone())?)),
    }
}

fn write(paths: &GoatPaths, slug: &str, workflows: Option<&Workflows>) -> Result<()> {
    let dir = paths.agents_dir.join(slug);
    let mut config = read_agent_config(&dir)?;
    let object = config
        .as_object_mut()
        .ok_or_else(|| anyhow!("the agent config must be a JSON object"))?;
    match workflows {
        Some(workflows) => {
            object.insert(SECTION.to_owned(), serde_json::to_value(workflows)?);
        }
        None => {
            object.remove(SECTION);
        }
    }
    write_agent_config(&dir, &config)
}

fn defaults(agent: &AgentConfig, connections: &goat_integration::Connections) -> Workflows {
    goat_runtime::effective_watch(agent, connections)
        .workflows
        .into_iter()
        .map(|(workflow, sources)| {
            let entries = sources
                .into_iter()
                .map(|(source, spec)| Entry {
                    source,
                    stream: (spec.stream != workflow).then_some(spec.stream),
                    query: spec.query,
                    id: None,
                })
                .collect();
            (workflow, entries)
        })
        .collect()
}

fn to_config(workflows: &Workflows) -> Vec<WatchWorkflow> {
    workflows
        .iter()
        .map(|(name, entries)| WatchWorkflow {
            name: name.clone(),
            sources: entries
                .iter()
                .map(|entry| WatchSourceEntry {
                    source: entry.source.clone(),
                    query: entry.query.clone(),
                    id: entry.id.clone(),
                    stream: entry.stream.clone(),
                })
                .collect(),
        })
        .collect()
}
