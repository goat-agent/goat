mod exit;
mod plan;
mod reload_skills;

use goat_command::Command;

pub use exit::Exit;
pub use plan::{Plan, PlanScreen};
pub use reload_skills::ReloadSkills;

pub fn all() -> Vec<Box<dyn Command>> {
    vec![Box::new(Exit), Box::new(Plan), Box::new(ReloadSkills)]
}
