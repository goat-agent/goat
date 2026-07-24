use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

pub fn init() -> Option<WorkerGuard> {
    let dir = goat_config::log_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let appender = tracing_appender::rolling::daily(&dir, "goat.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_from_env("GOAT_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_ansi(false).with_writer(writer))
        .init();
    Some(guard)
}
