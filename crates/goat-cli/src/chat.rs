use anyhow::{Context, bail};
use goat_core::account::Account;
use goat_core::provider::{Endpoints, Model, Provider};
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
/effort LEVEL  set effort          /effort     show effort levels
/account NAME  switch account      /accounts   list accounts
/exit          quit";

const DEFAULT_EFFORT: &str = "default";

pub(crate) struct Choice {
    pub(crate) model: Option<String>,
    pub(crate) account: Option<String>,
    pub(crate) effort: Option<String>,
}

pub(crate) async fn start(paths: &Paths, choice: Choice) -> anyhow::Result<()> {
    let store = Store::open(&paths.database)?;
    let account = pick_account(&store, choice.account.as_deref(), None)?;
    let model = pick_model(&store, &account, choice.model)?;
    let session = Session::create(store, http()?, &environment(paths), account, &model)?;
    run(session, choice.effort.as_deref()).await
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
    run(session, choice.effort.as_deref()).await
}

async fn run(mut session: Session, effort: Option<&str>) -> anyhow::Result<()> {
    let mut terminal = Terminal::new();
    let mut editor = Editor::default();
    if let Some(effort) = effort
        && let Err(err) = choose_effort(&mut session, effort).await
    {
        terminal.error(&format!("{err:#}"));
    }
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
    "exit", "quit", "model", "models", "effort", "account", "accounts", "help",
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
            keep_supported_effort(session, terminal).await?;
            terminal.note(&status(session));
        }
        ("models", _) => {
            let models = session.models().await?;
            let lines: Vec<String> = models.iter().map(describe_model).collect();
            terminal.note(&lines.join("\n"));
        }
        ("effort", "") => {
            let model = current_model(session).await?;
            terminal.note(&describe_effort(session.effort(), model.as_ref()));
        }
        ("effort", effort) => {
            choose_effort(session, effort).await?;
            terminal.note(&status(session));
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
            keep_supported_effort(session, terminal).await?;
            terminal.note(&status(session));
        }
        _ => terminal.note(HELP),
    }
    Ok(Flow::Continue)
}

async fn current_model(session: &mut Session) -> anyhow::Result<Option<Model>> {
    let models = session.models().await?;
    Ok(models
        .into_iter()
        .find(|model| *model.id == *session.model()))
}

async fn choose_effort(session: &mut Session, effort: &str) -> anyhow::Result<()> {
    if effort == DEFAULT_EFFORT {
        session.set_effort(None);
        return Ok(());
    }
    if let Some(model) = current_model(session).await?
        && !model.accepts(effort)
    {
        bail!("{} supports effort {}", model.id, levels(&model));
    }
    session.set_effort(Some(effort));
    Ok(())
}

async fn keep_supported_effort(
    session: &mut Session,
    terminal: &mut Terminal,
) -> anyhow::Result<()> {
    let Some(effort) = session.effort().map(str::to_owned) else {
        return Ok(());
    };
    if let Some(model) = current_model(session).await?
        && !model.accepts(&effort)
    {
        session.set_effort(None);
        terminal.note(&format!(
            "{} has no effort {effort}; using its default",
            model.id
        ));
    }
    Ok(())
}

fn levels(model: &Model) -> String {
    match &model.efforts {
        Some(efforts) if efforts.is_empty() => "none".to_owned(),
        Some(efforts) => efforts.join(" "),
        None => "unknown".to_owned(),
    }
}

fn describe_effort(effort: Option<&str>, model: Option<&Model>) -> String {
    let default = model.and_then(|model| model.default_effort.as_deref());
    let current = match (effort, default) {
        (Some(effort), _) => effort.to_owned(),
        (None, Some(default)) => format!("{DEFAULT_EFFORT} ({default})"),
        (None, None) => DEFAULT_EFFORT.to_owned(),
    };
    match model.filter(|model| model.efforts.is_some()) {
        Some(model) => format!("effort {current} · {}", levels(model)),
        None => format!("effort {current}"),
    }
}

fn describe_model(model: &Model) -> String {
    match &model.default_effort {
        Some(default) => format!("{} · effort {default}", model.id),
        None => model.id.to_string(),
    }
}

fn status(session: &Session) -> String {
    let effort = session
        .effort()
        .map_or_else(String::new, |effort| format!(" · effort {effort}"));
    format!(
        "goat · {}{effort} · {} · {}",
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sol() -> Model {
        Model {
            id: "gpt-6.2-sol".into(),
            efforts: Some(vec!["low".into(), "high".into()]),
            default_effort: Some("low".into()),
        }
    }

    #[test]
    fn unset_effort_shows_the_model_default_and_levels() {
        assert_eq!(
            describe_effort(None, Some(&sol())),
            "effort default (low) · low high"
        );
    }

    #[test]
    fn chosen_effort_is_shown_as_is() {
        assert_eq!(
            describe_effort(Some("high"), Some(&sol())),
            "effort high · low high"
        );
    }

    #[test]
    fn effort_of_an_unlisted_model_shows_no_levels() {
        assert_eq!(describe_effort(None, None), "effort default");
    }
}
