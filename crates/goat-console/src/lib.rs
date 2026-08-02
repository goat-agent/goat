pub mod color;
pub mod error;
pub mod interact;
pub mod layout;
pub mod theme;

pub use color::{ColorMode, Palette, truncate_to_width};
pub use error::{
    ConsoleError, ConsoleResult, fail, fail_hint, format_failure, report, report_hint,
};
pub use interact::{
    confirm, note, pick, prompt, require_terminal, secret, select_index, select_indices, success,
    warning,
};
pub use layout::{
    Cell, Footer, Table, blank, cell, cell_async, dim, line, pair, pair_styled, raw, section,
};
