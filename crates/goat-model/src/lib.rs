mod error;
mod model;

pub use error::*;
pub use model::*;

pub use goat_provider::ProviderId;

pub fn canonicalize_provider_id(id: &str) -> &str {
    match id {
        "codex" => "openai-codex",
        "zhipu" => "zai",
        "moonshot" => "kimi",
        other => other,
    }
}
