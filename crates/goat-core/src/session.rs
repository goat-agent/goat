mod prompt;

use std::path::Path;

use crate::account::{Account, Credential};
use crate::message::{self, Message, Origin, Part, Role};
use crate::provider::{self, Endpoints, Provider, Request, Usage};
use crate::store::{self, Store};
use crate::tool::{self, Outcome, Toolbox};

crate::id::define!(Id);

const COMPACT_AT_TOKENS: u64 = 200_000;
const INTERRUPTED: &str = "interrupted by the user before this tool finished";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("store")]
    Store(#[from] store::Error),
    #[error("model")]
    Provider(#[from] provider::Error),
    #[error("sign-in")]
    Auth(#[from] crate::account::chatgpt::Error),
    #[error("tools")]
    Tool(#[from] tool::Error),
    #[error("session {0} not found")]
    NotFound(Id),
}

#[derive(Debug, Clone, Copy)]
pub enum Event<'a> {
    Text(&'a str),
    ToolStarted { name: &'a str, summary: &'a str },
    ToolFinished { outcome: &'a Outcome },
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
    tools: Toolbox,
    specs: Vec<tool::Spec>,
    messages: Vec<Message>,
    finished: Vec<Part>,
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
            provider: Provider::new(account.kind, http.clone(), &env.endpoints),
            store,
            http,
            endpoints: env.endpoints.clone(),
            account,
            model: model.into(),
            instructions,
            tools: Toolbox::new(&env.workspace, &env.home, &env.spill)?,
            specs: tool::specs(),
            messages,
            finished: Vec::new(),
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
        self.tools.workspace()
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn set_model(&mut self, model: &str) {
        self.model = model.into();
    }

    pub fn set_account(&mut self, account: Account, model: &str) {
        self.provider = Provider::new(account.kind, self.http.clone(), &self.endpoints);
        self.account = account;
        self.model = model.into();
    }

    pub async fn models(&mut self) -> Result<Vec<Box<str>>, Error> {
        let bearer = self.bearer().await?;
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
                .request(None, true, &mut |text| sink.emit(Event::Text(text)))
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
                let summary = tool::summarize(&name, &input);
                sink.emit(Event::ToolStarted {
                    name: &name,
                    summary: &summary,
                });
                let outcome = self.tools.run(&name, &input).await;
                sink.emit(Event::ToolFinished { outcome: &outcome });
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

    async fn request(
        &mut self,
        trailing: Option<&Message>,
        use_tools: bool,
        on_text: &mut dyn FnMut(&str),
    ) -> Result<provider::Reply, Error> {
        let bearer = self.bearer().await?;
        let origin = self.origin();
        let cache_key = self.id.to_string();
        let request = Request {
            model: &self.model,
            instructions: &self.instructions,
            messages: &self.messages,
            trailing,
            tools: &self.specs,
            use_tools,
            origin: &origin,
            cache_key: &cache_key,
        };
        Ok(self.provider.send(&request, &bearer, on_text).await?)
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
                        output: INTERRUPTED.into(),
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

    async fn bearer(&mut self) -> Result<Box<str>, Error> {
        let current = match &self.account.credential {
            Credential::ApiKey { key } => return Ok(key.clone()),
            Credential::Chatgpt(token) => token,
        };
        if !current.expires_soon() {
            return Ok(current.access_token.clone());
        }
        let stored =
            self.store
                .account(self.account.id)?
                .and_then(|account| match account.credential {
                    Credential::Chatgpt(token) if token.expires_at > current.expires_at => {
                        Some(token)
                    }
                    _ => None,
                });
        let freshest = stored.unwrap_or_else(|| current.clone());
        let token = if freshest.expires_soon() {
            let refreshed = freshest.refresh(&self.http).await?;
            self.account.credential = Credential::Chatgpt(refreshed.clone());
            self.store.save_account(&self.account)?;
            refreshed
        } else {
            self.account.credential = Credential::Chatgpt(freshest.clone());
            freshest
        };
        Ok(token.access_token)
    }
}
