use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use crossterm::style::Stylize;
use goat_core::account::{self, Account};
use goat_core::catalog::{self, Model};
use goat_core::mcp::{self, Address, Decision, Server, State};
use goat_core::plugin;
use goat_core::provider::{Endpoints, Provider};
use goat_core::session::{self, Environment, Event, Prompt, Session, Sink};
use goat_core::store::{Recent, Sent, Store};

use crate::ask;
use crate::command::{self, Name};
use crate::input::{self, Editor, Entry, Input, Keys, Mode, RawMode, TurnKeys};
use crate::kitty;
use crate::paths::Paths;
use crate::plugins;
use crate::render::{Banner, Ending, Terminal, Work};
use crate::target::{
    self, Choices, Current, DEFAULT_EFFORT, Effort, Files, Offer, plural, server_line,
};
use crate::usage::{self, Row};

const USER_AGENT: &str = concat!("goat/", env!("CARGO_PKG_VERSION"));
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const TICK: Duration = Duration::from_millis(80);
const RECENT: u32 = 30;

const KEYS: &str = "\
Enter sends, and Alt+Enter or Ctrl+J adds a line
Tab completes a command or its value, and @ completes a path
Esc or Ctrl+C stops a turn, Esc twice goes back to an earlier prompt, and Ctrl+D quits";

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
    recent: Vec<Recent>,
    sent: Vec<Sent>,
    files: Files,
    servers: Vec<(String, State)>,
    environment: Environment,
    database: PathBuf,
    naming: Option<Naming>,
    warned: Vec<String>,
}

struct Naming {
    session: session::Id,
    job: Pin<Box<dyn Future<Output = Option<String>>>>,
}

struct Live<'a, 'b>(&'a RefCell<&'b mut Terminal>);

enum Flow {
    Continue,
    Exit,
}

enum Job<'a> {
    Turn(Prompt<'a>),
    Shell(&'a str),
    Compact(&'a str),
}

impl Chat {
    async fn read(&mut self) -> anyhow::Result<Entry> {
        loop {
            let Self {
                session,
                terminal,
                editor,
                keys,
                offers,
                catalog,
                recent,
                sent,
                files,
                servers,
                naming,
                environment,
                ..
            } = self;
            *recent = session
                .store()
                .recent_sessions(session.workspace(), RECENT)?
                .into_iter()
                .filter(|past| past.id != session.id())
                .collect();
            *sent = session.sent()?;
            *files = Files::new(session.workspace().to_owned());
            *servers = session.servers().0;
            let choices = Choices {
                offers,
                catalog,
                sessions: recent,
                sent,
                files,
                skills: &session.skills().skills,
                servers,
                plugins: &environment.plugins,
                current: current(session),
            };
            let Some(pending) = naming else {
                return Ok(editor.read(keys, &choices, terminal).await?);
            };
            tokio::select! {
                entry = editor.read(keys, &choices, terminal) => return Ok(entry?),
                title = &mut pending.job => {
                    if let Some(title) = title {
                        session.store().set_title(pending.session, &title)?;
                    }
                    *naming = None;
                }
            }
        }
    }

    async fn name(&mut self) {
        if self.naming.is_some() {
            return;
        }
        if let Ok(Some(naming)) = self.session.naming().await {
            self.naming = Some(Naming {
                session: self.session.id(),
                job: Box::pin(naming.run()),
            });
        }
    }

    async fn work(&mut self, job: Job<'_>) -> anyhow::Result<Option<bool>> {
        let Self {
            session,
            terminal,
            editor,
            keys,
            offers,
            catalog,
            recent,
            sent,
            files,
            servers,
            environment,
            ..
        } = self;
        let model = session.model().to_owned();
        let effort = session.effort().map(str::to_owned);
        let skills = session.skills().skills.clone();
        let choices = Choices {
            offers,
            catalog,
            sessions: recent,
            sent,
            files,
            skills: &skills,
            servers,
            plugins: &environment.plugins,
            current: Current {
                account: session.account().id,
                model: &model,
                effort: effort.as_deref(),
            },
        };
        terminal.begin_turn(match job {
            Job::Turn(_) => Work::Turn,
            Job::Shell(_) => Work::Shell,
            Job::Compact(_) => Work::Compact,
        });
        let shared = RefCell::new(terminal);
        let mut done = None;
        let ending = {
            let mut sink = Live(&shared);
            let mut turn = pin!(async {
                match job {
                    Job::Turn(prompt) => session.turn(prompt, &mut sink).await.map(|()| true),
                    Job::Shell(command) => session.shell(command, &mut sink).await.map(|()| true),
                    Job::Compact(focus) => session.compact(focus, &mut sink).await,
                }
            });
            let mut ticks = tokio::time::interval(TICK);
            loop {
                tokio::select! {
                    result = &mut turn => break match result {
                        Ok(changed) => {
                            done = Some(changed);
                            Ending::Complete
                        }
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
        Ok(done)
    }

    async fn input(&mut self, input: &Input) -> anyhow::Result<()> {
        let plain = input.plain();
        let text = plain.trim();
        let job = match input.mode {
            Mode::Agent => Job::Turn(Prompt {
                text,
                images: &input.images,
            }),
            Mode::Shell => Job::Shell(text),
        };
        self.work(job).await?;
        Ok(())
    }

    async fn compact(&mut self, focus: &str) -> anyhow::Result<()> {
        match self.work(Job::Compact(focus)).await? {
            Some(true) => self.terminal.outcome(Ok("Done")),
            Some(false) => self.terminal.outcome(Ok("Nothing to compact yet")),
            None => {}
        }
        Ok(())
    }

    fn clear(&mut self) -> anyhow::Result<()> {
        let mut session = Session::create(
            Store::open(&self.database)?,
            http()?,
            &self.environment,
            self.session.account().clone(),
            self.session.model(),
        )?;
        session.set_effort(self.session.effort());
        self.session = session;
        self.terminal.clear();
        self.greet();
        Ok(())
    }

    fn greet(&mut self) {
        let model = self.status();
        let directory = directory(self.session.workspace(), &self.environment.home);
        self.terminal.banner(&Banner {
            model: &model,
            directory: &directory,
        });
    }

    async fn command(&mut self, name: Name, argument: &str) -> anyhow::Result<Flow> {
        match (name, argument) {
            (Name::Exit, _) => return Ok(Flow::Exit),
            (Name::Clear, _) => self.clear()?,
            (Name::Compact, focus) => self.compact(focus).await?,
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
            (Name::Resume, "") => self
                .terminal
                .reply("Type /resume and pick a session from the list"),
            (Name::Resume, id) => self.resume(id)?,
            (Name::Rewind, picked) => self.rewind(picked).await?,
            (Name::Status, _) => self.show_status()?,
            (Name::Mcp, "") => self.show_servers(),
            (Name::Mcp, name) => self.sign_in(name).await?,
            (Name::Plugin, "") => self.show_plugins()?,
            (Name::Plugin, name) => self.sign_in_plugin(name).await?,
            (Name::Usage, _) => {
                let lines: Vec<String> = self.usage().iter().map(Row::line).collect();
                self.terminal.lines(&lines);
            }
        }
        Ok(Flow::Continue)
    }

    fn switch(&mut self, spec: &str) -> anyhow::Result<()> {
        let choices = self.choices();
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
        let effort = self.choices().effort(level).map_err(anyhow::Error::msg)?;
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
        let choices = self.choices();
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

    fn resume(&mut self, raw: &str) -> anyhow::Result<()> {
        let id: session::Id = raw
            .parse()
            .with_context(|| format!("{raw} is not a session id"))?;
        if id == self.session.id() {
            self.terminal.outcome(Ok("This is the current session"));
            return Ok(());
        }
        let store = Store::open(&self.database)?;
        let opening = reopen(&store, &self.offers, &self.catalog, id, None)?;
        let before = self.status();
        let mut session = Session::resume(
            store,
            http()?,
            &self.environment,
            id,
            opening.account,
            &opening.model,
        )?;
        session.set_effort(opening.effort.as_deref());
        self.session = session;
        let after = self.status();
        let note = if after == before {
            "Resumed the session".to_owned()
        } else {
            format!("Resumed the session on {after}")
        };
        self.terminal.outcome(Ok(&note));
        self.session.replay(&mut self.terminal)?;
        Ok(())
    }

    async fn rewind(&mut self, picked: &str) -> anyhow::Result<()> {
        let sent = self.session.sent()?;
        if picked.is_empty() {
            self.terminal.reply(if sent.is_empty() {
                "There is no earlier prompt to go back to"
            } else {
                "Type /rewind and pick a prompt from the list"
            });
            return Ok(());
        }
        let (number, what) = picked
            .split_once(' ')
            .map_or((picked, ""), |(number, what)| (number, what.trim()));
        let target = number
            .parse::<usize>()
            .ok()
            .and_then(|number| sent.get(number.checked_sub(1)?))
            .with_context(|| format!("{number} is not one of your prompts"))?;
        let (conversation, code) = match what {
            "" | "both" => (true, true),
            "conversation" => (true, false),
            "code" => (false, true),
            other => bail!("{other} is not both, conversation or code"),
        };
        let restored = if code {
            self.session.restore(target.id).await?
        } else {
            Some(Vec::new())
        };
        if conversation {
            self.session.rewind(target.id)?;
            self.terminal.clear();
            self.greet();
            self.session.replay(&mut self.terminal)?;
            self.editor.prefill(&target.text, &target.images);
        }
        let files = match &restored {
            None => {
                "the files have no snapshot from then, so they were left as they are".to_owned()
            }
            Some(paths) if paths.is_empty() => "no files had changed".to_owned(),
            Some(paths) => format!(
                "restored {}",
                plural(u64::try_from(paths.len()).unwrap_or(0), "file")
            ),
        };
        let note = match (conversation, code) {
            (true, true) => format!("Went back to before this prompt; {files}"),
            (true, false) => "Went back to before this prompt".to_owned(),
            (false, _) => format!("Kept the conversation; {files}"),
        };
        self.terminal.outcome(Ok(&note));
        Ok(())
    }

    fn choices(&self) -> Choices<'_> {
        Choices {
            offers: &self.offers,
            catalog: &self.catalog,
            sessions: &self.recent,
            sent: &self.sent,
            files: &self.files,
            skills: &self.session.skills().skills,
            servers: &self.servers,
            plugins: &self.environment.plugins,
            current: current(&self.session),
        }
    }

    fn show_servers(&mut self) {
        let (states, problems) = self.session.servers();
        if states.is_empty() && problems.is_empty() {
            self.terminal
                .reply("No MCP servers. Add one with `goat mcp add <name> <url>`");
            return;
        }
        let mut lines: Vec<String> = states
            .iter()
            .map(|(name, state)| server_line(name, state))
            .collect();
        lines.extend(problems.iter().cloned());
        self.terminal.lines(&lines);
    }

    async fn sign_in(&mut self, name: &str) -> anyhow::Result<()> {
        let sign_in = Box::pin(self.session.sign_in(name)).await?;
        if self.wait_for(name, sign_in).await? {
            self.session.reconnect(name);
            self.terminal.outcome(Ok(&format!("Signed in to {name}")));
        }
        Ok(())
    }

    fn show_plugins(&mut self) -> anyhow::Result<()> {
        let logins = self.session.store().logins()?;
        let lines = plugins::lines(&self.environment.plugins, &logins)?;
        self.terminal.lines(&lines);
        Ok(())
    }

    async fn sign_in_plugin(&mut self, name: &str) -> anyhow::Result<()> {
        let address = Address::parse(name)?;
        let plugin = plugins::find(&self.environment.plugins, &address.name)?;
        let servers: Vec<Server> = plugin.remotes().cloned().collect();
        let logins = Arc::new(self.session.store().logins()?);
        for server in &servers {
            let target = Address {
                name: server.name.clone(),
                account: address.account.clone(),
            };
            let sign_in = Box::pin(mcp::SignIn::start(
                server,
                target.account.as_deref(),
                Arc::clone(&logins),
            ))
            .await?;
            if !self.wait_for(&target.to_string(), sign_in).await? {
                return Ok(());
            }
        }
        self.terminal.outcome(Ok(&format!(
            "Signed in to {address}; start a new session with /clear to use it"
        )));
        Ok(())
    }

    async fn wait_for(&mut self, name: &str, sign_in: mcp::SignIn) -> anyhow::Result<bool> {
        self.terminal.reply(&format!(
            "Opening your browser to sign in to {name}. If it does not open, visit:\n{}\nEsc cancels.",
            sign_in.url()
        ));
        webbrowser::open(sign_in.url()).ok();
        let mut finish = Box::pin(sign_in.finish());
        let finished = loop {
            tokio::select! {
                result = &mut finish => break Some(result),
                event = self.keys.next() => {
                    if input::interrupts(&event?) {
                        break None;
                    }
                }
            }
        };
        let Some(result) = finished else {
            self.terminal.outcome(Ok("Sign-in cancelled"));
            return Ok(false);
        };
        result.with_context(|| format!("sign in to {name}"))?;
        Ok(true)
    }

    fn show_status(&mut self) -> anyhow::Result<()> {
        let id = self.session.id();
        let session = match self.session.title()? {
            Some(title) => format!("{title} ({id})"),
            None => id.to_string(),
        };
        let files = self.session.instruction_files();
        let instructions = if files.is_empty() {
            "none".to_owned()
        } else {
            files.join(", ")
        };
        let (states, problems) = self.session.servers();
        let mut shown: Vec<String> = states
            .iter()
            .map(|(name, state)| server_line(name, state))
            .collect();
        shown.extend(problems.iter().cloned());
        let servers = if shown.is_empty() {
            "none".to_owned()
        } else {
            shown.join(", ")
        };
        let found = self.session.skills();
        let mut skills = match found.skills.len() {
            0 => "none".to_owned(),
            count => count.to_string(),
        };
        if !found.skipped.is_empty() {
            let skipped: Vec<String> = found
                .skipped
                .iter()
                .map(|(path, problem)| format!("{} ({problem})", path.display()))
                .collect();
            skills.push_str("; skipped ");
            skills.push_str(&skipped.join(", "));
        }
        let snapshots = self
            .session
            .snapshots_off()
            .map_or_else(|| "on".to_owned(), |reason| format!("off, {reason}"));
        let rows = [
            ("Session", session),
            ("Model", self.status()),
            (
                "Directory",
                directory(self.session.workspace(), &self.environment.home),
            ),
            ("Instructions", instructions),
            ("Skills", skills),
            ("MCP", servers),
            ("Snapshots", snapshots),
        ];
        let lines: Vec<String> = rows
            .iter()
            .map(|(label, value)| format!("{}  {value}", format!("{label:<12}").dim()))
            .collect();
        self.terminal.lines(&lines);
        Ok(())
    }

    fn usage(&self) -> Vec<Row> {
        usage::rows(self.session.context(), self.session.limits(), input::now())
    }

    fn warn(&mut self) {
        let mut shown = Vec::new();
        for row in self.usage() {
            let seen = self.warned.iter().position(|label| *label == row.label);
            match (row.is_high(), seen) {
                (true, None) => {
                    shown.push(row.line());
                    self.warned.push(row.label);
                }
                (false, Some(at)) => {
                    self.warned.swap_remove(at);
                }
                _ => {}
            }
        }
        if !shown.is_empty() {
            self.terminal.lines(&shown);
        }
    }

    fn status(&self) -> String {
        let session = &self.session;
        let name = self.choices().name(session.account().id, session.model());
        let effort = session
            .effort()
            .map_or_else(String::new, |effort| format!(" with {effort} effort"));
        format!("{name} on {}{effort}", session.account().label())
    }

    fn describe_effort(&self) -> String {
        let choices = self.choices();
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
    review_servers(&store, paths)?;
    let mut session = Session::create(
        store,
        http()?,
        &environment(paths)?,
        opening.account,
        &opening.model,
    )?;
    session.set_effort(opening.effort.as_deref());
    run(session, offers, catalog, paths).await
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
    let opening = reopen(&store, &offers, &catalog, id, choice.model.as_deref())?;
    review_servers(&store, paths)?;
    let mut session = Session::resume(
        store,
        http()?,
        &environment(paths)?,
        id,
        opening.account,
        &opening.model,
    )?;
    session.set_effort(opening.effort.as_deref());
    run(session, offers, catalog, paths).await
}

fn review_servers(store: &Store, paths: &Paths) -> anyhow::Result<()> {
    let reviews = store.mcp_reviews()?;
    let config = mcp::config(&paths.workspace, &paths.home, |_| None, &reviews);
    for file in config.unreviewed {
        ask::say(&format!(
            "This project defines MCP servers in {}. goat runs them only if you allow it:",
            directory(&file.path, &paths.home)
        ))?;
        for line in file.text.lines() {
            ask::say(&format!("  {line}"))?;
        }
        let decision = match ask::choose(
            "Allow these servers? goat asks again when the file changes.",
            &["Allow", "Deny"],
        )? {
            0 => Decision::Allowed,
            _ => Decision::Denied,
        };
        store.save_mcp_review(&file.decide(decision))?;
    }
    Ok(())
}

fn reopen(
    store: &Store,
    offers: &[Offer],
    catalog: &[Model],
    id: session::Id,
    spec: Option<&str>,
) -> anyhow::Result<Opening> {
    let last = store.last_origin(id)?;
    let account = pick_account(store, last.as_ref().map(|origin| origin.account))?;
    let model = match last {
        Some(origin) if origin.account == account.id => origin.model.into(),
        _ => pick_model(store, &account)?,
    };
    open(store, offers, catalog, account, model, spec)
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
        sessions: &[],
        sent: &[],
        files: &Files::listed(Vec::new()),
        skills: &[],
        servers: &[],
        plugins: &[],
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
    paths: &Paths,
) -> anyhow::Result<()> {
    let _raw = RawMode::enable()?;
    let mut terminal = Terminal::new();
    terminal.set_graphics(kitty::detect(std::env::var_os("TMUX").is_some()));
    let mut chat = Chat {
        session,
        terminal,
        editor: Editor::new(Store::open(&paths.database)?),
        keys: Keys::start(),
        offers,
        catalog,
        recent: Vec::new(),
        sent: Vec::new(),
        files: Files::listed(Vec::new()),
        servers: Vec::new(),
        environment: environment(paths)?,
        database: paths.database.clone(),
        naming: None,
        warned: Vec::new(),
    };
    chat.greet();
    chat.session.replay(&mut chat.terminal)?;
    loop {
        chat.terminal.settle();
        let Entry::Input(input) = chat.read().await? else {
            chat.leave();
            return Ok(());
        };
        if input.mode == Mode::Agent
            && let Some(run) = command::parse(&input.text)
        {
            match chat.command(run.name, run.argument).await {
                Ok(Flow::Continue) => {}
                Ok(Flow::Exit) => {
                    chat.leave();
                    return Ok(());
                }
                Err(err) => chat.terminal.outcome(Err(&format!("{err:#}"))),
            }
            continue;
        }
        chat.input(&input).await?;
        chat.warn();
        if input.mode == Mode::Agent {
            chat.name().await;
        }
    }
}

pub(crate) fn directory(workspace: &Path, home: &Path) -> String {
    match workspace.strip_prefix(home) {
        Ok(inside) if inside.as_os_str().is_empty() => "~".to_owned(),
        Ok(inside) => format!("~/{}", inside.display()),
        Err(_) => workspace.display().to_string(),
    }
}

fn current(session: &Session) -> Current<'_> {
    Current {
        account: session.account().id,
        model: session.model(),
        effort: session.effort(),
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

fn environment(paths: &Paths) -> anyhow::Result<Environment> {
    Ok(Environment {
        workspace: paths.workspace.clone(),
        home: paths.home.clone(),
        spill: paths.spill.clone(),
        snapshots: paths.snapshots.clone(),
        personal: paths.personal.clone(),
        variable: |name| std::env::var(name).ok(),
        endpoints: Endpoints::default(),
        plugins: plugin::builtin()?,
    })
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
