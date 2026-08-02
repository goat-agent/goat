use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::IsTerminal;
use std::path::PathBuf;

use goat_mcp::{McpConfig, ServerConfig};

use crate::cli::{ConflictPolicy, McpScope, SecretPolicy, UnsupportedPolicy};
use crate::mcp::{Context, approve_if_project, open, scope_name};
use crate::mcp_secrets::{has_sensitive_literals, protect_sensitive, redacted, save_with_secrets};
use crate::ui;

pub struct Options {
    pub path: Option<PathBuf>,
    pub scope: Option<McpScope>,
    pub all: bool,
    pub conflict: Option<ConflictPolicy>,
    pub secret: Option<SecretPolicy>,
    pub unsupported: Option<UnsupportedPolicy>,
    pub rename: Vec<String>,
    pub yes: bool,
    pub dry_run: bool,
}

pub fn run(context: &Context, options: Options) -> color_eyre::Result<()> {
    let interactive = is_interactive();
    let source = resolve_path(context, options.path, interactive)?;
    let scope = resolve_scope(options.scope, interactive)?;
    let mut target = open(context, scope)?;
    let source_canonical = source.canonicalize().ok();
    if source_canonical.is_some() && source_canonical == target.path.canonicalize().ok() {
        ui::note("source is already the target MCP config");
        return Ok(());
    }
    let raw = fs::read(&source)
        .map_err(|error| ui::report(format!("could not read {}: {error}", source.display())))?;
    let imported = goat_mcp::parse_import(&raw).map_err(|error| ui::report(error.to_string()))?;
    let renames = parse_renames(&options.rename)?;
    for original in renames.keys() {
        if !imported
            .candidates
            .iter()
            .any(|candidate| &candidate.name == original)
        {
            return Err(ui::report(format!(
                "rename source `{original}` is not present in {}",
                source.display()
            )));
        }
    }

    ui::pair(
        "source",
        &format!("{} ({})", source.display(), imported.format),
    );
    ui::pair(
        "target",
        &format!("{} ({})", target.path.display(), scope_name(scope)),
    );
    for warning in &imported.warnings {
        ui::warning(warning);
    }
    render_candidates(&imported.candidates, &target.config);

    handle_invalid_candidates(&imported.candidates, options.unsupported, interactive)?;
    let selected = select_candidates(
        &imported.candidates,
        &target.config,
        &renames,
        options.all,
        interactive,
    )?;
    let mut additions = Vec::new();
    let mut writes = Vec::new();
    let account = context.account(scope);
    let mut planned = BTreeSet::new();

    for index in selected {
        let candidate = &imported.candidates[index];
        let Some(mut server) = candidate.server.clone() else {
            continue;
        };
        if !candidate.unsupported.is_empty()
            && !accept_unsupported(candidate, options.unsupported, interactive)?
        {
            continue;
        }
        let mut name = renames
            .get(&candidate.name)
            .cloned()
            .unwrap_or_else(|| candidate.name.clone());
        goat_mcp::validate_server_name(&name).map_err(|error| ui::report(error.to_string()))?;
        let mut skipped = false;
        while let Some(existing) = target.config.servers.get(&name) {
            if existing == &server {
                skipped = true;
                break;
            }
            let Some(resolution) =
                resolve_conflict(&name, existing, &server, options.conflict, interactive)?
            else {
                skipped = true;
                break;
            };
            if resolution == name {
                break;
            }
            name = resolution;
        }
        if skipped {
            continue;
        }
        if !planned.insert(name.clone()) {
            return Err(ui::report(format!(
                "imported MCP server name `{name}` is used more than once"
            )));
        }
        let Some(policy) = resolve_secret_policy(&name, &server, options.secret, interactive)?
        else {
            continue;
        };
        writes.extend(protect_sensitive(&name, &account, &mut server, policy)?);
        additions.push((name, server));
    }

    if additions.is_empty() {
        ui::note("nothing to import");
        return Ok(());
    }
    for (name, server) in &additions {
        target.config.servers.insert(name.clone(), server.clone());
    }
    if options.dry_run {
        ui::success(&format!("would import {} MCP server(s)", additions.len()));
        return Ok(());
    }
    if interactive
        && !options.yes
        && !ui::confirm(&format!("Import {} MCP server(s)?", additions.len()), true)?
    {
        ui::note("cancelled");
        return Ok(());
    }
    save_with_secrets(&mut target, &context.credentials(), &writes)?;
    for (name, server) in &additions {
        approve_if_project(context, scope, name, server)?;
    }
    ui::success(&format!("imported {} MCP server(s)", additions.len()));
    Ok(())
}

fn render_candidates(candidates: &[goat_mcp::ImportCandidate], target: &McpConfig) {
    let mut table = ui::Table::new(["server", "status", "detail"]);
    for candidate in candidates {
        let (status, detail) = candidate_status(candidate, target);
        table.row([candidate.name.clone(), status, detail]);
    }
    table.render();
}

fn candidate_status(candidate: &goat_mcp::ImportCandidate, target: &McpConfig) -> (String, String) {
    if let Some(error) = &candidate.error {
        return ("unavailable".to_owned(), error.clone());
    }
    let Some(server) = &candidate.server else {
        return ("unavailable".to_owned(), "invalid entry".to_owned());
    };
    let unsupported = if candidate.unsupported.is_empty() {
        String::new()
    } else {
        format!("unsupported: {}", candidate.unsupported.join(", "))
    };
    match target.servers.get(&candidate.name) {
        None => ("new".to_owned(), unsupported),
        Some(existing) if existing == server => ("same".to_owned(), unsupported),
        Some(_) => ("conflict".to_owned(), unsupported),
    }
}

fn handle_invalid_candidates(
    candidates: &[goat_mcp::ImportCandidate],
    policy: Option<UnsupportedPolicy>,
    interactive: bool,
) -> color_eyre::Result<()> {
    let invalid: Vec<_> = candidates
        .iter()
        .filter(|candidate| !candidate.usable())
        .collect();
    if invalid.is_empty() || interactive || policy == Some(UnsupportedPolicy::Skip) {
        return Ok(());
    }
    Err(ui::report_hint(
        format!("{} imported server(s) cannot be used", invalid.len()),
        "pass `--on-unsupported skip` to ignore them",
    ))
}

fn select_candidates(
    candidates: &[goat_mcp::ImportCandidate],
    target: &McpConfig,
    renames: &BTreeMap<String, String>,
    all: bool,
    interactive: bool,
) -> color_eyre::Result<Vec<usize>> {
    let selectable: Vec<_> = candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| {
            candidate.usable()
                && candidate.server.as_ref().is_some_and(|server| {
                    let name = renames.get(&candidate.name).unwrap_or(&candidate.name);
                    target.servers.get(name) != Some(server)
                })
        })
        .collect();
    if all {
        return Ok(selectable.into_iter().map(|(index, _)| index).collect());
    }
    if !interactive {
        return Err(ui::report_hint(
            "non-interactive import needs an explicit server selection",
            "pass `--all` to import every usable server",
        ));
    }
    let labels: Vec<_> = selectable
        .iter()
        .map(|(_, candidate)| {
            let (status, _) = candidate_status(candidate, target);
            format!("{}  ({status})", candidate.name)
        })
        .collect();
    let defaults: Vec<_> = selectable
        .iter()
        .map(|(_, candidate)| candidate.unsupported.is_empty())
        .collect();
    let Some(selected) = ui::select_indices("servers to import", &labels, &defaults)? else {
        return Ok(Vec::new());
    };
    Ok(selected
        .into_iter()
        .map(|index| selectable[index].0)
        .collect())
}

fn accept_unsupported(
    candidate: &goat_mcp::ImportCandidate,
    policy: Option<UnsupportedPolicy>,
    interactive: bool,
) -> color_eyre::Result<bool> {
    match policy {
        Some(UnsupportedPolicy::Accept) => Ok(true),
        Some(UnsupportedPolicy::Skip) => Ok(false),
        Some(UnsupportedPolicy::Error) => Err(ui::report(format!(
            "MCP server `{}` has unsupported fields: {}",
            candidate.name,
            candidate.unsupported.join(", ")
        ))),
        None if !interactive => Err(ui::report_hint(
            format!(
                "MCP server `{}` has unsupported fields: {}",
                candidate.name,
                candidate.unsupported.join(", ")
            ),
            "pass `--on-unsupported accept` or `--on-unsupported skip`",
        )),
        None => {
            let labels = [
                "import the supported fields".to_owned(),
                "skip this server".to_owned(),
            ];
            Ok(ui::select_index(
                &format!("unsupported fields in `{}`", candidate.name),
                &labels,
            )? == Some(0))
        }
    }
}

fn resolve_conflict(
    name: &str,
    existing: &ServerConfig,
    imported: &ServerConfig,
    policy: Option<ConflictPolicy>,
    interactive: bool,
) -> color_eyre::Result<Option<String>> {
    match policy {
        Some(ConflictPolicy::Replace) => Ok(Some(name.to_owned())),
        Some(ConflictPolicy::Skip) => Ok(None),
        Some(ConflictPolicy::Error) => Err(ui::report(format!(
            "MCP server `{name}` conflicts with the target config"
        ))),
        None if !interactive => Err(ui::report_hint(
            format!("MCP server `{name}` conflicts with the target config"),
            "pass `--on-conflict error`, `skip`, or `replace`, or `--rename old=new`",
        )),
        None => loop {
            let labels = [
                "keep Goat version".to_owned(),
                "replace with imported version".to_owned(),
                "import under another name".to_owned(),
                "show differences".to_owned(),
            ];
            match ui::select_index(&format!("conflict for `{name}`"), &labels)? {
                Some(0) | None => return Ok(None),
                Some(1) => return Ok(Some(name.to_owned())),
                Some(2) => {
                    let Some(rename) = ui::prompt("new server name", None)? else {
                        continue;
                    };
                    goat_mcp::validate_server_name(&rename)
                        .map_err(|error| ui::report(error.to_string()))?;
                    return Ok(Some(rename));
                }
                Some(3) => show_difference(existing, imported)?,
                Some(_) => {}
            }
        },
    }
}

fn show_difference(existing: &ServerConfig, imported: &ServerConfig) -> color_eyre::Result<()> {
    ui::section("Goat");
    ui::raw(&serde_json::to_string_pretty(&redacted(existing))?);
    ui::section("Imported");
    ui::raw(&serde_json::to_string_pretty(&redacted(imported))?);
    Ok(())
}

fn resolve_secret_policy(
    name: &str,
    server: &ServerConfig,
    policy: Option<SecretPolicy>,
    interactive: bool,
) -> color_eyre::Result<Option<SecretPolicy>> {
    if !has_sensitive_literals(server) {
        return Ok(Some(SecretPolicy::Literal));
    }
    if let Some(policy) = policy {
        if policy == SecretPolicy::Error {
            return Err(ui::report(format!(
                "MCP server `{name}` contains literal credentials"
            )));
        }
        return Ok(Some(policy));
    }
    if !interactive {
        return Err(ui::report_hint(
            format!("MCP server `{name}` contains literal credentials"),
            "pass `--on-secret store`, `literal`, `omit`, or `error`",
        ));
    }
    let labels = [
        "store privately in Goat credentials".to_owned(),
        "keep literal values in mcp.json".to_owned(),
        "omit the credential values".to_owned(),
        "skip this import".to_owned(),
    ];
    match ui::select_index(&format!("credentials in `{name}`"), &labels)? {
        Some(0) => Ok(Some(SecretPolicy::Store)),
        Some(1) => Ok(Some(SecretPolicy::Literal)),
        Some(2) => Ok(Some(SecretPolicy::Omit)),
        _ => Ok(None),
    }
}

fn resolve_path(
    context: &Context,
    requested: Option<PathBuf>,
    interactive: bool,
) -> color_eyre::Result<PathBuf> {
    if let Some(path) = requested {
        return Ok(path);
    }
    if !interactive {
        return Err(ui::report_hint(
            "non-interactive import needs a source path",
            "pass a file path, for example `goat mcp import .mcp.json`",
        ));
    }
    let candidates: Vec<_> = [
        context.project_root.join(".mcp.json"),
        context.project_root.join(".vscode").join("mcp.json"),
    ]
    .into_iter()
    .filter(|path| path.exists())
    .collect();
    match candidates.as_slice() {
        [only] => Ok(only.clone()),
        [] => {
            let Some(path) = ui::prompt("MCP config to import", Some(".mcp.json"))? else {
                return Err(ui::report("cancelled"));
            };
            Ok(PathBuf::from(path))
        }
        _ => {
            let labels: Vec<_> = candidates
                .iter()
                .map(|path| path.display().to_string())
                .collect();
            let Some(index) = ui::select_index("MCP config to import", &labels)? else {
                return Err(ui::report("cancelled"));
            };
            Ok(candidates[index].clone())
        }
    }
}

fn resolve_scope(requested: Option<McpScope>, interactive: bool) -> color_eyre::Result<McpScope> {
    if let Some(scope) = requested {
        return Ok(scope);
    }
    if !interactive {
        return Err(ui::report_hint(
            "non-interactive import needs a target scope",
            "pass `--scope project` or `--scope user`",
        ));
    }
    let labels = [
        "project  .goat/mcp.json".to_owned(),
        "user     ~/.goat/mcp.json".to_owned(),
    ];
    Ok(match ui::select_index("import into", &labels)? {
        Some(0) => McpScope::Project,
        Some(1) => McpScope::User,
        _ => return Err(ui::report("cancelled")),
    })
}

fn parse_renames(values: &[String]) -> color_eyre::Result<BTreeMap<String, String>> {
    let mut renames = BTreeMap::new();
    let mut destinations = BTreeSet::new();
    for value in values {
        let Some((old, new)) = value.split_once('=') else {
            return Err(ui::report(format!(
                "invalid rename `{value}`; expected OLD=NEW"
            )));
        };
        goat_mcp::validate_server_name(old).map_err(|error| ui::report(error.to_string()))?;
        goat_mcp::validate_server_name(new).map_err(|error| ui::report(error.to_string()))?;
        if renames.insert(old.to_owned(), new.to_owned()).is_some() {
            return Err(ui::report(format!("rename for `{old}` was provided twice")));
        }
        if !destinations.insert(new.to_owned()) {
            return Err(ui::report(format!(
                "more than one server was renamed to `{new}`"
            )));
        }
    }
    Ok(renames)
}

fn is_interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}
