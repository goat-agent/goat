use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use crate::keys::PtyInputItem;

pub const MAX_SESSIONS: usize = 8;

const DEFAULT_ROWS: u16 = 40;
const DEFAULT_COLS: u16 = 120;
const MIN_ROWS: u16 = 8;
const MAX_ROWS: u16 = 200;
const MIN_COLS: u16 = 20;
const MAX_COLS: u16 = 400;
const SCROLLBACK: usize = 500;
const MAX_SCREEN_CHARS: usize = 12_000;

pub type SessionId = String;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionStatus {
    Running,
    Exited(Option<i32>),
    Killed,
}

impl std::fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionStatus::Running => write!(f, "running"),
            SessionStatus::Exited(Some(c)) => write!(f, "exited({c})"),
            SessionStatus::Exited(None) => write!(f, "exited"),
            SessionStatus::Killed => write!(f, "killed"),
        }
    }
}

struct PtySession {
    command: String,
    screen: Mutex<vt100::Parser>,
    writer: tokio::sync::Mutex<pty_process::OwnedWritePty>,
    child: tokio::sync::Mutex<tokio::process::Child>,
    child_pid: Option<u32>,
    pty_fd: i32,
    status: Mutex<SessionStatus>,
    rows: AtomicU16,
    cols: AtomicU16,
    touched_ms: AtomicI64,
    reader: OnceLock<tokio::task::AbortHandle>,
}

impl Drop for PtySession {
    fn drop(&mut self) {
        if let Some(h) = self.reader.get() {
            h.abort();
        }
        if let Ok(mut c) = self.child.try_lock() {
            let _ = c.start_kill();
        }
        if self.pty_fd >= 0 {
            unsafe { libc::close(self.pty_fd) };
        }
    }
}

pub struct PtyManager {
    sessions: Mutex<HashMap<SessionId, Arc<PtySession>>>,
    cancel: CancellationToken,
    next_id: AtomicU64,
    max_sessions: usize,
}

pub struct ScreenSnapshot {
    pub session_id: SessionId,
    pub status: SessionStatus,
    pub rows: u16,
    pub cols: u16,
    pub cursor: (u16, u16),
    pub screen: String,
}

pub struct SessionInfo {
    pub id: SessionId,
    pub command: String,
    pub status: SessionStatus,
    pub rows: u16,
    pub cols: u16,
    pub idle_ms: i64,
}

impl PtyManager {
    pub fn new(cancel: CancellationToken, max_sessions: usize) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            cancel,
            next_id: AtomicU64::new(1),
            max_sessions,
        }
    }

    fn now_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    fn next_session_id(&self) -> SessionId {
        format!("s{}", self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    fn get(&self, id: &str) -> Option<Arc<PtySession>> {
        self.sessions.lock().unwrap().get(id).cloned()
    }

    fn require(&self, id: &str) -> Result<Arc<PtySession>, String> {
        self.get(id).ok_or_else(|| format!("no session: {id}"))
    }

    pub async fn open(
        &self,
        command: &str,
        rows: Option<u16>,
        cols: Option<u16>,
    ) -> Result<(SessionId, u16, u16), String> {
        let rows = rows.unwrap_or(DEFAULT_ROWS).clamp(MIN_ROWS, MAX_ROWS);
        let cols = cols.unwrap_or(DEFAULT_COLS).clamp(MIN_COLS, MAX_COLS);

        {
            let table = self.sessions.lock().unwrap();
            if table.len() >= self.max_sessions {
                return Err(format!(
                    "too many sessions (max {}); close one first",
                    self.max_sessions
                ));
            }
        }

        let cmd = if command.trim().is_empty() {
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
        } else {
            command.to_string()
        };

        let (pty, pts) = pty_process::open().map_err(|e| format!("open pty: {e}"))?;
        pty.resize(pty_process::Size::new(rows, cols))
            .map_err(|e| format!("resize pty: {e}"))?;

        #[cfg(unix)]
        let pty_fd = {
            use std::os::unix::io::AsRawFd;
            let fd = unsafe { libc::dup(pty.as_raw_fd()) };
            if fd < 0 {
                return Err(format!("dup pty fd: {}", std::io::Error::last_os_error()));
            }
            fd
        };
        #[cfg(not(unix))]
        let pty_fd = -1i32;

        let child = pty_process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&cmd)
            .spawn(pts)
            .map_err(|e| format!("spawn: {e}"))?;
        let child_pid = child.id();

        let (pty_reader, writer) = pty.into_split();

        let session = Arc::new(PtySession {
            command: cmd,
            screen: Mutex::new(vt100::Parser::new(rows, cols, SCROLLBACK)),
            writer: tokio::sync::Mutex::new(writer),
            child: tokio::sync::Mutex::new(child),
            child_pid,
            pty_fd,
            status: Mutex::new(SessionStatus::Running),
            rows: AtomicU16::new(rows),
            cols: AtomicU16::new(cols),
            touched_ms: AtomicI64::new(Self::now_ms()),
            reader: OnceLock::new(),
        });

        let sess = Arc::clone(&session);
        let cancel = self.cancel.clone();
        let jh = tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            let mut reader = pty_reader;
            loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        let _ = sess.child.lock().await.start_kill();
                        *sess.status.lock().unwrap() = SessionStatus::Killed;
                        break;
                    }
                    r = reader.read(&mut buf) => match r {
                        Ok(0) | Err(_) => {
                            let code = sess.child.lock().await.wait().await
                                .ok().and_then(|s| s.code());
                            *sess.status.lock().unwrap() = SessionStatus::Exited(code);
                            break;
                        }
                        Ok(n) => {
                            sess.screen.lock().unwrap().process(&buf[..n]);
                            sess.touched_ms.store(Self::now_ms(), Ordering::Relaxed);
                        }
                    }
                }
            }
        });
        session.reader.set(jh.abort_handle()).ok();

        let id = self.next_session_id();
        self.sessions.lock().unwrap().insert(id.clone(), session);

        Ok((id, rows, cols))
    }

    pub async fn input(&self, id: &str, items: &[PtyInputItem]) -> Result<usize, String> {
        let session = self.require(id)?;
        if !matches!(*session.status.lock().unwrap(), SessionStatus::Running) {
            return Err(format!("session {id} is not running"));
        }
        let bytes: Vec<u8> = items.iter().flat_map(|i| i.to_bytes()).collect();
        let n = items.len();
        session
            .writer
            .lock()
            .await
            .write_all(&bytes)
            .await
            .map_err(|e| format!("write: {e}"))?;
        session.touched_ms.store(Self::now_ms(), Ordering::Relaxed);
        Ok(n)
    }

    pub fn read(&self, id: &str) -> Result<ScreenSnapshot, String> {
        let session = self.require(id)?;
        let (screen, cursor) = {
            let parser = session.screen.lock().unwrap();
            let s = parser.screen();
            let text = s.contents();
            let trimmed = text.trim_end_matches('\n');
            let truncated = if trimmed.chars().count() > MAX_SCREEN_CHARS {
                let s: String = trimmed.chars().take(MAX_SCREEN_CHARS).collect();
                format!("{s}\n...[truncated]")
            } else {
                trimmed.to_string()
            };
            (truncated, s.cursor_position())
        };
        let status = session.status.lock().unwrap().clone();
        Ok(ScreenSnapshot {
            session_id: id.to_string(),
            status,
            rows: session.rows.load(Ordering::Relaxed),
            cols: session.cols.load(Ordering::Relaxed),
            cursor,
            screen,
        })
    }

    pub fn list(&self) -> Vec<SessionInfo> {
        let now = Self::now_ms();
        let mut infos: Vec<SessionInfo> = self
            .sessions
            .lock()
            .unwrap()
            .iter()
            .map(|(id, s)| SessionInfo {
                id: id.clone(),
                command: s.command.clone(),
                status: s.status.lock().unwrap().clone(),
                rows: s.rows.load(Ordering::Relaxed),
                cols: s.cols.load(Ordering::Relaxed),
                idle_ms: now - s.touched_ms.load(Ordering::Relaxed),
            })
            .collect();
        infos.sort_by(|a, b| a.id.cmp(&b.id));
        infos
    }

    pub async fn close(&self, id: &str) -> Result<SessionStatus, String> {
        let session = self
            .sessions
            .lock()
            .unwrap()
            .remove(id)
            .ok_or_else(|| format!("no session: {id}"))?;

        if let Some(h) = session.reader.get() {
            h.abort();
        }

        let code = {
            let mut child = session.child.lock().await;
            let _ = child.start_kill();
            child.wait().await.ok().and_then(|s| s.code())
        };

        Ok(SessionStatus::Exited(code))
    }

    pub fn resize(&self, id: &str, rows: u16, cols: u16) -> Result<(), String> {
        let rows = rows.clamp(MIN_ROWS, MAX_ROWS);
        let cols = cols.clamp(MIN_COLS, MAX_COLS);
        let session = self.require(id)?;

        session.screen.lock().unwrap().set_size(rows, cols);
        session.rows.store(rows, Ordering::Relaxed);
        session.cols.store(cols, Ordering::Relaxed);

        #[cfg(unix)]
        {
            let ws = libc::winsize {
                ws_row: rows,
                ws_col: cols,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            let ret = unsafe { libc::ioctl(session.pty_fd, libc::TIOCSWINSZ, &ws) };
            if ret != 0 {
                return Err(format!(
                    "TIOCSWINSZ failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }

        Ok(())
    }

    pub async fn signal(&self, id: &str, sig: &str) -> Result<(), String> {
        let session = self.require(id)?;

        #[cfg(unix)]
        {
            let signum: libc::c_int = match sig {
                "int" => libc::SIGINT,
                "term" => libc::SIGTERM,
                "hup" => libc::SIGHUP,
                "kill" => libc::SIGKILL,
                other => return Err(format!("unknown signal: {other}; use int, term, hup, kill")),
            };
            if let Some(pid) = session.child_pid {
                let ret = unsafe { libc::kill(pid as libc::pid_t, signum) };
                if ret != 0 {
                    return Err(format!("kill: {}", std::io::Error::last_os_error()));
                }
            } else {
                return Err(format!("session {id} has no pid"));
            }
        }

        #[cfg(not(unix))]
        {
            let _ = (session, sig);
            return Err("signal not supported on this platform".into());
        }

        Ok(())
    }
}
