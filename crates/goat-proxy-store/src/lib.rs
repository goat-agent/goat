use std::path::Path;
use std::time::Duration;

use sqlx::ConnectOptions;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

const READER_POOL_MAX: u32 = 4;

#[derive(Debug, thiserror::Error)]
pub enum ProxyStoreError {
    #[error("sqlx: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("migrate: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type ProxyResult<T> = Result<T, ProxyStoreError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRequest {
    pub ts: i64,
    pub source: String,
    pub provider: String,
    pub account: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub duration_ms: i64,
    pub status: String,
    pub error_kind: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct RequestRow {
    pub id: i64,
    pub ts: i64,
    pub source: String,
    pub provider: String,
    pub account: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub duration_ms: i64,
    pub status: String,
    pub error_kind: Option<String>,
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct UsageBucket {
    pub key: String,
    pub requests: i64,
    pub errors: i64,
    pub cancelled: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub avg_duration_ms: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct RateLimitRow {
    pub provider: String,
    pub account: String,
    pub snapshot: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, sqlx::FromRow)]
pub struct Totals {
    pub requests: i64,
    pub errors: i64,
    pub cancelled: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub avg_duration_ms: f64,
}

#[derive(Clone)]
pub struct ProxyStore {
    writer: SqlitePool,
    readers: SqlitePool,
}

fn connect_opts(path: &Path) -> ProxyResult<SqliteConnectOptions> {
    let opts = format!("sqlite://{}", path.display())
        .parse::<SqliteConnectOptions>()?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5))
        .disable_statement_logging();
    Ok(opts)
}

async fn run_migrations(pool: &SqlitePool) -> ProxyResult<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS _sqlx_migrations_proxy (
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
            "INSERT OR IGNORE INTO _sqlx_migrations_proxy
             SELECT * FROM _sqlx_migrations WHERE version = 19",
        )
        .execute(pool)
        .await?;
    }
    let mut migrator = sqlx::migrate!("./migrations");
    migrator.dangerous_set_table_name("_sqlx_migrations_proxy");
    migrator.run(pool).await?;
    Ok(())
}

impl ProxyStore {
    pub async fn open(path: &Path) -> ProxyResult<Self> {
        goat_sqlite_vec::register();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let writer = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(connect_opts(path)?)
            .await?;
        run_migrations(&writer).await?;
        let readers = SqlitePoolOptions::new()
            .max_connections(READER_POOL_MAX)
            .connect_with(connect_opts(path)?.read_only(true))
            .await?;
        Ok(Self { writer, readers })
    }

    pub async fn open_in_memory() -> ProxyResult<Self> {
        goat_sqlite_vec::register();
        let opts = "sqlite::memory:"
            .parse::<SqliteConnectOptions>()?
            .disable_statement_logging();
        let writer = SqlitePoolOptions::new()
            .max_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .acquire_timeout(Duration::from_hours(1))
            .test_before_acquire(false)
            .connect_with(opts)
            .await?;
        run_migrations(&writer).await?;
        let readers = writer.clone();
        Ok(Self { writer, readers })
    }

    pub async fn insert_request(&self, request: NewRequest) -> ProxyResult<i64> {
        let id = sqlx::query(
            "INSERT INTO proxy_requests
             (ts, source, provider, account, model, input_tokens, output_tokens,
              cache_read_tokens, cache_write_tokens, duration_ms, status, error_kind)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(request.ts)
        .bind(request.source)
        .bind(request.provider)
        .bind(request.account)
        .bind(request.model)
        .bind(request.input_tokens)
        .bind(request.output_tokens)
        .bind(request.cache_read_tokens)
        .bind(request.cache_write_tokens)
        .bind(request.duration_ms)
        .bind(request.status)
        .bind(request.error_kind)
        .execute(&self.writer)
        .await?
        .last_insert_rowid();
        Ok(id)
    }

    pub async fn upsert_rate_limits(
        &self,
        provider: &str,
        account: &str,
        snapshot: &str,
        updated_at: i64,
    ) -> ProxyResult<()> {
        sqlx::query(
            "INSERT INTO proxy_rate_limits (provider, account, snapshot, updated_at)
             VALUES (?, ?, ?, ?)
             ON CONFLICT (provider, account)
             DO UPDATE SET snapshot = excluded.snapshot, updated_at = excluded.updated_at",
        )
        .bind(provider)
        .bind(account)
        .bind(snapshot)
        .bind(updated_at)
        .execute(&self.writer)
        .await?;
        Ok(())
    }

    pub async fn upsert_rate_limits_if_newer(
        &self,
        provider: &str,
        account: &str,
        snapshot: &str,
        updated_at: i64,
    ) -> ProxyResult<bool> {
        let result = sqlx::query(
            "INSERT INTO proxy_rate_limits (provider, account, snapshot, updated_at)
             VALUES (?, ?, ?, ?)
             ON CONFLICT (provider, account)
             DO UPDATE SET snapshot = excluded.snapshot, updated_at = excluded.updated_at
             WHERE excluded.updated_at > proxy_rate_limits.updated_at",
        )
        .bind(provider)
        .bind(account)
        .bind(snapshot)
        .bind(updated_at)
        .execute(&self.writer)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn latest_rate_limits(&self) -> ProxyResult<Vec<RateLimitRow>> {
        let rows = sqlx::query_as::<_, RateLimitRow>(
            "SELECT provider, account, snapshot, updated_at
             FROM proxy_rate_limits ORDER BY provider, account",
        )
        .fetch_all(&self.readers)
        .await?;
        Ok(rows)
    }

    pub async fn usage_by_day(&self, since_ms: i64) -> ProxyResult<Vec<UsageBucket>> {
        let rows = sqlx::query_as::<_, UsageBucket>(
            "SELECT date(ts / 1000, 'unixepoch') AS key,
                    COUNT(*) AS requests,
                    SUM(status = 'error') AS errors,
                    SUM(status = 'cancelled') AS cancelled,
                    SUM(input_tokens) AS input_tokens,
                    SUM(output_tokens) AS output_tokens,
                    SUM(cache_read_tokens) AS cache_read_tokens,
                    SUM(cache_write_tokens) AS cache_write_tokens,
                    AVG(duration_ms) AS avg_duration_ms
             FROM proxy_requests WHERE ts >= ?
             GROUP BY key ORDER BY key",
        )
        .bind(since_ms)
        .fetch_all(&self.readers)
        .await?;
        Ok(rows)
    }

    pub async fn usage_by_hour(&self, since_ms: i64) -> ProxyResult<Vec<UsageBucket>> {
        let rows = sqlx::query_as::<_, UsageBucket>(
            "SELECT strftime('%Y-%m-%d %H:00', ts / 1000, 'unixepoch') AS key,
                    COUNT(*) AS requests,
                    SUM(status = 'error') AS errors,
                    SUM(status = 'cancelled') AS cancelled,
                    SUM(input_tokens) AS input_tokens,
                    SUM(output_tokens) AS output_tokens,
                    SUM(cache_read_tokens) AS cache_read_tokens,
                    SUM(cache_write_tokens) AS cache_write_tokens,
                    AVG(duration_ms) AS avg_duration_ms
             FROM proxy_requests WHERE ts >= ?
             GROUP BY key ORDER BY key",
        )
        .bind(since_ms)
        .fetch_all(&self.readers)
        .await?;
        Ok(rows)
    }

    pub async fn usage_by_provider(&self, since_ms: i64) -> ProxyResult<Vec<UsageBucket>> {
        let rows = sqlx::query_as::<_, UsageBucket>(
            "SELECT provider AS key,
                    COUNT(*) AS requests,
                    SUM(status = 'error') AS errors,
                    SUM(status = 'cancelled') AS cancelled,
                    SUM(input_tokens) AS input_tokens,
                    SUM(output_tokens) AS output_tokens,
                    SUM(cache_read_tokens) AS cache_read_tokens,
                    SUM(cache_write_tokens) AS cache_write_tokens,
                    AVG(duration_ms) AS avg_duration_ms
             FROM proxy_requests WHERE ts >= ?
             GROUP BY key ORDER BY SUM(input_tokens) + SUM(output_tokens) DESC",
        )
        .bind(since_ms)
        .fetch_all(&self.readers)
        .await?;
        Ok(rows)
    }

    pub async fn usage_by_model(&self, since_ms: i64) -> ProxyResult<Vec<UsageBucket>> {
        let rows = sqlx::query_as::<_, UsageBucket>(
            "SELECT provider || '/' || model AS key,
                    COUNT(*) AS requests,
                    SUM(status = 'error') AS errors,
                    SUM(status = 'cancelled') AS cancelled,
                    SUM(input_tokens) AS input_tokens,
                    SUM(output_tokens) AS output_tokens,
                    SUM(cache_read_tokens) AS cache_read_tokens,
                    SUM(cache_write_tokens) AS cache_write_tokens,
                    AVG(duration_ms) AS avg_duration_ms
             FROM proxy_requests WHERE ts >= ?
             GROUP BY provider, model ORDER BY SUM(input_tokens) + SUM(output_tokens) DESC",
        )
        .bind(since_ms)
        .fetch_all(&self.readers)
        .await?;
        Ok(rows)
    }

    pub async fn totals_since(&self, since_ms: i64) -> ProxyResult<Totals> {
        let row = sqlx::query_as::<_, Totals>(
            "SELECT COUNT(*) AS requests,
                    COALESCE(SUM(status = 'error'), 0) AS errors,
                    COALESCE(SUM(status = 'cancelled'), 0) AS cancelled,
                    COALESCE(SUM(input_tokens), 0) AS input_tokens,
                    COALESCE(SUM(output_tokens), 0) AS output_tokens,
                    COALESCE(SUM(cache_read_tokens), 0) AS cache_read_tokens,
                    COALESCE(SUM(cache_write_tokens), 0) AS cache_write_tokens,
                    COALESCE(AVG(duration_ms), 0.0) AS avg_duration_ms
             FROM proxy_requests WHERE ts >= ?",
        )
        .bind(since_ms)
        .fetch_one(&self.readers)
        .await?;
        Ok(row)
    }

    pub async fn recent_requests(
        &self,
        limit: i64,
        offset: i64,
        provider: Option<&str>,
        status: Option<&str>,
        source: Option<&str>,
    ) -> ProxyResult<Vec<RequestRow>> {
        let rows = sqlx::query_as::<_, RequestRow>(
            "SELECT id, ts, source, provider, account, model, input_tokens, output_tokens,
                    cache_read_tokens, cache_write_tokens, duration_ms, status, error_kind
             FROM proxy_requests
             WHERE (? IS NULL OR provider = ?) AND (? IS NULL OR status = ?)
               AND (? IS NULL OR source = ?)
             ORDER BY id DESC LIMIT ? OFFSET ?",
        )
        .bind(provider)
        .bind(provider)
        .bind(status)
        .bind(status)
        .bind(source)
        .bind(source)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.readers)
        .await?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::{NewRequest, ProxyStore};

    fn sample(ts: i64, provider: &str, model: &str) -> NewRequest {
        NewRequest {
            ts,
            source: "code".into(),
            provider: provider.into(),
            account: "default".into(),
            model: model.into(),
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: 10,
            cache_write_tokens: 5,
            duration_ms: 1200,
            status: "ok".into(),
            error_kind: None,
        }
    }

    #[tokio::test]
    async fn request_round_trips_and_aggregates() {
        let store = ProxyStore::open_in_memory().await.unwrap();
        store
            .insert_request(sample(1_700_000_000_000, "openai", "gpt-5"))
            .await
            .unwrap();
        store
            .insert_request(sample(1_700_000_060_000, "openai", "gpt-5"))
            .await
            .unwrap();
        store
            .insert_request(sample(1_700_000_120_000, "kimi", "k2"))
            .await
            .unwrap();

        let recent = store
            .recent_requests(10, 0, None, None, None)
            .await
            .unwrap();
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].provider, "kimi");

        let filtered = store
            .recent_requests(10, 0, Some("openai"), None, None)
            .await
            .unwrap();
        assert_eq!(filtered.len(), 2);

        let totals = store.totals_since(0).await.unwrap();
        assert_eq!(totals.requests, 3);
        assert_eq!(totals.input_tokens, 300);
        assert_eq!(totals.output_tokens, 150);
        assert_eq!(totals.errors, 0);
        assert_eq!(totals.cancelled, 0);
        assert!((totals.avg_duration_ms - 1200.0).abs() < f64::EPSILON);

        let by_provider = store.usage_by_provider(0).await.unwrap();
        assert_eq!(by_provider.len(), 2);
        assert_eq!(by_provider[0].key, "openai");
        assert_eq!(by_provider[0].requests, 2);

        let by_day = store.usage_by_day(0).await.unwrap();
        assert_eq!(by_day.len(), 1);
        assert_eq!(by_day[0].requests, 3);
        assert_eq!(by_day[0].errors, 0);
        assert!((by_day[0].avg_duration_ms - 1200.0).abs() < f64::EPSILON);

        let by_hour = store.usage_by_hour(0).await.unwrap();
        assert_eq!(by_hour.len(), 1);
        assert!(by_hour[0].key.ends_with(":00"));
        assert_eq!(by_hour[0].requests, 3);

        let none = store.totals_since(i64::MAX).await.unwrap();
        assert_eq!(none.requests, 0);
        assert_eq!(none.input_tokens, 0);
        assert!((none.avg_duration_ms - 0.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn rate_limits_upsert_keeps_latest() {
        let store = ProxyStore::open_in_memory().await.unwrap();
        store
            .upsert_rate_limits("openai", "default", "{\"a\":1}", 100)
            .await
            .unwrap();
        store
            .upsert_rate_limits("openai", "default", "{\"a\":2}", 200)
            .await
            .unwrap();
        store
            .upsert_rate_limits("kimi", "work", "{\"b\":1}", 150)
            .await
            .unwrap();

        let rows = store.latest_rate_limits().await.unwrap();
        assert_eq!(rows.len(), 2);
        let openai = rows.iter().find(|r| r.provider == "openai").unwrap();
        assert_eq!(openai.snapshot, "{\"a\":2}");
        assert_eq!(openai.updated_at, 200);
    }

    #[tokio::test]
    async fn opening_proxy_store_creates_only_proxy_schema() {
        let store = ProxyStore::open_in_memory().await.unwrap();
        let tables: Vec<String> =
            sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
                .fetch_all(&store.writer)
                .await
                .unwrap();
        assert!(tables.iter().any(|table| table == "proxy_requests"));
        assert!(tables.iter().any(|table| table == "proxy_rate_limits"));
        assert!(!tables.iter().any(|table| table == "scheduled_tasks"));
        assert!(!tables.iter().any(|table| table == "goals"));
        assert!(!tables.iter().any(|table| table == "facts"));
        assert!(!tables.iter().any(|table| table == "code_threads"));
    }

    #[tokio::test]
    async fn status_filter_matches_errors() {
        let store = ProxyStore::open_in_memory().await.unwrap();
        let mut failed = sample(1_700_000_000_000, "openai", "gpt-5");
        failed.status = "error".into();
        failed.error_kind = Some("rate_limited".into());
        store.insert_request(failed).await.unwrap();
        store
            .insert_request(sample(1_700_000_060_000, "openai", "gpt-5"))
            .await
            .unwrap();

        let errors = store
            .recent_requests(10, 0, None, Some("error"), None)
            .await
            .unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].error_kind.as_deref(), Some("rate_limited"));
    }
}
