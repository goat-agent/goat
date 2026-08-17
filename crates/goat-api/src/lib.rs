mod client;
mod cursor;
mod methods;
mod registry;
mod resume;
mod router;

pub use client::{Api, Stream, StreamEvent};
pub use cursor::{Cursor, CursorError};
pub use methods::*;
pub use registry::{
    Direction, Grant, Method, MethodSchema, Shape, describe, registry, schema_document,
};
pub use resume::{Retained, WatchStart, cursor_for, decide as decide_watch_start};
pub use router::{RouteCtx, Router};

pub const EPOCH_UNKNOWN: &str = "e0";
