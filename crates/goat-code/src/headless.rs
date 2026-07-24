use std::io::Write as _;
use std::sync::Arc;

use goat_protocol::{Event, Op, TaskId, ToolCallId};
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::sync::Mutex;
use tokio::sync::mpsc::{Receiver, Sender};

#[derive(Debug, thiserror::Error)]
pub enum HeadlessError {
    #[error("unknown protocol: {0}")]
    UnknownProtocol(String),
}

pub fn codec_for(protocol: &str) -> Result<Box<dyn Codec>, HeadlessError> {
    match protocol {
        "json" => Ok(Box::new(JsonCodec)),
        other => Err(HeadlessError::UnknownProtocol(other.to_owned())),
    }
}

#[derive(Default)]
pub struct Correlation {
    next_task: u64,
    active_turn: Option<TaskId>,
    last_prompt: Option<(TaskId, ToolCallId)>,
}

impl Correlation {
    fn new() -> Self {
        Self {
            next_task: 1,
            active_turn: None,
            last_prompt: None,
        }
    }

    fn allocate_task(&mut self) -> TaskId {
        let id = TaskId(self.next_task);
        self.next_task += 1;
        id
    }

    fn observe_event(&mut self, event: &Event) {
        match event {
            Event::TaskStarted { id } => self.active_turn = Some(*id),
            Event::TaskDone { .. } => self.active_turn = None,
            Event::AskStarted { id, call, .. } => {
                self.last_prompt = Some((*id, *call));
            }
            _ => {}
        }
    }
}

pub trait Codec: Send + Sync {
    fn decode(&self, line: &str, ctx: &mut Correlation) -> Result<Option<Op>, String>;
    fn encode(&self, event: &Event) -> Option<String>;
}

struct JsonCodec;

impl Codec for JsonCodec {
    fn decode(&self, line: &str, ctx: &mut Correlation) -> Result<Option<Op>, String> {
        let mut value: serde_json::Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
        default_id_fields(&mut value);
        let mut op: Op = serde_json::from_value(value).map_err(|e| e.to_string())?;
        fill_ids(&mut op, ctx);
        Ok(Some(op))
    }

    fn encode(&self, event: &Event) -> Option<String> {
        serde_json::to_string(event).ok()
    }
}

fn default_id_fields(value: &mut serde_json::Value) {
    let Some(map) = value.as_object_mut() else {
        return;
    };
    for field in ["id", "call"] {
        map.entry(field)
            .or_insert_with(|| serde_json::Value::String("0".to_owned()));
    }
}

fn fill_ids(op: &mut Op, ctx: &mut Correlation) {
    match op {
        Op::SubmitMessage { id, .. } | Op::SubmitShell { id, .. } | Op::Compact { id, .. }
            if id.0 == 0 =>
        {
            *id = ctx.allocate_task();
        }
        Op::Interrupt { id } | Op::DequeueMessage { id } => {
            if id.0 == 0
                && let Some(active) = ctx.active_turn
            {
                *id = active;
            }
        }
        Op::Answer { id, call, .. } => {
            if let Some((prompt_id, prompt_call)) = ctx.last_prompt {
                if id.0 == 0 {
                    *id = prompt_id;
                }
                if call.0 == 0 {
                    *call = prompt_call;
                }
            }
        }
        _ => {}
    }
}

const SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(5);
const STDIN_EOF_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

pub enum Exit {
    Ok,
    Disconnected,
}

pub async fn run(
    ops: Sender<Op>,
    events: Receiver<Event>,
    codec: Box<dyn Codec>,
    one_shot: bool,
) -> Exit {
    let (stdin_tx, stdin_rx) = tokio::sync::mpsc::channel::<String>(32);
    let reader = tokio::spawn(read_stdin(stdin_tx));
    let exit = drive(ops, events, stdin_rx, codec, one_shot).await;
    reader.abort();
    exit
}

async fn drive(
    ops: Sender<Op>,
    mut events: Receiver<Event>,
    mut stdin_rx: Receiver<String>,
    codec: Box<dyn Codec>,
    one_shot: bool,
) -> Exit {
    let correlation = Arc::new(Mutex::new(Correlation::new()));
    let codec: Arc<dyn Codec> = Arc::from(codec);

    let mut turn_active = false;
    let mut stdin_open = true;
    let mut events_closed = false;
    let mut idle_deadline: Option<tokio::time::Instant> = None;

    loop {
        let idle_sleep = idle_deadline.map(tokio::time::sleep_until);
        tokio::select! {
            biased;
            maybe = events.recv() => {
                let Some(event) = maybe else { events_closed = true; break };
                {
                    let mut ctx = correlation.lock().await;
                    ctx.observe_event(&event);
                }
                match &event {
                    Event::TaskStarted { .. } => {
                        turn_active = true;
                        idle_deadline = None;
                    }
                    Event::TaskDone { .. } => turn_active = false,
                    _ => {}
                }
                if let Some(line) = codec.encode(&event) {
                    emit_line(&line);
                }
                if matches!(event, Event::TaskDone { .. }) && (one_shot || !stdin_open) {
                    break;
                }
            }
            maybe = stdin_rx.recv(), if stdin_open => {
                if let Some(line) = maybe {
                    let decoded = {
                        let mut ctx = correlation.lock().await;
                        codec.decode(&line, &mut ctx)
                    };
                    match decoded {
                        Ok(Some(op)) => {
                            if ops.send(op).await.is_err() {
                                events_closed = true;
                                break;
                            }
                        }
                        Ok(None) => {}
                        Err(message) => emit_decode_error(&message),
                    }
                } else {
                    stdin_open = false;
                    if turn_active {
                        continue;
                    }
                    idle_deadline = Some(tokio::time::Instant::now() + STDIN_EOF_GRACE);
                }
            }
            () = async { idle_sleep.unwrap().await }, if idle_deadline.is_some() => {
                if !turn_active {
                    break;
                }
                idle_deadline = None;
            }
            signal = tokio::signal::ctrl_c() => {
                if signal.is_err() {
                    events_closed = true;
                }
                break;
            }
        }
    }

    if turn_active && !events_closed {
        let id = correlation.lock().await.active_turn;
        if let Some(id) = id {
            let _ = ops.send(Op::Interrupt { id }).await;
            drain_until_done(&mut events, &codec, &correlation).await;
        }
    }

    let _ = ops.send(Op::Shutdown {}).await;
    if events_closed {
        Exit::Disconnected
    } else {
        Exit::Ok
    }
}

async fn read_stdin(tx: Sender<String>) {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if tx.send(trimmed.to_owned()).await.is_err() {
            break;
        }
    }
}

async fn drain_until_done(
    events: &mut Receiver<Event>,
    codec: &Arc<dyn Codec>,
    correlation: &Arc<Mutex<Correlation>>,
) {
    let deadline = tokio::time::Instant::now() + SHUTDOWN_GRACE;
    loop {
        tokio::select! {
            biased;
            () = async {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                tokio::time::sleep(remaining).await;
            } => return,
            _ = tokio::signal::ctrl_c() => return,
            maybe = events.recv() => {
                let Some(event) = maybe else { return };
                {
                    let mut ctx = correlation.lock().await;
                    ctx.observe_event(&event);
                }
                if let Some(line) = codec.encode(&event) {
                    emit_line(&line);
                }
                if matches!(event, Event::TaskDone { .. }) {
                    return;
                }
            }
        }
    }
}

fn emit_line(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

fn emit_decode_error(message: &str) {
    let event = Event::Error {
        id: None,
        message: format!("headless decode error: {message}"),
        hint: None,
    };
    if let Ok(line) = serde_json::to_string(&event) {
        emit_line(&line);
    }
}

#[cfg(test)]
mod tests {
    use super::{Correlation, codec_for, fill_ids};
    use goat_protocol::{Event, Op, TaskId, ToolCallId};

    #[test]
    fn unknown_protocol_rejected() {
        assert!(codec_for("acp").is_err());
        assert!(codec_for("json").is_ok());
    }

    #[test]
    fn submit_message_allocates_task_id() {
        let codec = codec_for("json").unwrap();
        let mut ctx = Correlation::new();
        let op = codec
            .decode(r#"{"type":"SubmitMessage","id":"0","text":"hi"}"#, &mut ctx)
            .unwrap()
            .unwrap();
        match op {
            Op::SubmitMessage { id, text, .. } => {
                assert_eq!(id, TaskId(1));
                assert_eq!(text, "hi");
            }
            _ => panic!("wrong op"),
        }
    }

    #[test]
    fn submit_message_without_id_field_allocates() {
        let codec = codec_for("json").unwrap();
        let mut ctx = Correlation::new();
        let op = codec
            .decode(r#"{"type":"SubmitMessage","text":"hi"}"#, &mut ctx)
            .unwrap()
            .unwrap();
        match op {
            Op::SubmitMessage { id, .. } => assert_eq!(id, TaskId(1)),
            _ => panic!("wrong op"),
        }
    }

    #[test]
    fn answer_without_id_fields_echoes_prompt() {
        let codec = codec_for("json").unwrap();
        let mut ctx = Correlation::new();
        ctx.observe_event(&Event::AskStarted {
            id: TaskId(7),
            call: ToolCallId(42),
            questions: Vec::new(),
        });
        let op = codec
            .decode(r#"{"type":"Answer","answers":["yes"]}"#, &mut ctx)
            .unwrap()
            .unwrap();
        match op {
            Op::Answer { id, call, .. } => {
                assert_eq!(id, TaskId(7));
                assert_eq!(call, ToolCallId(42));
            }
            _ => panic!("wrong op"),
        }
    }

    #[test]
    fn answer_echoes_last_prompt_ids() {
        let mut ctx = Correlation::new();
        ctx.observe_event(&Event::TaskStarted { id: TaskId(7) });
        ctx.observe_event(&Event::AskStarted {
            id: TaskId(7),
            call: ToolCallId(42),
            questions: Vec::new(),
        });
        let mut op = Op::Answer {
            id: TaskId(0),
            call: ToolCallId(0),
            answers: vec!["yes".to_owned()],
        };
        fill_ids(&mut op, &mut ctx);
        match op {
            Op::Answer { id, call, .. } => {
                assert_eq!(id, TaskId(7));
                assert_eq!(call, ToolCallId(42));
            }
            _ => panic!("wrong op"),
        }
    }

    #[test]
    fn interrupt_uses_active_turn() {
        let mut ctx = Correlation::new();
        ctx.observe_event(&Event::TaskStarted { id: TaskId(3) });
        let mut op = Op::Interrupt { id: TaskId(0) };
        fill_ids(&mut op, &mut ctx);
        match op {
            Op::Interrupt { id } => assert_eq!(id, TaskId(3)),
            _ => panic!("wrong op"),
        }
    }

    #[test]
    fn task_done_clears_active_turn() {
        let mut ctx = Correlation::new();
        ctx.observe_event(&Event::TaskStarted { id: TaskId(5) });
        ctx.observe_event(&Event::TaskDone {
            id: TaskId(5),
            interrupted: false,
        });
        let mut op = Op::Interrupt { id: TaskId(0) };
        fill_ids(&mut op, &mut ctx);
        match op {
            Op::Interrupt { id } => assert_eq!(id, TaskId(0)),
            _ => panic!("wrong op"),
        }
    }

    #[test]
    fn event_round_trips_to_json_line() {
        let codec = codec_for("json").unwrap();
        let line = codec
            .encode(&Event::TextDone {
                id: TaskId(1),
                text: "done".to_owned(),
            })
            .unwrap();
        assert!(!line.contains('\n'));
        let back: Event = serde_json::from_str(&line).unwrap();
        assert!(matches!(back, Event::TextDone { .. }));
    }

    #[tokio::test]
    async fn drain_returns_on_task_done() {
        use std::sync::Arc;
        use tokio::sync::{Mutex, mpsc};

        let (tx, mut rx) = mpsc::channel::<Event>(8);
        let codec: Arc<dyn super::Codec> = Arc::from(codec_for("json").unwrap());
        let correlation = Arc::new(Mutex::new(Correlation::new()));

        tx.send(Event::TextDelta {
            id: TaskId(1),
            chunk: "x".to_owned(),
        })
        .await
        .unwrap();
        tx.send(Event::TaskDone {
            id: TaskId(1),
            interrupted: true,
        })
        .await
        .unwrap();
        tx.send(Event::TextDelta {
            id: TaskId(1),
            chunk: "after".to_owned(),
        })
        .await
        .unwrap();

        super::drain_until_done(&mut rx, &codec, &correlation).await;

        let leftover = rx.try_recv();
        assert!(matches!(leftover, Ok(Event::TextDelta { .. })));
    }

    #[tokio::test]
    async fn stdin_eof_lets_active_turn_finish_then_exits() {
        use tokio::sync::mpsc;

        let (ops_tx, mut ops_rx) = mpsc::channel::<Op>(8);
        let (events_tx, events_rx) = mpsc::channel::<Event>(8);
        let (stdin_tx, stdin_rx) = mpsc::channel::<String>(8);
        let codec = codec_for("json").unwrap();

        let driver =
            tokio::spawn(
                async move { super::drive(ops_tx, events_rx, stdin_rx, codec, false).await },
            );

        stdin_tx
            .send(r#"{"type":"SubmitMessage","text":"go"}"#.to_owned())
            .await
            .unwrap();
        let first = ops_rx.recv().await.unwrap();
        let turn_id = match first {
            Op::SubmitMessage { id, .. } => id,
            other => panic!("expected SubmitMessage, got {other:?}"),
        };

        events_tx
            .send(Event::TaskStarted { id: turn_id })
            .await
            .unwrap();
        drop(stdin_tx);

        events_tx
            .send(Event::TaskDone {
                id: turn_id,
                interrupted: false,
            })
            .await
            .unwrap();

        let next = ops_rx.recv().await.unwrap();
        assert!(
            matches!(next, Op::Shutdown {}),
            "eof should let the turn finish without an interrupt, got {next:?}"
        );

        let exit = driver.await.unwrap();
        assert!(matches!(exit, super::Exit::Ok));
    }

    #[tokio::test]
    async fn disconnect_during_turn_skips_interrupt() {
        use tokio::sync::mpsc;

        let (ops_tx, mut ops_rx) = mpsc::channel::<Op>(8);
        let (events_tx, events_rx) = mpsc::channel::<Event>(8);
        let (_stdin_tx, stdin_rx) = mpsc::channel::<String>(8);
        let codec = codec_for("json").unwrap();

        let driver =
            tokio::spawn(
                async move { super::drive(ops_tx, events_rx, stdin_rx, codec, false).await },
            );

        events_tx
            .send(Event::TaskStarted { id: TaskId(1) })
            .await
            .unwrap();
        drop(events_tx);

        let next = ops_rx.recv().await.unwrap();
        assert!(
            matches!(next, Op::Shutdown {}),
            "events disconnect must not emit an interrupt, got {next:?}"
        );

        let exit = driver.await.unwrap();
        assert!(matches!(exit, super::Exit::Disconnected));
    }
}
