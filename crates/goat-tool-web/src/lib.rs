mod browser;
mod content;
mod decode;
mod error;
mod extract;
mod fetch;
mod normalize;
mod pdf;
mod readability;
mod render;
mod ssrf;

use error::WebFetchError;

const DEFAULT_QUERY_PASSAGES: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
enum RenderMode {
    Auto,
    Always,
    Never,
}

fn parse_render(mode: Option<&str>) -> RenderMode {
    match mode {
        Some("always") => RenderMode::Always,
        Some("never") => RenderMode::Never,
        _ => RenderMode::Auto,
    }
}

async fn obtain(url: &str, mode: RenderMode) -> Result<fetch::RawFetch, WebFetchError> {
    match mode {
        RenderMode::Never => fetch::fetch_raw(url).await,
        RenderMode::Always => browser::render_to_raw(url).await,
        RenderMode::Auto => {
            let raw = fetch::fetch_raw(url).await?;
            if content::is_empty_html_shell(&raw) {
                match browser::render_to_raw(url).await {
                    Ok(rendered) if !content::is_empty_html_shell(&rendered) => Ok(rendered),
                    _ => Ok(raw),
                }
            } else {
                Ok(raw)
            }
        }
    }
}

use goat_protocol::ToolDisplay;
use goat_tool::{Tool, ToolContext, ToolFuture, display};
use serde::Deserialize;

pub fn all() -> Vec<Box<dyn Tool>> {
    vec![Box::new(WebFetchTool::new())]
}

pub struct WebFetchTool {
    readability: bool,
    render_enabled: bool,
    max_length: usize,
}

impl WebFetchTool {
    pub fn new() -> Self {
        let config = goat_config::Config::load();
        Self {
            readability: config.web_fetch.readability,
            render_enabled: config.web_fetch.render_enabled,
            max_length: config.web_fetch.max_length.max(1024),
        }
    }
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct Input {
    url: String,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    raw: bool,
    #[serde(default)]
    render: Option<String>,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    max_length: Option<usize>,
}

impl Tool for WebFetchTool {
    fn name(&self) -> &'static str {
        "WebFetch"
    }

    fn description(&self) -> &'static str {
        "Fetch a URL over HTTPS and return its content as Markdown. Detects the page charset, pretty-prints JSON, refuses binary blobs with a typed notice, and prefixes page metadata (title, final URL, status, size). Large pages are paged with offset/max_length. Private and link-local addresses are refused."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "http(s) URL. http is upgraded to https; github blob URLs are rewritten to raw."},
                "query": {"type": "string", "description": "If set, return only the passages most relevant to this query (with heading context) instead of the whole page."},
                "raw": {"type": "boolean", "description": "If true, skip main-content extraction and return the full converted document. Default false."},
                "render": {"type": "string", "enum": ["auto", "always", "never"], "description": "Headless-browser rendering for JavaScript pages. auto renders only when the static fetch looks like an empty shell. Default auto."},
                "offset": {"type": "integer", "description": "Byte offset into the processed text to start from. Default 0."},
                "max_length": {"type": "integer", "description": "Max bytes of processed text to return. Default 49152, capped at 49152."}
            },
            "required": ["url"]
        })
    }

    fn display_input(&self, input: &str) -> ToolDisplay {
        match serde_json::from_str::<Input>(input) {
            Ok(args) => ToolDisplay::primary(display::call_sig(
                "WebFetch",
                &[display::flatten(&args.url).as_str()],
            )),
            Err(_) => display::generic(input),
        }
    }

    fn run<'a>(&'a self, input: &'a str, _ctx: &'a ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let args: Input = serde_json::from_str(input)?;
            let url = normalize::normalize_url(&args.url);
            normalize::reject_blocked_literal(&url)?;
            let mode = if self.render_enabled {
                parse_render(args.render.as_deref())
            } else {
                RenderMode::Never
            };
            let raw = obtain(&url, mode).await?;
            let kind = content::classify(raw.content_type.as_deref(), &raw.body);
            let raw_mode = args.raw || !self.readability;
            let mut processed = if matches!(kind, content::Kind::Pdf) {
                let body = raw.body.clone();
                let extracted = tokio::task::spawn_blocking(move || pdf::extract_text(body))
                    .await
                    .ok()
                    .flatten();
                content::pdf_processed(extracted, &raw)
            } else {
                let decoded = decode::decode(&raw);
                content::process(&kind, decoded, &raw, raw_mode)
            };
            if let Some(query) = args
                .query
                .as_deref()
                .map(str::trim)
                .filter(|q| !q.is_empty())
            {
                processed.text = match extract::extract_relevant(
                    &processed.text,
                    query,
                    DEFAULT_QUERY_PASSAGES,
                ) {
                    Some(relevant) => {
                        format!("relevant passages for query: {query}\n\n{relevant}")
                    }
                    None => format!(
                        "no strongly matching sections for query: {query}; showing the document start\n\n{}",
                        processed.text
                    ),
                };
            }
            let window = render::Window {
                offset: args.offset.unwrap_or(0),
                max_length: args
                    .max_length
                    .map_or(self.max_length, |value| value.min(self.max_length)),
            };
            Ok(render::render(&raw, processed, window))
        })
    }
}

#[cfg(test)]
mod tests {
    use goat_tool::Tool;

    #[tokio::test]
    #[ignore = "requires network access"]
    async fn live_fetch_example() {
        let ctx = goat_tool::ToolContext::new(&std::env::temp_dir()).unwrap();
        let out = super::WebFetchTool::new()
            .run(r#"{"url":"https://example.com"}"#, &ctx)
            .await
            .unwrap();
        assert!(out.as_text().unwrap().contains("Example Domain"));
    }
}
