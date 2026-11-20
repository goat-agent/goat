mod prompt;

use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

use crate::account::Account;
use crate::image::{Image, Picture};
use crate::message::{self, Message, Origin, Part, Role};
use crate::provider::{self, Chunk, Endpoints, Images, Provider, Request, Usage};
use crate::store::{self, Store};
use crate::tool::{self, Outcome, Runner, Step};

crate::id::define!(Id);

const COMPACT_AT_TOKENS: u64 = 200_000;
const INTERRUPTED: &str = "interrupted by the user before this run finished";
const CALL_CHARS: usize = 100;
const IMAGES_PER_REQUEST: usize = 20;
const IMAGE_BATCH: usize = 10;

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
}

#[derive(Debug, Clone, Copy)]
pub enum Event<'a> {
    Text(&'a str),
    Reasoning(&'a str),
    RunStarted {
        description: &'a str,
    },
    Call(&'a tool::Call),
    CallFailed {
        problem: &'a str,
    },
    RunFinished {
        outcome: &'a Outcome,
    },
    ShellOutput(&'a str),
    ShellFinished {
        code: Option<i32>,
        problem: Option<&'a str>,
    },
    Usage(Usage),
    Compacting,
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
    pub endpoints: Endpoints,
}

#[derive(Debug)]
pub struct Session {
    id: Id,
    store: Store,
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
    started: Vec<tool::Call>,
    last_input: u64,
}

impl Session {
    pub fn create(
        store: Store,
        http: reqwest::Client,
        env: &Environment,
        account: Account,
        model: &str,
    ) -> Result<Self, Error> {
        let instructions = prompt::instructions(&env.workspace);
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
        Self::assemble(
            id,
            store,
            http,
            env,
            account,
            model,
            info.instructions,
            messages,
        )
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
            started: Vec::new(),
            last_input: 0,
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
        let mut parts = vec![Part::text(prompt.text)];
        parts.extend(self.keep(prompt.images)?.into_iter().map(Part::Image));
        let message = Message::new(Role::User, None, parts);
        if self.last_input > COMPACT_AT_TOKENS {
            sink.emit(Event::Compacting);
            self.compact(message.id).await?;
        }
        self.push(message)?;
        loop {
            let reply = self
                .request(None, true, &mut |chunk| sink.emit(chunk.into()))
                .await?;
            self.last_input = reply.usage.input;
            sink.emit(Event::Usage(reply.usage));
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
                let outcome = self.run(&name, &input, sink).await;
                let images = self.keep(&outcome.images)?;
                self.finished.push(Part::ToolResult {
                    call_id: id,
                    output: outcome.output,
                    is_error: outcome.is_error,
                    images,
                });
            }
            let results = std::mem::take(&mut self.finished);
            self.push(Message::new(Role::Tool, None, results))?;
        }
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
        let text = prompt::shell(command, &shelled.output, shelled.code);
        self.push(Message::user(&text))
    }

    async fn run(&mut self, name: &str, input: &str, sink: &mut impl Sink) -> Outcome {
        let run = match tool::Run::parse(name, input) {
            Ok(run) => run,
            Err(problem) => {
                let outcome = Outcome::error(problem);
                sink.emit(Event::RunFinished { outcome: &outcome });
                return outcome;
            }
        };
        sink.emit(Event::RunStarted {
            description: run.description(),
        });
        self.started.clear();
        let started = &mut self.started;
        let outcome = self
            .runner
            .run(&run, &mut |step| match step {
                Step::Started(call) => {
                    sink.emit(Event::Call(&call));
                    started.push(call);
                }
                Step::Failed(problem) => sink.emit(Event::CallFailed { problem: &problem }),
                Step::Shown(_) => {}
            })
            .await;
        sink.emit(Event::RunFinished { outcome: &outcome });
        outcome
    }

    async fn request(
        &mut self,
        trailing: Option<&Message>,
        use_tools: bool,
        on_chunk: &mut dyn FnMut(Chunk<'_>),
    ) -> Result<provider::Reply, Error> {
        let images = self.images(trailing)?;
        let mut refreshed = false;
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
            match self.provider.send(&request, &bearer, on_chunk).await {
                Err(provider::Error::Status { status: 401, .. })
                    if !refreshed && self.account.credential.expires_at().is_some() =>
                {
                    refreshed = true;
                }
                result => return Ok(result?),
            }
        }
    }

    async fn compact(&mut self, next: message::Id) -> Result<(), Error> {
        let instruction = Message::user(prompt::COMPACTION);
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
        if text.trim().is_empty() {
            return Ok(());
        }
        let summary = Message::new(
            Role::Summary,
            None,
            vec![Part::Summary {
                text: format!("{}{text}", prompt::SUMMARY_PREFIX).into(),
                tail: next,
            }],
        );
        self.store.append(self.id, &summary)?;
        self.messages = vec![summary];
        Ok(())
    }

    fn close_interrupted_calls(&mut self) -> Result<(), Error> {
        let finished = std::mem::take(&mut self.finished);
        let Some(last) = self.messages.last().filter(|m| m.role == Role::Assistant) else {
            return Ok(());
        };
        let mut started = Some(std::mem::take(&mut self.started));
        let results: Vec<Part> = last
            .tool_calls()
            .map(|(id, _, _)| {
                finished
                    .iter()
                    .find(
                        |part| matches!(part, Part::ToolResult { call_id, .. } if &**call_id == id),
                    )
                    .cloned()
                    .unwrap_or_else(|| Part::ToolResult {
                        call_id: id.into(),
                        output: interrupted(&started.take().unwrap_or_default()).into(),
                        is_error: true,
                        images: Vec::new(),
                    })
            })
            .collect();
        if results.is_empty() {
            return Ok(());
        }
        self.push(Message::new(Role::Tool, None, results))
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

impl<'a> From<&'a str> for Prompt<'a> {
    fn from(text: &'a str) -> Self {
        Self { text, images: &[] }
    }
}

fn sent(shown: usize) -> usize {
    if shown <= IMAGES_PER_REQUEST {
        return shown;
    }
    IMAGE_BATCH + shown % IMAGE_BATCH
}

fn interrupted(started: &[tool::Call]) -> String {
    if started.is_empty() {
        return INTERRUPTED.to_owned();
    }
    let mut text = format!("{INTERRUPTED}. Calls it had started, which may have changed files:");
    for call in started {
        let mut line = format!(
            "{} {}",
            call.kind.as_str(),
            call.target.lines().next().unwrap_or_default()
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
