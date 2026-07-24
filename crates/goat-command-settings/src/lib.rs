mod config;
mod effort;
mod model;
mod provider;
mod search;
mod usage;

use goat_command::Command;

pub use config::Config;
pub use effort::Effort;
pub use model::Model;
pub use provider::Provider;
pub use search::Search;
pub use usage::Usage;

pub fn all() -> Vec<Box<dyn Command>> {
    vec![
        Box::new(Model),
        Box::new(Effort),
        Box::new(Config),
        Box::new(Provider),
        Box::new(Search),
        Box::new(Usage),
    ]
}
