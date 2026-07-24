mod engine;

pub use engine::{Engine, Session};

#[cfg(test)]
mod tests {
    use goat_protocol::{Event, Op, TaskId};
    use tokio::{sync::mpsc, task::JoinHandle};

    use crate::{Engine, Session};

    struct EchoEngine;

    impl Engine for EchoEngine {
        fn spawn(self, mut ops: mpsc::Receiver<Op>, events: mpsc::Sender<Event>) -> JoinHandle<()> {
            tokio::spawn(async move {
                while let Some(op) = ops.recv().await {
                    match op {
                        Op::SubmitMessage { id, .. } => {
                            let _ = events.send(Event::TaskStarted { id }).await;
                            let _ = events
                                .send(Event::TextDone {
                                    id,
                                    text: "pong".to_owned(),
                                })
                                .await;
                            let _ = events
                                .send(Event::TaskDone {
                                    id,
                                    interrupted: false,
                                })
                                .await;
                        }
                        Op::Shutdown {} => break,
                        _ => {}
                    }
                }
            })
        }
    }

    #[tokio::test]
    async fn session_receives_task_started_and_done() {
        let session = Session::spawn(EchoEngine);
        let (ops, mut events, _handle) = session.into_parts();
        ops.send(Op::SubmitMessage {
            id: TaskId(1),
            text: "hi".to_owned(),
            display: None,
            attachments: Vec::new(),
        })
        .await
        .unwrap();

        let mut saw_started = false;
        let mut saw_text_done = false;
        loop {
            match events.recv().await.expect("engine stopped early") {
                Event::TaskStarted { id } => {
                    assert_eq!(id, TaskId(1));
                    saw_started = true;
                }
                Event::TextDone { text, .. } => {
                    assert_eq!(text, "pong");
                    saw_text_done = true;
                }
                Event::TaskDone { interrupted, id } => {
                    assert_eq!(id, TaskId(1));
                    assert!(!interrupted);
                    break;
                }
                _ => {}
            }
        }
        assert!(saw_started);
        assert!(saw_text_done);
    }

    #[tokio::test]
    async fn session_ops_returns_sender() {
        let session = Session::spawn(EchoEngine);
        let _sender = session.ops();
    }
}
