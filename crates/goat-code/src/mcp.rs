use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use goat_auth::{Credential, CredentialKey, CredentialService, CredentialStore};
use goat_mcp::{Approvals, ConfigFile, HttpConfig, Scope, ServerConfig, StdioConfig, ValueSource};

use crate::cli::{McpCommand, McpScope, SecretPolicy};
use crate::mcp_secrets::{
    protect_sensitive, redacted, redacted_url, save_with_secrets, server_summary, shell_words,
    value_names,
};
use crate::ui;

pub async fn run(command: McpCommand) -> color_eyre::Result<()> {
    let context = Context::load()?;
    match command {
        McpCommand::List { scope, json } => list(&context, scope, json),
        McpCommand::Get { name, scope, json } => get(&context, &name, scope, json),
        McpCommand::Add {
            name,
            scope,
            url,
            env,
            bearer_token_env_var,
            force,
            command,
        } => add(
            &context,
            &name,
            AddOptions {
                scope,
                url,
                env,
                bearer_token_env_var,
                force,
                command,
            },
        ),
        McpCommand::Remove { name, scope } => remove(&context, &name, scope),
        McpCommand::Import {
            path,
            scope,
            all,
            on_conflict,
            on_secret,
            on_unsupported,
            rename,
            yes,
            dry_run,
        } => crate::mcp_import::run(
            &context,
            crate::mcp_import::Options {
                path,
                scope,
                all,
                conflict: on_conflict,
                secret: on_secret,
                unsupported: on_unsupported,
                rename,
                yes,
                dry_run,
            },
        ),
        McpCommand::Login { name, scope } => login(&context, &name, scope).await,
        McpCommand::Logout { name, scope } => logout(&context, &name, scope),
    }
}

pub fn approve_project_servers(cwd: &Path) -> color_eyre::Result<()> {
    let context = Context::load_from(cwd.to_path_buf())?;
    let path = goat_mcp::project_config_path(&context.project_root);
    if !path.exists() {
        return Ok(());
    }
    let file = match ConfigFile::open(path) {
        Ok(file) => file,
        Err(error) => {
            ui::warning(&format!("project MCP config was skipped: {error}"));
            return Ok(());
        }
    };
    let mut approvals = Approvals::load(context.paths.mcp_approvals_json.clone())
        .map_err(|error| ui::report(error.to_string()))?;
    for (name, server) in &file.config.servers {
        if approvals.approved(&context.project_root, name, server) {
            continue;
        }
        ui::blank();
        ui::pair(
            "project MCP",
            &format!("{name}  {}", server_summary(server)),
        );
        if ui::confirm(&format!("Connect project MCP `{name}`?"), false)? {
            approvals
                .approve(&context.project_root, name, server)
                .map_err(|error| ui::report(error.to_string()))?;
        }
    }
    Ok(())
}

pub(crate) struct Context {
    pub(crate) paths: goat_config::GoatPaths,
    pub(crate) project_root: PathBuf,
}

impl Context {
    fn load() -> color_eyre::Result<Self> {
        Self::load_from(std::env::current_dir()?)
    }

    fn load_from(cwd: PathBuf) -> color_eyre::Result<Self> {
        let paths = goat_config::GoatPaths::default_layout()
            .map_err(|error| ui::report(error.to_string()))?;
        let project_root =
            goat_worktree::workspace(&cwd).map_or(cwd, |workspace| workspace.repo_root);
        Ok(Self {
            paths,
            project_root,
        })
    }

    fn path(&self, scope: McpScope) -> PathBuf {
        match scope {
            McpScope::User => self.paths.mcp_json.clone(),
            McpScope::Project => goat_mcp::project_config_path(&self.project_root),
        }
    }

    pub(crate) fn account(&self, scope: McpScope) -> String {
        Scope::from(scope).account(&self.project_root)
    }

    pub(crate) fn credentials(&self) -> CredentialStore {
        CredentialStore::new(self.paths.credentials_json.clone())
    }
}

impl From<McpScope> for Scope {
    fn from(value: McpScope) -> Self {
        match value {
            McpScope::User => Self::User,
            McpScope::Project => Self::Project,
        }
    }
}

#[derive(Clone)]
struct Entry {
    scope: McpScope,
    server: ServerConfig,
}

fn entries(
    context: &Context,
    filter: Option<McpScope>,
) -> color_eyre::Result<BTreeMap<String, Entry>> {
    let mut entries = BTreeMap::new();
    if filter != Some(McpScope::Project) {
        for (name, server) in open(context, McpScope::User)?.config.servers {
            entries.insert(
                name,
                Entry {
                    scope: McpScope::User,
                    server,
                },
            );
        }
    }
    if filter != Some(McpScope::User) {
        for (name, server) in open(context, McpScope::Project)?.config.servers {
            entries.insert(
                name,
                Entry {
                    scope: McpScope::Project,
                    server,
                },
            );
        }
    }
    Ok(entries)
}

pub(crate) fn open(context: &Context, scope: McpScope) -> color_eyre::Result<ConfigFile> {
    ConfigFile::open(context.path(scope)).map_err(|error| ui::report(error.to_string()))
}

fn list(context: &Context, scope: Option<McpScope>, json: bool) -> color_eyre::Result<()> {
    let entries = entries(context, scope)?;
    if json {
        let output: BTreeMap<_, _> = entries
            .into_iter()
            .map(|(name, entry)| {
                (
                    name,
                    serde_json::json!({
                        "scope": scope_name(entry.scope),
                        "config": redacted(&entry.server),
                    }),
                )
            })
            .collect();
        ui::raw(&serde_json::to_string_pretty(&output)?);
        return Ok(());
    }
    if entries.is_empty() {
        ui::note("no MCP servers configured");
        return Ok(());
    }
    let approvals = Approvals::load(context.paths.mcp_approvals_json.clone())
        .map_err(|error| ui::report(error.to_string()))?;
    let mut table = ui::Table::new(["server", "scope", "transport", "status"]);
    for (name, entry) in entries {
        let status = if entry.scope == McpScope::Project
            && !approvals.approved(&context.project_root, &name, &entry.server)
        {
            "pending approval"
        } else {
            "configured"
        };
        table.row([
            name,
            scope_name(entry.scope).to_owned(),
            server_summary(&entry.server),
            status.to_owned(),
        ]);
    }
    table.render();
    Ok(())
}

fn get(
    context: &Context,
    name: &str,
    scope: Option<McpScope>,
    json: bool,
) -> color_eyre::Result<()> {
    let entry = resolve_entry(context, name, scope)?;
    if json {
        ui::raw(&serde_json::to_string_pretty(&redacted(&entry.server))?);
        return Ok(());
    }
    ui::pair("server", name);
    ui::pair("scope", scope_name(entry.scope));
    match &entry.server {
        ServerConfig::Stdio(server) => {
            ui::pair("transport", "stdio");
            ui::pair("command", &shell_words(&server.command, &server.args));
            if !server.env.is_empty() {
                ui::pair("env", &value_names(&server.env));
            }
        }
        ServerConfig::Http(server) => {
            ui::pair("transport", "http");
            ui::pair("url", &redacted_url(&server.url));
            if !server.headers.is_empty() {
                ui::pair("headers", &value_names(&server.headers));
            }
            if let Some(variable) = &server.bearer_token_env_var {
                ui::pair("bearer", &format!("environment `{variable}`"));
            }
        }
    }
    Ok(())
}

struct AddOptions {
    scope: McpScope,
    url: Option<String>,
    env: Vec<String>,
    bearer_token_env_var: Option<String>,
    force: bool,
    command: Vec<String>,
}

fn add(context: &Context, name: &str, options: AddOptions) -> color_eyre::Result<()> {
    let AddOptions {
        scope,
        url,
        env,
        bearer_token_env_var,
        force,
        command,
    } = options;
    goat_mcp::validate_server_name(name).map_err(|error| ui::report(error.to_string()))?;
    let server = match (url, command.split_first()) {
        (Some(_), Some(_)) => {
            return Err(ui::report("choose either `--url` or a command after `--`"));
        }
        (None, None) => {
            return Err(ui::report_hint(
                "an MCP server needs `--url <url>` or a command after `--`",
                "example: goat mcp add context7 -- npx -y @upstash/context7-mcp",
            ));
        }
        (Some(url), None) => {
            if !env.is_empty() {
                return Err(ui::report("`--env` is only valid for stdio MCP servers"));
            }
            reqwest::Url::parse(&url)
                .map_err(|error| ui::report(format!("invalid URL: {error}")))?;
            ServerConfig::Http(HttpConfig {
                url,
                headers: BTreeMap::new(),
                bearer_token_env_var,
            })
        }
        (None, Some((command, args))) => {
            if bearer_token_env_var.is_some() {
                return Err(ui::report(
                    "`--bearer-token-env-var` is only valid with `--url`",
                ));
            }
            ServerConfig::Stdio(StdioConfig {
                command: command.clone(),
                args: args.iter().cloned().map(ValueSource::Literal).collect(),
                env: parse_env(env)?,
            })
        }
    };
    let mut file = open(context, scope)?;
    if file.config.servers.contains_key(name)
        && !force
        && is_interactive()
        && !ui::confirm(&format!("Replace MCP server `{name}`?"), false)?
    {
        return Ok(());
    }
    if file.config.servers.contains_key(name) && !force && !is_interactive() {
        return Err(ui::report_hint(
            format!(
                "MCP server `{name}` already exists in {} scope",
                scope_name(scope)
            ),
            "pass `--force` to replace it non-interactively",
        ));
    }
    let account = context.account(scope);
    let mut server = server;
    let writes = protect_sensitive(name, &account, &mut server, SecretPolicy::Store)?;
    file.config.servers.insert(name.to_owned(), server.clone());
    save_with_secrets(&mut file, &context.credentials(), &writes)?;
    approve_if_project(context, scope, name, &server)?;
    ui::success(&format!(
        "added MCP server `{name}` ({})",
        scope_name(scope)
    ));
    Ok(())
}

fn remove(context: &Context, name: &str, scope: McpScope) -> color_eyre::Result<()> {
    let mut file = open(context, scope)?;
    if file.config.servers.remove(name).is_none() {
        ui::warning(&format!(
            "no MCP server `{name}` in {} scope",
            scope_name(scope)
        ));
        return Ok(());
    }
    file.save().map_err(|error| ui::report(error.to_string()))?;
    let credentials = context.credentials();
    let account = context.account(scope);
    for (key, _) in credentials.entries() {
        if key.service == CredentialService::Mcp && key.provider == name && key.account == account {
            credentials
                .remove(&key)
                .map_err(|error| ui::report(error.to_string()))?;
        }
    }
    if scope == McpScope::Project {
        Approvals::load(context.paths.mcp_approvals_json.clone())
            .and_then(|mut approvals| approvals.revoke(&context.project_root, name))
            .map_err(|error| ui::report(error.to_string()))?;
    }
    ui::success(&format!(
        "removed MCP server `{name}` ({})",
        scope_name(scope)
    ));
    Ok(())
}

pub(crate) fn approve_if_project(
    context: &Context,
    scope: McpScope,
    name: &str,
    server: &ServerConfig,
) -> color_eyre::Result<()> {
    if scope != McpScope::Project {
        return Ok(());
    }
    Approvals::load(context.paths.mcp_approvals_json.clone())
        .and_then(|mut approvals| approvals.approve(&context.project_root, name, server))
        .map_err(|error| ui::report(error.to_string()))
}

async fn login(context: &Context, name: &str, scope: Option<McpScope>) -> color_eyre::Result<()> {
    let entry = resolve_entry(context, name, scope)?;
    let ServerConfig::Http(server) = entry.server else {
        return Err(ui::report(
            "OAuth login is only available for HTTP MCP servers",
        ));
    };
    let authorization = goat_mcp::auth::run_login(&server.url, &[], &|url| {
        ui::pair("approve in browser", url);
        let _ = open::that(url);
    })
    .await
    .map_err(|error| ui::report(error.to_string()))?;
    context
        .credentials()
        .store(
            &CredentialKey::mcp(name, context.account(entry.scope), "oauth"),
            Credential::OAuth(authorization.tokens),
        )
        .map_err(|error| ui::report(error.to_string()))?;
    ui::success(&format!("signed in to MCP server `{name}`"));
    Ok(())
}

fn logout(context: &Context, name: &str, scope: Option<McpScope>) -> color_eyre::Result<()> {
    let entry = resolve_entry(context, name, scope)?;
    let key = CredentialKey::mcp(name, context.account(entry.scope), "oauth");
    if context
        .credentials()
        .remove(&key)
        .map_err(|error| ui::report(error.to_string()))?
    {
        ui::success(&format!("signed out of MCP server `{name}`"));
    } else {
        ui::warning(&format!("no OAuth login found for MCP server `{name}`"));
    }
    Ok(())
}

fn resolve_entry(
    context: &Context,
    name: &str,
    scope: Option<McpScope>,
) -> color_eyre::Result<Entry> {
    entries(context, scope)?.remove(name).ok_or_else(|| {
        ui::report_hint(
            format!("unknown MCP server `{name}`"),
            "run `goat mcp list` to see configured servers",
        )
    })
}

fn parse_env(values: Vec<String>) -> color_eyre::Result<BTreeMap<String, ValueSource>> {
    let mut env = BTreeMap::new();
    for value in values {
        let Some((name, value)) = value.split_once('=') else {
            return Err(ui::report(format!(
                "invalid environment value `{value}`; expected KEY=VALUE"
            )));
        };
        if name.is_empty() {
            return Err(ui::report("environment variable name must not be empty"));
        }
        if env.contains_key(name) {
            return Err(ui::report(format!(
                "environment variable `{name}` was provided more than once"
            )));
        }
        let source = value
            .strip_prefix("${env:")
            .and_then(|value| value.strip_suffix('}'))
            .filter(|value| !value.is_empty())
            .map_or_else(
                || ValueSource::Literal(value.to_owned()),
                |env| ValueSource::Env {
                    env: env.to_owned(),
                },
            );
        env.insert(name.to_owned(), source);
    }
    Ok(env)
}

pub(crate) fn scope_name(scope: McpScope) -> &'static str {
    match scope {
        McpScope::User => "user",
        McpScope::Project => "project",
    }
}

fn is_interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_literal_and_host_environment_values() {
        let parsed =
            parse_env(vec!["A=B".to_owned(), "TOKEN=${env:REAL_TOKEN}".to_owned()]).unwrap();
        assert_eq!(parsed.get("A"), Some(&ValueSource::Literal("B".to_owned())));
        assert_eq!(
            parsed.get("TOKEN"),
            Some(&ValueSource::Env {
                env: "REAL_TOKEN".to_owned()
            })
        );
    }

    #[test]
    fn duplicate_environment_values_are_rejected() {
        assert!(parse_env(vec!["A=1".to_owned(), "A=2".to_owned()]).is_err());
    }
}
