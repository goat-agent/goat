pub mod marker {
    pub const USER: &str = "❯ ";
    pub const AGENT: &str = "● ";
    pub const NOTICE: &str = "→ ";
    pub const ERROR: &str = "✗ ";
    pub const PROMPT: &str = "❯ ";
    pub const SHELL: &str = "! ";
}

pub mod ui {
    pub const CARET: &str = "›";
    pub const BANG: &str = "!";
    pub const BULLET: &str = "- ";
    pub const DOT_FULL: &str = "●";
    pub const DOT_EMPTY: &str = "○";
    pub const CHECK: &str = "✓";
    pub const CROSS: &str = "✗";
    pub const MIDDOT: &str = "·";
    pub const SEPARATOR: &str = " · ";
    pub const ELLIPSIS: &str = "…";
    pub const QUOTE_GUTTER: &str = "▎ ";
    pub const CHEVRON_RIGHT: &str = "▸";
    pub const CHEVRON_DOWN: &str = "▾";
    pub const RULE: &str = "──────────";
    pub const HRULE: &str = "\u{1}hrule\u{1}";
    pub const MORE_ABOVE: &str = "↑";
    pub const MORE_BELOW: &str = "↓";
    pub const STREAM_CURSOR: &str = "▌";
    pub const MASK: &str = "•";
    pub const BAR_FULL: &str = "█";
    pub const BAR_EMPTY: &str = "░";
}

pub mod key {
    pub const CTRL: &str = "⌃";
    pub const ESC: &str = "esc";
    pub const SHIFT: &str = "⇧";
    pub const ENTER: &str = "↵";
    pub const TAB: &str = "⇥";
    pub const BACKSPACE: &str = "⌫";
    pub const ARROW_UP: &str = "↑";
    pub const ARROW_DOWN: &str = "↓";
    pub const ARROW_LEFT: &str = "←";
    pub const ARROW_RIGHT: &str = "→";
    pub const SHIFT_ENTER: &str = "⇧↵";
    pub const ARROWS_UPDOWN: &str = "↑↓";
    pub const ARROWS_LEFTRIGHT: &str = "←→";
}

pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
