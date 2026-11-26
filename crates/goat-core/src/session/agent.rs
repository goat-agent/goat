use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use tokio::sync::mpsc;

use super::{Depth, Error, Event, Id, Parts, Session, Shared, Sink, prompt};
use crate::account::Account;
use crate::agent::GENERAL;
use crate::message::{self, Part, Role};
use crate::store::Store;
use crate::tool::{Report, Runner, Spawn, Step, Task};

const NO_ANSWER: &str = "the agent ended without an answer";

#[derive(Debug)]
pub(super) struct Spawner {
    shared: Rc<Shared>,
    parent: Id,
    call: message::Id,
    instructions: Rc<str>,
    account: Account,
    model: Box<str>,
    effort: Option<Box<str>>,
}

struct Running<'a> {
    shared: &'a Shared,
    id: Id,
}

struct Relay<'a>(&'a mpsc::UnboundedSender<Step>);

impl Session {
    pub(super) fn spawner(&self) -> Option<Rc<dyn Spawn>> {
        if self.depth == Depth::Agent {
            return None;
        }
        let call = self.messages.last()?.id;
        Some(Rc::new(Spawner {
            shared: Rc::clone(&self.shared),
            parent: self.id,
            call,
            instructions: Rc::clone(&self.instructions),
            account: self.account.clone(),
            model: self.model.clone(),
            effort: self.effort.clone(),
        }))
    }

    async fn work(&mut self, task: &str, sink: &mut impl Sink) -> Result<(), Error> {
        self.prepare(sink).await?;
        self.push(message::Message::user(task))?;
        self.respond(sink).await
    }

    fn answer(&self) -> String {
        let text: String = self
            .messages
            .iter()
            .rev()
            .find(|message| message.role == Role::Assistant)
            .into_iter()
            .flat_map(|message| &message.parts)
            .filter_map(|part| match part {
                Part::Text { text, .. } => Some(&**text),
                _ => None,
            })
            .collect();
        match text.trim() {
            "" => NO_ANSWER.to_owned(),
            text => text.to_owned(),
        }
    }
}

impl Spawner {
    async fn run(&self, task: Task, steps: &mpsc::UnboundedSender<Step>) -> Result<Report, String> {
        let _slot = self
            .shared
            .slots
            .acquire()
            .await
            .map_err(|err| err.to_string())?;
        let (mut agent, _running) = match task.id {
            Some(id) => self.resume(&id)?,
            None => self.start(task.agent.as_deref().unwrap_or(GENERAL))?,
        };
        agent
            .work(&task.text, &mut Relay(steps))
            .await
            .map_err(|err| format!("the agent stopped: {}", describe(&err)))?;
        Ok(Report {
            id: agent.id.to_string(),
            text: agent.answer(),
        })
    }

    fn start(&self, name: &str) -> Result<(Session, Running<'_>), String> {
        let agent = self
            .shared
            .agents
            .find(name)
            .ok_or_else(|| format!("there is no agent named {name}"))?;
        let store = Store::connect(&self.shared.database).map_err(|err| describe(&err))?;
        let (account, model, effort) = match &agent.model {
            Some(model) => (
                self.account_for(&store, model)?,
                model.clone(),
                agent.effort.clone(),
            ),
            None => (
                self.account.clone(),
                self.model.clone(),
                self.effort.clone(),
            ),
        };
        let instructions = format!(
            "{}\n\n{}\n\n{}",
            self.instructions,
            prompt::AGENT,
            agent.prompt()?
        );
        let id = store
            .create_agent_session(&self.shared.workspace, &instructions, self.call)
            .map_err(|err| describe(&err))?;
        let running = Running::start(&self.shared, id)?;
        let parts = Parts {
            id,
            depth: Depth::Agent,
            store,
            account,
            model: &model,
            effort,
            instructions: instructions.into(),
            messages: Vec::new(),
        };
        Ok((self.build(parts, false)?, running))
    }

    fn resume(&self, raw: &str) -> Result<(Session, Running<'_>), String> {
        let missing = || format!("there is no agent {raw} in this session");
        let id: Id = raw.parse().map_err(|_| missing())?;
        let store = Store::connect(&self.shared.database).map_err(|err| describe(&err))?;
        if !store
            .is_agent_of(id, self.parent)
            .map_err(|err| describe(&err))?
        {
            return Err(missing());
        }
        let running = Running::start(&self.shared, id)?;
        let info = store
            .session(id)
            .map_err(|err| describe(&err))?
            .ok_or_else(missing)?;
        let messages = store.context(id).map_err(|err| describe(&err))?;
        let origin = store.last_origin(id).map_err(|err| describe(&err))?;
        let used = match origin {
            Some(origin) => store
                .account(origin.account)
                .map_err(|err| describe(&err))?
                .map(|account| (account, origin.model)),
            None => None,
        };
        let (account, model) = used.unwrap_or_else(|| (self.account.clone(), self.model.clone()));
        let effort = (model == self.model).then(|| self.effort.clone()).flatten();
        let parts = Parts {
            id,
            depth: Depth::Agent,
            store,
            account,
            model: &model,
            effort,
            instructions: info.instructions.into(),
            messages,
        };
        Ok((self.build(parts, true)?, running))
    }

    fn build(&self, parts: Parts<'_>, has_sandbox: bool) -> Result<Session, String> {
        let runner = Runner::new(
            &self.shared.workspace,
            &self.shared.home,
            &self.shared.spill,
            has_sandbox,
        )
        .map_err(|err| describe(&err))?;
        Ok(Session::build(Rc::clone(&self.shared), runner, parts))
    }

    fn account_for(&self, store: &Store, model: &str) -> Result<Account, String> {
        let kind = self
            .shared
            .catalog
            .iter()
            .find(|known| &*known.id == model)
            .map(|known| known.kind)
            .ok_or_else(|| format!("goat does not know the model {model}"))?;
        if self.account.kind == kind {
            return Ok(self.account.clone());
        }
        let mut offering = store
            .accounts()
            .map_err(|err| describe(&err))?
            .into_iter()
            .filter(|account| account.kind == kind);
        match (offering.next(), offering.next()) {
            (Some(account), None) => Ok(account),
            (None, _) => Err(format!("no account offers {model}")),
            (Some(_), Some(_)) => Err(format!(
                "several accounts offer {model} and the current one does not"
            )),
        }
    }
}

impl Spawn for Spawner {
    fn spawn<'a>(
        &'a self,
        task: Task,
        steps: &'a mpsc::UnboundedSender<Step>,
    ) -> Pin<Box<dyn Future<Output = Result<Report, String>> + 'a>> {
        Box::pin(self.run(task, steps))
    }
}

impl<'a> Running<'a> {
    fn start(shared: &'a Shared, id: Id) -> Result<Self, String> {
        if !shared.running.borrow_mut().insert(id) {
            return Err(format!("the agent {id} is still running"));
        }
        Ok(Self { shared, id })
    }
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.shared.running.borrow_mut().remove(&self.id);
    }
}

impl Sink for Relay<'_> {
    fn emit(&mut self, event: Event<'_>) {
        let step = match event {
            Event::Call(call) => Step::AgentCall(call.clone()),
            Event::Changed(diff) => Step::Changed(diff.clone()),
            Event::Confirm(question) => match question.forward() {
                Some(question) => Step::Confirm(question),
                None => return,
            },
            _ => return,
        };
        self.0.send(step).ok();
    }
}

fn describe(err: &dyn std::error::Error) -> String {
    let mut text = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}
