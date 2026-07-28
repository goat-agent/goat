mod backfill;
mod http;
mod meter;
mod web;

pub use backfill::backfill_rate_limits;
pub use http::{AccountOps, ProviderMeta, serve};
pub use meter::{Meter, MeteredProvider, ProxyEvent, Recorder, SOURCE_AGENT, SOURCE_CODE};

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("store: {0}")]
    Store(#[from] goat_store::ProxyStoreError),
}
