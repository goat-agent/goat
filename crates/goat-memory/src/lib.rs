use thiserror::Error;

pub mod embed;
pub mod engine;
pub mod facts;
pub mod files;
pub mod scope;
pub mod search;
pub mod vector;

pub use embed::Embedder;
pub use engine::MemoryEngine;
pub use facts::{Fact, FactOrigin, NewFact};
pub use files::{Chunk, FileError, MemoryFiles};
pub use scope::{Scope, ScopeError};
pub use search::{IndexChunk, Recall};

#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("sqlx: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("file: {0}")]
    File(String),
}

pub type MemoryResult<T> = Result<T, MemoryError>;
