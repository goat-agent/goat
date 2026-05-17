pub mod doctor;
pub mod persona;
pub mod provider;
pub mod setup;
pub mod skill;
pub mod ui;

use goat_llm::{SetupError, UserPrompt};

pub struct CliPrompt;

impl UserPrompt for CliPrompt {
    fn secret(&self, label: &str, _hint: &str) -> Result<String, SetupError> {
        ui::secret(label).map_err(|e| SetupError::Other(e.to_string()))
    }

    fn info(&self, message: &str) {
        ui::line(message);
    }
}
