use anyhow::{Context, bail};
use clap::Subcommand;
use goat_core::mcp::{self, Address};
use goat_core::plugin::{self, Plugin};
use goat_core::store::Store;

use crate::ask;
use crate::paths::Paths;
use crate::servers;

#[derive(Subcommand)]
pub(crate) enum Action {
    #[command(about = "List plugins, whether they are on, and their sign-ins")]
    List,
    #[command(about = "Turn a plugin on, signing in first when it needs to")]
    Enable { name: String },
    #[command(about = "Turn a plugin off without forgetting its sign-in")]
    Disable { name: String },
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
            for line in lines(&plugins, &store)? {
                ask::say(&line)?;
            }
            Ok(())
        }
        Action::Enable { name } => enable(&store, paths, &plugins, &name).await,
        Action::Disable { name } => {
            let plugin = find(&plugins, &name)?;
            store.set_plugin_enabled(&plugin.name, false)?;
            plugin.forget_grants(&store.logins()?)?;
            ask::say(&format!("Turned {name} off. New sessions will not use it."))?;
            Ok(())
        }
        Action::Login { name } => login(&store, paths, &plugins, &name).await,
        Action::Logout { name } => logout(&store, &plugins, &name),
    }
}

pub(crate) fn lines(plugins: &[Plugin], store: &Store) -> anyhow::Result<Vec<String>> {
    let logins = store.logins()?;
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
                for account in mcp::sign_ins(&logins, server)? {
                    if !accounts.contains(&account) {
                        accounts.push(account);
                    }
                }
            }
            let setting = store.plugin_enabled(&plugin.name)?;
            let mut state = if plugin.is_on(setting, &logins)? {
                "on".to_owned()
            } else {
                "off".to_owned()
            };
            if let Some(accounts) = servers::signed_in(&accounts) {
                state = format!("{state}, {accounts}");
            } else if plugin.needs_sign_in() {
                state.push_str(", not signed in");
            }
            match plugin.missing_permissions(false) {
                Ok(missing) if missing.is_empty() => {}
                Ok(missing) => state = format!("{state}, needs {}", missing.join(", ")),
                Err(_) => state.push_str(", not available here"),
            }
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

async fn enable(
    store: &Store,
    paths: &Paths,
    plugins: &[Plugin],
    name: &str,
) -> anyhow::Result<()> {
    let plugin = find(plugins, name)?;
    let missing = plugin
        .missing_permissions(true)
        .map_err(|problem| anyhow::anyhow!("{name} cannot run here: {problem}"))?;
    if plugin.needs_sign_in() && !plugin.is_signed_in(&store.logins()?)? {
        login(store, paths, plugins, name).await?;
    }
    store.set_plugin_enabled(&plugin.name, true)?;
    ask::say(&format!("Turned {name} on. New sessions will use it."))?;
    if !missing.is_empty() {
        ask::say(&format!(
            "Allow the terminal goat runs in under System Settings > Privacy & Security > {}, then restart the terminal.",
            missing.join(", ")
        ))?;
    }
    Ok(())
}

async fn login(store: &Store, paths: &Paths, plugins: &[Plugin], name: &str) -> anyhow::Result<()> {
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
        servers::sign_in(store, &paths.home, server, &target).await?;
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
