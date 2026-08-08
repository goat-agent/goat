use std::{collections::HashMap, sync::Arc};

use goat_protocol::{AskQuestion, Event, TaskId, ToolCallId};
use goat_tool_ask::{QuestionBroker, QuestionFuture};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

pub(crate) struct EngineQuestionBroker {
    asks: Arc<Mutex<HashMap<ToolCallId, oneshot::Sender<Vec<String>>>>>,
    events: mpsc::Sender<Event>,
}

impl EngineQuestionBroker {
    pub(crate) fn new(
        asks: Arc<Mutex<HashMap<ToolCallId, oneshot::Sender<Vec<String>>>>>,
        events: mpsc::Sender<Event>,
    ) -> Self {
        Self { asks, events }
    }
}

impl QuestionBroker for EngineQuestionBroker {
    fn ask<'a>(
        &'a self,
        task: TaskId,
        call: ToolCallId,
        questions: Vec<AskQuestion>,
        cancellation: &'a CancellationToken,
    ) -> QuestionFuture<'a> {
        Box::pin(async move {
            let (sender, receiver) = oneshot::channel::<Vec<String>>();
            self.asks.lock().await.insert(call, sender);
            let _ = self
                .events
                .send(Event::AskStarted {
                    id: task,
                    call,
                    questions,
                })
                .await;
            let result = tokio::select! {
                biased;
                () = cancellation.cancelled() => {
                    self.asks.lock().await.remove(&call);
                    let _ = self.events.send(Event::AskDismissed { id: task, call }).await;
                    return Err("interrupted".to_owned());
                }
                result = receiver => result,
            };
            result.map_err(|_| "answer channel closed".to_owned())
        })
    }
}
