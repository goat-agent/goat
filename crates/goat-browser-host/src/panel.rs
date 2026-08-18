use std::sync::Arc;

use goat_api::{
    Api, ResumeMode, SessionControl, SessionControlParams, SessionId, SessionOpen,
    SessionOpenParams, SessionSubmit, SessionSubmitParams, SessionWatch, SessionWatchParams,
    StreamEvent, WatchFrom, WatchItem,
};
use goat_protocol::{Event, Op, TaskId, ToolCallId};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::NativePort;

pub const PREFIX: &str = "panel.";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type")]
pub enum PanelRequest {
    #[serde(rename = "panel.open")]
    Open {},
    #[serde(rename = "panel.submit")]
    Submit { text: String },
    #[serde(rename = "panel.interrupt")]
    Interrupt {},
    #[serde(rename = "panel.answer")]
    Answer {
        call: ToolCallId,
        answers: Vec<String>,
    },
}

struct Live {
    session: SessionId,
    active: Option<TaskId>,
    watch: CancellationToken,
}

pub struct Panel {
    api: Api,
    port: Arc<dyn NativePort>,
    cwd: String,
    live: Mutex<Option<Live>>,
}

impl Panel {
    #[must_use]
    pub fn new(api: Api, port: Arc<dyn NativePort>, cwd: String) -> Self {
        Self {
            api,
            port,
            cwd,
            live: Mutex::new(None),
        }
    }

    pub async fn accept(self: &Arc<Self>, body: &Value) {
        let Some(kind) = body.get("type").and_then(Value::as_str) else {
            return;
        };
        if !kind.starts_with(PREFIX) {
            return;
        }
        let outcome = match serde_json::from_value::<PanelRequest>(body.clone()) {
            Ok(PanelRequest::Open {}) => self.open().await,
            Ok(PanelRequest::Submit { text }) => self.submit(text).await,
            Ok(PanelRequest::Interrupt {}) => self.interrupt().await,
            Ok(PanelRequest::Answer { call, answers }) => self.answer(call, answers).await,
            Err(err) => Err(err.to_string()),
        };
        if let Err(reason) = outcome {
            self.fail(&reason).await;
        }
    }

    async fn open(self: &Arc<Self>) -> Result<(), String> {
        let opened = self
            .api
            .call::<SessionOpen>(SessionOpenParams {
                cwd: self.cwd.clone(),
                resume: ResumeMode::Latest {},
            })
            .await
            .map_err(|err| err.message)?;

        let watch = CancellationToken::new();
        let replaced = self.live.lock().await.replace(Live {
            session: opened.session,
            active: None,
            watch: watch.clone(),
        });
        if let Some(replaced) = replaced {
            replaced.watch.cancel();
        }

        let mut stream = self
            .api
            .open::<SessionWatch>(SessionWatchParams {
                session: opened.session,
                from: WatchFrom::Snapshot {},
            })
            .await
            .map_err(|err| err.message)?;

        let panel = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    () = watch.cancelled() => break,
                    next = stream.recv() => match next {
                        Some(StreamEvent::Item { item, .. }) => panel.relay(item).await,
                        Some(StreamEvent::End(Err(err))) => {
                            panel.fail(&err.message).await;
                            break;
                        }
                        Some(StreamEvent::End(Ok(()))) | None => break,
                    },
                }
            }
        });
        Ok(())
    }

    async fn submit(&self, text: String) -> Result<(), String> {
        let session = self.session().await?;
        self.api
            .call::<SessionSubmit>(SessionSubmitParams {
                session,
                op: Op::SubmitMessage {
                    id: TaskId(0),
                    text,
                    display: None,
                    attachments: Vec::new(),
                },
            })
            .await
            .map_err(|err| err.message)?;
        Ok(())
    }

    async fn interrupt(&self) -> Result<(), String> {
        let (session, active) = {
            let live = self.live.lock().await;
            let live = live.as_ref().ok_or("the panel has no session open")?;
            (live.session, live.active)
        };
        let Some(id) = active else { return Ok(()) };
        self.api
            .call::<SessionControl>(SessionControlParams {
                session,
                op: Op::Interrupt { id },
            })
            .await
            .map_err(|err| err.message)?;
        Ok(())
    }

    async fn answer(&self, call: ToolCallId, answers: Vec<String>) -> Result<(), String> {
        let session = self.session().await?;
        self.api
            .call::<SessionControl>(SessionControlParams {
                session,
                op: Op::Answer {
                    id: TaskId(0),
                    call,
                    answers,
                },
            })
            .await
            .map_err(|err| err.message)?;
        Ok(())
    }

    async fn relay(&self, item: WatchItem) {
        match &item {
            WatchItem::Snapshot { state, .. } => self.mark(state.active).await,
            WatchItem::Event { event, .. } => match event.as_ref() {
                Event::TaskStarted { id } => self.mark(Some(*id)).await,
                Event::TaskDone { id, .. } => self.finish(*id).await,
                _ => {}
            },
            WatchItem::Presence { .. } => {}
        }
        match serde_json::to_value(&item) {
            Ok(item) => {
                let _ = self
                    .port
                    .emit(&json!({"type": "panel.item", "item": item}))
                    .await;
            }
            Err(err) => self.fail(&err.to_string()).await,
        }
    }

    async fn session(&self) -> Result<SessionId, String> {
        let live = self.live.lock().await;
        live.as_ref()
            .map(|live| live.session)
            .ok_or_else(|| "the panel has no session open".to_owned())
    }

    async fn mark(&self, task: Option<TaskId>) {
        if let Some(live) = self.live.lock().await.as_mut() {
            live.active = task;
        }
    }

    async fn finish(&self, task: TaskId) {
        if let Some(live) = self.live.lock().await.as_mut()
            && live.active == Some(task)
        {
            live.active = None;
        }
    }

    async fn fail(&self, message: &str) {
        let _ = self
            .port
            .emit(&json!({"type": "panel.error", "message": message}))
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::{Panel, PanelRequest};
    use crate::NativePort;
    use goat_api::{
        Api, Cursor, Empty, Grant, Router, SessionControl, SessionControlParams, SessionId,
        SessionOpen, SessionOpenOutput, SessionOpenParams, SessionSubmit, SessionSubmitOutput,
        SessionSubmitParams, SessionWatch, SessionWatchParams, WatchItem,
    };
    use goat_protocol::{Op, TaskId, ToolCallId};
    use goat_wire::envelope::{Frame, Role};
    use goat_wire::peer::{RejectAll, StreamSink, spawn};
    use goat_wire::{WireConn, envelope::CallError};
    use serde_json::{Value, json};
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    type Conn = WireConn<tokio::io::DuplexStream, Frame, Frame>;

    struct Recorder(mpsc::UnboundedSender<Value>);

    #[async_trait::async_trait]
    impl NativePort for Recorder {
        async fn emit(&self, body: &Value) -> Result<(), String> {
            let _ = self.0.send(body.clone());
            Ok(())
        }
    }

    fn snapshot(active: Option<TaskId>) -> WatchItem {
        WatchItem::Snapshot {
            cursor: Cursor::new("e1", 0),
            reset: false,
            state: Box::new(goat_api::SessionSnapshot {
                session: SessionId(1),
                cwd: "/home/person".to_owned(),
                target: None,
                transcript: Vec::new(),
                pending: Vec::new(),
                context_tokens: None,
                compaction_threshold: None,
                skills: Vec::new(),
                accounts: Vec::new(),
                models: Vec::new(),
                selected: None,
                mode: goat_protocol::Mode::Normal,
                plan_path: None,
                processes: Vec::new(),
                usage: Vec::new(),
                rate_limits: Vec::new(),
                active,
                retry: None,
            }),
        }
    }

    struct Daemon {
        ops: mpsc::UnboundedReceiver<Op>,
        _peers: [goat_wire::peer::Peer; 2],
        _closed: CancellationToken,
    }

    fn daemon(active: Option<TaskId>) -> (Api, Daemon) {
        let (a, b) = tokio::io::duplex(1024 * 1024);
        let (client_sink, client_source) = Conn::new(a).split();
        let (daemon_sink, daemon_source) = Conn::new(b).split();
        let closed = CancellationToken::new();
        let (ops_tx, ops) = mpsc::unbounded_channel();

        let submit_ops = ops_tx.clone();
        let router = Router::new([Grant::Any])
            .unary::<SessionOpen, _, _>(|_params: SessionOpenParams, _ctx| async {
                Ok(SessionOpenOutput {
                    session: SessionId(1),
                    cwd: "/home/person".to_owned(),
                    epoch: "e1".to_owned(),
                })
            })
            .unary::<SessionSubmit, _, _>(move |params: SessionSubmitParams, _ctx| {
                let ops = submit_ops.clone();
                async move {
                    let _ = ops.send(params.op);
                    Ok(SessionSubmitOutput { task: TaskId(9) })
                }
            })
            .unary::<SessionControl, _, _>(move |params: SessionControlParams, _ctx| {
                let ops = ops_tx.clone();
                async move {
                    let _ = ops.send(params.op);
                    Ok(Empty {})
                }
            })
            .stream::<SessionWatch, _, _>(
                move |_params: SessionWatchParams, _ctx, sink: StreamSink| async move {
                    sink.send_dropping(serde_json::to_value(snapshot(active)).unwrap(), 0)
                        .await?;
                    Ok::<Empty, CallError>(Empty {})
                },
            );

        let daemon_peer = spawn(
            Role::Daemon,
            Box::pin(daemon_sink),
            Box::pin(daemon_source),
            Arc::new(router),
            closed.clone(),
        );
        let client = spawn(
            Role::Client,
            Box::pin(client_sink),
            Box::pin(client_source),
            Arc::new(RejectAll),
            closed.clone(),
        );
        let api = Api::new(client.handle.clone());
        (
            api,
            Daemon {
                ops,
                _peers: [daemon_peer, client],
                _closed: closed,
            },
        )
    }

    fn panel(api: Api) -> (Arc<Panel>, mpsc::UnboundedReceiver<Value>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let panel = Arc::new(Panel::new(
            api,
            Arc::new(Recorder(tx)),
            "/home/person".to_owned(),
        ));
        (panel, rx)
    }

    #[test]
    fn every_panel_request_parses_from_its_tagged_shape() {
        let open: PanelRequest = serde_json::from_value(json!({"type": "panel.open"})).unwrap();
        assert_eq!(open, PanelRequest::Open {});
        let submit: PanelRequest =
            serde_json::from_value(json!({"type": "panel.submit", "text": "hi"})).unwrap();
        assert_eq!(
            submit,
            PanelRequest::Submit {
                text: "hi".to_owned()
            }
        );
        let stop: PanelRequest =
            serde_json::from_value(json!({"type": "panel.interrupt"})).unwrap();
        assert_eq!(stop, PanelRequest::Interrupt {});
        let answer: PanelRequest = serde_json::from_value(
            json!({"type": "panel.answer", "call": "42", "answers": ["yes"]}),
        )
        .unwrap();
        assert_eq!(
            answer,
            PanelRequest::Answer {
                call: ToolCallId(42),
                answers: vec!["yes".to_owned()],
            }
        );
    }

    #[tokio::test]
    async fn a_browser_message_is_not_the_panels_to_answer() {
        let (api, _daemon) = daemon(None);
        let (panel, mut out) = panel(api);
        panel.accept(&json!({"type": "browser.reply"})).await;
        panel.accept(&json!({"result": 1})).await;
        assert!(out.try_recv().is_err());
    }

    #[tokio::test]
    async fn opening_the_panel_streams_the_snapshot_back_over_the_port() {
        let (api, _daemon) = daemon(None);
        let (panel, mut out) = panel(api);
        panel.accept(&json!({"type": "panel.open"})).await;
        let item = out.recv().await.expect("the snapshot reaches the panel");
        assert_eq!(item["type"], "panel.item");
        assert_eq!(item["item"]["t"], "snapshot");
        assert_eq!(item["item"]["state"]["cwd"], "/home/person");
    }

    #[tokio::test]
    async fn submitting_without_a_session_reports_an_error_instead_of_a_panic() {
        let (api, _daemon) = daemon(None);
        let (panel, mut out) = panel(api);
        panel
            .accept(&json!({"type": "panel.submit", "text": "hi"}))
            .await;
        let item = out.recv().await.expect("the failure reaches the panel");
        assert_eq!(item["type"], "panel.error");
        assert!(item["message"].as_str().unwrap().contains("no session"));
    }

    #[tokio::test]
    async fn a_malformed_panel_request_is_claimed_and_reported() {
        let (api, _daemon) = daemon(None);
        let (panel, mut out) = panel(api);
        panel.accept(&json!({"type": "panel.submit"})).await;
        let item = out.recv().await.expect("the failure reaches the panel");
        assert_eq!(item["type"], "panel.error");
    }

    #[tokio::test]
    async fn submitted_text_reaches_the_daemon_as_a_message_op() {
        let (api, mut daemon) = daemon(None);
        let (panel, mut out) = panel(api);
        panel.accept(&json!({"type": "panel.open"})).await;
        let _ = out.recv().await;
        panel
            .accept(&json!({"type": "panel.submit", "text": "ship it"}))
            .await;
        match daemon.ops.recv().await.expect("the daemon sees the op") {
            Op::SubmitMessage { text, .. } => assert_eq!(text, "ship it"),
            other => panic!("expected a message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn stopping_carries_the_task_the_snapshot_named() {
        let (api, mut daemon) = daemon(Some(TaskId(42)));
        let (panel, mut out) = panel(api);
        panel.accept(&json!({"type": "panel.open"})).await;
        let _ = out.recv().await;
        panel.accept(&json!({"type": "panel.interrupt"})).await;
        match daemon.ops.recv().await.expect("the daemon sees the op") {
            Op::Interrupt { id } => assert_eq!(id, TaskId(42)),
            other => panic!("expected an interrupt, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_answer_carries_the_call_the_question_named() {
        let (api, mut daemon) = daemon(None);
        let (panel, mut out) = panel(api);
        panel.accept(&json!({"type": "panel.open"})).await;
        let _ = out.recv().await;
        panel
            .accept(&json!({"type": "panel.answer", "call": "7", "answers": ["ship it"]}))
            .await;
        match daemon.ops.recv().await.expect("the daemon sees the op") {
            Op::Answer { call, answers, .. } => {
                assert_eq!(call, ToolCallId(7));
                assert_eq!(answers, vec!["ship it".to_owned()]);
            }
            other => panic!("expected an answer, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn stopping_an_idle_session_sends_nothing() {
        let (api, mut daemon) = daemon(None);
        let (panel, mut out) = panel(api);
        panel.accept(&json!({"type": "panel.open"})).await;
        let _ = out.recv().await;
        panel.accept(&json!({"type": "panel.interrupt"})).await;
        assert!(out.try_recv().is_err(), "an idle stop is not an error");
        assert!(daemon.ops.try_recv().is_err());
    }
}
