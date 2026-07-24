mod help;

use goat_command::Command;

pub use help::Help;

pub fn all() -> Vec<Box<dyn Command>> {
    vec![Box::new(Help)]
}
