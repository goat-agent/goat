mod prompt;

use std::path::Path;

use crate::account::Account;
use crate::message::{self, Message, Origin, Part, Role};
use crate::provider::{self, Chunk, Endpoints, Provider, Request, Usage};
use crate::store::{self, Store};
use crate::tool::{self, Outcome, Runner, Step};

crate::id::define!(Id);

const COMPACT_AT_TOKENS: u64 = 200_000;
const INTERRUPTED: &str = "interrupted by the user before this run finished";

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
    RunStarted { description: &'a str },
    Call { summary: &'a str },
    CallFailed { problem: &'a str },
    RunFinished { outcome: &'a Outcome },
    Usage(Usage),
    Compacting,
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
    instructions: Box<str>,
    runner: Runner,
    tool: tool::Spec,
    messages: Vec<Message>,
    finished: Vec<Part>,
    started: Vec<Box<str>>,
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

    pub fn set_account(&mut self, account: Account, model: &str) {
        self.provider = Provider::new(&account, self.http.clone(), &self.endpoints);
        self.account = account;
        self.model = model.into();
    }

    pub async fn models(&mut self) -> Result<Vec<Box<str>>, Error> {
        let bearer = self.bearer(false).await?;
        Ok(self.provider.models(&bearer).await?)
    }

    pub async fn turn(&mut self, input: &str, sink: &mut impl Sink) -> Result<(), Error> {
        self.close_interrupted_calls()?;
        let message = Message::user(input);
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
                self.finished.push(Part::ToolResult {
                    call_id: id,
                    output: outcome.output,
                    is_error: outcome.is_error,
                });
            }
            let results = std::mem::take(&mut self.finished);
            self.push(Message::new(Role::Tool, None, results))?;
        }
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
                Step::Started(summary) => {
                    sink.emit(Event::Call { summary: &summary });
                    started.push(summary);
                }
                Step::Failed(problem) => sink.emit(Event::CallFailed { problem: &problem }),
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
        let mut refreshed = false;
        loop {
            let bearer = self.bearer(refreshed).await?;
            let origin = self.origin();
            let cache_key = self.id.to_string();
            let request = Request {
                model: &self.model,
                instructions: &self.instructions,
                messages: &self.messages,
                trailing,
                tools: std::slice::from_ref(&self.tool),
                use_tools,
                origin: &origin,
                cache_key: &cache_key,
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
                    })
            })
            .collect();
        if results.is_empty() {
            return Ok(());
        }
        self.push(Message::new(Role::Tool, None, results))
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
        let current = &self.account.credential;
        if current.expires_at().is_none() || !(force || current.needs_refresh()) {
            return Ok(current.secret().into());
        }
        let stored = self
            .store
            .account(self.account.id)?
            .map(|account| account.credential)
            .filter(|credential| credential.is_newer_than(current));
        let freshest = stored.unwrap_or_else(|| current.clone());
        if force || freshest.needs_refresh() {
            self.account.credential = freshest.refresh(&self.http).await?;
            self.store.save_account(&self.account)?;
        } else {
            self.account.credential = freshest;
        }
        Ok(self.account.credential.secret().into())
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

fn interrupted(started: &[Box<str>]) -> String {
    if started.is_empty() {
        return INTERRUPTED.to_owned();
    }
    let mut text = format!("{INTERRUPTED}. Calls it had started, which may have changed files:");
    for summary in started {
        text.push_str("\n- ");
        text.push_str(summary);
    }
    text
}
