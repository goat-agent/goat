use goat_protocol::{Event, Op};
use tokio::{sync::mpsc, task::JoinHandle};

const OPS_CAPACITY: usize = 32;
const EVENTS_CAPACITY: usize = 512;

pub trait Engine: Send + 'static {
    fn spawn(self, ops: mpsc::Receiver<Op>, events: mpsc::Sender<Event>) -> JoinHandle<()>;
}

pub struct Session {
    ops: mpsc::Sender<Op>,
    events: mpsc::Receiver<Event>,
    handle: JoinHandle<()>,
}

impl Session {
    pub fn spawn<E: Engine>(engine: E) -> Self {
        let (op_tx, op_rx) = mpsc::channel(OPS_CAPACITY);
        let (event_tx, event_rx) = mpsc::channel(EVENTS_CAPACITY);
        let handle = engine.spawn(op_rx, event_tx);
        Self {
            ops: op_tx,
            events: event_rx,
            handle,
        }
    }

    pub fn ops(&self) -> mpsc::Sender<Op> {
        self.ops.clone()
    }

    pub fn into_parts(self) -> (mpsc::Sender<Op>, mpsc::Receiver<Event>, JoinHandle<()>) {
        (self.ops, self.events, self.handle)
    }

    pub async fn shutdown(self) {
        let _ = self.ops.send(Op::Shutdown {}).await;
        let _ = self.handle.await;
    }
}
