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
    #[error("migrate: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("file: {0}")]
    File(String),
}

pub type MemoryResult<T> = Result<T, MemoryError>;

async fn run_migrations(pool: &sqlx::sqlite::SqlitePool) -> MemoryResult<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS _sqlx_migrations_memory (
             version BIGINT PRIMARY KEY,
             description TEXT NOT NULL,
             installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
             success BOOLEAN NOT NULL,
             checksum BLOB NOT NULL,
             execution_time BIGINT NOT NULL
         )",
    )
    .execute(pool)
    .await?;
    let legacy: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations')",
    )
    .fetch_one(pool)
    .await?;
    if legacy {
        sqlx::query(
            "INSERT OR IGNORE INTO _sqlx_migrations_memory
             SELECT * FROM _sqlx_migrations WHERE version = 11",
        )
        .execute(pool)
        .await?;
    }
    let mut migrator = sqlx::migrate!("./migrations");
    migrator.dangerous_set_table_name("_sqlx_migrations_memory");
    migrator.run(pool).await?;
    Ok(())
}
