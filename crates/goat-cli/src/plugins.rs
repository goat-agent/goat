use anyhow::{Context, bail};
use clap::Subcommand;
use goat_core::mcp::{self, Address};
use goat_core::plugin::{self, Plugin};
use goat_core::store::{Logins, Store};

use crate::ask;
use crate::paths::Paths;
use crate::servers;

#[derive(Subcommand)]
pub(crate) enum Action {
    #[command(about = "List plugins and their sign-ins")]
    List,
    #[command(about = "Sign in to a plugin in the browser; name#account adds another account")]
    Login { name: String },
    #[command(about = "Forget the sign-in to a plugin")]
    Logout { name: String },
}

pub(crate) async fn run(paths: &Paths, action: Action) -> anyhow::Result<()> {
    let store = Store::open(&paths.database)?;
    let plugins = plugin::builtin()?;
    match action {
        Action::List => {
            for line in lines(&plugins, &store.logins()?)? {
                ask::say(&line)?;
            }
            Ok(())
        }
        Action::Login { name } => login(&store, &plugins, &name).await,
        Action::Logout { name } => logout(&store, &plugins, &name),
    }
}

pub(crate) fn lines(plugins: &[Plugin], logins: &Logins) -> anyhow::Result<Vec<String>> {
    if plugins.is_empty() {
        return Ok(vec!["No plugins are built in yet.".to_owned()]);
    }
    let width = plugins
        .iter()
        .map(|plugin| plugin.name.len())
        .max()
        .unwrap_or(0);
    plugins
        .iter()
        .map(|plugin| {
            let mut accounts = Vec::new();
            for server in plugin.remotes() {
                for account in mcp::sign_ins(logins, server)? {
                    if !accounts.contains(&account) {
                        accounts.push(account);
                    }
                }
            }
            let state = servers::signed_in(&accounts).unwrap_or_else(|| "not signed in".to_owned());
            Ok(format!(
                "{:width$}  {state}  {}",
                plugin.name, plugin.description
            ))
        })
        .collect()
}

pub(crate) fn find<'a>(plugins: &'a [Plugin], name: &str) -> anyhow::Result<&'a Plugin> {
    plugins
        .iter()
        .find(|plugin| plugin.name == name)
        .with_context(|| format!("no plugin named {name}; see `goat plugin list`"))
}

async fn login(store: &Store, plugins: &[Plugin], name: &str) -> anyhow::Result<()> {
    let address = Address::parse(name)?;
    let plugin = find(plugins, &address.name)?;
    let remotes: Vec<_> = plugin.remotes().collect();
    if remotes.is_empty() {
        bail!("{} has nothing to sign in to", plugin.name);
    }
    for server in remotes {
        let target = Address {
            name: server.name.clone(),
            account: address.account.clone(),
        };
        servers::sign_in(store, server, &target).await?;
    }
    Ok(())
}

fn logout(store: &Store, plugins: &[Plugin], name: &str) -> anyhow::Result<()> {
    let address = Address::parse(name)?;
    let plugin = find(plugins, &address.name)?;
    let logins = store.logins()?;
    let mut is_signed_out = false;
    for server in plugin.remotes() {
        is_signed_out |= mcp::sign_out(&logins, server, address.account.as_deref())?;
    }
    let text = if is_signed_out {
        format!("Signed out of {address}.")
    } else {
        format!("{address} was not signed in.")
    };
    ask::say(&text)?;
    Ok(())
}
