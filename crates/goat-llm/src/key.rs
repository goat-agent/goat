use std::time::Duration;

use crate::ProviderId;

pub trait KeyProvider: Send + Sync + 'static {
    fn next(&self, provider: ProviderId) -> Option<PooledKey>;
    fn report_429(&self, provider: ProviderId, api_key: &str, retry_after: Option<Duration>);
}

#[derive(Clone, Debug)]
pub struct PooledKey {
    pub api_key: String,
    pub label: Option<String>,
}
