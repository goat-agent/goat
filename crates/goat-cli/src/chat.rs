use anyhow::{Context, bail};
use goat_core::account::Account;
use goat_core::provider::{Endpoints, Provider};
use goat_core::session::{self, Environment, Session};
use goat_core::store::Store;

use crate::input::{Editor, Entry};
use crate::login;
use crate::paths::Paths;
use crate::render::Terminal;

const USER_AGENT: &str = concat!("goat/", env!("CARGO_PKG_VERSION"));
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

const HELP: &str = "\
/model NAME    switch model        /models     list models
/account NAME  switch account      /accounts   list accounts
/exit          quit";

pub(crate) struct Choice {
    pub(crate) model: Option<String>,
    pub(crate) account: Option<String>,
}

pub(crate) async fn start(paths: &Paths, choice: Choice) -> anyhow::Result<()> {
    let store = Store::open(&paths.database)?;
    let account = pick_account(&store, choice.account.as_deref(), None)?;
    let model = pick_model(&store, &account, choice.model)?;
    let session = Session::create(store, http()?, &environment(paths), account, &model)?;
    run(session).await
}

pub(crate) async fn resume(paths: &Paths, choice: Choice, id: Option<&str>) -> anyhow::Result<()> {
    let store = Store::open(&paths.database)?;
    let id: session::Id = match id {
        Some(raw) => raw
            .parse()
            .with_context(|| format!("{raw} is not a session id"))?,
        None => {
            store
                .latest_session(&paths.workspace)?
                .context("no session in this directory yet; start one with `goat`")?
                .id
        }
    };
    let last = store.last_origin(id)?;
    let account = pick_account(
        &store,
        choice.account.as_deref(),
        last.as_ref().map(|o| o.account),
    )?;
    let model = match (choice.model, last) {
        (Some(model), _) => model,
        (None, Some(origin)) if origin.account == account.id => origin.model.into(),
        (None, _) => pick_model(&store, &account, None)?,
    };
    let session = Session::resume(store, http()?, &environment(paths), id, account, &model)?;
    run(session).await
}

async fn run(mut session: Session) -> anyhow::Result<()> {
    let mut terminal = Terminal::new();
    let mut editor = Editor::default();
    terminal.note(&status(&session));
    loop {
        let Entry::Line(line) = editor.read()? else {
            return Ok(());
        };
        let line = line.trim();
        if let Some(command) = line.strip_prefix('/').filter(|c| is_command(c)) {
            match command_result(&mut session, &mut terminal, command).await {
                Ok(Flow::Continue) => {}
                Ok(Flow::Exit) => return Ok(()),
                Err(err) => terminal.error(&format!("{err:#}")),
            }
            continue;
        }
        tokio::select! {
            result = session.turn(line, &mut terminal) => {
                terminal.finish_turn();
                if let Err(err) = result {
                    terminal.error(&format!("{:#}", anyhow::Error::new(err)));
                }
            }
            _ = tokio::signal::ctrl_c() => {
                terminal.finish_turn();
                terminal.note("interrupted");
            }
        }
    }
}

const COMMANDS: &[&str] = &[
    "exit", "quit", "model", "models", "account", "accounts", "help",
];

fn is_command(input: &str) -> bool {
    COMMANDS.contains(&input.split_whitespace().next().unwrap_or_default())
}

enum Flow {
    Continue,
    Exit,
}

async fn command_result(
    session: &mut Session,
    terminal: &mut Terminal,
    command: &str,
) -> anyhow::Result<Flow> {
    let (name, argument) = command.split_once(' ').unwrap_or((command, ""));
    let argument = argument.trim();
    match (name, argument) {
        ("exit" | "quit", _) => return Ok(Flow::Exit),
        ("model", "") => terminal.note(&status(session)),
        ("model", model) => {
            session.set_model(model);
            terminal.note(&status(session));
        }
        ("models", _) => {
            let models = session.models().await?;
            terminal.note(&models.join("\n"));
        }
        ("account", "") | ("accounts", _) => {
            let current = session.account().id;
            for account in session.store().accounts()? {
                let marker = if account.id == current { "*" } else { " " };
                terminal.note(&format!("{marker} {}", account.label()));
            }
        }
        ("account", label) => {
            let account = login::find(session.store(), label)?;
            let model = pick_model(session.store(), &account, None)?;
            session.set_account(account, &model);
            terminal.note(&status(session));
        }
        _ => terminal.note(HELP),
    }
    Ok(Flow::Continue)
}

fn status(session: &Session) -> String {
    format!(
        "goat · {} · {} · {}",
        session.model(),
        session.account().label(),
        session.workspace().display()
    )
}

fn pick_account(
    store: &Store,
    label: Option<&str>,
    preferred: Option<goat_core::account::Id>,
) -> anyhow::Result<Account> {
    if let Some(label) = label {
        return login::find(store, label);
    }
    let accounts = store.accounts()?;
    if accounts.is_empty() {
        bail!("no accounts yet; add one with `goat login`");
    }
    let wanted = preferred.or(store.last_used_account()?);
    let index = wanted
        .and_then(|id| accounts.iter().position(|account| account.id == id))
        .unwrap_or(0);
    Ok(accounts
        .into_iter()
        .nth(index)
        .unwrap_or_else(|| unreachable!("accounts is not empty")))
}

fn pick_model(store: &Store, account: &Account, chosen: Option<String>) -> anyhow::Result<String> {
    if let Some(model) = chosen {
        return Ok(model);
    }
    Ok(store.last_model(account.id)?.map_or_else(
        || Provider::default_model(account.kind).to_owned(),
        String::from,
    ))
}

fn environment(paths: &Paths) -> Environment {
    Environment {
        workspace: paths.workspace.clone(),
        home: paths.home.clone(),
        spill: paths.spill.clone(),
        endpoints: Endpoints::default(),
    }
}

pub(crate) fn http() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(CONNECT_TIMEOUT)
        .build()?)
}
