mod action;
mod backend;
mod error;
mod tool;

pub use backend::{ComputerBackend, DesktopBackend, Image};
pub use error::ComputerError;
pub use tool::ComputerTool;

use std::sync::Arc;

pub fn desktop_tool() -> Result<ComputerTool, ComputerError> {
    let backend = DesktopBackend::new()?;
    Ok(ComputerTool::new(Arc::new(backend)))
}
