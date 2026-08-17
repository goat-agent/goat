use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::Cursor;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use chromiumoxide_cdp::cdp::browser_protocol::dom::SetFileInputFilesParams;
use chromiumoxide_cdp::cdp::browser_protocol::input::DispatchMouseEventType;
use chromiumoxide_cdp::cdp::browser_protocol::network::{GetCookiesParams, SetCookieParams};
use goat_api::{BrowserCommand, BrowserTab};
use goat_tool::{ToolImage, ToolOutput};
use serde_json::Value;
use tokio::sync::Mutex;
use tokio::time::sleep;

use crate::action::{Action, BrowserRef, ScrollDirection, StorageOp, TabOp};
use crate::cdp::{Cdp, Element};
use crate::dialog::DialogGuard;
use crate::error::BrowserError;
use crate::keys::stroke;
use crate::navigation;
use crate::observe::SessionObservers;
use crate::resilience::{
    OP_CLICK, OP_EVAL, OP_FILL, OP_FIND, OP_HEALTH, OP_META, OP_NAV_ACK, OP_OPEN, OP_SCREENSHOT,
    with_timeout,
};
use crate::snapshot::{BrowserSnapshot, RawSnapshot, SNAPSHOT_JS, format_snapshot};
use crate::transport::Transport;

const SNAPSHOT_MAX_BYTES: usize = 32 * 1024;
const SCREENSHOT_MAX_DIM: u32 = 1280;
const DEFAULT_TEXT_MAX_BYTES: usize = 8 * 1024;

pub type SessionHandle = Arc<Mutex<Option<BrowserSession>>>;

pub fn new_handle() -> SessionHandle {
    Arc::new(Mutex::new(None))
}

pub struct BrowserSession {
    cdp: Cdp,
    dialog: DialogGuard,
    observers: SessionObservers,
    known_tabs: HashSet<i64>,
    snapshot_seq: u64,
    current_snapshot_id: Option<String>,
}

pub async fn ensure_session<'a>(
    slot: &'a mut Option<BrowserSession>,
    transport: &Arc<dyn Transport>,
) -> Result<&'a mut BrowserSession, BrowserError> {
    let mut healthy = false;
    if let Some(session) = slot.as_ref() {
        healthy = session.is_healthy().await;
    }
    if !healthy {
        if let Some(old) = slot.take() {
            old.dispose().await;
        }
        *slot = Some(open_session(transport.clone()).await?);
    }
    slot.as_mut()
        .ok_or_else(|| BrowserError::Message("browser session unavailable".to_owned()))
}

async fn open_session(transport: Arc<dyn Transport>) -> Result<BrowserSession, BrowserError> {
    let cdp = Cdp::new(transport);
    with_timeout(OP_OPEN, "attach", cdp.attach())
        .await
        .map_err(|err| {
            match err {
            BrowserError::Timeout { .. } => BrowserError::Message(
                "the browser did not answer the attach; open Chrome with the goat extension enabled"
                    .to_owned(),
            ),
            other => other,
        }
        })?;
    let dialog = DialogGuard::spawn(&cdp);
    let observers = SessionObservers::spawn(&cdp);
    let known_tabs = with_timeout(OP_META, "tabs", cdp.tabs(BrowserCommand::TabList {}))
        .await
        .map(|tabs| tabs.iter().map(|tab| tab.id).collect())
        .unwrap_or_default();
    Ok(BrowserSession {
        cdp,
        dialog,
        observers,
        known_tabs,
        snapshot_seq: 0,
        current_snapshot_id: None,
    })
}

pub async fn close(slot: &mut Option<BrowserSession>) -> String {
    let Some(session) = slot.take() else {
        return "browser is not attached".to_owned();
    };
    session.dispose().await;
    "browser released".to_owned()
}

impl BrowserSession {
    async fn dispose(self) {
        self.dialog.abort();
        self.observers.abort();
        let _ = with_timeout(OP_META, "detach", self.cdp.detach()).await;
    }

    async fn is_healthy(&self) -> bool {
        with_timeout(OP_HEALTH, "health", self.cdp.eval("1"))
            .await
            .is_ok()
    }

    pub async fn dispatch(
        &mut self,
        action: Action,
        max_bytes: usize,
    ) -> Result<ToolOutput, BrowserError> {
        let output = match action {
            Action::Navigate { url } => ToolOutput::text(self.navigate(&url, max_bytes).await?),
            Action::Snapshot => ToolOutput::text(
                self.snapshot("snapshot -> complete", "complete", false, max_bytes)
                    .await?,
            ),
            Action::Click { reference } => {
                ToolOutput::text(self.click(&reference, max_bytes).await?)
            }
            Action::Fill {
                reference,
                text,
                submit,
            } => ToolOutput::text(self.fill(&reference, &text, submit, max_bytes).await?),
            Action::Select { reference, value } => {
                ToolOutput::text(self.select(&reference, &value, max_bytes).await?)
            }
            Action::Hover { reference } => {
                ToolOutput::text(self.hover(&reference, max_bytes).await?)
            }
            Action::Drag { from, to } => ToolOutput::text(self.drag(&from, &to, max_bytes).await?),
            Action::Upload { reference, path } => {
                ToolOutput::text(self.upload(&reference, &path, max_bytes).await?)
            }
            Action::PressKey { key } => ToolOutput::text(self.press_key(&key, max_bytes).await?),
            Action::Scroll { direction, amount } => {
                ToolOutput::text(self.scroll(direction, amount, max_bytes).await?)
            }
            Action::GoBack => ToolOutput::text(self.history(-1, max_bytes).await?),
            Action::GoForward => ToolOutput::text(self.history(1, max_bytes).await?),
            Action::FindText { query, max_chars } => ToolOutput::text(
                self.find_text(
                    &query,
                    max_chars.unwrap_or(DEFAULT_TEXT_MAX_BYTES),
                    max_bytes,
                )
                .await?,
            ),
            Action::Inspect {
                reference,
                max_chars,
            } => ToolOutput::text(
                self.inspect(
                    &reference,
                    max_chars.unwrap_or(DEFAULT_TEXT_MAX_BYTES),
                    max_bytes,
                )
                .await?,
            ),
            Action::ReadViewport { max_chars } => ToolOutput::text(
                self.read_viewport(max_chars.unwrap_or(DEFAULT_TEXT_MAX_BYTES), max_bytes)
                    .await?,
            ),
            Action::ReadContent { max_chars } => ToolOutput::text(
                self.read_content(max_chars.unwrap_or(DEFAULT_TEXT_MAX_BYTES), max_bytes)
                    .await?,
            ),
            Action::ReadNetwork { filter, limit } => ToolOutput::text(
                self.read_network_out(filter.as_deref(), limit, max_bytes)
                    .await,
            ),
            Action::ReadConsole { level, limit } => ToolOutput::text(
                self.read_console_out(level.as_deref(), limit, max_bytes)
                    .await,
            ),
            Action::Storage { op, name, value } => ToolOutput::text(
                self.storage(op, name.as_deref(), value.as_deref(), max_bytes)
                    .await?,
            ),
            Action::Tab { op, tab_id, url } => {
                ToolOutput::text(self.tab(op, tab_id, url.as_deref(), max_bytes).await?)
            }
            Action::WaitFor {
                text,
                state,
                timeout_ms,
            } => ToolOutput::text(
                self.wait_for(text.as_deref(), state.as_deref(), timeout_ms, max_bytes)
                    .await?,
            ),
            Action::Screenshot => ToolOutput::image(self.screenshot().await?),
            Action::DebugEval { js } => ToolOutput::text(self.debug_eval(&js, max_bytes).await?),
            Action::Close => ToolOutput::text("browser released".to_owned()),
        };
        Ok(output)
    }

    async fn navigate(&mut self, url: &str, max_bytes: usize) -> Result<String, BrowserError> {
        let target = normalize_url(url)?;
        let acked = with_timeout(OP_NAV_ACK, "navigate", self.cdp.navigate(target))
            .await
            .is_ok();
        let load = if acked {
            navigation::await_navigation_ready(&self.cdp).await
        } else {
            let _ = with_timeout(OP_META, "stop_loading", self.cdp.stop_loading()).await;
            "nav_error"
        };
        let switched = self.follow_new_tab().await;
        self.snapshot("navigate -> usable", load, switched, max_bytes)
            .await
    }

    async fn click(
        &mut self,
        reference: &BrowserRef,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        self.validate_snapshot(reference)?;
        let element = self.find_ref(&reference.reference).await?;
        ensure_actionable(&element, "click").await?;
        let _ = with_timeout(OP_META, "scroll_into_view", element.scroll_into_view()).await;
        with_timeout(OP_CLICK, "click", element.click()).await?;
        self.settle_and_snapshot("click -> changed", max_bytes)
            .await
    }

    async fn fill(
        &mut self,
        reference: &BrowserRef,
        text: &str,
        submit: bool,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        self.validate_snapshot(reference)?;
        let element = self.find_ref(&reference.reference).await?;
        ensure_actionable(&element, "fill").await?;
        let _ = with_timeout(OP_META, "scroll_into_view", element.scroll_into_view()).await;
        with_timeout(OP_CLICK, "focus", element.click()).await?;
        let _ = with_timeout(
            OP_EVAL,
            "clear",
            element.call_js_fn(
                "function() { if ('value' in this) { this.value = ''; this.dispatchEvent(new Event('input', { bubbles: true })); } }",
            ),
        )
        .await;
        with_timeout(OP_FILL, "type", self.cdp.insert_text(text)).await?;
        if submit {
            with_timeout(OP_FILL, "submit", self.cdp.key(&stroke("Enter")?)).await?;
        }
        self.settle_and_snapshot("fill -> changed", max_bytes).await
    }

    async fn select(
        &mut self,
        reference: &BrowserRef,
        value: &str,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        self.validate_snapshot(reference)?;
        let element = self.find_ref(&reference.reference).await?;
        ensure_actionable(&element, "select").await?;
        let literal = json_literal(value)?;
        let declaration = format!(
            "function() {{ const v = {literal}; for (let i = 0; i < this.options.length; i++) {{ const o = this.options[i]; if (o.value === v || o.text.trim() === v) {{ this.selectedIndex = i; this.dispatchEvent(new Event('input', {{ bubbles: true }})); this.dispatchEvent(new Event('change', {{ bubbles: true }})); return true; }} }} return false; }}"
        );
        let matched = with_timeout(OP_EVAL, "select", element.call_js_fn(declaration))
            .await?
            .as_ref()
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !matched {
            return Err(BrowserError::Input(format!(
                "no option matching \"{value}\" in {}",
                reference.reference
            )));
        }
        self.settle_and_snapshot("select -> changed", max_bytes)
            .await
    }

    async fn press_key(&mut self, key: &str, max_bytes: usize) -> Result<String, BrowserError> {
        with_timeout(OP_FILL, "press_key", self.cdp.key(&stroke(key)?)).await?;
        self.settle_and_snapshot("press_key -> changed", max_bytes)
            .await
    }

    async fn scroll(
        &mut self,
        direction: ScrollDirection,
        amount: Option<i64>,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        let amount = amount.unwrap_or(640).abs();
        let (x, y) = match direction {
            ScrollDirection::Up => (0, -amount),
            ScrollDirection::Down => (0, amount),
            ScrollDirection::Left => (-amount, 0),
            ScrollDirection::Right => (amount, 0),
        };
        let js = format!("window.scrollBy({{ left: {x}, top: {y}, behavior: 'instant' }}); true");
        with_timeout(OP_EVAL, "scroll", self.cdp.eval(&js)).await?;
        self.snapshot("scroll -> changed", "complete", false, max_bytes)
            .await
    }

    async fn history(&mut self, delta: i32, max_bytes: usize) -> Result<String, BrowserError> {
        let js = format!("history.go({delta}); true");
        with_timeout(OP_EVAL, "history", self.cdp.eval(&js)).await?;
        let action = if delta < 0 {
            "go_back -> navigation"
        } else {
            "go_forward -> navigation"
        };
        self.settle_and_snapshot(action, max_bytes).await
    }

    async fn find_text(
        &mut self,
        query: &str,
        max_chars: usize,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        let literal = json_literal(query)?;
        let js = format!(
            "(() => {{ const q = {literal}.toLowerCase(); const walker = document.createTreeWalker(document.body || document.documentElement, NodeFilter.SHOW_TEXT); const out = []; while (walker.nextNode() && out.length < 20) {{ const t = walker.currentNode.textContent.trim().replace(/\\s+/g, ' '); if (t.toLowerCase().includes(q)) out.push(t.slice(0, 240)); }} return out; }})()"
        );
        let result = with_timeout(OP_EVAL, "find_text", self.cdp.eval(&js)).await?;
        let mut out = self
            .state_header("find_text -> complete", "complete", max_bytes)
            .await?;
        out.push_str("\nuntrusted_text_matches:\n");
        match result.as_ref().and_then(Value::as_array) {
            Some(items) if !items.is_empty() => {
                for item in items {
                    if let Some(text) = item.as_str() {
                        let _ = writeln!(out, "- \"{}\"", cap_chars(text, max_chars.min(240)));
                    }
                }
            }
            _ => out.push_str("- none\n"),
        }
        Ok(cap(out, max_bytes))
    }

    async fn inspect(
        &mut self,
        reference: &BrowserRef,
        max_chars: usize,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        self.validate_snapshot(reference)?;
        let element = self.find_ref(&reference.reference).await?;
        let detail = with_timeout(
            OP_EVAL,
            "inspect",
            element.call_js_fn(
                "function() { return { role: this.getAttribute('role') || this.tagName.toLowerCase(), text: (this.innerText || this.value || '').trim().replace(/\\s+/g, ' ').slice(0, 4000), disabled: !!this.disabled, readonly: !!this.readOnly }; }",
            ),
        )
        .await?;
        let mut out = self
            .state_header("inspect -> complete", "complete", max_bytes)
            .await?;
        out.push_str("\nuntrusted_region:\n");
        if let Some(value) = detail {
            let rendered =
                serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
            out.push_str(&cap_chars(&rendered, max_chars));
            out.push('\n');
        } else {
            out.push_str("- none\n");
        }
        Ok(cap(out, max_bytes))
    }

    async fn read_viewport(
        &mut self,
        max_chars: usize,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        let js = "(() => Array.from(document.querySelectorAll('body *')).filter(e => { const r = e.getBoundingClientRect(); const s = getComputedStyle(e); return r.bottom >= 0 && r.top <= innerHeight && r.width > 0 && r.height > 0 && s.display !== 'none' && s.visibility !== 'hidden'; }).map(e => (e.innerText || '').trim().replace(/\\s+/g, ' ')).filter(Boolean).slice(0, 80).join('\\n'))()";
        let result = with_timeout(OP_EVAL, "read_viewport", self.cdp.eval(js)).await?;
        let mut out = self
            .state_header("read_viewport -> complete", "complete", max_bytes)
            .await?;
        out.push_str("\nuntrusted_viewport_text:\n");
        if let Some(value) = result.as_ref().and_then(Value::as_str) {
            out.push_str(&cap_chars(value, max_chars));
            out.push('\n');
        } else {
            out.push_str("none\n");
        }
        Ok(cap(out, max_bytes))
    }

    async fn wait_for(
        &mut self,
        text: Option<&str>,
        state: Option<&str>,
        timeout_ms: Option<u64>,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        let limit = Duration::from_millis(timeout_ms.unwrap_or(5_000).clamp(100, 30_000));
        let started = Instant::now();
        loop {
            if let Some(expected) = text {
                let literal = json_literal(expected)?;
                let js = format!("document.body && document.body.innerText.includes({literal})");
                if with_timeout(OP_EVAL, "wait_for", self.cdp.eval(&js))
                    .await?
                    .as_ref()
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    return self
                        .snapshot("wait_for -> changed", "complete", false, max_bytes)
                        .await;
                }
            } else if let Some(expected) = state {
                match expected {
                    "usable" | "idle" => {
                        return self
                            .snapshot("wait_for -> complete", "complete", false, max_bytes)
                            .await;
                    }
                    "complete" => {
                        if self.ready_state_complete().await? {
                            return self
                                .snapshot("wait_for -> complete", "complete", false, max_bytes)
                                .await;
                        }
                    }
                    other => {
                        return Err(BrowserError::Input(format!(
                            "unsupported wait_for state '{other}'; valid states: usable, idle, complete"
                        )));
                    }
                }
            } else {
                return Err(BrowserError::Input(
                    "action 'wait_for' requires 'text' or 'state'".to_owned(),
                ));
            }
            if started.elapsed() >= limit {
                return self
                    .snapshot("wait_for -> timeout", "timeout", false, max_bytes)
                    .await;
            }
            sleep(Duration::from_millis(200)).await;
        }
    }

    async fn debug_eval(&self, js: &str, max_bytes: usize) -> Result<String, BrowserError> {
        let result = with_timeout(OP_EVAL, "debug_eval", self.cdp.eval(js)).await?;
        let rendered = match result {
            Some(value) => {
                serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
            }
            None => "undefined".to_owned(),
        };
        Ok(goat_tool::truncate(rendered, max_bytes))
    }

    async fn screenshot(&self) -> Result<ToolImage, BrowserError> {
        let encoded = with_timeout(OP_SCREENSHOT, "screenshot", self.cdp.screenshot()).await?;
        Ok(ToolImage {
            media_type: "image/jpeg".to_owned(),
            data: downscale(&encoded).unwrap_or(encoded),
        })
    }

    async fn hover(
        &mut self,
        reference: &BrowserRef,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        self.validate_snapshot(reference)?;
        let element = self.find_ref(&reference.reference).await?;
        let _ = with_timeout(OP_META, "scroll_into_view", element.scroll_into_view()).await;
        with_timeout(OP_CLICK, "hover", element.hover()).await?;
        self.settle_and_snapshot("hover -> changed", max_bytes)
            .await
    }

    async fn drag(
        &mut self,
        from: &BrowserRef,
        to: &BrowserRef,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        self.validate_snapshot(from)?;
        self.validate_snapshot(to)?;
        let from_el = self.find_ref(&from.reference).await?;
        let to_el = self.find_ref(&to.reference).await?;
        let _ = with_timeout(OP_META, "scroll_into_view", from_el.scroll_into_view()).await;
        let (sx, sy) = with_timeout(OP_META, "box", from_el.rect()).await?.center();
        let (ex, ey) = with_timeout(OP_META, "box", to_el.rect()).await?.center();
        self.cdp
            .mouse(DispatchMouseEventType::MouseMoved, sx, sy, false)
            .await?;
        self.cdp
            .mouse(DispatchMouseEventType::MousePressed, sx, sy, true)
            .await?;
        self.cdp
            .mouse(DispatchMouseEventType::MouseMoved, ex, ey, false)
            .await?;
        self.cdp
            .mouse(DispatchMouseEventType::MouseReleased, ex, ey, true)
            .await?;
        self.settle_and_snapshot("drag -> changed", max_bytes).await
    }

    async fn upload(
        &mut self,
        reference: &BrowserRef,
        path: &str,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        self.validate_snapshot(reference)?;
        let element = self.find_ref(&reference.reference).await?;
        let params = SetFileInputFilesParams::builder()
            .files(vec![path.to_owned()])
            .object_id(element.object_id())
            .build()
            .map_err(BrowserError::Message)?;
        with_timeout(OP_FILL, "upload", self.cdp.send(params)).await?;
        self.settle_and_snapshot("upload -> changed", max_bytes)
            .await
    }

    async fn read_content(
        &self,
        max_chars: usize,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        let js = "(() => { const p = document.querySelector('article, main, [role=\"main\"]') || document.body; return p ? (p.innerText || '').trim().replace(/\\n{3,}/g, '\\n\\n') : ''; })()";
        let result = with_timeout(OP_EVAL, "read_content", self.cdp.eval(js)).await?;
        let mut out = self
            .state_header("read_content -> complete", "complete", max_bytes)
            .await?;
        out.push_str("\nmain_content:\n");
        match result.as_ref().and_then(Value::as_str) {
            Some(text) if !text.is_empty() => {
                out.push_str(&cap_chars(text, max_chars));
                out.push('\n');
            }
            _ => out.push_str("none\n"),
        }
        Ok(cap(out, max_bytes))
    }

    async fn read_network_out(
        &self,
        filter: Option<&str>,
        limit: Option<usize>,
        max_bytes: usize,
    ) -> String {
        cap(
            self.observers
                .read_network(filter, limit.unwrap_or(20))
                .await,
            max_bytes,
        )
    }

    async fn read_console_out(
        &self,
        level: Option<&str>,
        limit: Option<usize>,
        max_bytes: usize,
    ) -> String {
        cap(
            self.observers
                .read_console(level, limit.unwrap_or(20))
                .await,
            max_bytes,
        )
    }

    async fn storage(
        &mut self,
        op: StorageOp,
        name: Option<&str>,
        value: Option<&str>,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        match op {
            StorageOp::GetCookies => {
                let returns = with_timeout(
                    OP_META,
                    "cookies",
                    self.cdp.send(GetCookiesParams::default()),
                )
                .await?;
                let mut out = String::from("cookies:\n");
                if returns.cookies.is_empty() {
                    out.push_str("- none\n");
                }
                for cookie in &returns.cookies {
                    let _ = writeln!(
                        out,
                        "- {}={} ({})",
                        cookie.name,
                        cap_chars(&cookie.value, 60),
                        cookie.domain
                    );
                }
                Ok(cap(out, max_bytes))
            }
            StorageOp::SetCookie => {
                let name = name.ok_or_else(|| {
                    BrowserError::Input("storage set_cookie requires 'name'".to_owned())
                })?;
                let url = with_timeout(OP_META, "url", self.cdp.url())
                    .await
                    .unwrap_or_default();
                let params = SetCookieParams::builder()
                    .name(name.to_owned())
                    .value(value.unwrap_or_default().to_owned())
                    .url(url)
                    .build()
                    .map_err(BrowserError::Message)?;
                with_timeout(OP_META, "set_cookie", self.cdp.send(params)).await?;
                Ok(format!("cookie {name} set"))
            }
            StorageOp::GetLocal => {
                let js = match name {
                    Some(key) => format!("localStorage.getItem({})", json_literal(key)?),
                    None => "JSON.stringify(Object.keys(localStorage))".to_owned(),
                };
                let rendered = with_timeout(OP_EVAL, "get_local", self.cdp.eval(&js))
                    .await?
                    .map_or_else(|| "null".to_owned(), |value| value.to_string());
                Ok(cap(format!("local_storage: {rendered}"), max_bytes))
            }
            StorageOp::SetLocal => {
                let name = name.ok_or_else(|| {
                    BrowserError::Input("storage set_local requires 'name'".to_owned())
                })?;
                let js = format!(
                    "localStorage.setItem({}, {}); true",
                    json_literal(name)?,
                    json_literal(value.unwrap_or_default())?
                );
                with_timeout(OP_EVAL, "set_local", self.cdp.eval(&js)).await?;
                Ok(format!("local_storage[{name}] set"))
            }
        }
    }

    async fn tab(
        &mut self,
        op: TabOp,
        tab_id: Option<i64>,
        url: Option<&str>,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        match op {
            TabOp::List => {
                let tabs = self.tabs(BrowserCommand::TabList {}).await?;
                Ok(cap(render_tabs(&tabs), max_bytes))
            }
            TabOp::Switch => {
                let id = tab_id.ok_or_else(|| {
                    BrowserError::Input("tab switch requires 'tab_id'".to_owned())
                })?;
                self.tabs(BrowserCommand::TabSelect { id }).await?;
                self.reattach(id).await;
                self.snapshot("tab -> switched", "complete", false, max_bytes)
                    .await
            }
            TabOp::Close => {
                let id = tab_id
                    .ok_or_else(|| BrowserError::Input("tab close requires 'tab_id'".to_owned()))?;
                let tabs = self.tabs(BrowserCommand::TabClose { id }).await?;
                self.known_tabs.remove(&id);
                let Some(selected) = tabs.iter().find(|tab| tab.selected) else {
                    return Ok(cap(render_tabs(&tabs), max_bytes));
                };
                self.reattach(selected.id).await;
                self.snapshot("tab -> closed", "complete", false, max_bytes)
                    .await
            }
            TabOp::New => {
                let dest = match url {
                    Some(raw) => normalize_url(raw)?,
                    None => "about:blank".to_owned(),
                };
                let tabs = self.tabs(BrowserCommand::TabOpen { url: dest }).await?;
                let opened = tabs
                    .iter()
                    .find(|tab| tab.selected)
                    .ok_or_else(|| BrowserError::Message("the browser opened no tab".to_owned()))?;
                self.reattach(opened.id).await;
                let load = navigation::await_navigation_ready(&self.cdp).await;
                self.snapshot("tab -> new", load, false, max_bytes).await
            }
        }
    }

    async fn tabs(&mut self, command: BrowserCommand) -> Result<Vec<BrowserTab>, BrowserError> {
        let tabs = with_timeout(OP_META, "tabs", self.cdp.tabs(command)).await?;
        self.known_tabs = tabs.iter().map(|tab| tab.id).collect();
        Ok(tabs)
    }

    async fn reattach(&mut self, id: i64) {
        self.dialog.abort();
        self.observers.abort();
        let _ = with_timeout(OP_OPEN, "attach", self.cdp.attach()).await;
        self.dialog = DialogGuard::spawn(&self.cdp);
        self.observers = SessionObservers::spawn(&self.cdp);
        self.known_tabs.insert(id);
    }

    async fn find_ref(&self, reference: &str) -> Result<Element, BrowserError> {
        let selector = format!("[data-goat-ref='{reference}']");
        match with_timeout(OP_FIND, "find_ref", self.cdp.find_element(&selector)).await {
            Ok(element) => Ok(element),
            Err(err @ BrowserError::Timeout { .. }) => Err(err),
            Err(_) => Err(BrowserError::Input(format!(
                "ref {reference} not found; the page changed - take a new snapshot"
            ))),
        }
    }

    async fn settle_and_snapshot(
        &mut self,
        last_action: &str,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        let load = navigation::settle_after_action(&self.cdp).await;
        let switched = self.follow_new_tab().await;
        self.snapshot(last_action, load, switched, max_bytes).await
    }

    async fn snapshot(
        &mut self,
        last_action: &str,
        load: &str,
        switched: bool,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        let raw = with_timeout(OP_EVAL, "snapshot", run_snapshot(&self.cdp)).await?;
        let url = with_timeout(OP_META, "url", self.cdp.url())
            .await
            .unwrap_or_default();
        self.snapshot_seq = self.snapshot_seq.saturating_add(1);
        let snapshot_id = format!("s{}", self.snapshot_seq);
        self.current_snapshot_id = Some(snapshot_id.clone());
        let mut out = format_snapshot(
            &BrowserSnapshot {
                snapshot_id: &snapshot_id,
                url: &url,
                state: "usable",
                load,
                last_action: Some(last_action),
                switched,
                raw: &raw,
            },
            max_bytes.min(SNAPSHOT_MAX_BYTES),
        );
        self.append_notices(&mut out).await;
        Ok(out)
    }

    async fn append_notices(&self, out: &mut String) {
        let dialogs = self.dialog.drain().await;
        let error = self.observers.last_error_hint().await;
        if dialogs.is_empty() && error.is_none() {
            return;
        }
        out.push_str("\nnotices:\n");
        for entry in dialogs {
            let _ = writeln!(out, "- dialog_auto_handled: {entry}");
        }
        if let Some(error) = error {
            let _ = writeln!(out, "- {error}");
        }
    }

    async fn state_header(
        &self,
        last_action: &str,
        load: &str,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        let url = with_timeout(OP_META, "url", self.cdp.url())
            .await
            .unwrap_or_default();
        let title = with_timeout(OP_EVAL, "title", self.cdp.eval("document.title || ''"))
            .await
            .ok()
            .flatten()
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .unwrap_or_default();
        let snapshot_id = self.current_snapshot_id.as_deref().unwrap_or("none");
        let mut out = String::new();
        let _ = writeln!(out, "snapshot_id: {snapshot_id}");
        let _ = writeln!(out, "url: {url}");
        let _ = writeln!(out, "title: {title}");
        out.push_str("state: usable\n");
        let _ = writeln!(out, "load: {load}");
        let _ = writeln!(out, "\nlast_action: {last_action}");
        out.push_str("\nwarnings:\n- page_content_untrusted\n- refs_expire_after_next_snapshot\n");
        Ok(cap(out, max_bytes))
    }

    fn validate_snapshot(&self, reference: &BrowserRef) -> Result<(), BrowserError> {
        if let Some(expected) = &reference.snapshot_id
            && self.current_snapshot_id.as_ref() != Some(expected)
        {
            return Err(BrowserError::Input(format!(
                "stale ref {}:{}; current snapshot is {}",
                expected,
                reference.reference,
                self.current_snapshot_id.as_deref().unwrap_or("none")
            )));
        }
        Ok(())
    }

    async fn ready_state_complete(&self) -> Result<bool, BrowserError> {
        Ok(with_timeout(
            OP_EVAL,
            "ready_state",
            self.cdp.eval("document.readyState === 'complete'"),
        )
        .await?
        .as_ref()
        .and_then(Value::as_bool)
        .unwrap_or(false))
    }

    async fn follow_new_tab(&mut self) -> bool {
        let Ok(tabs) =
            with_timeout(OP_META, "tabs", self.cdp.tabs(BrowserCommand::TabList {})).await
        else {
            return false;
        };
        let opened: Vec<i64> = tabs
            .iter()
            .map(|tab| tab.id)
            .filter(|id| !self.known_tabs.contains(id))
            .collect();
        self.known_tabs = tabs.iter().map(|tab| tab.id).collect();
        let Some(newest) = opened.last().copied() else {
            return false;
        };
        if self
            .cdp
            .tabs(BrowserCommand::TabSelect { id: newest })
            .await
            .is_err()
        {
            return false;
        }
        self.reattach(newest).await;
        true
    }
}

async fn run_snapshot(cdp: &Cdp) -> Result<RawSnapshot, BrowserError> {
    let value = cdp
        .eval(SNAPSHOT_JS)
        .await?
        .ok_or_else(|| BrowserError::Message("the page returned no snapshot".to_owned()))?;
    serde_json::from_value(value)
        .map_err(|err| BrowserError::Message(format!("could not parse snapshot: {err}")))
}

async fn ensure_actionable(element: &Element, action: &str) -> Result<(), BrowserError> {
    let probed = with_timeout(
        OP_EVAL,
        "actionable",
        element.call_js_fn(
            "function() { const r = this.getBoundingClientRect(); const s = getComputedStyle(this); const tag = this.tagName.toLowerCase(); const type = (this.getAttribute('type') || '').toLowerCase(); const cx = r.left + r.width / 2; const cy = r.top + r.height / 2; let cover = null; if (r.width > 0 && r.height > 0) { const top = document.elementFromPoint(cx, cy); if (top && top !== this && !this.contains(top) && !top.contains(this)) { cover = (top.getAttribute('aria-label') || top.tagName.toLowerCase() + (top.id ? '#' + top.id : '')).slice(0, 60); } } return { visible: r.width > 0 && r.height > 0 && s.display !== 'none' && s.visibility !== 'hidden' && s.opacity !== '0', disabled: !!this.disabled || this.getAttribute('aria-disabled') === 'true', readonly: !!this.readOnly, editable: tag === 'textarea' || tag === 'select' || this.isContentEditable || (tag === 'input' && type !== 'button' && type !== 'submit' && type !== 'reset'), selectable: tag === 'select', cover: cover }; }",
        ),
    )
    .await?;
    let Some(value) = probed else {
        return Err(BrowserError::Input(format!(
            "cannot determine whether ref is actionable for {action}"
        )));
    };
    let flag = |key: &str| value.get(key).and_then(Value::as_bool).unwrap_or(false);
    let cover = value.get("cover").and_then(Value::as_str);
    if !flag("visible") {
        return Err(BrowserError::Input(format!(
            "ref is not visible for {action}; take a new snapshot or scroll"
        )));
    }
    if flag("disabled") {
        return Err(BrowserError::Input(format!(
            "ref is disabled for {action}; choose another element"
        )));
    }
    if action == "click"
        && let Some(cover) = cover
    {
        return Err(BrowserError::Input(format!(
            "click blocked: ref is covered by \"{cover}\"; dismiss it first, then take a new snapshot"
        )));
    }
    if action == "fill" && (!flag("editable") || flag("readonly")) {
        return Err(BrowserError::Input(
            "ref is not editable for fill; choose a textbox-like element".to_owned(),
        ));
    }
    if action == "select" && !flag("selectable") {
        return Err(BrowserError::Input(
            "ref is not a select element for select; choose a combobox".to_owned(),
        ));
    }
    Ok(())
}

fn render_tabs(tabs: &[BrowserTab]) -> String {
    let mut out = String::from("tabs:\n");
    if tabs.is_empty() {
        out.push_str("- none\n");
    }
    for tab in tabs {
        let mark = if tab.selected { " (attached)" } else { "" };
        let _ = writeln!(
            out,
            "- [tab_id={}] {} \"{}\"{mark}",
            tab.id,
            tab.url,
            cap_chars(&tab.title, 60)
        );
    }
    out
}

fn json_literal(text: &str) -> Result<String, BrowserError> {
    serde_json::to_string(text).map_err(|err| BrowserError::Message(err.to_string()))
}

fn normalize_url(url: &str) -> Result<String, BrowserError> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err(BrowserError::Input("url is empty".to_owned()));
    }
    for scheme in ["http://", "https://", "about:", "file://", "data:"] {
        if trimmed.starts_with(scheme) {
            return Ok(trimmed.to_owned());
        }
    }
    if trimmed.contains("://") {
        return Err(BrowserError::Input(format!(
            "unsupported url scheme in '{trimmed}'"
        )));
    }
    Ok(format!("https://{trimmed}"))
}

fn cap(mut text: String, max_bytes: usize) -> String {
    if text.len() > max_bytes {
        let boundary = text.floor_char_boundary(max_bytes);
        text.truncate(boundary);
        text.push_str("\n[output truncated]");
    }
    text
}

fn cap_chars(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

fn downscale(encoded: &str) -> Option<String> {
    let bytes = BASE64.decode(encoded).ok()?;
    let image = image::load_from_memory(&bytes).ok()?;
    if image.width() <= SCREENSHOT_MAX_DIM && image.height() <= SCREENSHOT_MAX_DIM {
        return None;
    }
    let scaled = image.resize(
        SCREENSHOT_MAX_DIM,
        SCREENSHOT_MAX_DIM,
        image::imageops::FilterType::Triangle,
    );
    let mut buffer = Cursor::new(Vec::new());
    scaled
        .write_to(&mut buffer, image::ImageFormat::Jpeg)
        .ok()?;
    Some(BASE64.encode(buffer.into_inner()))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use goat_api::BrowserTab;
    use serde_json::json;

    use super::{BrowserSession, normalize_url, open_session, render_tabs};
    use crate::action;
    use crate::transport::fake::{FakeBrowser, tab as fake_tab};

    const MAX: usize = 64 * 1024;

    const PAGE: &str = r#"{
        "title": "Sign in",
        "nodes": [
            { "depth": 0, "role": "heading", "name": "Sign in", "level": 1 },
            { "depth": 1, "role": "textbox", "name": "Email", "ref": "e1", "states": ["in_viewport"] },
            { "depth": 1, "role": "button", "name": "Continue", "ref": "e2", "states": ["in_viewport"] }
        ],
        "scrollY": 0,
        "viewportHeight": 800,
        "documentHeight": 800
    }"#;

    fn browser() -> Arc<FakeBrowser> {
        let fake = Arc::new(FakeBrowser::new());
        fake.returns(
            "refCount",
            &serde_json::from_str::<serde_json::Value>(PAGE).unwrap(),
        );
        fake.returns("document.readyState === ", &json!(true));
        fake.returns("document.readyState", &json!("complete"));
        fake.returns("location.href", &json!("https://start.test"));
        fake.handle("document.querySelector");
        fake.returns(
            "x: r.left",
            &json!({ "x": 10.0, "y": 20.0, "width": 100.0, "height": 40.0 }),
        );
        fake.returns(
            "elementFromPoint",
            &json!({
                "visible": true,
                "disabled": false,
                "readonly": false,
                "editable": true,
                "selectable": false,
                "cover": null
            }),
        );
        fake
    }

    async fn session(fake: &Arc<FakeBrowser>) -> BrowserSession {
        open_session(fake.clone()).await.unwrap()
    }

    async fn act(session: &mut BrowserSession, input: &str) -> String {
        session
            .dispatch(action::parse(input).unwrap(), MAX)
            .await
            .unwrap()
            .as_text()
            .expect("action returns text")
            .to_owned()
    }

    async fn act_err(session: &mut BrowserSession, input: &str) -> String {
        match session.dispatch(action::parse(input).unwrap(), MAX).await {
            Ok(_) => panic!("action should fail"),
            Err(err) => err.to_string(),
        }
    }

    #[tokio::test]
    async fn snapshot_scopes_refs_and_advances_the_id() {
        let fake = browser();
        let mut session = session(&fake).await;

        let first = act(&mut session, r#"{"action":"snapshot"}"#).await;
        assert!(first.starts_with("snapshot_id: s1\n"), "{first}");
        assert!(
            first.contains(r#"- textbox "Email" [ref=s1:e1] in_viewport"#),
            "{first}"
        );

        let second = act(&mut session, r#"{"action":"snapshot"}"#).await;
        assert!(second.contains("[ref=s2:e1]"), "{second}");
    }

    #[tokio::test]
    async fn a_stale_snapshot_scoped_ref_is_refused() {
        let fake = browser();
        let mut session = session(&fake).await;
        act(&mut session, r#"{"action":"snapshot"}"#).await;
        act(&mut session, r#"{"action":"snapshot"}"#).await;

        let err = act_err(&mut session, r#"{"action":"click","ref":"s1:e1"}"#).await;
        assert!(err.contains("stale ref s1:e1"), "{err}");
        assert!(err.contains("current snapshot is s2"), "{err}");
    }

    #[tokio::test]
    async fn a_covered_click_names_the_blocker() {
        let fake = browser();
        fake.returns(
            "elementFromPoint",
            &json!({
                "visible": true,
                "disabled": false,
                "readonly": false,
                "editable": false,
                "selectable": false,
                "cover": "div#consent"
            }),
        );
        let mut session = session(&fake).await;
        act(&mut session, r#"{"action":"snapshot"}"#).await;

        let err = act_err(&mut session, r#"{"action":"click","ref":"e2"}"#).await;
        assert!(err.contains("covered by \"div#consent\""), "{err}");
    }

    #[tokio::test]
    async fn fill_inserts_text_in_one_call() {
        let fake = browser();
        let mut session = session(&fake).await;
        act(&mut session, r#"{"action":"snapshot"}"#).await;
        act(
            &mut session,
            r#"{"action":"fill","ref":"e1","text":"me@example.test"}"#,
        )
        .await;

        let inserted = fake.sent("Input.insertText");
        assert_eq!(inserted.len(), 1);
        assert_eq!(inserted[0]["text"], json!("me@example.test"));
        assert!(
            fake.sent("Input.dispatchKeyEvent").is_empty(),
            "fill must not type character by character"
        );
    }

    #[tokio::test]
    async fn fill_with_submit_adds_one_enter() {
        let fake = browser();
        let mut session = session(&fake).await;
        act(&mut session, r#"{"action":"snapshot"}"#).await;
        act(
            &mut session,
            r#"{"action":"fill","ref":"e1","text":"hi","submit":true}"#,
        )
        .await;

        let keys = fake.sent("Input.dispatchKeyEvent");
        assert_eq!(keys.len(), 2, "one keydown and one keyup");
        assert_eq!(keys[0]["key"], json!("Enter"));
    }

    #[tokio::test]
    async fn a_dialog_is_dismissed_and_reported_once() {
        let fake = browser();
        let mut session = session(&fake).await;
        fake.emit(
            "Page.javascriptDialogOpening",
            json!({ "url": "https://start.test", "frameId": "F1", "message": "boom", "type": "alert", "hasBrowserHandler": false }),
        );
        for _ in 0..64 {
            if !fake.sent("Page.handleJavaScriptDialog").is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }

        let first = act(&mut session, r#"{"action":"snapshot"}"#).await;
        assert!(
            first.contains("dialog_auto_handled: alert auto-dismissed: \"boom\""),
            "{first}"
        );
        assert_eq!(
            fake.sent("Page.handleJavaScriptDialog")[0]["accept"],
            json!(false)
        );

        let second = act(&mut session, r#"{"action":"snapshot"}"#).await;
        assert!(!second.contains("dialog_auto_handled"), "{second}");
    }

    #[tokio::test]
    async fn a_new_tab_is_followed_after_a_click() {
        let fake = browser();
        let mut session = session(&fake).await;
        act(&mut session, r#"{"action":"snapshot"}"#).await;
        fake.set_tabs(vec![
            fake_tab(1, "https://start.test", true),
            fake_tab(2, "https://popup.test", false),
        ]);

        let out = act(&mut session, r#"{"action":"click","ref":"e2"}"#).await;
        assert!(out.contains("tabs: switched_to_new_tab"), "{out}");
    }

    #[tokio::test]
    async fn a_preexisting_tab_is_not_mistaken_for_a_new_one() {
        let fake = browser();
        fake.set_tabs(vec![
            fake_tab(1, "https://start.test", true),
            fake_tab(2, "https://mail.test", false),
        ]);
        let mut session = session(&fake).await;

        let out = act(&mut session, r#"{"action":"click","ref":"e2"}"#).await;
        assert!(!out.contains("switched_to_new_tab"), "{out}");
    }

    #[tokio::test]
    async fn tab_list_names_tabs_by_id() {
        let fake = browser();
        fake.set_tabs(vec![
            fake_tab(4, "https://a.test", true),
            fake_tab(9, "https://b.test", false),
        ]);
        let mut session = session(&fake).await;

        let out = act(&mut session, r#"{"action":"tab","op":"list"}"#).await;
        assert!(out.contains("- [tab_id=4] https://a.test"), "{out}");
        assert!(out.contains("- [tab_id=9] https://b.test"), "{out}");
    }

    #[tokio::test]
    async fn closing_a_session_detaches_rather_than_closing_the_tab() {
        let fake = browser();
        let mut slot = Some(session(&fake).await);
        assert_eq!(super::close(&mut slot).await, "browser released");
        assert!(
            !fake.methods().iter().any(|m| m == "Target.closeTarget"),
            "the tab belongs to the user"
        );
    }

    fn tab(id: i64, url: &str, selected: bool) -> BrowserTab {
        BrowserTab {
            id,
            url: url.to_owned(),
            title: "Title".to_owned(),
            selected,
        }
    }

    #[test]
    fn adds_https_scheme() {
        assert_eq!(normalize_url("example.com").unwrap(), "https://example.com");
    }

    #[test]
    fn preserves_known_schemes() {
        assert_eq!(normalize_url("http://x.com/a").unwrap(), "http://x.com/a");
        assert_eq!(normalize_url("about:blank").unwrap(), "about:blank");
    }

    #[test]
    fn rejects_unknown_scheme() {
        assert!(normalize_url("ftp://x.com").is_err());
    }

    #[test]
    fn rejects_empty() {
        assert!(normalize_url("   ").is_err());
    }

    #[test]
    fn marks_the_attached_tab() {
        let out = render_tabs(&[
            tab(7, "https://a.test", false),
            tab(9, "https://b.test", true),
        ]);
        assert!(out.contains("- [tab_id=7] https://a.test \"Title\"\n"));
        assert!(out.contains("- [tab_id=9] https://b.test \"Title\" (attached)\n"));
    }

    #[test]
    fn renders_no_tabs() {
        assert_eq!(render_tabs(&[]), "tabs:\n- none\n");
    }
}
