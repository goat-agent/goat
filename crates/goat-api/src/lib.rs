mod cursor;
mod methods;
mod registry;
mod resume;

pub use cursor::{Cursor, CursorError};
pub use methods::*;
pub use registry::{Direction, Grant, Method, MethodSchema, Shape, describe, registry};
pub use resume::{Retained, WatchStart, cursor_for, decide as decide_watch_start};

pub const EPOCH_UNKNOWN: &str = "e0";
