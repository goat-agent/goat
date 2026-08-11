use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use goat_wire::envelope::{CallError, ErrorCode, Execution};
use tokio::sync::Mutex;

pub const MAX_CHUNK_BYTES: usize = 32 * 1024;

fn not_found(id: &str) -> CallError {
    CallError::new(
        ErrorCode::NotFound,
        format!("terminal {id} is not open on this daemon"),
    )
    .with_execution(Execution::NotStarted)
}

fn failed(message: String) -> CallError {
    CallError::new(ErrorCode::Internal, message).with_execution(Execution::KnownFailed)
}

pub trait Terminal: Send + Sync {
    fn write(&self, data: &[u8]) -> Result<(), String>;
    fn resize(&self, cols: u16, rows: u16) -> Result<(), String>;
    fn close(&self);
}

#[derive(Default)]
pub struct Terminals {
    next: AtomicU64,
    open: Mutex<HashMap<String, Arc<dyn Terminal>>>,
}

impl Terminals {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mint_id(&self) -> String {
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        format!("pty_{id}")
    }

    pub async fn insert(&self, id: String, terminal: Arc<dyn Terminal>) {
        self.open.lock().await.insert(id, terminal);
    }

    pub async fn remove(&self, id: &str) -> Option<Arc<dyn Terminal>> {
        self.open.lock().await.remove(id)
    }

    pub async fn count(&self) -> usize {
        self.open.lock().await.len()
    }

    pub async fn write(&self, id: &str, data: &str) -> Result<(), CallError> {
        let terminal = self.open.lock().await.get(id).cloned();
        let terminal = terminal.ok_or_else(|| not_found(id))?;
        terminal.write(data.as_bytes()).map_err(failed)
    }

    pub async fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), CallError> {
        let terminal = self.open.lock().await.get(id).cloned();
        let terminal = terminal.ok_or_else(|| not_found(id))?;
        terminal.resize(cols, rows).map_err(failed)
    }

    pub async fn close(&self, id: &str) {
        if let Some(terminal) = self.remove(id).await {
            terminal.close();
        }
    }

    pub async fn shutdown(&self) {
        let drained: Vec<Arc<dyn Terminal>> = {
            let mut open = self.open.lock().await;
            open.drain().map(|(_, terminal)| terminal).collect()
        };
        for terminal in drained {
            terminal.close();
        }
    }
}

pub struct Chunker {
    dropped: u64,
}

impl Default for Chunker {
    fn default() -> Self {
        Self::new()
    }
}

impl Chunker {
    #[must_use]
    pub fn new() -> Self {
        Self { dropped: 0 }
    }

    pub fn note_drop(&mut self, bytes: u64) {
        self.dropped = self.dropped.saturating_add(bytes);
    }

    pub fn take_dropped(&mut self) -> u64 {
        std::mem::take(&mut self.dropped)
    }

    #[must_use]
    pub fn split(data: &[u8]) -> Vec<String> {
        if data.is_empty() {
            return Vec::new();
        }
        let text = String::from_utf8_lossy(data);
        let mut out = Vec::new();
        let mut current = String::new();
        for ch in text.chars() {
            if current.len() + ch.len_utf8() > MAX_CHUNK_BYTES && !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            current.push(ch);
        }
        if !current.is_empty() {
            out.push(current);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{Chunker, MAX_CHUNK_BYTES, Terminal, Terminals};
    use goat_wire::envelope::{ErrorCode, Execution};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::sync::mpsc;

    struct Fake {
        writes: mpsc::UnboundedSender<String>,
        resizes: mpsc::UnboundedSender<(u16, u16)>,
        closed: AtomicBool,
        closes: AtomicUsize,
        fail: bool,
    }

    impl Fake {
        fn new(
            fail: bool,
        ) -> (
            Arc<Self>,
            mpsc::UnboundedReceiver<String>,
            mpsc::UnboundedReceiver<(u16, u16)>,
        ) {
            let (writes, writes_rx) = mpsc::unbounded_channel();
            let (resizes, resizes_rx) = mpsc::unbounded_channel();
            (
                Arc::new(Self {
                    writes,
                    resizes,
                    closed: AtomicBool::new(false),
                    closes: AtomicUsize::new(0),
                    fail,
                }),
                writes_rx,
                resizes_rx,
            )
        }
    }

    impl Terminal for Fake {
        fn write(&self, data: &[u8]) -> Result<(), String> {
            if self.fail {
                return Err("pipe closed".to_owned());
            }
            let _ = self.writes.send(String::from_utf8_lossy(data).into_owned());
            Ok(())
        }

        fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
            let _ = self.resizes.send((cols, rows));
            Ok(())
        }

        fn close(&self) {
            self.closed.store(true, Ordering::SeqCst);
            self.closes.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn ids_are_unique_and_prefixed() {
        let terminals = Terminals::new();
        let first = terminals.mint_id();
        let second = terminals.mint_id();
        assert_ne!(first, second);
        assert!(first.starts_with("pty_"));
    }

    #[tokio::test]
    async fn writing_and_resizing_reach_the_terminal() {
        let terminals = Terminals::new();
        let (fake, mut writes, mut resizes) = Fake::new(false);
        terminals.insert("pty_1".to_owned(), fake).await;

        terminals.write("pty_1", "ls\n").await.unwrap();
        assert_eq!(writes.recv().await.as_deref(), Some("ls\n"));

        terminals.resize("pty_1", 120, 40).await.unwrap();
        assert_eq!(resizes.recv().await, Some((120, 40)));
    }

    #[tokio::test]
    async fn addressing_an_unknown_terminal_is_not_started() {
        let terminals = Terminals::new();
        let write = terminals.write("pty_9", "x").await.unwrap_err();
        assert_eq!(write.code, ErrorCode::NotFound);
        assert_eq!(write.execution, Some(Execution::NotStarted));
        assert!(write.retry_is_safe());

        let resize = terminals.resize("pty_9", 1, 1).await.unwrap_err();
        assert_eq!(resize.code, ErrorCode::NotFound);
    }

    #[tokio::test]
    async fn a_failing_write_is_reported_as_known_failed() {
        let terminals = Terminals::new();
        let (fake, _writes, _resizes) = Fake::new(true);
        terminals.insert("pty_1".to_owned(), fake).await;
        let err = terminals.write("pty_1", "x").await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Internal);
        assert_eq!(err.execution, Some(Execution::KnownFailed));
        assert!(
            err.retry_is_safe(),
            "a write that never reached the terminal is safe to retry"
        );
    }

    #[tokio::test]
    async fn closing_removes_the_terminal_and_stops_the_process() {
        let terminals = Terminals::new();
        let (fake, _writes, _resizes) = Fake::new(false);
        terminals.insert("pty_1".to_owned(), fake.clone()).await;
        assert_eq!(terminals.count().await, 1);

        terminals.close("pty_1").await;
        assert!(fake.closed.load(Ordering::SeqCst));
        assert_eq!(terminals.count().await, 0);

        terminals.close("pty_1").await;
        assert_eq!(
            fake.closes.load(Ordering::SeqCst),
            1,
            "closing twice must not double-signal the process"
        );
    }

    #[tokio::test]
    async fn shutdown_closes_every_open_terminal() {
        let terminals = Terminals::new();
        let (a, _wa, _ra) = Fake::new(false);
        let (b, _wb, _rb) = Fake::new(false);
        terminals.insert("pty_1".to_owned(), a.clone()).await;
        terminals.insert("pty_2".to_owned(), b.clone()).await;

        terminals.shutdown().await;
        assert!(a.closed.load(Ordering::SeqCst));
        assert!(b.closed.load(Ordering::SeqCst));
        assert_eq!(terminals.count().await, 0);
    }

    #[test]
    fn output_is_split_into_bounded_chunks() {
        let data = vec![b'x'; MAX_CHUNK_BYTES * 2 + 7];
        let chunks = Chunker::split(&data);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].len(), MAX_CHUNK_BYTES);
        assert_eq!(chunks[2].len(), 7);
        assert_eq!(chunks.concat().len(), data.len());
    }

    #[test]
    fn splitting_never_cuts_a_multibyte_character() {
        let unit = "한".as_bytes();
        assert_eq!(unit.len(), 3);
        let data = "한".repeat(MAX_CHUNK_BYTES);
        let chunks = Chunker::split(data.as_bytes());
        for chunk in &chunks {
            assert!(chunk.len() <= MAX_CHUNK_BYTES);
            assert!(std::str::from_utf8(chunk.as_bytes()).is_ok());
        }
        assert_eq!(chunks.concat(), data);
    }

    #[test]
    fn an_empty_read_produces_no_chunks() {
        assert!(Chunker::split(&[]).is_empty());
    }

    #[test]
    fn dropped_bytes_accumulate_until_reported_once() {
        let mut chunker = Chunker::new();
        chunker.note_drop(10);
        chunker.note_drop(5);
        assert_eq!(chunker.take_dropped(), 15);
        assert_eq!(chunker.take_dropped(), 0);
    }
}
