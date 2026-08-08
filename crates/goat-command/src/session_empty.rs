use goat_protocol::{
    AccountEntry, ConversationSummary, Mode, ModelEntry, ModelTarget, NotifyKind, TaskId,
};

use crate::{Composer, Session, SessionSnapshot, Settings, Theme, UsageState, Viewport};

pub struct EmptySession {
    pub models: Vec<ModelEntry>,
    pub current_model: Option<ModelTarget>,
    pub conversations: Vec<ConversationSummary>,
    pub usage: UsageState,
    pub mode: Mode,
    pub accounts: Vec<AccountEntry>,
    theme: Theme,
    mouse_capture: bool,
    computer_use: bool,
    browser: bool,
    text: String,
    scroll: usize,
    follow: bool,
    next_task: u64,
    notifications: Vec<(NotifyKind, String)>,
}

impl Default for EmptySession {
    fn default() -> Self {
        Self {
            models: Vec::new(),
            current_model: None,
            conversations: Vec::new(),
            usage: UsageState::default(),
            mode: Mode::default(),
            accounts: Vec::new(),
            theme: Theme::default(),
            mouse_capture: false,
            computer_use: false,
            browser: false,
            text: String::new(),
            scroll: 0,
            follow: true,
            next_task: 1,
            notifications: Vec::new(),
        }
    }
}

impl EmptySession {
    pub fn notifications(&self) -> &[(NotifyKind, String)] {
        &self.notifications
    }
}

impl Settings for EmptySession {
    fn theme(&self) -> Theme {
        self.theme
    }

    fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    fn mouse_capture(&self) -> bool {
        self.mouse_capture
    }

    fn set_mouse_capture(&mut self, enabled: bool) {
        self.mouse_capture = enabled;
    }

    fn computer_use(&self) -> bool {
        self.computer_use
    }

    fn set_computer_use(&mut self, enabled: bool) {
        self.computer_use = enabled;
    }

    fn browser(&self) -> bool {
        self.browser
    }

    fn set_browser(&mut self, enabled: bool) {
        self.browser = enabled;
    }
}

impl Composer for EmptySession {
    fn text(&self) -> String {
        self.text.clone()
    }

    fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    fn shell(&self) -> bool {
        false
    }

    fn set_plain_text(&mut self, text: &str) {
        text.clone_into(&mut self.text);
    }

    fn replace_at_query(&mut self, replacement: &str) {
        self.text.push_str(replacement);
    }

    fn insert_str(&mut self, text: &str) {
        self.text.push_str(text);
    }

    fn insert_char(&mut self, ch: char) {
        self.text.push(ch);
    }

    fn backspace(&mut self) {
        self.text.pop();
    }

    fn delete_forward(&mut self) {}

    fn move_left(&mut self) -> bool {
        false
    }

    fn move_right(&mut self) -> bool {
        false
    }

    fn move_word_left(&mut self) -> bool {
        false
    }

    fn move_word_right(&mut self) -> bool {
        false
    }

    fn move_home(&mut self) -> bool {
        false
    }

    fn move_end(&mut self) -> bool {
        false
    }

    fn move_up(&mut self) -> bool {
        false
    }

    fn move_down(&mut self) -> bool {
        false
    }

    fn newline(&mut self) {
        self.text.push('\n');
    }

    fn at_query(&self) -> Option<String> {
        None
    }
}

impl Viewport for EmptySession {
    fn scroll(&self) -> usize {
        self.scroll
    }

    fn set_scroll(&mut self, scroll: usize) {
        self.scroll = scroll;
    }

    fn follow(&self) -> bool {
        self.follow
    }

    fn set_follow(&mut self, follow: bool) {
        self.follow = follow;
    }

    fn page_rows(&self) -> usize {
        1
    }

    fn run_cursor(&self) -> Option<usize> {
        None
    }

    fn run_count(&self) -> usize {
        0
    }

    fn move_run_cursor(&mut self, _cursor: usize) {}

    fn open_run(&mut self, _cursor: usize) {}

    fn close_run_selector(&mut self) {}
}

impl Session for EmptySession {
    fn models(&self) -> &[ModelEntry] {
        &self.models
    }

    fn current_model(&self) -> Option<&ModelTarget> {
        self.current_model.as_ref()
    }

    fn conversations(&self) -> &[ConversationSummary] {
        &self.conversations
    }

    fn usage(&self) -> &UsageState {
        &self.usage
    }

    fn mode(&self) -> Mode {
        self.mode
    }

    fn accounts(&self) -> &[AccountEntry] {
        &self.accounts
    }

    fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot {
            session_id: None,
            client_id: None,
            conversation_id: None,
            daemon: None,
            model: self.current_model.clone(),
            models_loaded: !self.models.is_empty(),
            mode: self.mode,
            plan_path: None,
            cwd: String::new(),
            remote: None,
            workspace: None,
            pull_request: None,
            window_count: 1,
            queued_count: 0,
            process_count: 0,
            skill_count: 0,
            transcript_entries: 0,
            mouse_capture: self.mouse_capture,
            computer_use: self.computer_use,
            browser: self.browser,
            dark_theme: self.theme.is_dark(),
            log_path: None,
            started: std::time::Instant::now(),
        }
    }

    fn is_busy(&self) -> bool {
        false
    }

    fn queued_len(&self) -> usize {
        0
    }

    fn settings(&mut self) -> &mut dyn Settings {
        self
    }

    fn composer(&mut self) -> &mut dyn Composer {
        self
    }

    fn viewport(&mut self) -> &mut dyn Viewport {
        self
    }

    fn notify(&mut self, kind: NotifyKind, message: String) {
        self.notifications.push((kind, message));
    }

    fn allocate_task(&mut self) -> TaskId {
        let id = TaskId(self.next_task);
        self.next_task += 1;
        id
    }
}
