use thiserror::Error;

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("model must be `<provider>/<id>`, got `{0}`")]
    BadFormat(String),
    #[error("unknown provider `{0}`")]
    UnknownProvider(String),
}
