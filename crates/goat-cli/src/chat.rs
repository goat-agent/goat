use std::cell::RefCell;
use std::path::Path;
use std::pin::pin;
use std::time::Duration;

use anyhow::{Context, bail};
use goat_core::account::{self, Account};
use goat_core::catalog::{self, Model};
use goat_core::provider::{Endpoints, Provider};
use goat_core::session::{self, Environment, Event, Session, Sink};
use goat_core::store::Store;

use crate::command::{self, Name};
use crate::input::{Editor, Entry, Input, Keys, Mode, RawMode, TurnKeys};
use crate::paths::Paths;
use crate::render::{Banner, Ending, Terminal, Work};
use crate::target::{self, Choices, Current, DEFAULT_EFFORT, Effort, Offer};

const USER_AGENT: &str = concat!("goat/", env!("CARGO_PKG_VERSION"));
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const TICK: Duration = Duration::from_millis(80);

const KEYS: &str = "\
Enter sends, and Alt+Enter or Ctrl+J adds a line
Tab completes a command or its value
Esc or Ctrl+C stops a turn, and Ctrl+D quits";

pub(crate) struct Choice {
    pub(crate) model: Option<String>,
}

struct Opening {
    account: Account,
    model: String,
    effort: Option<String>,
}

struct Chat {
    session: Session,
    terminal: Terminal,
    editor: Editor,
    keys: Keys,
    offers: Vec<Offer>,
    catalog: Vec<Model>,
}

struct Live<'a, 'b>(&'a RefCell<&'b mut Terminal>);

enum Flow {
    Continue,
    Exit,
}

impl Chat {
    async fn read(&mut self) -> anyhow::Result<Entry> {
        let Self {
            session,
            terminal,
            editor,
            keys,
            offers,
            catalog,
        } = self;
        let choices = choices(session, offers, catalog);
        Ok(editor.read(keys, &choices, terminal).await?)
    }

    async fn work(&mut self, input: &Input) -> anyhow::Result<()> {
        let Self {
            session,
            terminal,
            editor,
            keys,
            offers,
            catalog,
        } = self;
        let model = session.model().to_owned();
        let effort = session.effort().map(str::to_owned);
        let choices = Choices {
            offers,
            catalog,
            current: Current {
                account: session.account().id,
                model: &model,
                effort: effort.as_deref(),
            },
        };
        let text = input.text.trim();
        terminal.begin_turn(match input.mode {
            Mode::Agent => Work::Turn,
            Mode::Shell => Work::Shell,
        });
        let shared = RefCell::new(terminal);
        let ending = {
            let mut sink = Live(&shared);
            let mut turn = pin!(async {
                match input.mode {
                    Mode::Agent => session.turn(text, &mut sink).await,
                    Mode::Shell => session.shell(text, &mut sink).await,
                }
            });
            let mut ticks = tokio::time::interval(TICK);
            loop {
                tokio::select! {
                    result = &mut turn => break match result {
                        Ok(()) => Ending::Complete,
                        Err(err) => Ending::Failed(format!("{:#}", anyhow::Error::new(err))),
                    },
                    event = keys.next() => {
                        let mut terminal = shared.borrow_mut();
                        if let TurnKeys::Interrupt = editor.during_turn(event?, &choices, &mut terminal) {
                            break Ending::Interrupted;
                        }
                    }
                    _ = ticks.tick() => shared.borrow_mut().tick(),
                }
            }
        };
        if !matches!(ending, Ending::Complete) {
            editor.unqueue();
        }
        shared.into_inner().finish_turn(ending);
        Ok(())
    }

    fn command(&mut self, name: Name, argument: &str) -> anyhow::Result<Flow> {
        match (name, argument) {
            (Name::Exit, _) => return Ok(Flow::Exit),
            (Name::Help, _) => self.terminal.reply(&help()),
            (Name::Model, "") => {
                let status = self.status();
                self.terminal.reply(&status);
            }
            (Name::Model, spec) => self.switch(spec)?,
            (Name::Effort, "") => {
                let effort = self.describe_effort();
                self.terminal.reply(&effort);
            }
            (Name::Effort, level) => self.set_effort(level)?,
        }
        Ok(Flow::Continue)
    }

    fn switch(&mut self, spec: &str) -> anyhow::Result<()> {
        let choices = choices(&self.session, &self.offers, &self.catalog);
        let selection = choices.resolve(spec).map_err(anyhow::Error::msg)?;
        let name = choices.name(selection.account, &selection.model);
        let is_new_account = selection.account != self.session.account().id;
        if is_new_account {
            let account = self
                .session
                .store()
                .account(selection.account)?
                .context("the account was removed")?;
            self.session.set_account(account, &selection.model);
        } else {
            self.session.set_model(&selection.model);
        }
        let effort = match selection.effort {
            Effort::Level(level) => {
                self.session.set_effort(Some(&level));
                Some(level)
            }
            Effort::Default => {
                self.session.set_effort(None);
                Some(DEFAULT_EFFORT.to_owned())
            }
            Effort::Keep => self
                .drop_unsupported_effort()
                .then(|| DEFAULT_EFFORT.to_owned()),
        };
        let account = if is_new_account {
            format!(" on {}", self.session.account().label())
        } else {
            String::new()
        };
        let effort = effort.map_or_else(String::new, |effort| format!(" with {effort} effort"));
        let result = format!("Set model to {name}{account}{effort}");
        self.terminal.outcome(Ok(&result));
        Ok(())
    }

    fn set_effort(&mut self, level: &str) -> anyhow::Result<()> {
        let effort = choices(&self.session, &self.offers, &self.catalog)
            .effort(level)
            .map_err(anyhow::Error::msg)?;
        match effort {
            Effort::Level(level) => self.session.set_effort(Some(&level)),
            Effort::Default | Effort::Keep => self.session.set_effort(None),
        }
        self.terminal.outcome(Ok(&format!("Set effort to {level}")));
        Ok(())
    }

    fn drop_unsupported_effort(&mut self) -> bool {
        let Some(effort) = self.session.effort() else {
            return false;
        };
        let choices = choices(&self.session, &self.offers, &self.catalog);
        let is_supported = choices
            .current_model()
            .is_none_or(|model| model.accepts(effort));
        if !is_supported {
            self.session.set_effort(None);
        }
        !is_supported
    }

    fn leave(&mut self) {
        self.terminal.close();
        if self.session.messages().is_empty() {
            return;
        }
        self.terminal
            .farewell(&format!("goat --resume {}", self.session.id()));
    }

    fn status(&self) -> String {
        let session = &self.session;
        let name = choices(session, &self.offers, &self.catalog)
            .name(session.account().id, session.model());
        let effort = session
            .effort()
            .map_or_else(String::new, |effort| format!(" with {effort} effort"));
        format!("{name} on {}{effort}", session.account().label())
    }

    fn describe_effort(&self) -> String {
        let choices = choices(&self.session, &self.offers, &self.catalog);
        let effort = self.session.effort().unwrap_or(DEFAULT_EFFORT);
        match choices
            .current_model()
            .filter(|model| model.efforts.is_some())
        {
            Some(model) => format!(
                "Effort is {effort}. {} offers {}.",
                model.name(),
                target::levels(model)
            ),
            None => format!("Effort is {effort}"),
        }
    }
}

impl Sink for Live<'_, '_> {
    fn emit(&mut self, event: Event<'_>) {
        self.0.borrow_mut().emit(event);
    }
}

pub(crate) async fn start(paths: &Paths, choice: Choice) -> anyhow::Result<()> {
    let store = Store::open(&paths.database)?;
    let catalog = catalog::models()?;
    let offers = offers(&store)?;
    let account = pick_account(&store, None)?;
    let model = pick_model(&store, &account)?;
    let opening = open(
        &store,
        &offers,
        &catalog,
        account,
        model,
        choice.model.as_deref(),
    )?;
    let mut session = Session::create(
        store,
        http()?,
        &environment(paths),
        opening.account,
        &opening.model,
    )?;
    session.set_effort(opening.effort.as_deref());
    run(session, offers, catalog, &paths.home).await
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
    let catalog = catalog::models()?;
    let offers = offers(&store)?;
    let last = store.last_origin(id)?;
    let account = pick_account(&store, last.as_ref().map(|origin| origin.account))?;
    let model = match last {
        Some(origin) if origin.account == account.id => origin.model.into(),
        _ => pick_model(&store, &account)?,
    };
    let opening = open(
        &store,
        &offers,
        &catalog,
        account,
        model,
        choice.model.as_deref(),
    )?;
    let mut session = Session::resume(
        store,
        http()?,
        &environment(paths),
        id,
        opening.account,
        &opening.model,
    )?;
    session.set_effort(opening.effort.as_deref());
    run(session, offers, catalog, &paths.home).await
}

fn offers(store: &Store) -> anyhow::Result<Vec<Offer>> {
    Ok(store
        .accounts()?
        .iter()
        .map(|account| Offer {
            account: account.id,
            kind: account.kind,
            label: account.label(),
        })
        .collect())
}

fn open(
    store: &Store,
    offers: &[Offer],
    catalog: &[Model],
    account: Account,
    model: String,
    spec: Option<&str>,
) -> anyhow::Result<Opening> {
    let Some(spec) = spec else {
        return Ok(Opening {
            account,
            model,
            effort: None,
        });
    };
    let choices = Choices {
        offers,
        catalog,
        current: Current {
            account: account.id,
            model: &model,
            effort: None,
        },
    };
    let selection = choices.resolve(spec).map_err(anyhow::Error::msg)?;
    let account = store
        .account(selection.account)?
        .context("the account was removed")?;
    let effort = match selection.effort {
        Effort::Level(level) => Some(level),
        Effort::Keep | Effort::Default => None,
    };
    Ok(Opening {
        account,
        model: selection.model,
        effort,
    })
}

async fn run(
    session: Session,
    offers: Vec<Offer>,
    catalog: Vec<Model>,
    home: &Path,
) -> anyhow::Result<()> {
    let _raw = RawMode::enable()?;
    let mut chat = Chat {
        session,
        terminal: Terminal::new(),
        editor: Editor::default(),
        keys: Keys::start(),
        offers,
        catalog,
    };
    let model = chat.status();
    let directory = directory(chat.session.workspace(), home);
    chat.terminal.banner(&Banner {
        model: &model,
        directory: &directory,
    });
    loop {
        chat.terminal.settle();
        let Entry::Input(input) = chat.read().await? else {
            chat.leave();
            return Ok(());
        };
        if input.mode == Mode::Agent
            && let Some(run) = command::parse(&input.text)
        {
            match chat.command(run.name, run.argument) {
                Ok(Flow::Continue) => {}
                Ok(Flow::Exit) => {
                    chat.leave();
                    return Ok(());
                }
                Err(err) => chat.terminal.outcome(Err(&format!("{err:#}"))),
            }
            continue;
        }
        chat.work(&input).await?;
    }
}

fn directory(workspace: &Path, home: &Path) -> String {
    match workspace.strip_prefix(home) {
        Ok(inside) if inside.as_os_str().is_empty() => "~".to_owned(),
        Ok(inside) => format!("~/{}", inside.display()),
        Err(_) => workspace.display().to_string(),
    }
}

fn choices<'a>(session: &'a Session, offers: &'a [Offer], catalog: &'a [Model]) -> Choices<'a> {
    Choices {
        offers,
        catalog,
        current: Current {
            account: session.account().id,
            model: session.model(),
            effort: session.effort(),
        },
    }
}

fn help() -> String {
    let mut lines: Vec<String> = command::SPECS
        .iter()
        .map(|spec| format!("/{:<10}{}", spec.word, spec.about))
        .collect();
    lines.push(String::new());
    lines.push(KEYS.to_owned());
    lines.join("\n")
}

fn pick_account(store: &Store, preferred: Option<account::Id>) -> anyhow::Result<Account> {
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

fn pick_model(store: &Store, account: &Account) -> anyhow::Result<String> {
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

    #[test]
    fn a_directory_under_home_starts_with_a_tilde() {
        let shown = directory(Path::new("/home/me/work/goat"), Path::new("/home/me"));

        assert_eq!(shown, "~/work/goat");
    }

    #[test]
    fn home_itself_is_a_tilde() {
        let shown = directory(Path::new("/home/me"), Path::new("/home/me"));

        assert_eq!(shown, "~");
    }

    #[test]
    fn a_directory_outside_home_is_shown_in_full() {
        let shown = directory(Path::new("/srv/goat"), Path::new("/home/me"));

        assert_eq!(shown, "/srv/goat");
    }
}
