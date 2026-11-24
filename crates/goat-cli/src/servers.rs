use std::fmt::Write as _;
use std::sync::Arc;

use anyhow::{Context, bail};
use clap::{Args, Subcommand};
use goat_core::mcp::{self, Address, Config, OauthClient, Server, Source, State, Transport};
use goat_core::store::Store;

use crate::ask;
use crate::chat::directory;
use crate::paths::Paths;

#[derive(Subcommand)]
pub(crate) enum Action {
    #[command(about = "Add a server reached at a URL, or started by the command after --")]
    Add(Addition),
    #[command(about = "Remove a server and its sign-in")]
    Remove { name: String },
    #[command(about = "Sign in to a server in the browser; name#account adds another account")]
    Login { name: String },
    #[command(about = "Forget the sign-in to a server")]
    Logout { name: String },
}

#[derive(Args)]
pub(crate) struct Addition {
    name: String,
    url: Option<String>,
    #[arg(
        long = "project",
        help = "Add it to this project's .goat/mcp.toml instead of ~/.goat/mcp.toml"
    )]
    is_project: bool,
    #[arg(
        short = 'H',
        long = "header",
        value_name = "NAME: VALUE",
        help = "Send a header to the URL; $NAME in the value is read from the environment"
    )]
    headers: Vec<String>,
    #[arg(
        short = 'e',
        long = "env",
        value_name = "NAME=VALUE",
        help = "Set an environment variable for the command; $NAME is read from the environment"
    )]
    env: Vec<String>,
    #[arg(
        long,
        value_name = "ID",
        help = "Sign in with this registered OAuth client instead of registering one"
    )]
    client_id: Option<String>,
    #[arg(
        long,
        value_name = "SECRET",
        requires = "client_id",
        help = "Secret of the OAuth client; write $NAME to read it from the environment"
    )]
    client_secret: Option<String>,
    #[arg(
        long,
        value_name = "PORT",
        help = "Port of the redirect URI registered for the OAuth client"
    )]
    callback_port: Option<u16>,
    #[arg(last = true, value_name = "COMMAND")]
    command: Vec<String>,
}

pub(crate) async fn run(paths: &Paths, action: Option<Action>) -> anyhow::Result<()> {
    let store = Store::open(&paths.database)?;
    match action {
        None => list(&store, paths),
        Some(Action::Add(addition)) => add(&store, paths, addition).await,
        Some(Action::Remove { name }) => remove(&store, paths, &name),
        Some(Action::Login { name }) => login(&store, paths, &name).await,
        Some(Action::Logout { name }) => logout(&store, paths, &name),
    }
}

fn list(store: &Store, paths: &Paths) -> anyhow::Result<()> {
    let config = load(store, paths)?;
    if config.servers.is_empty() && config.unreviewed.is_empty() && config.problems.is_empty() {
        return Ok(ask::say(
            "No MCP servers. Add one with `goat mcp add <name> <url>`.",
        )?);
    }
    let logins = store.logins()?;
    let personal = mcp::personal_file(&paths.home);
    let width = config
        .servers
        .iter()
        .map(|server| server.name.len())
        .max()
        .unwrap_or(0);
    for server in &config.servers {
        let mut line = format!("{:width$}  {}", server.name, target(server));
        if let Some(accounts) = signed_in(&mcp::sign_ins(&logins, server)?) {
            write!(line, "  {accounts}").ok();
        }
        if let Source::File(file) = &server.source
            && *file != personal
        {
            write!(line, "  {}", directory(file, &paths.home)).ok();
        }
        ask::say(&line)?;
    }
    for file in &config.unreviewed {
        ask::say(&format!(
            "{} is waiting for review; start goat to review it",
            directory(&file.path, &paths.home)
        ))?;
    }
    for problem in &config.problems {
        ask::say(problem)?;
    }
    Ok(())
}

async fn add(store: &Store, paths: &Paths, addition: Addition) -> anyhow::Result<()> {
    let Addition {
        name,
        url,
        is_project,
        headers,
        env,
        client_id,
        client_secret,
        callback_port,
        command,
    } = addition;
    let transport = match (url, command.split_first()) {
        (Some(_), Some(_)) => bail!("give a url or a command after --, not both"),
        (None, None) => bail!("give a url, or a command after --"),
        (Some(url), None) => {
            if !env.is_empty() {
                bail!("--env is for a command; a url takes --header");
            }
            Transport::Url {
                url,
                headers: header_pairs(&headers)?,
                client: client_id.map(|id| OauthClient {
                    id,
                    secret: client_secret,
                    port: callback_port,
                }),
            }
        }
        (None, Some((program, args))) => {
            if !headers.is_empty() || client_id.is_some() || callback_port.is_some() {
                bail!("--header, --client-id and --callback-port are for a url");
            }
            Transport::Command {
                program: program.clone(),
                args: args.to_vec(),
                env: pairs(&env)?,
            }
        }
    };
    let file = if is_project {
        mcp::project_file(&paths.workspace)
    } else {
        mcp::personal_file(&paths.home)
    };
    mcp::add(store, &file, &name, &transport)?;
    ask::say(&format!(
        "Added {name} to {}.",
        directory(&file, &paths.home)
    ))?;
    let config = load(store, paths)?;
    let Some(server) = config
        .servers
        .iter()
        .find(|server| server.name == name && matches!(server.transport, Transport::Url { .. }))
    else {
        return Ok(());
    };
    let state = mcp::check(server, &crate::chat::http()?, Arc::new(store.logins()?)).await;
    match state {
        State::NeedsSignIn => ask::say(&format!(
            "{name} needs sign-in; run `goat mcp login {name}`."
        ))?,
        State::Failed(problem) => ask::say(&problem)?,
        State::Connected | State::Idle => {}
    }
    Ok(())
}

fn remove(store: &Store, paths: &Paths, name: &str) -> anyhow::Result<()> {
    let config = load(store, paths)?;
    let server = find(&config, name)?;
    mcp::remove(store, server)?;
    let shown = match &server.source {
        Source::File(file) => directory(file, &paths.home),
        Source::Plugin(plugin) => format!("the {plugin} plugin"),
    };
    ask::say(&format!("Removed {name} from {shown}."))?;
    Ok(())
}

async fn login(store: &Store, paths: &Paths, name: &str) -> anyhow::Result<()> {
    let address = Address::parse(name)?;
    let config = load(store, paths)?;
    let server = find(&config, &address.name)?;
    sign_in(store, server, &address).await
}

fn logout(store: &Store, paths: &Paths, name: &str) -> anyhow::Result<()> {
    let address = Address::parse(name)?;
    let config = load(store, paths)?;
    let server = find(&config, &address.name)?;
    let text = if mcp::sign_out(&store.logins()?, server, address.account.as_deref())? {
        format!("Signed out of {address}.")
    } else {
        format!("{address} was not signed in.")
    };
    ask::say(&text)?;
    Ok(())
}

pub(crate) async fn sign_in(
    store: &Store,
    server: &Server,
    address: &Address,
) -> anyhow::Result<()> {
    let sign_in = Box::pin(mcp::SignIn::start(
        server,
        address.account.as_deref(),
        Arc::new(store.logins()?),
    ))
    .await?;
    ask::say(&format!(
        "Opening your browser to sign in to {address}. If it does not open, visit:"
    ))?;
    ask::say(sign_in.url())?;
    webbrowser::open(sign_in.url()).ok();
    Box::pin(sign_in.finish())
        .await
        .with_context(|| format!("sign in to {address}"))?;
    ask::say(&format!("Signed in to {address}."))?;
    Ok(())
}

pub(crate) fn signed_in(accounts: &[Option<String>]) -> Option<String> {
    if accounts.is_empty() {
        return None;
    }
    let names: Vec<&str> = accounts
        .iter()
        .map(|account| account.as_deref().unwrap_or("default"))
        .collect();
    Some(format!("signed in ({})", names.join(", ")))
}

fn load(store: &Store, paths: &Paths) -> anyhow::Result<Config> {
    Ok(mcp::config(
        &paths.workspace,
        &paths.home,
        |name| std::env::var(name).ok(),
        &store.mcp_reviews()?,
    ))
}

fn find<'a>(config: &'a Config, name: &str) -> anyhow::Result<&'a Server> {
    config
        .servers
        .iter()
        .find(|server| server.name == name)
        .with_context(|| format!("no MCP server named {name}; see `goat mcp`"))
}

fn target(server: &Server) -> &str {
    match &server.transport {
        Transport::Url { url, .. } => url,
        Transport::Command { program, .. } => program,
    }
}

fn header_pairs(raw: &[String]) -> anyhow::Result<Vec<(String, String)>> {
    raw.iter()
        .map(|header| {
            header
                .split_once(':')
                .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
                .with_context(|| format!("{header} is not NAME: VALUE"))
        })
        .collect()
}

fn pairs(raw: &[String]) -> anyhow::Result<Vec<(String, String)>> {
    raw.iter()
        .map(|pair| {
            pair.split_once('=')
                .map(|(key, value)| (key.to_owned(), value.to_owned()))
                .with_context(|| format!("{pair} is not NAME=VALUE"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounts_read_as_default_and_their_names() {
        let shown = signed_in(&[None, Some("work".to_owned())]);

        assert_eq!(shown.as_deref(), Some("signed in (default, work)"));
    }

    #[test]
    fn a_pair_without_an_equals_sign_is_rejected() {
        let err = pairs(&["TOKEN".to_owned()]).unwrap_err();

        assert_eq!(err.to_string(), "TOKEN is not NAME=VALUE");
    }

    #[test]
    fn a_header_is_written_as_in_curl() {
        let parsed = header_pairs(&["Authorization: Bearer a:b".to_owned()]).unwrap();

        assert_eq!(
            parsed,
            [("Authorization".to_owned(), "Bearer a:b".to_owned())]
        );
    }

    #[test]
    fn a_header_without_a_colon_is_rejected() {
        let err = header_pairs(&["Authorization=Bearer x".to_owned()]).unwrap_err();

        assert_eq!(err.to_string(), "Authorization=Bearer x is not NAME: VALUE");
    }

    #[test]
    fn a_value_keeps_its_equals_signs() {
        let parsed = pairs(&["QUERY=a=b".to_owned()]).unwrap();

        assert_eq!(parsed, [("QUERY".to_owned(), "a=b".to_owned())]);
    }
}
