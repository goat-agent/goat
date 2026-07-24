use std::fmt::Write as _;

use goat_tool::ToolOutput;

use crate::content::Processed;
use crate::fetch::RawFetch;

#[derive(Clone, Copy)]
pub(crate) struct Window {
    pub offset: usize,
    pub max_length: usize,
}

pub(crate) fn render(raw: &RawFetch, processed: Processed, window: Window) -> ToolOutput {
    let host = host_of(&raw.final_url);
    let size = human_size(raw.body.len());
    let content_type = raw
        .content_type
        .as_deref()
        .and_then(|value| value.split(';').next())
        .map_or("unknown", str::trim);
    let Processed { text, title } = processed;
    let title = title.unwrap_or_else(|| host.clone());

    let mut out = String::new();
    let _ = writeln!(out, "# {title}");
    let _ = writeln!(out, "url: {}", raw.final_url);
    let _ = writeln!(out, "status: {}", raw.status);
    let _ = writeln!(out, "content_type: {content_type}");
    let _ = writeln!(out, "size: {size}");
    out.push_str("source_trust: untrusted_web\n\n");

    let body = text.as_str();
    let total = body.len();
    let start = body.floor_char_boundary(window.offset.min(total));
    let end = body.floor_char_boundary(start.saturating_add(window.max_length).min(total));
    out.push_str(&body[start..end]);
    if end < total {
        let _ = write!(
            out,
            "\n\n[showing bytes {start}\u{2013}{end} of {total}; pass offset={end} to continue]"
        );
    } else if start > 0 {
        let _ = write!(out, "\n\n[showing bytes {start}\u{2013}{end} of {total}]");
    }
    if raw.overflowed {
        out.push_str(
            "\n\n[source exceeded the 5 MB download limit; later content was not fetched]",
        );
    }

    let summary = format!("{title} \u{2014} {host} ({size}, {content_type})");
    ToolOutput::text(out).with_summary(summary)
}

pub(crate) fn human_size(bytes: usize) -> String {
    const KIB: usize = 1024;
    const MIB: usize = 1024 * 1024;
    if bytes >= MIB {
        format!("{}.{} MB", bytes / MIB, (bytes % MIB) * 10 / MIB)
    } else if bytes >= KIB {
        format!("{}.{} KB", bytes / KIB, (bytes % KIB) * 10 / KIB)
    } else {
        format!("{bytes} B")
    }
}

fn host_of(url: &str) -> String {
    url.split_once("://")
        .map_or(url, |(_, rest)| rest)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(url)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::{Window, host_of, human_size, render};
    use crate::content::Processed;
    use crate::fetch::RawFetch;

    fn raw() -> RawFetch {
        RawFetch {
            final_url: "https://docs.example.com/page?q=1".to_owned(),
            status: 200,
            content_type: Some("text/html; charset=utf-8".to_owned()),
            body: vec![0u8; 2048],
            overflowed: false,
        }
    }

    #[test]
    fn header_carries_metadata_and_summary() {
        let processed = Processed {
            text: "hello world".to_owned(),
            title: Some("A Page".to_owned()),
        };
        let out = render(
            &raw(),
            processed,
            Window {
                offset: 0,
                max_length: 48 * 1024,
            },
        );
        let text = out.as_text().unwrap();
        assert!(text.contains("# A Page"));
        assert!(text.contains("url: https://docs.example.com/page?q=1"));
        assert!(text.contains("status: 200"));
        assert!(text.contains("source_trust: untrusted_web"));
        assert!(text.contains("hello world"));
        assert_eq!(
            out.summary.as_deref(),
            Some("A Page \u{2014} docs.example.com (2.0 KB, text/html)")
        );
    }

    #[test]
    fn paginates_with_continuation_marker() {
        let processed = Processed {
            text: "abcdefghij".to_owned(),
            title: None,
        };
        let out = render(
            &raw(),
            processed,
            Window {
                offset: 0,
                max_length: 4,
            },
        );
        let text = out.as_text().unwrap();
        assert!(text.contains("abcd"));
        assert!(!text.contains("efgh"));
        assert!(text.contains("pass offset=4 to continue"));
    }

    #[test]
    fn offset_past_end_is_clamped() {
        let processed = Processed {
            text: "short".to_owned(),
            title: None,
        };
        let out = render(
            &raw(),
            processed,
            Window {
                offset: 999,
                max_length: 48 * 1024,
            },
        );
        assert!(out.as_text().unwrap().contains("of 5"));
    }

    #[test]
    fn human_size_scales() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(2 * 1024 * 1024), "2.0 MB");
    }

    #[test]
    fn host_of_extracts_authority() {
        assert_eq!(host_of("https://a.b.com/x?y=1"), "a.b.com");
    }
}
