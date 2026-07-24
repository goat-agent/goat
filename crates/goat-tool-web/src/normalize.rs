use std::net::IpAddr;

use crate::error::WebFetchError;
use crate::ssrf;

pub(crate) fn normalize_url(raw: &str) -> String {
    let upgraded = raw
        .strip_prefix("http://")
        .map_or_else(|| raw.to_owned(), |rest| format!("https://{rest}"));
    github_blob_to_raw(&upgraded)
}

fn github_blob_to_raw(url: &str) -> String {
    let Some(rest) = url.strip_prefix("https://github.com/") else {
        return url.to_owned();
    };
    let parts: Vec<&str> = rest.splitn(4, '/').collect();
    if parts.len() == 4 && parts[2] == "blob" {
        format!(
            "https://raw.githubusercontent.com/{}/{}/{}",
            parts[0], parts[1], parts[3]
        )
    } else {
        url.to_owned()
    }
}

pub(crate) fn reject_blocked_literal(url: &str) -> Result<(), WebFetchError> {
    let authority = url
        .split_once("://")
        .map_or(url, |(_, rest)| rest)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("");
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, after)| after);
    let host = if let Some(rest) = host_port.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        host_port.split(':').next().unwrap_or("")
    };
    if let Ok(ip) = host.parse::<IpAddr>()
        && ssrf::is_blocked(ip)
    {
        return Err(WebFetchError::Blocked(host.to_owned()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{github_blob_to_raw, normalize_url, reject_blocked_literal};

    #[test]
    fn upgrades_http_to_https() {
        assert_eq!(
            normalize_url("http://example.com/x"),
            "https://example.com/x"
        );
    }

    #[test]
    fn rewrites_github_blob() {
        assert_eq!(
            github_blob_to_raw("https://github.com/o/r/blob/main/src/lib.rs"),
            "https://raw.githubusercontent.com/o/r/main/src/lib.rs"
        );
    }

    #[test]
    fn leaves_other_github_urls() {
        assert_eq!(
            github_blob_to_raw("https://github.com/o/r/issues/1"),
            "https://github.com/o/r/issues/1"
        );
    }

    #[test]
    fn rejects_ip_literal_localhost() {
        assert!(reject_blocked_literal("https://127.0.0.1/secret").is_err());
        assert!(reject_blocked_literal("https://[::1]/secret").is_err());
        assert!(reject_blocked_literal("https://169.254.169.254/latest/meta-data").is_err());
    }

    #[test]
    fn allows_public_literal() {
        assert!(reject_blocked_literal("https://8.8.8.8/").is_ok());
    }
}
