use std::{collections::HashMap, time::Instant};

use crossterm::event::Event as InputEvent;
use goat_client::Identity;
use goat_github::PrInfo;
use goat_protocol::{
    AccountEntry, Event, Mode, ModelEntry, ModelTarget, NotifyKind, RateLimitSnapshot, TaskId,
    ThreadSummary, Usage,
};
use goat_worktree::Workspace;
use ratatui::{Frame, layout::Rect};

use crate::{CommandEffect, Theme};

pub enum InputOutcome {
    Ignored,
    Handled(ScreenOutcome),
}

pub enum ScreenOutcome {
    Continue,
    Close,
    Effect(CommandEffect),
}

pub struct KeyHint {
    pub key: &'static str,
    pub label: &'static str,
}

pub enum Placement {
    Hidden,
    Full {
        reserve_bottom: Option<u16>,
    },
    Overlay,
    Panel {
        height: u16,
        hints: Option<Vec<KeyHint>>,
        composer_focused: bool,
    },
}

pub trait Screen: Send {
    fn placement(&self) -> Placement;
    fn captures_text(&self) -> bool {
        false
    }
    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme);
    fn handle_input(&mut self, event: &InputEvent, session: &mut dyn Session) -> InputOutcome;
    fn on_event(&mut self, _event: &Event, _session: &mut dyn Session) -> ScreenOutcome {
        ScreenOutcome::Continue
    }
    fn tick(&mut self) -> ScreenOutcome {
        ScreenOutcome::Continue
    }
}

#[derive(Default)]
pub struct UsageState {
    pub last: HashMap<(String, String), Usage>,
    pub total: HashMap<(String, String), (u64, u64)>,
    pub rate_limits: HashMap<(String, String), (RateLimitSnapshot, i64)>,
    pub scroll: usize,
    pub turn_tokens: u64,
}

#[derive(Clone)]
pub struct SessionSnapshot {
    pub session_id: Option<u64>,
    pub client_id: Option<u64>,
    pub thread_id: Option<i64>,
    pub daemon: Option<Identity>,
    pub model: Option<ModelTarget>,
    pub models_loaded: bool,
    pub mode: Mode,
    pub plan_path: Option<String>,
    pub cwd: String,
    pub remote: Option<String>,
    pub workspace: Option<Workspace>,
    pub pull_request: Option<PrInfo>,
    pub window_count: usize,
    pub queued_count: usize,
    pub process_count: usize,
    pub skill_count: usize,
    pub transcript_entries: usize,
    pub mouse_capture: bool,
    pub computer_use: bool,
    pub browser: bool,
    pub dark_theme: bool,
    pub log_path: Option<String>,
    pub started: Instant,
}

pub trait Settings {
    fn theme(&self) -> Theme;
    fn set_theme(&mut self, theme: Theme);
    fn mouse_capture(&self) -> bool;
    fn set_mouse_capture(&mut self, enabled: bool);
    fn computer_use(&self) -> bool;
    fn set_computer_use(&mut self, enabled: bool);
    fn browser(&self) -> bool;
    fn set_browser(&mut self, enabled: bool);
}

pub trait Composer {
    fn text(&self) -> String;
    fn is_empty(&self) -> bool;
    fn shell(&self) -> bool;
    fn set_plain_text(&mut self, text: &str);
    fn replace_at_query(&mut self, replacement: &str);
    fn insert_str(&mut self, text: &str);
    fn insert_char(&mut self, ch: char);
    fn backspace(&mut self);
    fn delete_forward(&mut self);
    fn move_left(&mut self) -> bool;
    fn move_right(&mut self) -> bool;
    fn move_word_left(&mut self) -> bool;
    fn move_word_right(&mut self) -> bool;
    fn move_home(&mut self) -> bool;
    fn move_end(&mut self) -> bool;
    fn move_up(&mut self) -> bool;
    fn move_down(&mut self) -> bool;
    fn newline(&mut self);
    fn at_query(&self) -> Option<String>;
}

pub trait Viewport {
    fn scroll(&self) -> usize;
    fn set_scroll(&mut self, scroll: usize);
    fn follow(&self) -> bool;
    fn set_follow(&mut self, follow: bool);
    fn page_rows(&self) -> usize;
    fn run_cursor(&self) -> Option<usize>;
    fn run_count(&self) -> usize;
    fn move_run_cursor(&mut self, cursor: usize);
    fn open_run(&mut self, cursor: usize);
    fn close_run_selector(&mut self);
}

pub trait Session {
    fn models(&self) -> &[ModelEntry];
    fn current_model(&self) -> Option<&ModelTarget>;
    fn threads(&self) -> &[ThreadSummary];
    fn usage(&self) -> &UsageState;
    fn mode(&self) -> Mode;
    fn accounts(&self) -> &[AccountEntry];
    fn snapshot(&self) -> SessionSnapshot;
    fn is_busy(&self) -> bool;
    fn queued_len(&self) -> usize;
    fn settings(&mut self) -> &mut dyn Settings;
    fn composer(&mut self) -> &mut dyn Composer;
    fn viewport(&mut self) -> &mut dyn Viewport;
    fn notify(&mut self, kind: NotifyKind, message: String);
    fn allocate_task(&mut self) -> TaskId;
}
