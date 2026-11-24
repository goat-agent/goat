use serde::Serialize;

use super::super::worker::{self, Worker};

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub(crate) struct Rect {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) width: f64,
    pub(crate) height: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct App {
    pub(crate) name: String,
    pub(crate) id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Display {
    pub(crate) id: String,
    #[serde(flatten)]
    pub(crate) frame: Rect,
    pub(crate) scale: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Window {
    #[serde(rename = "window")]
    pub(crate) id: String,
    pub(crate) title: String,
    #[serde(flatten)]
    pub(crate) frame: Rect,
    pub(crate) minimized: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Running {
    #[serde(flatten)]
    pub(crate) app: App,
    pub(crate) windows: Vec<Window>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Screen {
    pub(crate) displays: Vec<Display>,
    pub(crate) focused_app: Option<App>,
    pub(crate) focused_window: Option<String>,
    pub(crate) apps: Vec<Running>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Element {
    #[serde(rename = "ref")]
    pub(crate) reference: String,
    pub(crate) role: String,
    pub(crate) name: String,
    pub(crate) value: Option<String>,
    #[serde(flatten)]
    pub(crate) frame: Rect,
    pub(crate) enabled: bool,
    pub(crate) focused: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Capture {
    pub(crate) png: Vec<u8>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) origin_x: f64,
    pub(crate) origin_y: f64,
    pub(crate) scale: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Button {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Pointer {
    Move {
        x: f64,
        y: f64,
    },
    Click {
        x: f64,
        y: f64,
        button: Button,
        count: u8,
        keys: Vec<Key>,
    },
    Drag {
        from: (f64, f64),
        to: (f64, f64),
    },
    Scroll {
        x: f64,
        y: f64,
        direction: Direction,
        amount: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {
    Command,
    Control,
    Option,
    Shift,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Chord {
    pub(crate) modifiers: Vec<Key>,
    pub(crate) key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Opening {
    App(String),
    Path(String),
    Url(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowState {
    Focused,
    Minimized,
    Maximized,
    Normal,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Fault {
    #[cfg_attr(
        all(not(target_os = "macos"), not(test)),
        expect(dead_code, reason = "only the macOS adapter finds stale refs so far")
    )]
    Stale(String),
    Missing(String),
    Unsupported(String),
    Denied(String),
    Failed(String),
}

pub(crate) trait Platform {
    fn screen(&mut self) -> Result<Screen, Fault>;
    fn elements(&mut self, window: Option<&str>, find: Option<&str>)
    -> Result<Vec<Element>, Fault>;
    fn element_at(&mut self, x: f64, y: f64) -> Result<Option<Element>, Fault>;
    fn capture(&mut self, window: Option<&str>, region: Option<Rect>) -> Result<Capture, Fault>;
    fn owner(&mut self, reference: &str) -> Result<(App, Rect), Fault>;
    fn app_at(&mut self, x: f64, y: f64) -> Result<App, Fault>;
    fn window_app(&mut self, window: &str) -> Result<App, Fault>;
    fn focused_app(&mut self) -> Result<App, Fault>;
    fn press(&mut self, reference: &str, button: Button) -> Result<(), Fault>;
    fn set_text(&mut self, reference: &str, text: &str) -> Result<(), Fault>;
    fn pointer(&mut self, input: &Pointer) -> Result<(), Fault>;
    fn type_text(&mut self, text: &str) -> Result<(), Fault>;
    fn press_keys(&mut self, chord: &Chord) -> Result<(), Fault>;
    fn open(&mut self, opening: &Opening) -> Result<App, Fault>;
    fn quit(&mut self, app: &App) -> Result<(), Fault>;
    fn set_window(
        &mut self,
        window: &str,
        state: Option<WindowState>,
        frame: Option<Rect>,
    ) -> Result<(), Fault>;
    fn read_clipboard(&mut self) -> Result<String, Fault>;
    fn write_clipboard(&mut self, text: &str) -> Result<(), Fault>;
    fn fingerprint(&mut self, app: &App) -> u64;
    fn is_secure_input(&mut self) -> bool;
    fn host_app(&mut self) -> Option<App>;
}

pub(crate) type Desktop = Worker<dyn Platform>;

impl Fault {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Stale(_) => "stale_ref",
            Self::Missing(_) => "not_found",
            Self::Unsupported(_) => "unsupported",
            Self::Denied(_) => "denied",
            Self::Failed(_) => "failed",
        }
    }

    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Stale(text)
            | Self::Missing(text)
            | Self::Unsupported(text)
            | Self::Denied(text)
            | Self::Failed(text) => text,
        }
    }
}

impl From<worker::Error> for Fault {
    fn from(err: worker::Error) -> Self {
        Self::Failed(err.to_string())
    }
}
