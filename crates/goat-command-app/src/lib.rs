mod exit;
mod plan;

use goat_command::Command;

pub use exit::Exit;
pub use plan::{Plan, PlanScreen};

pub fn all() -> Vec<Box<dyn Command>> {
    vec![Box::new(Exit), Box::new(Plan)]
}
