use std::{
    net::IpAddr,
    sync::{Arc, OnceLock},
    time::Duration,
};

use futures::StreamExt;

use crate::error::WebFetchError;
use crate::ssrf;

const MAX_DOWNLOAD: usize = 5 * 1024 * 1024;
const USER_AGENT: &str = "goat-code/0.1 (+https://github.com/jbj338033/goat-code)";

pub(crate) struct RawFetch {
    pub final_url: String,
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
    pub overflowed: bool,
}

pub(crate) async fn fetch_raw(url: &str) -> Result<RawFetch, WebFetchError> {
    let client = shared_client()?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|err| WebFetchError::Request(err.to_string()))?;
    let status = response.status();
    let final_url = response.url().to_string();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    if !status.is_success() {
        return Err(WebFetchError::Status(status.as_u16()));
    }

    let mut stream = response.bytes_stream();
    let mut body: Vec<u8> = Vec::new();
    let mut overflowed = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| WebFetchError::Download(err.to_string()))?;
        if body.len() + chunk.len() > MAX_DOWNLOAD {
            let room = MAX_DOWNLOAD.saturating_sub(body.len());
            body.extend_from_slice(&chunk[..room]);
            overflowed = true;
            break;
        }
        body.extend_from_slice(&chunk);
    }

    Ok(RawFetch {
        final_url,
        status: status.as_u16(),
        content_type,
        body,
        overflowed,
    })
}

fn build_client() -> Result<reqwest::Client, WebFetchError> {
    let redirect = reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 10 {
            return attempt.error(RedirectBlocked("too many redirects"));
        }
        let blocked = attempt
            .url()
            .host_str()
            .map(|h| h.trim_start_matches('[').trim_end_matches(']').to_owned())
            .and_then(|h| h.parse::<IpAddr>().ok())
            .is_some_and(ssrf::is_blocked);
        if blocked {
            attempt.error(RedirectBlocked("redirect to a private or local address"))
        } else {
            attempt.follow()
        }
    });
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .redirect(redirect)
        .dns_resolver(Arc::new(ssrf::GuardedResolver))
        .build()
        .map_err(|err| WebFetchError::Client(err.to_string()))
}

#[derive(Debug)]
struct RedirectBlocked(&'static str);

impl std::fmt::Display for RedirectBlocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for RedirectBlocked {}

pub(crate) fn shared_client() -> Result<&'static reqwest::Client, WebFetchError> {
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| build_client().map_err(|err| err.to_string()))
        .as_ref()
        .map_err(|message| WebFetchError::Client(message.clone()))
}

#[cfg(test)]
mod tests {
    use super::shared_client;

    #[test]
    fn shared_client_uses_guarded_builder() {
        assert!(shared_client().is_ok());
    }
}
