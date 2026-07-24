mod gutter;
mod item;
mod render;
mod tool_gist;
mod tool_line;

use std::cell::RefCell;

use goat_protocol::{InputAttachment, TaskId, ToolCall, ToolCallId, ToolOutcome};
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::{highlight::Highlighter, markdown, symbols, theme::Theme};

use gutter::hang;
pub(crate) use item::{Item, ShellStatus, ToolStatus, UserMessage, Working};
use render::{build_static_lines, is_blank, queued_rows, stable_prefix_len, working_rows};

pub(crate) struct RenderCtx<'a> {
    pub theme: Theme,
    pub scroll: usize,
    pub left_pad: u16,
    pub cwd: &'a str,
    pub spinner: &'static str,
    pub working: Option<&'a Working>,
    pub queued: &'a [String],
    pub hl: &'a dyn Highlighter,
    pub picker: Option<&'a ratatui_image::picker::Picker>,
}

pub(super) struct ImagePlacement {
    pub(super) item: usize,
    pub(super) start: usize,
    pub(super) rows: u16,
}

struct RenderCache {
    width: u16,
    version: u64,
    lines: Vec<Line<'static>>,
    spinner_lines: Vec<usize>,
    images: Vec<ImagePlacement>,
}

struct StreamCache {
    prefix_len: usize,
    width: u16,
    prefix_content: Vec<ratatui::text::Line<'static>>,
}

#[derive(Default)]
pub struct Transcript {
    pub(crate) items: Vec<Item>,
    streaming: Option<String>,
    thinking_buffer: Option<String>,
    version: u64,
    cache: RefCell<Option<RenderCache>>,
    stream_cache: RefCell<Option<StreamCache>>,
    tail_cache: RefCell<Vec<Line<'static>>>,
    item_memo: RefCell<ItemMemoCache>,
}

#[derive(Default)]
struct ItemMemoCache {
    width: u16,
    entries: Vec<render::ItemMemo>,
}

const PROCESS_LOG_CAP_BYTES: usize = 128 * 1024;

fn trim_log_front(output: &mut String, cap: usize) {
    if output.len() <= cap {
        return;
    }
    let overflow = output.len() - cap;
    let cut = output[overflow..]
        .find('\n')
        .map_or(output.len(), |i| overflow + i + 1);
    output.drain(..cut);
}

fn pad_left(mut line: Line<'static>, width: u16, theme: Theme) -> Line<'static> {
    if width == 0 {
        return line;
    }
    let style = line
        .spans
        .first()
        .filter(|span| span.style.bg == theme.user_panel().bg)
        .map_or_else(|| theme.base(), |span| span.style);
    line.spans
        .insert(0, Span::styled(" ".repeat(usize::from(width)), style));
    line
}

impl Transcript {
    fn bump_version(&mut self) {
        self.version = self.version.wrapping_add(1);
        *self.cache.borrow_mut() = None;
        *self.stream_cache.borrow_mut() = None;
    }

    pub fn invalidate(&mut self) {
        self.bump_version();
        self.item_memo.borrow_mut().entries.clear();
    }

    pub fn clear(&mut self) {
        self.bump_version();
        self.item_memo.borrow_mut().entries.clear();
        self.items.clear();
        self.streaming = None;
    }

    #[cfg(test)]
    pub(crate) fn push_user(&mut self, text: impl Into<String>) {
        self.push_user_with_attachments(text, Vec::new());
    }

    pub fn push_user_with_attachments(
        &mut self,
        text: impl Into<String>,
        attachments: Vec<InputAttachment>,
    ) {
        self.push_user_with_display(text, attachments);
    }

    pub fn push_user_with_display(
        &mut self,
        text: impl Into<String>,
        attachments: Vec<InputAttachment>,
    ) {
        self.bump_version();
        self.items.push(Item::User(UserMessage {
            text: text.into(),
            attachments,
        }));
    }

    pub fn push_thinking_delta(&mut self, chunk: &str) {
        self.thinking_buffer
            .get_or_insert_with(String::new)
            .push_str(chunk);
    }

    pub fn flush_thinking(&mut self) {
        let Some(buffer) = self.thinking_buffer.take() else {
            return;
        };
        if buffer.trim().is_empty() {
            return;
        }
        self.bump_version();
        self.items.push(Item::Thinking {
            text: buffer,
            collapsed: true,
        });
    }

    pub fn push_thinking(&mut self, text: String) {
        if text.trim().is_empty() {
            return;
        }
        self.bump_version();
        self.items.push(Item::Thinking {
            text,
            collapsed: true,
        });
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn static_len(&self) -> usize {
        self.cache.borrow().as_ref().map_or(0, |c| c.lines.len())
    }

    pub fn selectable_len(&self) -> usize {
        self.static_len() + self.tail_cache.borrow().len()
    }

    pub fn selected_text(&self, anchor: (usize, u16), focus: (usize, u16)) -> String {
        let cache = self.cache.borrow();
        let tail = self.tail_cache.borrow();
        let head = cache.as_ref().map_or(&[][..], |c| c.lines.as_slice());
        crate::select::extract_split(head, &tail, anchor, focus)
    }

    pub fn word_bounds_at(&self, line: usize, col: u16) -> Option<(u16, u16)> {
        let cache = self.cache.borrow();
        let base = cache.as_ref().map_or(0, |c| c.lines.len());
        if line < base {
            return crate::select::word_bounds(cache.as_ref()?.lines.get(line)?, col);
        }
        drop(cache);
        let tail = self.tail_cache.borrow();
        crate::select::word_bounds(tail.get(line - base)?, col)
    }

    pub fn url_at(&self, line: usize, col: u16) -> Option<String> {
        let cache = self.cache.borrow();
        let base = cache.as_ref().map_or(0, |c| c.lines.len());
        if line < base {
            return crate::select::url_at(cache.as_ref()?.lines.get(line)?, col);
        }
        drop(cache);
        let tail = self.tail_cache.borrow();
        crate::select::url_at(tail.get(line - base)?, col)
    }

    pub fn image_at(&self, line: usize) -> Option<goat_protocol::ToolImageData> {
        let guard = self.cache.borrow();
        let cache = guard.as_ref()?;
        let placement = cache
            .images
            .iter()
            .find(|p| line >= p.start && line < p.start + usize::from(p.rows))?;
        match self.items.get(placement.item)? {
            Item::Tool {
                image: Some(img), ..
            } => Some(img.source()),
            _ => None,
        }
    }

    pub fn toggle_thinking(&mut self) -> bool {
        let mut any = false;
        let mut expand = false;
        for item in &self.items {
            if let Item::Thinking { collapsed, .. } = item {
                any = true;
                if *collapsed {
                    expand = true;
                    break;
                }
            }
        }
        if !any {
            return false;
        }
        for item in &mut self.items {
            if let Item::Thinking { collapsed, .. } = item {
                *collapsed = !expand;
            }
        }
        self.bump_version();
        true
    }

    pub fn push_delta(&mut self, chunk: &str) {
        self.flush_thinking();
        self.streaming
            .get_or_insert_with(String::new)
            .push_str(chunk);
    }

    pub fn commit_text(&mut self, text: &str) {
        self.flush_thinking();
        self.bump_version();
        self.streaming = None;
        self.items.push(Item::Agent(text.to_owned()));
    }

    pub fn push_tool(&mut self, call: ToolCall) {
        self.flush_thinking();
        self.bump_version();
        self.items.push(Item::Tool {
            id: call.id,
            name: call.name,
            display: call.display,
            status: ToolStatus::Running,
            image: None,
        });
    }

    pub fn finish_tool(
        &mut self,
        call_id: ToolCallId,
        mut outcome: ToolOutcome,
        picker: Option<&ratatui_image::picker::Picker>,
    ) {
        self.bump_version();
        let built = outcome
            .image
            .take()
            .map(|data| Box::new(crate::screenshot::TranscriptImage::new(data, picker)));
        for item in self.items.iter_mut().rev() {
            if let Item::Tool {
                id, status, image, ..
            } = item
                && *id == call_id
                && matches!(status, ToolStatus::Running)
            {
                *status = ToolStatus::Done(outcome);
                *image = built;
                return;
            }
        }
    }

    pub fn push_shell(&mut self, id: TaskId, command: String) {
        self.bump_version();
        self.items.push(Item::Shell {
            id,
            command,
            status: ShellStatus::Running,
        });
    }

    pub fn finish_shell(&mut self, task_id: TaskId, output: String) {
        self.bump_version();
        for item in self.items.iter_mut().rev() {
            if let Item::Shell { id, status, .. } = item
                && *id == task_id
                && matches!(status, ShellStatus::Running)
            {
                *status = ShellStatus::Done(output);
                return;
            }
        }
    }

    pub fn push_process(&mut self, command: String) {
        self.bump_version();
        self.items.push(Item::Process {
            command,
            output: String::new(),
            running: true,
            exit_code: None,
        });
    }

    pub fn append_process(&mut self, chunk: &str) {
        self.bump_version();
        for item in self.items.iter_mut().rev() {
            if let Item::Process { output, .. } = item {
                output.push_str(chunk);
                if !chunk.ends_with('\n') {
                    output.push('\n');
                }
                trim_log_front(output, PROCESS_LOG_CAP_BYTES);
                return;
            }
        }
    }

    pub fn finish_process(&mut self, exit_code: Option<i32>, marker: &str) {
        self.bump_version();
        for item in self.items.iter_mut().rev() {
            if let Item::Process {
                running,
                exit_code: code,
                output,
                ..
            } = item
                && *running
            {
                *running = false;
                *code = exit_code;
                output.push_str(marker);
                output.push('\n');
                return;
            }
        }
    }

    pub fn push_error(&mut self, text: impl Into<String>, hint: Option<String>) {
        self.bump_version();
        if let Some(buffer) = self.streaming.take() {
            let text = format!("{buffer} {} stopped", symbols::ui::ELLIPSIS);
            self.items.push(Item::Agent(text));
        }
        self.items.push(Item::Error {
            message: text.into(),
            hint,
        });
    }

    pub fn discard_stream(&mut self) {
        self.bump_version();
        self.streaming = None;
    }

    pub fn push_compaction(&mut self, tokens_before: u32, tokens_after: u32) {
        self.bump_version();
        self.items.push(Item::Compaction {
            tokens_before,
            tokens_after,
        });
    }

    pub fn complete(&mut self, interrupted: bool) {
        self.bump_version();
        if interrupted {
            for item in &mut self.items {
                if let Item::Tool { status, .. } = item
                    && matches!(status, ToolStatus::Running)
                {
                    *status = ToolStatus::Done(ToolOutcome {
                        ok: false,
                        summary: None,
                        image: None,
                    });
                }
                if let Item::Shell { status, .. } = item
                    && matches!(status, ShellStatus::Running)
                {
                    *status = ShellStatus::Done(String::new());
                }
            }
        }
        if let Some(buffer) = self.streaming.take() {
            let text = if interrupted {
                format!("{buffer}\n\n(interrupted)")
            } else {
                buffer
            };
            self.items.push(Item::Agent(text));
        } else if interrupted && !matches!(self.items.last(), Some(Item::Error { .. })) {
            self.items.push(Item::Interrupted);
        }
    }

    fn ensure_cache(&self, theme: Theme, width: u16, hl: &dyn Highlighter, cwd: &str) {
        let valid = self
            .cache
            .borrow()
            .as_ref()
            .is_some_and(|c| c.width == width && c.version == self.version);
        if valid {
            return;
        }
        let mut memo = self.item_memo.borrow_mut();
        if memo.width != width {
            memo.width = width;
            memo.entries.clear();
        }
        let (lines, spinner_lines, images) =
            build_static_lines(&self.items, theme, width, hl, cwd, &mut memo.entries);
        *self.cache.borrow_mut() = Some(RenderCache {
            width,
            version: self.version,
            lines,
            spinner_lines,
            images,
        });
    }

    fn streaming_rows(&self, theme: Theme, width: u16, hl: &dyn Highlighter) -> Vec<Line<'static>> {
        let Some(buffer) = &self.streaming else {
            return Vec::new();
        };
        let prefix_len = stable_prefix_len(buffer);
        let prefix_content = {
            let mut guard = self.stream_cache.borrow_mut();
            match guard.as_ref() {
                Some(cached) if cached.prefix_len == prefix_len && cached.width == width => {
                    cached.prefix_content.clone()
                }
                _ => {
                    let rendered = markdown::render(&buffer[..prefix_len], theme, hl);
                    *guard = Some(StreamCache {
                        prefix_len,
                        width,
                        prefix_content: rendered.clone(),
                    });
                    rendered
                }
            }
        };
        let mut content = prefix_content;
        let tail = markdown::render(&buffer[prefix_len..], theme, hl);
        if !content.is_empty() && !tail.is_empty() {
            content.push(Line::default());
        }
        content.extend(tail);
        while content.last().is_some_and(is_blank) {
            content.pop();
        }
        if let Some(last) = content.last_mut() {
            last.spans
                .push(Span::styled(symbols::ui::STREAM_CURSOR, theme.accent()));
        }
        hang(
            &content,
            Span::styled(symbols::marker::AGENT, theme.role_agent()),
            width,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn tail_rows(
        &self,
        theme: Theme,
        width: u16,
        hl: &dyn Highlighter,
        spinner: &'static str,
        working: Option<&Working>,
        queued: &[String],
        base_nonempty: bool,
    ) -> Vec<Line<'static>> {
        let mut tail: Vec<Line<'static>> = Vec::new();
        let streamed = self.streaming_rows(theme, width, hl);
        if !streamed.is_empty() {
            if base_nonempty {
                tail.push(Line::default());
            }
            tail.extend(streamed);
        }
        if let Some(w) = working {
            if base_nonempty || !tail.is_empty() {
                tail.push(Line::default());
            }
            tail.extend(working_rows(theme, width, spinner, w));
        }
        if !queued.is_empty() {
            if working.is_none() && (base_nonempty || !tail.is_empty()) {
                tail.push(Line::default());
            }
            tail.extend(queued_rows(theme, width, queued));
        }
        tail
    }

    pub fn content_height(
        &self,
        width: u16,
        theme: Theme,
        hl: &dyn Highlighter,
        cwd: &str,
        working: Option<&Working>,
        queued: &[String],
    ) -> usize {
        self.ensure_cache(theme, width, hl, cwd);
        let base = self.cache.borrow().as_ref().map_or(0, |c| c.lines.len());
        base + self
            .tail_rows(
                theme,
                width,
                hl,
                symbols::SPINNER[0],
                working,
                queued,
                base > 0,
            )
            .len()
    }

    pub(crate) fn render(&self, frame: &mut Frame, area: Rect, ctx: &RenderCtx<'_>) {
        let body_width = area.width.saturating_sub(ctx.left_pad);
        self.ensure_cache(ctx.theme, body_width, ctx.hl, ctx.cwd);
        let guard = self.cache.borrow();
        let Some(cache) = guard.as_ref() else {
            return;
        };
        let tail = self.tail_rows(
            ctx.theme,
            body_width,
            ctx.hl,
            ctx.spinner,
            ctx.working,
            ctx.queued,
            !cache.lines.is_empty(),
        );
        self.tail_cache.borrow_mut().clone_from(&tail);
        let total = cache.lines.len() + tail.len();
        let height = usize::from(area.height);
        let start = ctx.scroll.min(total.saturating_sub(height));
        let end = (start + height).min(total);
        let mut visible: Vec<Line<'static>> = Vec::with_capacity(end.saturating_sub(start));
        let static_end = end.min(cache.lines.len());
        for i in start.min(static_end)..static_end {
            let mut line = cache.lines[i].clone();
            if cache.spinner_lines.binary_search(&i).is_ok()
                && let Some(span) = line.spans.first_mut()
            {
                *span = Span::styled(format!("{} ", ctx.spinner), ctx.theme.accent());
            }
            visible.push(pad_left(line, ctx.left_pad, ctx.theme));
        }
        if end > cache.lines.len() {
            let from = start.saturating_sub(cache.lines.len());
            let to = end - cache.lines.len();
            visible.extend(
                tail.into_iter()
                    .take(to)
                    .skip(from)
                    .map(|line| pad_left(line, ctx.left_pad, ctx.theme)),
            );
        }
        frame.render_widget(Paragraph::new(visible), area);
        let Some(picker) = ctx.picker else {
            return;
        };
        for placement in &cache.images {
            if placement.start < start {
                continue;
            }
            let top = placement.start - start;
            let bottom = top + usize::from(placement.rows);
            if bottom > height {
                continue;
            }
            let Some(Item::Tool {
                image: Some(img), ..
            }) = self.items.get(placement.item)
            else {
                continue;
            };
            let rect = Rect {
                x: area.x + ctx.left_pad,
                y: area.y + u16::try_from(top).unwrap_or(u16::MAX),
                width: body_width,
                height: placement.rows,
            };
            img.render(frame, rect, picker);
        }
    }
}

#[cfg(test)]
mod tests {
    use goat_protocol::{InputAttachment, TaskId, ToolCall, ToolCallId, ToolOutcome};
    use ratatui::{Terminal, backend::TestBackend};

    use super::render::{
        SHELL_BLOCK_CAP, build_static_lines, format_elapsed, sanitize_shell_output, shell_rows,
        stable_prefix_len,
    };
    use super::{Item, ShellStatus, ToolStatus, Transcript, UserMessage, Working};
    use crate::{highlight::PlainHighlighter, markdown, symbols, theme::Theme};
    use ratatui::text::Line;

    fn call(id: u64, name: &str, input: &str) -> ToolCall {
        ToolCall {
            id: ToolCallId(id),
            name: name.to_owned(),
            display: goat_protocol::ToolDisplay::primary(input),
        }
    }

    fn ok() -> ToolOutcome {
        ToolOutcome {
            ok: true,
            summary: None,
            image: None,
        }
    }

    fn failed(summary: &str) -> ToolOutcome {
        ToolOutcome {
            ok: false,
            summary: Some(summary.to_owned()),
            image: None,
        }
    }

    fn commit(t: &mut Transcript, text: &str) {
        t.commit_text(text);
    }

    fn height(t: &Transcript, width: u16) -> usize {
        t.content_height(width, Theme::dark(), &PlainHighlighter, "/", None, &[])
    }

    #[test]
    fn agent_text_restyles_on_theme_change() {
        let dark = Theme::dark();
        let light = Theme::light();
        let items = vec![Item::Agent("plain body".to_owned())];
        let (dark_lines, _, _) =
            build_static_lines(&items, dark, 80, &PlainHighlighter, "/", &mut Vec::new());
        let (light_lines, _, _) =
            build_static_lines(&items, light, 80, &PlainHighlighter, "/", &mut Vec::new());
        let body_fg = |lines: &[Line<'static>]| {
            lines
                .iter()
                .flat_map(|l| &l.spans)
                .find(|s| s.content.contains("plain body"))
                .map(|s| s.style.fg)
        };
        assert_eq!(body_fg(&dark_lines), Some(Some(dark.fg_color())));
        assert_eq!(body_fg(&light_lines), Some(Some(light.fg_color())));
    }

    #[test]
    fn memoized_rebuild_matches_fresh_rebuild() {
        let theme = Theme::dark();
        let mut items = vec![
            Item::User(UserMessage {
                text: "hello".to_owned(),
                attachments: Vec::new(),
            }),
            Item::Agent("# title\n\nbody text".to_owned()),
            Item::Tool {
                id: ToolCallId(1),
                name: "Read".to_owned(),
                display: goat_protocol::ToolDisplay::primary("Read(a.txt)"),
                status: ToolStatus::Running,
                image: None,
            },
        ];
        let mut memo = Vec::new();
        let _ = build_static_lines(&items, theme, 80, &PlainHighlighter, "/", &mut memo);
        if let Item::Tool { status, .. } = &mut items[2] {
            *status = ToolStatus::Done(ok());
        }
        items.push(Item::Agent("second answer".to_owned()));
        let (memo_lines, _, _) =
            build_static_lines(&items, theme, 80, &PlainHighlighter, "/", &mut memo);
        let (fresh_lines, _, _) =
            build_static_lines(&items, theme, 80, &PlainHighlighter, "/", &mut Vec::new());
        let render = |lines: &[Line<'static>]| {
            lines
                .iter()
                .map(|l| {
                    l.spans
                        .iter()
                        .map(|s| s.content.clone())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(render(&memo_lines), render(&fresh_lines));
    }

    fn buffer_row(terminal: &Terminal<TestBackend>, y: u16) -> String {
        let buffer = terminal.backend().buffer();
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    }

    #[test]
    fn tool_lifecycle() {
        let mut t = Transcript::default();
        t.push_tool(call(1, "Read", "src/lib.rs"));
        assert!(matches!(
            &t.items[0],
            Item::Tool {
                status: ToolStatus::Running,
                ..
            }
        ));
        t.finish_tool(ToolCallId(1), ok(), None);
        assert!(matches!(&t.items[0], Item::Tool { status: ToolStatus::Done(o), .. } if o.ok));
    }

    #[test]
    fn tool_failed_with_summary() {
        let mut t = Transcript::default();
        t.push_tool(call(2, "Bash", "cargo build"));
        t.finish_tool(ToolCallId(2), failed("error[E0308]"), None);
        if let Some(Item::Tool {
            status: ToolStatus::Done(outcome),
            ..
        }) = t.items.last()
        {
            assert!(!outcome.ok);
            assert_eq!(outcome.summary.as_deref(), Some("error[E0308]"));
        } else {
            panic!("expected done tool");
        }
    }

    #[test]
    fn agent_loop_ordering() {
        let mut t = Transcript::default();
        t.push_user("hi");
        t.push_delta("step one");
        commit(&mut t, "step one");
        t.push_tool(call(1, "Read", "src/lib.rs"));
        t.finish_tool(ToolCallId(1), ok(), None);
        t.push_delta("step two");
        commit(&mut t, "step two");

        assert!(matches!(&t.items[0], Item::User(_)));
        assert!(matches!(&t.items[1], Item::Agent(_)));
        assert!(matches!(&t.items[2], Item::Tool { .. }));
        assert!(matches!(&t.items[3], Item::Agent(_)));
    }

    #[test]
    fn complete_interrupted_clears_running_tools() {
        let mut t = Transcript::default();
        t.push_tool(call(5, "Bash", "long cmd"));
        t.complete(true);
        if let Some(Item::Tool {
            status: ToolStatus::Done(o),
            ..
        }) = t.items.first()
        {
            assert!(!o.ok);
        } else {
            panic!("expected failed tool");
        }
        assert!(
            matches!(t.items.last(), Some(Item::Interrupted)),
            "interrupt with no stream must append Interrupted"
        );
    }

    #[test]
    fn finish_tool_by_id_reverse_order() {
        let mut t = Transcript::default();
        t.push_tool(call(10, "Read", "a"));
        t.push_tool(call(11, "Grep", "b"));
        t.finish_tool(ToolCallId(11), ok(), None);
        t.finish_tool(ToolCallId(10), failed("err"), None);
        assert!(matches!(&t.items[0], Item::Tool { status: ToolStatus::Done(o), .. } if !o.ok));
        assert!(matches!(&t.items[1], Item::Tool { status: ToolStatus::Done(o), .. } if o.ok));
    }

    #[test]
    fn content_height_counts_streaming() {
        let mut t = Transcript::default();
        commit(&mut t, "hello world");
        let h1 = height(&t, 80);
        t.push_delta("line one\nline two\nline three\nline four");
        let h2 = height(&t, 80);
        assert!(
            h2 > h1,
            "content_height must grow while streaming is active"
        );
    }

    #[test]
    fn content_height_reserves_image_rows() {
        let mut t = Transcript::default();
        t.push_tool(call(1, "Browser", "screenshot"));
        t.finish_tool(ToolCallId(1), ok(), None);
        let without = height(&t, 80);
        if let Some(Item::Tool { image, .. }) = t.items.last_mut() {
            *image = Some(Box::new(crate::screenshot::TranscriptImage::fixed(5)));
        }
        t.bump_version();
        let with = height(&t, 80);
        assert_eq!(with, without + 5);
    }

    #[test]
    fn content_height_includes_working_line() {
        let mut t = Transcript::default();
        commit(&mut t, "hello world");
        let idle = height(&t, 80);
        let working = Working {
            elapsed: None,
            label: None,
            thinking: false,
            tokens: None,
        };
        let busy = t.content_height(
            80,
            Theme::dark(),
            &PlainHighlighter,
            "/",
            Some(&working),
            &[],
        );
        assert!(
            busy > idle,
            "content_height must be larger when busy (working line)"
        );
    }

    #[test]
    fn interrupted_without_stream_pushes_notice() {
        let mut t = Transcript::default();
        t.complete(true);
        assert!(
            matches!(t.items.last(), Some(Item::Interrupted)),
            "interrupting with no stream must push Interrupted"
        );
    }

    #[test]
    fn thinking_buffer_flushes_before_text_and_toggles() {
        let mut t = Transcript::default();
        t.push_thinking_delta("weighing options");
        assert!(t.items.is_empty(), "thinking stays buffered until flushed");
        t.push_delta("answer");
        assert!(
            matches!(
                t.items.first(),
                Some(Item::Thinking {
                    collapsed: true,
                    ..
                })
            ),
            "first content delta flushes thinking as a collapsed item"
        );
        assert!(t.toggle_thinking(), "toggle reports thinking present");
        assert!(matches!(
            t.items.first(),
            Some(Item::Thinking {
                collapsed: false,
                ..
            })
        ));
        assert!(t.toggle_thinking());
        assert!(matches!(
            t.items.first(),
            Some(Item::Thinking {
                collapsed: true,
                ..
            })
        ));
    }

    #[test]
    fn blank_thinking_is_dropped() {
        let mut t = Transcript::default();
        t.push_thinking_delta("   ");
        t.flush_thinking();
        assert!(t.items.is_empty());
        assert!(!t.toggle_thinking(), "no thinking means toggle is a no-op");
    }

    #[test]
    fn error_commits_partial_stream_before_error_row() {
        let mut t = Transcript::default();
        t.push_delta("partial answer");
        t.push_error("boom", None);
        assert!(matches!(&t.items[0], Item::Agent(_)));
        assert!(matches!(&t.items[1], Item::Error { .. }));
        t.complete(true);
        assert!(
            !matches!(t.items.last(), Some(Item::Interrupted)),
            "interrupted row must be suppressed right after an error row"
        );
    }

    #[test]
    fn discard_stream_drops_partial_only() {
        let mut t = Transcript::default();
        commit(&mut t, "committed");
        t.push_delta("doomed partial");
        let before = height(&t, 80);
        t.discard_stream();
        assert!(height(&t, 80) < before);
        assert_eq!(t.items.len(), 1);
    }

    #[test]
    fn word_wrapped_bottom_is_visible_at_max_scroll() {
        let mut t = Transcript::default();
        commit(&mut t, "aaaaaaa bbbbbbb ccccccc");
        let h = height(&t, 12);
        assert_eq!(h, 3, "word wrap must yield three visual rows at width 12");
        let mut terminal = Terminal::new(TestBackend::new(12, 2)).unwrap();
        terminal
            .draw(|frame| {
                t.render(
                    frame,
                    frame.area(),
                    &super::RenderCtx {
                        theme: Theme::dark(),
                        scroll: h - 2,
                        left_pad: 0,
                        cwd: "/",
                        spinner: symbols::SPINNER[0],
                        working: None,
                        queued: &[],
                        hl: &PlainHighlighter,
                        picker: None,
                    },
                );
            })
            .unwrap();
        assert!(buffer_row(&terminal, 1).contains("ccccccc"));
    }

    #[test]
    fn running_tool_renders_current_spinner_frame() {
        let mut t = Transcript::default();
        t.push_tool(call(1, "Read", "x"));
        let mut terminal = Terminal::new(TestBackend::new(20, 2)).unwrap();
        terminal
            .draw(|frame| {
                t.render(
                    frame,
                    frame.area(),
                    &super::RenderCtx {
                        theme: Theme::dark(),
                        scroll: 0,
                        left_pad: 0,
                        cwd: "/",
                        spinner: symbols::SPINNER[3],
                        working: None,
                        queued: &[],
                        hl: &PlainHighlighter,
                        picker: None,
                    },
                );
            })
            .unwrap();
        assert!(buffer_row(&terminal, 0).starts_with(&format!("{} ", symbols::SPINNER[3])));
    }

    fn cell_bg(terminal: &Terminal<TestBackend>, x: u16, y: u16) -> Option<ratatui::style::Color> {
        terminal.backend().buffer()[(x, y)].style().bg
    }

    fn row_has_bg(
        terminal: &Terminal<TestBackend>,
        y: u16,
        bg: Option<ratatui::style::Color>,
    ) -> bool {
        let buffer = terminal.backend().buffer();
        (0..buffer.area.width).all(|x| buffer[(x, y)].style().bg == bg)
    }

    #[test]
    fn user_rows_render_padded_panel_background() {
        let mut t = Transcript::default();
        t.push_user("hello\nworld");
        commit(&mut t, "answer");
        assert_eq!(height(&t, 20), 4);
        let theme = Theme::dark();
        let panel_bg = theme.user_panel().bg;
        let mut terminal = Terminal::new(TestBackend::new(21, 4)).unwrap();
        terminal
            .draw(|frame| {
                t.render(
                    frame,
                    frame.area(),
                    &super::RenderCtx {
                        theme,
                        scroll: 0,
                        left_pad: 1,
                        cwd: "/",
                        spinner: symbols::SPINNER[0],
                        working: None,
                        queued: &[],
                        hl: &PlainHighlighter,
                        picker: None,
                    },
                );
            })
            .unwrap();
        assert!(row_has_bg(&terminal, 0, panel_bg));
        assert!(row_has_bg(&terminal, 1, panel_bg));
        assert!(buffer_row(&terminal, 0).contains("hello"));
        assert!(buffer_row(&terminal, 1).contains("world"));
        assert_ne!(cell_bg(&terminal, 0, 2), panel_bg);
    }

    #[test]
    fn user_rows_keep_attachment_inside_padded_panel() {
        let mut t = Transcript::default();
        t.push_user_with_attachments(
            "hi",
            vec![InputAttachment {
                media_type: "image/png".to_owned(),
                data: "data".to_owned(),
                label: "demo.png".to_owned(),
            }],
        );
        assert_eq!(height(&t, 24), 2);
        let theme = Theme::dark();
        let panel_bg = theme.user_panel().bg;
        let mut terminal = Terminal::new(TestBackend::new(24, 3)).unwrap();
        terminal
            .draw(|frame| {
                t.render(
                    frame,
                    frame.area(),
                    &super::RenderCtx {
                        theme,
                        scroll: 0,
                        left_pad: 1,
                        cwd: "/",
                        spinner: symbols::SPINNER[0],
                        working: None,
                        queued: &[],
                        hl: &PlainHighlighter,
                        picker: None,
                    },
                );
            })
            .unwrap();
        assert!(buffer_row(&terminal, 0).contains("hi"));
        assert!(buffer_row(&terminal, 1).contains("[image: demo.png]"));
        assert!(row_has_bg(&terminal, 0, panel_bg));
        assert!(row_has_bg(&terminal, 1, panel_bg));
        assert_ne!(cell_bg(&terminal, 0, 2), panel_bg);
    }

    #[test]
    fn content_height_exceeds_u16_for_huge_transcripts() {
        let mut t = Transcript::default();
        t.push_user("x\n".repeat(70_000));
        assert!(height(&t, 80) > usize::from(u16::MAX));
    }

    #[test]
    fn queued_rows_render_below_working_and_cap() {
        let mut t = Transcript::default();
        commit(&mut t, "answer");
        let queued: Vec<String> = (0..5).map(|i| format!("queued {i}")).collect();
        let with_queue = t.content_height(80, Theme::dark(), &PlainHighlighter, "/", None, &queued);
        let without = t.content_height(80, Theme::dark(), &PlainHighlighter, "/", None, &[]);
        assert_eq!(with_queue - without, 5, "3 rows + overflow row + spacer");
    }

    #[test]
    fn format_elapsed_scales_units() {
        assert_eq!(format_elapsed(42), "42s");
        assert_eq!(format_elapsed(92), "1m32s");
        assert_eq!(format_elapsed(3_725), "1h02m");
    }

    fn line_text(line: &ratatui::text::Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.clone()).collect()
    }

    #[test]
    fn shell_lifecycle() {
        let mut t = Transcript::default();
        t.push_shell(TaskId(1), "echo hi".to_owned());
        assert!(matches!(
            &t.items[0],
            Item::Shell {
                status: ShellStatus::Running,
                ..
            }
        ));
        t.finish_shell(TaskId(1), "hi".to_owned());
        assert!(matches!(
            &t.items[0],
            Item::Shell {
                status: ShellStatus::Done(output),
                ..
            } if output == "hi"
        ));
    }

    #[test]
    fn complete_interrupted_finishes_running_shell() {
        let mut t = Transcript::default();
        t.push_shell(TaskId(2), "sleep 99".to_owned());
        t.complete(true);
        assert!(matches!(
            &t.items[0],
            Item::Shell {
                status: ShellStatus::Done(_),
                ..
            }
        ));
    }

    #[test]
    fn sanitize_strips_ansi_and_resolves_carriage_returns() {
        let lines = sanitize_shell_output("\u{1b}[31mred\u{1b}[0m\nstep1\rstep2\rdone\nlast\r\n\n");
        assert_eq!(lines, vec!["red", "done", "last"]);
    }

    #[test]
    fn shell_rows_render_command_and_output() {
        let rows = shell_rows(
            "echo hi",
            &ShellStatus::Done("hi".to_owned()),
            Theme::dark(),
            80,
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(line_text(&rows[0]), "! echo hi");
        assert_eq!(line_text(&rows[1]), "  hi");
    }

    #[test]
    fn shell_rows_caps_visual_rows_and_pins_exit_code() {
        let output = format!("{}\nexit code: 2", "x".repeat(30 * 76));
        let rows = shell_rows("badcmd", &ShellStatus::Done(output), Theme::dark(), 80);
        assert_eq!(rows.len(), 1 + SHELL_BLOCK_CAP + 2);
        assert!(line_text(&rows[1 + SHELL_BLOCK_CAP]).contains("more"));
        assert!(line_text(rows.last().unwrap()).contains("exit code: 2"));
    }

    #[test]
    fn shell_rows_show_no_output_placeholder() {
        let rows = shell_rows(
            "true",
            &ShellStatus::Done("  \n".to_owned()),
            Theme::dark(),
            80,
        );
        assert_eq!(rows.len(), 2);
        assert!(line_text(&rows[1]).contains("(no output)"));
    }

    #[test]
    fn stable_prefix_splits_outside_fences_only() {
        let text = "para one\n\npara two\n\nmore";
        let split = stable_prefix_len(text);
        assert_eq!(&text[..split], "para one\n\npara two\n\n");

        let fenced = "intro\n\n```\ncode\n\nstill code\n```\n\nafter";
        let split = stable_prefix_len(fenced);
        assert_eq!(
            &fenced[..split],
            "intro\n\n```\ncode\n\nstill code\n```\n\n"
        );

        let open_fence = "intro\n\n```\ncode\n\nstill code";
        let split = stable_prefix_len(open_fence);
        assert_eq!(
            &open_fence[..split],
            "intro\n\n",
            "must not split at a blank line inside an open fence"
        );
    }

    #[test]
    fn incremental_stream_render_matches_full_render() {
        let hl = PlainHighlighter;
        let theme = Theme::dark();
        let buffer = "# Title\n\nSome **bold** text.\n\n```rust\nfn main() {}\n```\n\nTail line";
        let split = stable_prefix_len(buffer);
        let mut incremental = markdown::render(&buffer[..split], theme, &hl);
        let tail = markdown::render(&buffer[split..], theme, &hl);
        if !incremental.is_empty() && !tail.is_empty() {
            incremental.push(ratatui::text::Line::default());
        }
        incremental.extend(tail);
        let full = markdown::render(buffer, theme, &hl);
        let render_text = |lines: &[ratatui::text::Line<'static>]| {
            lines
                .iter()
                .map(|l| {
                    l.spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(render_text(&incremental), render_text(&full));
    }
}
