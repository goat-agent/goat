use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::Cursor;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use chromiumoxide::cdp::browser_protocol::dom::SetFileInputFilesParams;
use chromiumoxide::cdp::browser_protocol::input::{
    DispatchMouseEventParams, DispatchMouseEventType, MouseButton,
};
use chromiumoxide::cdp::browser_protocol::network::{GetCookiesParams, SetCookieParams};
use chromiumoxide::cdp::browser_protocol::page::{CaptureScreenshotFormat, StopLoadingParams};
use chromiumoxide::cdp::browser_protocol::target::CloseTargetParams;
use chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;
use chromiumoxide::error::CdpError;
use chromiumoxide::handler::viewport::Viewport;
use chromiumoxide::page::ScreenshotParams;
use chromiumoxide::{Browser, BrowserConfig, Element, Page};
use goat_tool::{ToolImage, ToolOutput};
use tokio::sync::{Mutex, OnceCell};
use tokio::task::JoinHandle;
use tokio::time::sleep;

use crate::action::{Action, BrowserRef, ScrollDirection, StorageOp, TabOp};
use crate::dialog::DialogGuard;
use crate::error::BrowserError;
use crate::navigation;
use crate::observe::SessionObservers;
use crate::resilience::{
    OP_CLICK, OP_EVAL, OP_FILL, OP_FIND, OP_HEALTH, OP_META, OP_NAV_ACK, OP_OPEN, OP_SCREENSHOT,
    with_timeout,
};
use crate::snapshot::{BrowserSnapshot, RawSnapshot, SNAPSHOT_JS, format_snapshot};

const LAUNCH_TIMEOUT: Duration = Duration::from_secs(30);
const LAUNCH_HARD_CAP: Duration = Duration::from_secs(35);
const CMD_TIMEOUT: Duration = Duration::from_secs(30);
const SNAPSHOT_MAX_BYTES: usize = 32 * 1024;
const SCREENSHOT_MAX_DIM: u32 = 1280;
const DEFAULT_TEXT_MAX_BYTES: usize = 8 * 1024;
const HANDLER_MAX_CONSECUTIVE_ERRORS: u32 = 64;

const LAUNCH_ARGS: [&str; 21] = [
    "disable-background-networking",
    "enable-features=NetworkService,NetworkServiceInProcess",
    "disable-background-timer-throttling",
    "disable-backgrounding-occluded-windows",
    "disable-breakpad",
    "disable-client-side-phishing-detection",
    "disable-component-extensions-with-background-pages",
    "disable-default-apps",
    "disable-dev-shm-usage",
    "disable-features=TranslateUI",
    "disable-hang-monitor",
    "disable-ipc-flooding-protection",
    "disable-popup-blocking",
    "disable-prompt-on-repost",
    "disable-renderer-backgrounding",
    "disable-sync",
    "force-color-profile=srgb",
    "metrics-recording-only",
    "no-first-run",
    "no-default-browser-check",
    "disable-blink-features=AutomationControlled",
];

pub type SessionHandle = Arc<Mutex<Option<BrowserSession>>>;

pub fn new_handle() -> SessionHandle {
    Arc::new(Mutex::new(None))
}

struct SharedBrowser {
    browser: Browser,
    handler_task: JoinHandle<()>,
}

static SHARED: OnceCell<Mutex<Option<Arc<SharedBrowser>>>> = OnceCell::const_new();

async fn shared_cell() -> &'static Mutex<Option<Arc<SharedBrowser>>> {
    SHARED.get_or_init(|| async { Mutex::new(None) }).await
}

async fn shared_browser() -> Result<Arc<SharedBrowser>, BrowserError> {
    let cell = shared_cell().await;
    let mut guard = cell.lock().await;
    if let Some(shared) = guard.as_ref()
        && !shared.handler_task.is_finished()
    {
        return Ok(shared.clone());
    }
    if let Some(old) = guard.take() {
        old.handler_task.abort();
    }
    let shared = Arc::new(launch_shared().await?);
    *guard = Some(shared.clone());
    Ok(shared)
}

async fn launch_shared() -> Result<SharedBrowser, BrowserError> {
    match launch_shared_once().await {
        Ok(shared) => Ok(shared),
        Err(first) => {
            clear_singleton_locks();
            launch_shared_once().await.map_err(|_| first)
        }
    }
}

fn clear_singleton_locks() {
    let Some(profile) = goat_config::browser_profile_dir() else {
        return;
    };
    for name in ["SingletonLock", "SingletonSocket", "SingletonCookie"] {
        let _ = std::fs::remove_file(profile.join(name));
    }
}

async fn launch_shared_once() -> Result<SharedBrowser, BrowserError> {
    let profile = goat_config::browser_profile_dir().ok_or(BrowserError::NoProfile)?;
    std::fs::create_dir_all(&profile).map_err(|err| {
        BrowserError::Message(format!("could not create browser profile dir: {err}"))
    })?;

    let config = BrowserConfig::builder()
        .with_head()
        .user_data_dir(profile)
        .viewport(None::<Viewport>)
        .launch_timeout(LAUNCH_TIMEOUT)
        .request_timeout(CMD_TIMEOUT)
        .disable_default_args()
        .surface_invalid_messages()
        .args(LAUNCH_ARGS)
        .build()
        .map_err(BrowserError::NoChrome)?;

    let mut launch = tokio::spawn(async move {
        let (browser, mut handler) = Browser::launch(config).await?;
        let handler_task = tokio::spawn(async move {
            let mut consecutive_errors: u32 = 0;
            while let Some(event) = futures::StreamExt::next(&mut handler).await {
                if let Err(err) = event {
                    consecutive_errors += 1;
                    tracing::debug!(%err, consecutive_errors, "browser handler event error");
                    if handler_should_stop(consecutive_errors) {
                        tracing::warn!(
                            consecutive_errors,
                            "browser handler stopping after sustained errors; chrome will relaunch on next use"
                        );
                        break;
                    }
                } else {
                    consecutive_errors = 0;
                }
            }
        });
        Ok::<_, CdpError>(SharedBrowser {
            browser,
            handler_task,
        })
    });

    let Ok(joined) = tokio::time::timeout(LAUNCH_HARD_CAP, &mut launch).await else {
        launch.abort();
        return Err(BrowserError::Message(
            "Chrome did not start within the launch timeout".to_owned(),
        ));
    };
    joined
        .map_err(|err| BrowserError::Message(format!("browser launch task failed: {err}")))?
        .map_err(map_launch_err)
}

fn handler_should_stop(consecutive_errors: u32) -> bool {
    consecutive_errors >= HANDLER_MAX_CONSECUTIVE_ERRORS
}

pub struct BrowserSession {
    shared: Arc<SharedBrowser>,
    page: Page,
    dialog: DialogGuard,
    observers: SessionObservers,
    known_targets: HashSet<String>,
    snapshot_seq: u64,
    current_snapshot_id: Option<String>,
}

pub async fn ensure_session(
    slot: &mut Option<BrowserSession>,
) -> Result<&mut BrowserSession, BrowserError> {
    let mut healthy = false;
    if let Some(session) = slot.as_ref() {
        healthy = session.is_healthy().await;
    }
    if !healthy {
        if let Some(old) = slot.take() {
            old.dispose().await;
        }
        *slot = Some(open_session().await?);
    }
    slot.as_mut()
        .ok_or_else(|| BrowserError::Message("browser session unavailable".to_owned()))
}

async fn open_session() -> Result<BrowserSession, BrowserError> {
    if let Ok(session) = open_session_once().await {
        return Ok(session);
    }
    invalidate_shared().await;
    open_session_once().await
}

async fn invalidate_shared() {
    let cell = shared_cell().await;
    let mut guard = cell.lock().await;
    if let Some(old) = guard.take() {
        old.handler_task.abort();
    }
}

async fn open_session_once() -> Result<BrowserSession, BrowserError> {
    let shared = shared_browser().await?;
    let page = with_timeout(OP_OPEN, "open_page", shared.browser.new_page("about:blank"))
        .await
        .map_err(|err| match err {
            BrowserError::Timeout { .. } => BrowserError::Message(
                "Chrome was unresponsive opening a page; it will relaunch on the next action"
                    .to_owned(),
            ),
            other => other,
        })?;
    let _ = with_timeout(OP_META, "bring_to_front", page.bring_to_front()).await;
    let dialog = DialogGuard::spawn(&page).await;
    let observers = SessionObservers::spawn(&page).await;
    let mut known_targets = HashSet::new();
    known_targets.insert(page.target_id().inner().clone());
    Ok(BrowserSession {
        shared,
        page,
        dialog,
        observers,
        known_targets,
        snapshot_seq: 0,
        current_snapshot_id: None,
    })
}

fn map_launch_err(err: CdpError) -> BrowserError {
    match err {
        CdpError::LaunchExit(..) => BrowserError::Message(
            "Chrome exited during launch; the profile at ~/.goat/browser/profile may be locked by another goat instance".to_owned(),
        ),
        CdpError::LaunchTimeout(_) => {
            BrowserError::Message("Chrome did not start within the launch timeout".to_owned())
        }
        other => BrowserError::Message(format!("could not launch Chrome: {other}")),
    }
}

pub async fn close(slot: &mut Option<BrowserSession>) -> String {
    let Some(session) = slot.take() else {
        return "browser is not running".to_owned();
    };
    session.dispose().await;
    "browser closed".to_owned()
}

impl BrowserSession {
    async fn dispose(self) {
        self.dialog.abort();
        self.observers.abort();
        let _ = with_timeout(
            OP_META,
            "close_target",
            self.page
                .execute(CloseTargetParams::new(self.page.target_id().clone())),
        )
        .await;
    }

    async fn is_healthy(&self) -> bool {
        if self.shared.handler_task.is_finished() {
            return false;
        }
        with_timeout(OP_HEALTH, "health", self.page.evaluate("1"))
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
            Action::Tab { op, index, url } => {
                ToolOutput::text(self.tab(op, index, url.as_deref(), max_bytes).await?)
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
            Action::Close => ToolOutput::text("browser closed".to_owned()),
        };
        Ok(output)
    }

    async fn navigate(&mut self, url: &str, max_bytes: usize) -> Result<String, BrowserError> {
        let target = normalize_url(url)?;
        let acked = with_timeout(OP_NAV_ACK, "navigate", self.page.goto(target))
            .await
            .is_ok();
        let load = if acked {
            navigation::await_navigation_ready(&self.page).await
        } else {
            let _ = with_timeout(
                OP_META,
                "stop_loading",
                self.page.execute(StopLoadingParams::default()),
            )
            .await;
            "nav_error"
        };
        let switched = self.follow_new_tab().await;
        let _ = with_timeout(OP_META, "bring_to_front", self.page.bring_to_front()).await;
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
                false,
            ),
        )
        .await;
        with_timeout(OP_FILL, "type", element.type_str(text)).await?;
        if submit {
            with_timeout(OP_FILL, "submit", element.press_key("Enter")).await?;
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
        let literal =
            serde_json::to_string(value).map_err(|err| BrowserError::Message(err.to_string()))?;
        let declaration = format!(
            "function() {{ const v = {literal}; for (let i = 0; i < this.options.length; i++) {{ const o = this.options[i]; if (o.value === v || o.text.trim() === v) {{ this.selectedIndex = i; this.dispatchEvent(new Event('input', {{ bubbles: true }})); this.dispatchEvent(new Event('change', {{ bubbles: true }})); return true; }} }} return false; }}"
        );
        let returns =
            with_timeout(OP_EVAL, "select", element.call_js_fn(declaration, false)).await?;
        let matched = returns
            .result
            .value
            .as_ref()
            .and_then(serde_json::Value::as_bool)
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
        let element = match with_timeout(OP_FIND, "focus", self.page.find_element(":focus")).await {
            Ok(element) => element,
            Err(_) => with_timeout(OP_FIND, "body", self.page.find_element("body")).await?,
        };
        with_timeout(OP_FILL, "press_key", element.press_key(key)).await?;
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
        let _ = with_timeout(OP_EVAL, "scroll", self.page.evaluate(js.as_str())).await?;
        self.snapshot("scroll -> changed", "complete", false, max_bytes)
            .await
    }

    async fn history(&mut self, delta: i32, max_bytes: usize) -> Result<String, BrowserError> {
        let js = format!("history.go({delta}); true");
        let _ = with_timeout(OP_EVAL, "history", self.page.evaluate(js.as_str())).await?;
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
        let literal =
            serde_json::to_string(query).map_err(|err| BrowserError::Message(err.to_string()))?;
        let js = format!(
            "(() => {{ const q = {literal}.toLowerCase(); const walker = document.createTreeWalker(document.body || document.documentElement, NodeFilter.SHOW_TEXT); const out = []; while (walker.nextNode() && out.length < 20) {{ const t = walker.currentNode.textContent.trim().replace(/\\s+/g, ' '); if (t.toLowerCase().includes(q)) out.push(t.slice(0, 240)); }} return out; }})()"
        );
        let result = with_timeout(OP_EVAL, "find_text", self.page.evaluate(js.as_str())).await?;
        let mut out = self
            .state_header("find_text -> complete", "complete", max_bytes)
            .await?;
        out.push_str("\nuntrusted_text_matches:\n");
        if let Some(items) = result.value().and_then(serde_json::Value::as_array) {
            if items.is_empty() {
                out.push_str("- none\n");
            } else {
                for item in items {
                    if let Some(text) = item.as_str() {
                        let _ = writeln!(out, "- \"{}\"", cap_chars(text, max_chars.min(240)));
                    }
                }
            }
        } else {
            out.push_str("- none\n");
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
        let returns = with_timeout(
            OP_EVAL,
            "inspect",
            element.call_js_fn(
                "function() { return { role: this.getAttribute('role') || this.tagName.toLowerCase(), text: (this.innerText || this.value || '').trim().replace(/\\s+/g, ' ').slice(0, 4000), disabled: !!this.disabled, readonly: !!this.readOnly }; }",
                false,
            ),
        )
        .await?;
        let mut out = self
            .state_header("inspect -> complete", "complete", max_bytes)
            .await?;
        out.push_str("\nuntrusted_region:\n");
        if let Some(value) = returns.result.value {
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
        let result = with_timeout(OP_EVAL, "read_viewport", self.page.evaluate(js)).await?;
        let mut out = self
            .state_header("read_viewport -> complete", "complete", max_bytes)
            .await?;
        out.push_str("\nuntrusted_viewport_text:\n");
        if let Some(value) = result.value().and_then(serde_json::Value::as_str) {
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
                let literal = serde_json::to_string(expected)
                    .map_err(|err| BrowserError::Message(err.to_string()))?;
                let js = format!("document.body && document.body.innerText.includes({literal})");
                if with_timeout(OP_EVAL, "wait_for", self.page.evaluate(js.as_str()))
                    .await?
                    .value()
                    .and_then(serde_json::Value::as_bool)
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
        let result = with_timeout(OP_EVAL, "debug_eval", self.page.evaluate(js)).await?;
        let rendered = match result.value() {
            Some(value) => {
                serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
            }
            None => "undefined".to_owned(),
        };
        Ok(goat_tool::truncate(rendered, max_bytes))
    }

    async fn screenshot(&self) -> Result<ToolImage, BrowserError> {
        let params = ScreenshotParams::builder()
            .format(CaptureScreenshotFormat::Png)
            .full_page(false)
            .build();
        let bytes = with_timeout(OP_SCREENSHOT, "screenshot", self.page.screenshot(params)).await?;
        let encoded = downscale_png(&bytes).unwrap_or(bytes);
        Ok(ToolImage {
            media_type: "image/png".to_owned(),
            data: BASE64.encode(&encoded),
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
        let start = with_timeout(OP_META, "box", from_el.bounding_box()).await?;
        let end = with_timeout(OP_META, "box", to_el.bounding_box()).await?;
        let (sx, sy) = (start.x + start.width / 2.0, start.y + start.height / 2.0);
        let (ex, ey) = (end.x + end.width / 2.0, end.y + end.height / 2.0);
        self.dispatch_mouse(DispatchMouseEventType::MouseMoved, sx, sy, false)
            .await?;
        self.dispatch_mouse(DispatchMouseEventType::MousePressed, sx, sy, true)
            .await?;
        self.dispatch_mouse(DispatchMouseEventType::MouseMoved, ex, ey, false)
            .await?;
        self.dispatch_mouse(DispatchMouseEventType::MouseReleased, ex, ey, true)
            .await?;
        self.settle_and_snapshot("drag -> changed", max_bytes).await
    }

    async fn dispatch_mouse(
        &self,
        kind: DispatchMouseEventType,
        x: f64,
        y: f64,
        with_button: bool,
    ) -> Result<(), BrowserError> {
        let mut builder = DispatchMouseEventParams::builder().r#type(kind).x(x).y(y);
        if with_button {
            builder = builder.button(MouseButton::Left).click_count(1);
        }
        let params = builder.build().map_err(BrowserError::Message)?;
        with_timeout(OP_META, "mouse", self.page.execute(params)).await?;
        Ok(())
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
            .backend_node_id(element.backend_node_id)
            .build()
            .map_err(BrowserError::Message)?;
        with_timeout(OP_FILL, "upload", self.page.execute(params)).await?;
        self.settle_and_snapshot("upload -> changed", max_bytes)
            .await
    }

    async fn read_content(
        &self,
        max_chars: usize,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        let js = "(() => { const p = document.querySelector('article, main, [role=\"main\"]') || document.body; return p ? (p.innerText || '').trim().replace(/\\n{3,}/g, '\\n\\n') : ''; })()";
        let result = with_timeout(OP_EVAL, "read_content", self.page.evaluate(js)).await?;
        let mut out = self
            .state_header("read_content -> complete", "complete", max_bytes)
            .await?;
        out.push_str("\nmain_content:\n");
        match result.value().and_then(serde_json::Value::as_str) {
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
                let ret = with_timeout(
                    OP_META,
                    "cookies",
                    self.page.execute(GetCookiesParams::default()),
                )
                .await?;
                let mut out = String::from("cookies:\n");
                if ret.result.cookies.is_empty() {
                    out.push_str("- none\n");
                }
                for cookie in &ret.result.cookies {
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
                let value = value.unwrap_or_default();
                let url = with_timeout(OP_META, "url", self.page.url())
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                let params = SetCookieParams::builder()
                    .name(name.to_owned())
                    .value(value.to_owned())
                    .url(url)
                    .build()
                    .map_err(BrowserError::Message)?;
                with_timeout(OP_META, "set_cookie", self.page.execute(params)).await?;
                Ok(format!("cookie {name} set"))
            }
            StorageOp::GetLocal => {
                let js = match name {
                    Some(key) => format!(
                        "localStorage.getItem({})",
                        serde_json::to_string(key)
                            .map_err(|err| BrowserError::Message(err.to_string()))?
                    ),
                    None => "JSON.stringify(Object.keys(localStorage))".to_owned(),
                };
                let result =
                    with_timeout(OP_EVAL, "get_local", self.page.evaluate(js.as_str())).await?;
                let rendered = result
                    .value()
                    .map_or_else(|| "null".to_owned(), ToString::to_string);
                Ok(cap(format!("local_storage: {rendered}"), max_bytes))
            }
            StorageOp::SetLocal => {
                let name = name.ok_or_else(|| {
                    BrowserError::Input("storage set_local requires 'name'".to_owned())
                })?;
                let value = value.unwrap_or_default();
                let js = format!(
                    "localStorage.setItem({}, {}); true",
                    serde_json::to_string(name)
                        .map_err(|err| BrowserError::Message(err.to_string()))?,
                    serde_json::to_string(value)
                        .map_err(|err| BrowserError::Message(err.to_string()))?
                );
                let _ = with_timeout(OP_EVAL, "set_local", self.page.evaluate(js.as_str())).await?;
                Ok(format!("local_storage[{name}] set"))
            }
        }
    }

    async fn tab(
        &mut self,
        op: TabOp,
        index: Option<usize>,
        url: Option<&str>,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        match op {
            TabOp::List => {
                let pages = with_timeout(OP_META, "pages", self.shared.browser.pages()).await?;
                let current = self.page.target_id().inner().clone();
                let mut out = String::from("tabs:\n");
                for (i, page) in pages.iter().enumerate() {
                    let page_url = with_timeout(OP_META, "url", page.url())
                        .await
                        .ok()
                        .flatten()
                        .unwrap_or_default();
                    let mark = if page.target_id().inner() == &current {
                        " (current)"
                    } else {
                        ""
                    };
                    let _ = writeln!(out, "- [{i}] {page_url}{mark}");
                }
                Ok(cap(out, max_bytes))
            }
            TabOp::Switch => {
                let index = index
                    .ok_or_else(|| BrowserError::Input("tab switch requires 'index'".to_owned()))?;
                let pages = with_timeout(OP_META, "pages", self.shared.browser.pages()).await?;
                let page = pages.into_iter().nth(index).ok_or_else(|| {
                    BrowserError::Input(format!("tab index {index} out of range"))
                })?;
                self.set_active_page(page).await;
                self.snapshot("tab -> switched", "complete", false, max_bytes)
                    .await
            }
            TabOp::Close => {
                let index = index
                    .ok_or_else(|| BrowserError::Input("tab close requires 'index'".to_owned()))?;
                let pages = with_timeout(OP_META, "pages", self.shared.browser.pages()).await?;
                let current = self.page.target_id().inner().clone();
                let page = pages.into_iter().nth(index).ok_or_else(|| {
                    BrowserError::Input(format!("tab index {index} out of range"))
                })?;
                let was_current = page.target_id().inner() == &current;
                let _ = page.close().await;
                if was_current {
                    let remaining =
                        with_timeout(OP_META, "pages", self.shared.browser.pages()).await?;
                    if let Some(next) = remaining.into_iter().next() {
                        self.set_active_page(next).await;
                        return self
                            .snapshot("tab -> closed", "complete", false, max_bytes)
                            .await;
                    }
                }
                Ok("tab closed".to_owned())
            }
            TabOp::New => {
                let dest = match url {
                    Some(raw) => normalize_url(raw)?,
                    None => "about:blank".to_owned(),
                };
                let page = self
                    .shared
                    .browser
                    .new_page(dest)
                    .await
                    .map_err(map_launch_err)?;
                self.set_active_page(page).await;
                let load = navigation::await_navigation_ready(&self.page).await;
                self.snapshot("tab -> new", load, false, max_bytes).await
            }
        }
    }

    async fn set_active_page(&mut self, page: Page) {
        self.dialog.abort();
        self.observers.abort();
        self.dialog = DialogGuard::spawn(&page).await;
        self.observers = SessionObservers::spawn(&page).await;
        let _ = with_timeout(OP_META, "bring_to_front", page.bring_to_front()).await;
        self.known_targets.insert(page.target_id().inner().clone());
        self.page = page;
    }

    async fn find_ref(&self, reference: &str) -> Result<Element, BrowserError> {
        let selector = format!("[data-goat-ref='{reference}']");
        match with_timeout(OP_FIND, "find_ref", self.page.find_element(selector)).await {
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
        let load = navigation::settle_after_action(&self.page).await;
        let switched = self.follow_new_tab().await;
        let _ = with_timeout(OP_META, "bring_to_front", self.page.bring_to_front()).await;
        self.snapshot(last_action, load, switched, max_bytes).await
    }

    async fn snapshot(
        &mut self,
        last_action: &str,
        load: &str,
        switched: bool,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        let raw = with_timeout(OP_EVAL, "snapshot", run_snapshot(&self.page)).await?;
        let url = with_timeout(OP_META, "url", self.page.url())
            .await
            .ok()
            .flatten()
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
                profile: "persistent",
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
        let url = with_timeout(OP_META, "url", self.page.url())
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        let title = with_timeout(OP_EVAL, "title", self.page.evaluate("document.title || ''"))
            .await
            .ok()
            .and_then(|result| {
                result
                    .value()
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .unwrap_or_default();
        let snapshot_id = self.current_snapshot_id.as_deref().unwrap_or("none");
        let mut out = String::new();
        let _ = writeln!(out, "snapshot_id: {snapshot_id}");
        let _ = writeln!(out, "url: {url}");
        let _ = writeln!(out, "title: {title}");
        out.push_str("state: usable\n");
        let _ = writeln!(out, "load: {load}");
        out.push_str("profile: persistent\n");
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
            self.page.evaluate("document.readyState === 'complete'"),
        )
        .await?
        .value()
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false))
    }

    async fn follow_new_tab(&mut self) -> bool {
        let Ok(pages) = with_timeout(OP_META, "pages", self.shared.browser.pages()).await else {
            return false;
        };
        let mut newest: Option<Page> = None;
        for page in pages {
            let target = page.target_id().inner().clone();
            if self.known_targets.insert(target) {
                newest = Some(page);
            }
        }
        if let Some(page) = newest {
            self.set_active_page(page).await;
            return true;
        }
        false
    }
}

async fn run_snapshot(page: &Page) -> Result<RawSnapshot, BrowserError> {
    let params = EvaluateParams::builder()
        .expression(SNAPSHOT_JS)
        .return_by_value(true)
        .build()
        .map_err(|err| BrowserError::Message(format!("snapshot eval build: {err}")))?;
    let result = page.evaluate(params).await?;
    result
        .into_value()
        .map_err(|err| BrowserError::Message(format!("could not parse snapshot: {err}")))
}

async fn ensure_actionable(element: &Element, action: &str) -> Result<(), BrowserError> {
    let returns = with_timeout(
        OP_EVAL,
        "actionable",
        element.call_js_fn(
            "function() { const r = this.getBoundingClientRect(); const s = getComputedStyle(this); const tag = this.tagName.toLowerCase(); const type = (this.getAttribute('type') || '').toLowerCase(); const cx = r.left + r.width / 2; const cy = r.top + r.height / 2; let cover = null; if (r.width > 0 && r.height > 0) { const top = document.elementFromPoint(cx, cy); if (top && top !== this && !this.contains(top) && !top.contains(this)) { cover = (top.getAttribute('aria-label') || top.tagName.toLowerCase() + (top.id ? '#' + top.id : '')).slice(0, 60); } } return { visible: r.width > 0 && r.height > 0 && s.display !== 'none' && s.visibility !== 'hidden' && s.opacity !== '0', disabled: !!this.disabled || this.getAttribute('aria-disabled') === 'true', readonly: !!this.readOnly, editable: tag === 'textarea' || tag === 'select' || this.isContentEditable || (tag === 'input' && type !== 'button' && type !== 'submit' && type !== 'reset'), selectable: tag === 'select', cover: cover }; }",
            false,
        ),
    )
    .await?;
    let Some(value) = returns.result.value.as_ref() else {
        return Err(BrowserError::Input(format!(
            "cannot determine whether ref is actionable for {action}"
        )));
    };
    let visible = value
        .get("visible")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let disabled = value
        .get("disabled")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let readonly = value
        .get("readonly")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let editable = value
        .get("editable")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let selectable = value
        .get("selectable")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let cover = value.get("cover").and_then(serde_json::Value::as_str);
    if !visible {
        return Err(BrowserError::Input(format!(
            "ref is not visible for {action}; take a new snapshot or scroll"
        )));
    }
    if disabled {
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
    if action == "fill" && (!editable || readonly) {
        return Err(BrowserError::Input(
            "ref is not editable for fill; choose a textbox-like element".to_owned(),
        ));
    }
    if action == "select" && !selectable {
        return Err(BrowserError::Input(
            "ref is not a select element for select; choose a combobox".to_owned(),
        ));
    }
    Ok(())
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

fn downscale_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let image = image::load_from_memory(bytes).ok()?;
    if image.width() <= SCREENSHOT_MAX_DIM && image.height() <= SCREENSHOT_MAX_DIM {
        return None;
    }
    let scaled = image.resize(
        SCREENSHOT_MAX_DIM,
        SCREENSHOT_MAX_DIM,
        image::imageops::FilterType::Triangle,
    );
    let mut buffer = Cursor::new(Vec::new());
    scaled.write_to(&mut buffer, image::ImageFormat::Png).ok()?;
    Some(buffer.into_inner())
}

#[cfg(test)]
mod tests {
    use super::{HANDLER_MAX_CONSECUTIVE_ERRORS, handler_should_stop, normalize_url};

    #[test]
    fn handler_keeps_running_below_threshold() {
        assert!(!handler_should_stop(0));
        assert!(!handler_should_stop(1));
        assert!(!handler_should_stop(HANDLER_MAX_CONSECUTIVE_ERRORS - 1));
    }

    #[test]
    fn handler_stops_at_threshold() {
        assert!(handler_should_stop(HANDLER_MAX_CONSECUTIVE_ERRORS));
        assert!(handler_should_stop(HANDLER_MAX_CONSECUTIVE_ERRORS + 1));
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
}
