use std::net::IpAddr;
use std::time::Duration;

use chromiumoxide::{Browser, BrowserConfig};
use futures::StreamExt as _;
use tokio::time::timeout;

use crate::error::WebFetchError;
use crate::fetch::RawFetch;
use crate::ssrf;

const LAUNCH_TIMEOUT: Duration = Duration::from_secs(30);
const NAV_TIMEOUT: Duration = Duration::from_secs(30);
const SETTLE_TIMEOUT: Duration = Duration::from_secs(3);

const LAUNCH_ARGS: [&str; 15] = [
    "disable-background-networking",
    "disable-background-timer-throttling",
    "disable-backgrounding-occluded-windows",
    "disable-breakpad",
    "disable-client-side-phishing-detection",
    "disable-default-apps",
    "disable-dev-shm-usage",
    "disable-hang-monitor",
    "disable-ipc-flooding-protection",
    "disable-popup-blocking",
    "disable-renderer-backgrounding",
    "disable-sync",
    "no-first-run",
    "no-default-browser-check",
    "disable-blink-features=AutomationControlled",
];

pub(crate) async fn render_to_raw(url: &str) -> Result<RawFetch, WebFetchError> {
    ssrf_precheck(url).await?;
    let html = render(url).await?;
    Ok(RawFetch {
        final_url: url.to_owned(),
        status: 200,
        content_type: Some("text/html; charset=utf-8".to_owned()),
        body: html.into_bytes(),
        overflowed: false,
    })
}

async fn render(url: &str) -> Result<String, WebFetchError> {
    let profile = tempfile::Builder::new()
        .prefix("goat-webfetch-")
        .tempdir()
        .map_err(|err| WebFetchError::Render(err.to_string()))?;
    let config = BrowserConfig::builder()
        .new_headless_mode()
        .user_data_dir(profile.path())
        .launch_timeout(LAUNCH_TIMEOUT)
        .request_timeout(NAV_TIMEOUT)
        .disable_default_args()
        .args(LAUNCH_ARGS)
        .build()
        .map_err(WebFetchError::Render)?;
    let (mut browser, mut handler) = Browser::launch(config)
        .await
        .map_err(|err| WebFetchError::Render(err.to_string()))?;
    let handler_task = tokio::spawn(async move { while handler.next().await.is_some() {} });
    let outcome = render_page(&browser, url).await;
    let _ = browser.close().await;
    handler_task.abort();
    outcome
}

async fn render_page(browser: &Browser, url: &str) -> Result<String, WebFetchError> {
    let page = timeout(NAV_TIMEOUT, browser.new_page(url))
        .await
        .map_err(|_| WebFetchError::Render("navigation timed out".to_owned()))?
        .map_err(|err| WebFetchError::Render(err.to_string()))?;
    let _ = timeout(SETTLE_TIMEOUT, page.wait_for_navigation()).await;
    let html = timeout(NAV_TIMEOUT, page.content())
        .await
        .map_err(|_| WebFetchError::Render("content read timed out".to_owned()))?
        .map_err(|err| WebFetchError::Render(err.to_string()))?;
    Ok(html)
}

async fn ssrf_precheck(url: &str) -> Result<(), WebFetchError> {
    let (host, port) = host_and_port(url);
    if let Ok(ip) = host.parse::<IpAddr>() {
        return if ssrf::is_blocked(ip) {
            Err(WebFetchError::Blocked(host))
        } else {
            Ok(())
        };
    }
    let resolved = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|err| WebFetchError::Render(format!("dns resolution failed: {err}")))?;
    let mut saw = false;
    for addr in resolved {
        saw = true;
        if ssrf::is_blocked(addr.ip()) {
            return Err(WebFetchError::Blocked(host.clone()));
        }
    }
    if saw {
        Ok(())
    } else {
        Err(WebFetchError::Render(format!("could not resolve {host}")))
    }
}

fn host_and_port(url: &str) -> (String, u16) {
    let authority = url
        .split_once("://")
        .map_or(url, |(_, rest)| rest)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("");
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, after)| after);
    if let Some(rest) = host_port.strip_prefix('[') {
        let host = rest.split(']').next().unwrap_or("").to_owned();
        (host, 443)
    } else {
        let mut parts = host_port.splitn(2, ':');
        let host = parts.next().unwrap_or("").to_owned();
        let port = parts
            .next()
            .and_then(|value| value.parse().ok())
            .unwrap_or(443);
        (host, port)
    }
}

#[cfg(test)]
mod tests {
    use super::host_and_port;

    #[test]
    fn parses_host_and_port() {
        assert_eq!(host_and_port("https://a.com/x"), ("a.com".to_owned(), 443));
        assert_eq!(
            host_and_port("https://a.com:8443/x?y=1"),
            ("a.com".to_owned(), 8443)
        );
        assert_eq!(host_and_port("https://[::1]/x"), ("::1".to_owned(), 443));
        assert_eq!(
            host_and_port("https://user:pw@host.dev/p"),
            ("host.dev".to_owned(), 443)
        );
    }

    #[tokio::test]
    async fn precheck_rejects_localhost() {
        assert!(super::ssrf_precheck("https://127.0.0.1/x").await.is_err());
    }
}
