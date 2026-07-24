mod exit;

use goat_command::Command;

pub use exit::Exit;

pub fn all() -> Vec<Box<dyn Command>> {
    vec![Box::new(Exit)]
}
