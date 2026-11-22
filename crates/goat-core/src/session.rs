mod prompt;
mod replay;
mod snapshot;

use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

use crate::account::Account;
use crate::image::{Image, Picture};
use crate::instructions;
use crate::message::{self, Diff, Message, Origin, Part, Role, Trace};
use crate::provider::{self, Chunk, Endpoints, Images, Provider, Request, Stop, Usage};
use crate::store::{self, Sent, Store};
use crate::tool::{self, Outcome, Runner, Searcher, Step, Web};

use self::snapshot::Snapshots;

crate::id::define!(Id);

const COMPACT_AT_TOKENS: u64 = 200_000;
const INTERRUPTED: &str = "interrupted by the user before this run finished";
const CALL_CHARS: usize = 100;
const IMAGES_PER_REQUEST: usize = 20;
const IMAGE_BATCH: usize = 10;
const TITLE_CHARS: usize = 80;
const NAMING_CHARS: usize = 2000;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("store")]
    Store(#[from] store::Error),
    #[error("model")]
    Provider(#[from] provider::Error),
    #[error("sign-in")]
    Auth(#[from] crate::account::Error),
    #[error("tools")]
    Tool(#[from] tool::Error),
    #[error("session {0} not found")]
    NotFound(Id),
    #[error("could not restore the files: {0}")]
    Restore(String),
}

#[derive(Debug, Clone, Copy)]
pub enum Event<'a> {
    Prompt(&'a str),
    ShellCommand(&'a str),
    Text(&'a str),
    Reasoning(&'a str),
    RunStarted {
        description: &'a str,
    },
    Call(&'a tool::Call),
    CallFailed {
        problem: &'a str,
    },
    Changed(&'a Diff),
    Loaded {
        path: &'a str,
    },
    Untracked {
        reason: &'a str,
    },
    RunFinished {
        outcome: &'a Outcome,
    },
    Truncated,
    Refused,
    ShellOutput(&'a str),
    ShellFinished {
        code: Option<i32>,
        problem: Option<&'a str>,
    },
    Usage(Usage),
    Retrying {
        attempt: u32,
        restarts: bool,
    },
    Compacting,
}

#[derive(Debug)]
pub struct Naming {
    provider: Provider,
    bearer: Box<str>,
    origin: Origin,
    conversation: String,
}

#[derive(Debug, Clone, Copy)]
pub struct Prompt<'a> {
    pub text: &'a str,
    pub images: &'a [Picture],
}

pub trait Sink {
    fn emit(&mut self, event: Event<'_>);
}

#[derive(Debug, Clone)]
pub struct Environment {
    pub workspace: std::path::PathBuf,
    pub home: std::path::PathBuf,
    pub spill: std::path::PathBuf,
    pub snapshots: std::path::PathBuf,
    pub personal: std::path::PathBuf,
    pub endpoints: Endpoints,
}

#[derive(Debug)]
pub struct Session {
    id: Id,
    store: Store,
    snapshots: Snapshots,
    http: reqwest::Client,
    endpoints: Endpoints,
    provider: Provider,
    account: Account,
    model: Box<str>,
    effort: Option<Box<str>>,
    instructions: Box<str>,
    runner: Runner,
    tool: tool::Spec,
    messages: Vec<Message>,
    finished: Vec<Part>,
    trace: Vec<Trace>,
    last_input: u64,
    personal: PathBuf,
    home: PathBuf,
}

impl Session {
    pub fn create(
        store: Store,
        http: reqwest::Client,
        env: &Environment,
        account: Account,
        model: &str,
    ) -> Result<Self, Error> {
        let instructions = prompt::instructions(&env.workspace, &env.personal, &env.home);
        let id = store.create_session(&env.workspace, &instructions)?;
        Self::assemble(
            id,
            store,
            http,
            env,
            account,
            model,
            instructions.into(),
            Vec::new(),
        )
    }

    pub fn resume(
        store: Store,
        http: reqwest::Client,
        env: &Environment,
        id: Id,
        account: Account,
        model: &str,
    ) -> Result<Self, Error> {
        let info = store.session(id)?.ok_or(Error::NotFound(id))?;
        let messages = store.context(id)?;
        let session = Self::assemble(
            id,
            store,
            http,
            env,
            account,
            model,
            info.instructions,
            messages,
        )?;
        let loaded = session
            .messages
            .iter()
            .flat_map(|message| &message.parts)
            .filter_map(|part| match part {
                Part::ToolResult { trace, .. } => Some(trace),
                _ => None,
            })
            .flatten()
            .filter_map(|step| match step {
                Trace::Loaded { path } => Some(env.workspace.join(&**path)),
                _ => None,
            });
        for path in loaded {
            session.runner.mark_loaded(path);
        }
        Ok(session)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "private constructor shared by create and resume"
    )]
    fn assemble(
        id: Id,
        store: Store,
        http: reqwest::Client,
        env: &Environment,
        account: Account,
        model: &str,
        instructions: Box<str>,
        messages: Vec<Message>,
    ) -> Result<Self, Error> {
        Ok(Self {
            id,
            provider: Provider::new(&account, http.clone(), &env.endpoints),
            store,
            snapshots: Snapshots::new(&env.snapshots, &env.workspace, &env.home, id),
            http,
            endpoints: env.endpoints.clone(),
            account,
            model: model.into(),
            effort: None,
            instructions,
            runner: Runner::new(&env.workspace, &env.home, &env.spill, !messages.is_empty())?,
            tool: tool::spec(),
            messages,
            finished: Vec::new(),
            trace: Vec::new(),
            last_input: 0,
            personal: env.personal.clone(),
            home: env.home.clone(),
        })
    }

    pub fn id(&self) -> Id {
        self.id
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn effort(&self) -> Option<&str> {
        self.effort.as_deref()
    }

    pub fn account(&self) -> &Account {
        &self.account
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn workspace(&self) -> &Path {
        self.runner.workspace()
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn set_model(&mut self, model: &str) {
        self.model = model.into();
    }

    pub fn set_effort(&mut self, effort: Option<&str>) {
        self.effort = effort.map(Into::into);
    }

    pub fn set_account(&mut self, account: Account, model: &str) {
        self.provider = Provider::new(&account, self.http.clone(), &self.endpoints);
        self.account = account;
        self.model = model.into();
    }

    pub async fn turn<'p>(
        &mut self,
        prompt: impl Into<Prompt<'p>>,
        sink: &mut impl Sink,
    ) -> Result<(), Error> {
        let prompt = prompt.into();
        self.close_interrupted_calls()?;
        if self.last_input > COMPACT_AT_TOKENS {
            sink.emit(Event::Compacting);
            self.summarize("").await?;
        }
        let mut parts = vec![Part::text(prompt.text)];
        parts.extend(self.keep(prompt.images)?.into_iter().map(Part::Image));
        parts.extend(self.mentions(prompt.text)?);
        let message = Message::new(Role::User, None, parts);
        let tree = self.snapshots.take().await;
        if let Some(reason) = self.snapshots.notice() {
            sink.emit(Event::Untracked { reason: &reason });
        }
        let id = message.id;
        self.push(message)?;
        if let Some(tree) = tree {
            self.store.put_snapshot(id, &tree)?;
        }
        loop {
            let reply = self
                .request(None, true, &mut |event| sink.emit(event))
                .await?;
            self.last_input = reply.usage.input;
            sink.emit(Event::Usage(reply.usage));
            if reply.stop == Stop::Refused {
                sink.emit(Event::Refused);
                return Ok(());
            }
            if reply.stop == Stop::Truncated {
                sink.emit(Event::Truncated);
            }
            let message = Message::new(Role::Assistant, Some(self.origin()), reply.parts);
            let calls: Vec<(Box<str>, Box<str>, Box<str>)> = message
                .tool_calls()
                .map(|(id, name, input)| (id.into(), name.into(), input.into()))
                .collect();
            self.push(message)?;
            if calls.is_empty() {
                return Ok(());
            }

            for (id, name, input) in calls {
                let (outcome, trace) = self.run(&name, &input, sink).await;
                let images = self.keep(&outcome.images)?;
                self.finished.push(Part::ToolResult {
                    call_id: id,
                    output: outcome.output,
                    is_error: outcome.is_error,
                    images,
                    trace,
                });
            }
            let results = std::mem::take(&mut self.finished);
            self.push(Message::new(Role::Tool, None, results))?;
        }
    }

    pub async fn compact(&mut self, focus: &str, sink: &mut impl Sink) -> Result<bool, Error> {
        self.close_interrupted_calls()?;
        if self
            .messages
            .iter()
            .all(|message| message.role == Role::Summary)
        {
            return Ok(false);
        }
        sink.emit(Event::Compacting);
        self.summarize(focus).await
    }

    pub fn sent(&self) -> Result<Vec<Sent>, Error> {
        Ok(self.store.sent(self.id)?)
    }

    pub async fn restore(&mut self, since: message::Id) -> Result<Option<Vec<PathBuf>>, Error> {
        let Some(tree) = self.store.snapshot(since)? else {
            return Ok(None);
        };
        self.snapshots
            .restore(&tree)
            .await
            .map(Some)
            .map_err(Error::Restore)
    }

    pub fn rewind(&mut self, to: message::Id) -> Result<(), Error> {
        self.close_interrupted_calls()?;
        self.store.truncate(self.id, to)?;
        self.messages = self.store.context(self.id)?;
        self.finished.clear();
        self.trace.clear();
        self.last_input = 0;
        Ok(())
    }

    pub fn instruction_files(&self) -> Vec<String> {
        let workspace = self.workspace();
        instructions::sources(workspace, &self.personal)
            .into_iter()
            .chain(self.runner.loaded())
            .map(|path| instructions::shown(&path, workspace, &self.home))
            .collect()
    }

    pub fn title(&self) -> Result<Option<String>, Error> {
        Ok(self.store.title(self.id)?)
    }

    pub fn set_title(&self, title: &str) -> Result<(), Error> {
        Ok(self.store.set_title(self.id, title)?)
    }

    pub async fn naming(&mut self) -> Result<Option<Naming>, Error> {
        if self.title()?.is_some() {
            return Ok(None);
        }
        let first = |role: Role| {
            self.messages
                .iter()
                .filter(|message| message.role == role)
                .flat_map(|message| &message.parts)
                .find_map(|part| match part {
                    Part::Text { text, .. } if !text.trim().is_empty() => Some(text.clone()),
                    _ => None,
                })
        };
        let (Some(asked), Some(answered)) = (first(Role::User), first(Role::Assistant)) else {
            return Ok(None);
        };
        let conversation = format!(
            "{}\n\nUser: {}\n\nAssistant: {}",
            prompt::NAMING,
            clip(&asked),
            clip(&answered)
        );
        Ok(Some(Naming {
            provider: self.provider.clone(),
            bearer: self.bearer(false).await?,
            origin: self.origin(),
            conversation,
        }))
    }

    pub fn replay(&self, sink: &mut impl Sink) -> Result<(), Error> {
        let mut replay = replay::Replay::default();
        Ok(self
            .store
            .each_message(self.id, |message| replay.message(&message, sink))?)
    }

    pub async fn shell(&mut self, command: &str, sink: &mut impl Sink) -> Result<(), Error> {
        self.close_interrupted_calls()?;
        let shelled = self
            .runner
            .shell(command, &mut |chunk| sink.emit(Event::ShellOutput(chunk)))
            .await;
        let shelled = match shelled {
            Ok(shelled) => shelled,
            Err(problem) => {
                sink.emit(Event::ShellFinished {
                    code: None,
                    problem: Some(&problem),
                });
                return Ok(());
            }
        };
        sink.emit(Event::ShellFinished {
            code: shelled.code,
            problem: None,
        });
        let part = Part::Shell {
            command: command.into(),
            output: shelled.output.into(),
            code: shelled.code,
        };
        self.push(Message::new(Role::User, None, vec![part]))
    }

    async fn run(
        &mut self,
        name: &str,
        input: &str,
        sink: &mut impl Sink,
    ) -> (Outcome, Vec<Trace>) {
        let run = match tool::Run::parse(name, input) {
            Ok(run) => run,
            Err(problem) => {
                let outcome = Outcome::error(problem);
                sink.emit(Event::RunFinished { outcome: &outcome });
                return (outcome, Vec::new());
            }
        };
        sink.emit(Event::RunStarted {
            description: run.description(),
        });
        let web = Web {
            http: self.http.clone(),
            search: self.bearer(false).await.ok().map(|bearer| Searcher {
                provider: self.provider.clone(),
                model: self.model.clone(),
                bearer,
            }),
        };
        self.trace.clear();
        let trace = &mut self.trace;
        let outcome = self
            .runner
            .run(&run, web, &mut |step| match step {
                Step::Started(call) => {
                    sink.emit(Event::Call(&call));
                    trace.push(Trace::Call {
                        kind: call.kind,
                        target: call.target,
                    });
                }
                Step::Failed(problem) => {
                    sink.emit(Event::CallFailed { problem: &problem });
                    trace.push(Trace::Failed { problem });
                }
                Step::Changed(diff) => {
                    sink.emit(Event::Changed(&diff));
                    trace.push(Trace::Changed { diff });
                }
                Step::Loaded(path) => {
                    sink.emit(Event::Loaded { path: &path });
                    trace.push(Trace::Loaded { path });
                }
                Step::Shown(_) => {}
            })
            .await;
        sink.emit(Event::RunFinished { outcome: &outcome });
        (outcome, std::mem::take(&mut self.trace))
    }

    async fn request(
        &mut self,
        trailing: Option<&Message>,
        use_tools: bool,
        on_event: &mut dyn FnMut(Event<'_>),
    ) -> Result<provider::Reply, Error> {
        let images = self.images(trailing)?;
        let mut refreshed = false;
        let mut attempt = 0;
        loop {
            let bearer = self.bearer(refreshed).await?;
            let origin = self.origin();
            let cache_key = self.id.to_string();
            let request = Request {
                model: &self.model,
                instructions: &self.instructions,
                messages: &self.messages,
                images: &images,
                trailing,
                tools: std::slice::from_ref(&self.tool),
                use_tools,
                origin: &origin,
                cache_key: &cache_key,
                effort: self.effort.as_deref(),
            };
            let mut is_shown = false;
            let result = {
                let mut watch = |chunk: Chunk<'_>| {
                    is_shown = true;
                    on_event(chunk.into());
                };
                self.provider.send(&request, &bearer, &mut watch).await
            };
            match result {
                Err(provider::Error::Status { status: 401, .. })
                    if !refreshed && self.account.credential.expires_at().is_some() =>
                {
                    refreshed = true;
                }
                Err(err) if err.is_transient() => {
                    let wait = err.wait(attempt);
                    attempt += 1;
                    on_event(Event::Retrying {
                        attempt,
                        restarts: is_shown,
                    });
                    tokio::time::sleep(wait).await;
                }
                result => return Ok(result?),
            }
        }
    }

    async fn summarize(&mut self, focus: &str) -> Result<bool, Error> {
        let tail = message::Id::generate();
        let instruction = match focus.trim() {
            "" => Message::user(prompt::COMPACTION),
            focus => Message::user(&format!("{}{}{focus}", prompt::COMPACTION, prompt::FOCUS)),
        };
        let reply = self.request(Some(&instruction), false, &mut |_| {}).await?;
        self.last_input = 0;
        let text: String = reply
            .parts
            .iter()
            .filter_map(|part| match part {
                Part::Text { text, .. } => Some(&**text),
                _ => None,
            })
            .collect();
        let (text, title) = split_title(&text);
        if let Some(title) = title {
            self.store.set_title(self.id, &title)?;
        }
        if text.trim().is_empty() {
            return Ok(false);
        }
        let summary = Message::new(
            Role::Summary,
            None,
            vec![Part::Summary {
                text: format!("{}{text}", prompt::SUMMARY_PREFIX).into(),
                tail,
            }],
        );
        self.store.append(self.id, &summary)?;
        self.messages = vec![summary];
        Ok(true)
    }

    fn close_interrupted_calls(&mut self) -> Result<(), Error> {
        let finished = std::mem::take(&mut self.finished);
        let Some(last) = self.messages.last().filter(|m| m.role == Role::Assistant) else {
            return Ok(());
        };
        let mut trace = Some(std::mem::take(&mut self.trace));
        let results: Vec<Part> = last
            .tool_calls()
            .map(|(id, _, _)| {
                finished
                    .iter()
                    .find(
                        |part| matches!(part, Part::ToolResult { call_id, .. } if &**call_id == id),
                    )
                    .cloned()
                    .unwrap_or_else(|| {
                        let trace = trace.take().unwrap_or_default();
                        Part::ToolResult {
                            call_id: id.into(),
                            output: interrupted(&trace).into(),
                            is_error: true,
                            images: Vec::new(),
                            trace,
                        }
                    })
            })
            .collect();
        if results.is_empty() {
            return Ok(());
        }
        self.push(Message::new(Role::Tool, None, results))
    }

    fn mentions(&self, text: &str) -> Result<Vec<Part>, Error> {
        let mut parts = Vec::new();
        for raw in tool::mentioned(text) {
            match self.runner.attach(raw) {
                Some(tool::Attached::File { path, text }) => parts.push(Part::File { path, text }),
                Some(tool::Attached::Image(picture)) => {
                    parts.extend(self.keep(&[picture])?.into_iter().map(Part::Image));
                }
                None => {}
            }
        }
        Ok(parts)
    }

    fn keep(&self, pictures: &[Picture]) -> Result<Vec<Image>, Error> {
        pictures
            .iter()
            .map(|picture| {
                self.store.put_image(picture)?;
                Ok(picture.image.clone())
            })
            .collect()
    }

    fn images(&self, trailing: Option<&Message>) -> Result<Images, Error> {
        let shown: Vec<&Image> = self
            .messages
            .iter()
            .chain(trailing)
            .flat_map(|message| &message.parts)
            .flat_map(|part| match part {
                Part::Image(image) => std::slice::from_ref(image),
                Part::ToolResult { images, .. } => images.as_slice(),
                _ => &[],
            })
            .collect();
        let mut images = Images::default();
        for image in &shown[shown.len() - sent(shown.len())..] {
            if images.get(&image.id).is_some() {
                continue;
            }
            if let Some(bytes) = self.store.image(&image.id)? {
                images.insert(image.id.clone(), BASE64.encode(bytes));
            }
        }
        Ok(images)
    }

    fn push(&mut self, message: Message) -> Result<(), Error> {
        self.store.append(self.id, &message)?;
        self.messages.push(message);
        Ok(())
    }

    fn origin(&self) -> Origin {
        Origin {
            model: self.model.clone(),
            account: self.account.id,
        }
    }

    async fn bearer(&mut self, force: bool) -> Result<Box<str>, Error> {
        bearer(&self.store, &self.http, &mut self.account, force).await
    }
}

impl<'a> From<Chunk<'a>> for Event<'a> {
    fn from(chunk: Chunk<'a>) -> Self {
        match chunk {
            Chunk::Text(text) => Self::Text(text),
            Chunk::Reasoning(text) => Self::Reasoning(text),
        }
    }
}

async fn bearer(
    store: &Store,
    http: &reqwest::Client,
    account: &mut Account,
    force: bool,
) -> Result<Box<str>, Error> {
    let current = &account.credential;
    if current.expires_at().is_none() || !(force || current.needs_refresh()) {
        return Ok(current.secret().into());
    }
    let stored = store
        .account(account.id)?
        .map(|stored| stored.credential)
        .filter(|credential| credential.is_newer_than(current));
    let freshest = stored.unwrap_or_else(|| current.clone());
    if force || freshest.needs_refresh() {
        account.credential = freshest.refresh(http).await?;
        store.save_account(account)?;
    } else {
        account.credential = freshest;
    }
    Ok(account.credential.secret().into())
}

impl Naming {
    pub async fn run(self) -> Option<String> {
        let question = Message::user(&self.conversation);
        let tool = tool::spec();
        let request = Request {
            model: &self.origin.model,
            instructions: prompt::NAMING,
            messages: std::slice::from_ref(&question),
            images: &Images::default(),
            trailing: None,
            tools: std::slice::from_ref(&tool),
            use_tools: false,
            origin: &self.origin,
            cache_key: "goat-title",
            effort: None,
        };
        let reply = self
            .provider
            .send(&request, &self.bearer, &mut |_| {})
            .await
            .ok()?;
        let text: String = reply
            .parts
            .iter()
            .filter_map(|part| match part {
                Part::Text { text, .. } => Some(&**text),
                _ => None,
            })
            .collect();
        tidy(&text)
    }
}

impl<'a> From<&'a str> for Prompt<'a> {
    fn from(text: &'a str) -> Self {
        Self { text, images: &[] }
    }
}

fn split_title(text: &str) -> (String, Option<String>) {
    let trimmed = text.trim_end();
    let Some((body, last)) = trimmed.rsplit_once('\n') else {
        return (text.to_owned(), None);
    };
    match last.trim().strip_prefix(prompt::TITLE_LINE) {
        Some(title) => (body.trim_end().to_owned(), tidy(title)),
        None => (text.to_owned(), None),
    }
}

fn tidy(raw: &str) -> Option<String> {
    let line = raw.lines().map(str::trim).find(|line| !line.is_empty())?;
    let line = line.strip_prefix(prompt::TITLE_LINE).unwrap_or(line).trim();
    let line = line.trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | '*' | '“' | '”'));
    let line = line.trim_end_matches('.').trim();
    (!line.is_empty()).then(|| line.chars().take(TITLE_CHARS).collect())
}

fn clip(text: &str) -> String {
    text.chars().take(NAMING_CHARS).collect()
}

fn sent(shown: usize) -> usize {
    if shown <= IMAGES_PER_REQUEST {
        return shown;
    }
    IMAGE_BATCH + shown % IMAGE_BATCH
}

fn interrupted(trace: &[Trace]) -> String {
    let calls: Vec<(tool::Kind, &str)> = trace
        .iter()
        .filter_map(|step| match step {
            Trace::Call { kind, target } => Some((*kind, &**target)),
            _ => None,
        })
        .collect();
    if calls.is_empty() {
        return INTERRUPTED.to_owned();
    }
    let mut text = format!("{INTERRUPTED}. Calls it had started, which may have changed files:");
    for (kind, target) in calls {
        let mut line = format!(
            "{} {}",
            kind.as_str(),
            target.lines().next().unwrap_or_default()
        );
        if let Some((cut, _)) = line.char_indices().nth(CALL_CHARS) {
            line.truncate(cut);
            line.push('…');
        }
        text.push_str("\n- ");
        text.push_str(&line);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_is_tidied() {
        assert_eq!(
            tidy("\"Fix the flaky test.\"\n"),
            Some("Fix the flaky test".to_owned())
        );
        assert_eq!(tidy("Title: Shell mode"), Some("Shell mode".to_owned()));
        assert_eq!(tidy("  \n"), None);
    }

    #[test]
    fn the_title_line_is_taken_off_a_summary() {
        let (summary, title) = split_title("Goal: x\nNext: y\nTitle: Image support");

        assert_eq!(summary, "Goal: x\nNext: y");
        assert_eq!(title.as_deref(), Some("Image support"));
    }

    #[test]
    fn a_summary_without_a_title_line_is_kept_whole() {
        assert_eq!(
            split_title("Goal: x\nNext: y"),
            ("Goal: x\nNext: y".to_owned(), None)
        );
    }

    #[test]
    fn up_to_twenty_images_are_all_sent() {
        assert_eq!(sent(5), 5);
        assert_eq!(sent(20), 20);
    }

    #[test]
    fn past_twenty_the_oldest_images_leave_in_batches_of_ten() {
        assert_eq!(sent(21), 11);
        assert_eq!(sent(29), 19);
        assert_eq!(sent(30), 10);
        assert_eq!(sent(31), 11);
    }
}
