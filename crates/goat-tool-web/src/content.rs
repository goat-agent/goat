use crate::fetch::RawFetch;
use crate::readability;
use crate::render::human_size;

pub(crate) enum Kind {
    Html,
    Json,
    Text,
    Pdf,
    Binary,
}

pub(crate) struct Processed {
    pub text: String,
    pub title: Option<String>,
}

const EMPTY_SHELL_TEXT: usize = 200;

pub(crate) fn is_empty_html_shell(raw: &RawFetch) -> bool {
    if !matches!(classify(raw.content_type.as_deref(), &raw.body), Kind::Html) {
        return false;
    }
    let decoded = crate::decode::decode(raw);
    let markdown = htmd::convert(&decoded).unwrap_or_default();
    let visible: usize = markdown.split_whitespace().map(str::len).sum();
    visible < EMPTY_SHELL_TEXT
}

pub(crate) fn classify(content_type: Option<&str>, body: &[u8]) -> Kind {
    if body.starts_with(b"%PDF-") {
        return Kind::Pdf;
    }
    let essence = content_type
        .and_then(|value| value.split(';').next())
        .map(|value| value.trim().to_ascii_lowercase())
        .unwrap_or_default();
    if essence.contains("html") {
        Kind::Html
    } else if essence == "application/pdf" {
        Kind::Pdf
    } else if essence.contains("json") {
        Kind::Json
    } else if essence.starts_with("text/")
        || essence.contains("xml")
        || essence.contains("javascript")
        || essence.contains("markdown")
    {
        Kind::Text
    } else if essence.is_empty() {
        if looks_binary(body) {
            Kind::Binary
        } else {
            Kind::Text
        }
    } else {
        Kind::Binary
    }
}

pub(crate) fn process(kind: &Kind, decoded: String, raw: &RawFetch, raw_mode: bool) -> Processed {
    match kind {
        Kind::Html => {
            let extracted = if raw_mode {
                None
            } else {
                readability::extract(&decoded, &raw.final_url)
            };
            let (source_html, title) = if let Some(article) = extracted {
                let title = article.title.or_else(|| sniff_title(&decoded));
                (article.content_html, title)
            } else {
                let title = sniff_title(&decoded);
                (decoded, title)
            };
            let text = htmd::convert(&source_html).unwrap_or(source_html);
            Processed { text, title }
        }
        Kind::Json => {
            let text = serde_json::from_str::<serde_json::Value>(&decoded)
                .ok()
                .and_then(|value| serde_json::to_string_pretty(&value).ok())
                .unwrap_or(decoded);
            Processed { text, title: None }
        }
        Kind::Text => Processed {
            text: decoded,
            title: None,
        },
        Kind::Pdf => Processed {
            text: notice(raw, "a PDF document"),
            title: None,
        },
        Kind::Binary => Processed {
            text: notice(raw, "a non-text resource"),
            title: None,
        },
    }
}

pub(crate) fn pdf_processed(extracted: Option<String>, raw: &RawFetch) -> Processed {
    match extracted {
        Some(text) if !text.trim().is_empty() => Processed { text, title: None },
        _ => Processed {
            text: notice(raw, "a PDF document (no extractable text)"),
            title: None,
        },
    }
}

fn notice(raw: &RawFetch, what: &str) -> String {
    let content_type = raw.content_type.as_deref().unwrap_or("unknown");
    format!(
        "Fetched {what} (content-type: {content_type}, {}). WebFetch returns text; download the URL directly to use the file.",
        human_size(raw.body.len())
    )
}

fn looks_binary(body: &[u8]) -> bool {
    body.iter().take(1024).any(|byte| *byte == 0)
}

fn sniff_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let open = lower.find("<title")?;
    let gt = lower[open..].find('>')? + open + 1;
    let close = lower[gt..].find("</title>")? + gt;
    let title = html[gt..close]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if title.is_empty() { None } else { Some(title) }
}

#[cfg(test)]
mod tests {
    use super::{Kind, classify, process};
    use crate::fetch::RawFetch;

    fn raw(body: Vec<u8>, content_type: Option<&str>) -> RawFetch {
        RawFetch {
            final_url: "https://example.com/".to_owned(),
            status: 200,
            content_type: content_type.map(str::to_owned),
            body,
            overflowed: false,
        }
    }

    #[test]
    fn classifies_by_content_type() {
        assert!(matches!(
            classify(Some("text/html; charset=utf-8"), b""),
            Kind::Html
        ));
        assert!(matches!(
            classify(Some("application/json"), b"{}"),
            Kind::Json
        ));
        assert!(matches!(classify(Some("text/plain"), b"x"), Kind::Text));
        assert!(matches!(
            classify(Some("application/octet-stream"), b"\x89PNG"),
            Kind::Binary
        ));
    }

    #[test]
    fn detects_pdf_by_magic_bytes() {
        assert!(matches!(classify(None, b"%PDF-1.7 ..."), Kind::Pdf));
        assert!(matches!(classify(Some("application/pdf"), b""), Kind::Pdf));
    }

    #[test]
    fn pretty_prints_json() {
        let out = process(
            &Kind::Json,
            "{\"b\":1,\"a\":2}".to_owned(),
            &raw(vec![], Some("application/json")),
            false,
        );
        assert!(out.text.contains('\n'));
        assert!(out.text.contains("\"b\": 1"));
    }

    #[test]
    fn invalid_json_passes_through() {
        let out = process(
            &Kind::Json,
            "{not json".to_owned(),
            &raw(vec![], Some("application/json")),
            false,
        );
        assert_eq!(out.text, "{not json");
    }

    #[test]
    fn html_extracts_title_and_converts() {
        let html =
            "<html><head><title>Hello Title</title></head><body><p>Body text</p></body></html>";
        let out = process(
            &Kind::Html,
            html.to_owned(),
            &raw(vec![], Some("text/html")),
            false,
        );
        assert_eq!(out.title.as_deref(), Some("Hello Title"));
        assert!(out.text.contains("Body text"));
    }

    #[test]
    fn binary_returns_notice_not_bytes() {
        let out = process(
            &Kind::Binary,
            String::new(),
            &raw(vec![0, 1, 2, 3], Some("image/png")),
            false,
        );
        assert!(out.text.contains("non-text resource"));
        assert!(out.text.contains("image/png"));
    }
}
