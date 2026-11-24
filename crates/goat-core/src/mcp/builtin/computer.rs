mod desktop;
#[cfg(test)]
mod fake;
#[cfg(target_os = "macos")]
#[expect(
    unsafe_code,
    reason = "macOS accessibility, input and window APIs are C and Objective-C"
)]
mod macos;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rmcp::ErrorData;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ElicitRequestParams,
    ElicitationAction, Implementation, InitializeResult, ListToolsResult, PaginatedRequestParams,
    ServerCapabilities, Tool,
};
use rmcp::service::{Peer, RequestContext, RoleServer};
use serde_json::{Map, Value, json};

use crate::store::Logins;

use self::desktop::{
    App, Button, Capture, Chord, Desktop, Direction, Fault, Key, Opening, Platform, Pointer, Rect,
    WindowState,
};

const TOOLS: &str = include_str!("computer.json");
const GRANT: &str = "grant:computer:";
const SETTLE: Duration = Duration::from_millis(500);
const POLL: Duration = Duration::from_millis(50);
const WAIT_POLL: Duration = Duration::from_millis(200);
const WAIT_DEFAULT_MS: u64 = 10_000;
const WAIT_MOST_MS: u64 = 60_000;
const SCROLL_DEFAULT: u32 = 3;
const LOCK: &str = "computer.lock";

#[derive(Debug)]
pub(crate) struct Computer {
    desktop: Desktop,
    logins: Arc<Logins>,
    lock_path: PathBuf,
    held: Mutex<Option<std::fs::File>>,
    allowed: Mutex<HashSet<String>>,
    shots: Mutex<HashMap<String, Shot>>,
    tools: Arc<[Tool]>,
}

#[derive(Debug, Clone, Copy)]
struct Shot {
    origin_x: f64,
    origin_y: f64,
    scale: f64,
}

struct Args(Map<String, Value>);

impl Computer {
    pub(crate) fn new(desktop: Desktop, logins: Arc<Logins>, goat_dir: &std::path::Path) -> Self {
        Self {
            desktop,
            logins,
            lock_path: goat_dir.join(LOCK),
            held: Mutex::new(None),
            allowed: Mutex::new(HashSet::new()),
            shots: Mutex::new(HashMap::new()),
            tools: tools().into(),
        }
    }

    async fn run(&self, name: &str, args: &Args, peer: &Peer<RoleServer>) -> Result<Value, Fault> {
        match name {
            "list_windows" => self.on(Platform::screen).await.and_then(to_value),
            "list_elements" => self.list_elements(args).await,
            "wait" => self.wait(args).await,
            "click" => self.click(args, peer).await,
            "hover" => {
                let (x, y) = self.position(args, "ref", "x", "y").await?;
                self.act(peer, Target::Point(x, y), move |p| {
                    p.pointer(&Pointer::Move { x, y })
                })
                .await
            }
            "drag" => {
                let from = self.position(args, "ref", "x", "y").await?;
                let to = self.position(args, "to_ref", "to_x", "to_y").await?;
                self.act(peer, Target::Point(from.0, from.1), move |p| {
                    p.pointer(&Pointer::Drag { from, to })
                })
                .await
            }
            "scroll" => {
                let (x, y) = self.position(args, "ref", "x", "y").await?;
                let direction = match args.word("direction") {
                    Some("up") => Direction::Up,
                    Some("down") => Direction::Down,
                    Some("left") => Direction::Left,
                    Some("right") => Direction::Right,
                    _ => return Err(bad("direction must be up, down, left or right")),
                };
                let amount = args.count("amount").unwrap_or(SCROLL_DEFAULT);
                self.act(peer, Target::Point(x, y), move |p| {
                    p.pointer(&Pointer::Scroll {
                        x,
                        y,
                        direction,
                        amount,
                    })
                })
                .await
            }
            "type" => self.type_text(args, peer).await,
            "press" => {
                let chord = parse_chord(args.text("keys")?)?;
                self.guard_secure().await?;
                self.act(peer, Target::Focus, move |p| p.press_keys(&chord))
                    .await
            }
            "open" => self.open(args).await,
            "quit" => {
                let wanted = args.text("app")?.to_owned();
                let screen = self.on(Platform::screen).await?;
                let app = screen
                    .apps
                    .into_iter()
                    .map(|running| running.app)
                    .find(|app| app.name.eq_ignore_ascii_case(&wanted) || app.id == wanted)
                    .ok_or_else(|| Fault::Missing(format!("no running app named {wanted}")))?;
                let name = app.name.clone();
                self.on(move |p| p.quit(&app)).await?;
                Ok(json!({ "app": name }))
            }
            "set_window" => self.set_window(args, peer).await,
            "read_clipboard" => {
                let text = self.on(Platform::read_clipboard).await?;
                Ok(json!({ "text": text }))
            }
            "write_clipboard" => {
                let text = args.text("text")?.to_owned();
                self.on(move |p| p.write_clipboard(&text)).await?;
                Ok(json!({}))
            }
            _ => Err(Fault::Unsupported(format!(
                "computer has no tool named {name}"
            ))),
        }
    }

    async fn list_elements(&self, args: &Args) -> Result<Value, Fault> {
        if args.number("x").is_some() || args.number("y").is_some() {
            let (x, y) = self.point(args, "x", "y")?;
            let found = self.on(move |p| p.element_at(x, y)).await?;
            return Ok(json!({ "elements": found.into_iter().collect::<Vec<_>>() }));
        }
        let window = args.word("window").map(str::to_owned);
        let find = args.word("find").map(str::to_owned);
        let found = self
            .on(move |p| p.elements(window.as_deref(), find.as_deref()))
            .await?;
        Ok(json!({ "elements": found }))
    }

    async fn screenshot(&self, args: &Args) -> Result<CallToolResult, Fault> {
        let window = args.word("window").map(str::to_owned);
        let region = args
            .0
            .get("region")
            .filter(|value| value.is_object())
            .map(|value| Rect {
                x: value["x"].as_f64().unwrap_or_default(),
                y: value["y"].as_f64().unwrap_or_default(),
                width: value["width"].as_f64().unwrap_or_default(),
                height: value["height"].as_f64().unwrap_or_default(),
            });
        let Capture {
            png,
            width,
            height,
            origin_x,
            origin_y,
            scale,
        } = self
            .on(move |p| p.capture(window.as_deref(), region))
            .await?;
        let id = {
            let mut shots = lock(&self.shots);
            let id = format!("s{}", shots.len() + 1);
            shots.insert(
                id.clone(),
                Shot {
                    origin_x,
                    origin_y,
                    scale,
                },
            );
            id
        };
        let mut result =
            CallToolResult::structured(json!({ "shot": id, "width": width, "height": height }));
        result
            .content
            .push(ContentBlock::image(STANDARD.encode(png), "image/png"));
        Ok(result)
    }

    async fn wait(&self, args: &Args) -> Result<Value, Fault> {
        let window = args.word("window").map(str::to_owned);
        let text = args.word("text").map(str::to_owned);
        let reference = args.word("ref").map(str::to_owned);
        if text.is_none() && reference.is_none() {
            return Err(bad("wait needs text or ref"));
        }
        let gone = args.flag("gone");
        let limit = args
            .0
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(WAIT_DEFAULT_MS)
            .min(WAIT_MOST_MS);
        let deadline = tokio::time::Instant::now() + Duration::from_millis(limit);
        loop {
            let (window, text, reference) = (window.clone(), text.clone(), reference.clone());
            let is_present = self
                .on(move |p| match (&reference, &text) {
                    (Some(reference), _) => Ok(p.owner(reference).is_ok()),
                    (None, Some(text)) => p
                        .elements(window.as_deref(), Some(text))
                        .map(|found| !found.is_empty()),
                    (None, None) => Ok(false),
                })
                .await?;
            if is_present != gone {
                return Ok(json!({ "found": true }));
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(json!({ "found": false }));
            }
            tokio::time::sleep(WAIT_POLL).await;
        }
    }

    async fn click(&self, args: &Args, peer: &Peer<RoleServer>) -> Result<Value, Fault> {
        let button = match args.word("button") {
            None | Some("left") => Button::Left,
            Some("right") => Button::Right,
            Some("middle") => Button::Middle,
            Some(other) => return Err(bad(&format!("{other} is not a mouse button"))),
        };
        let count = u8::try_from(args.count("count").unwrap_or(1).clamp(1, 3)).unwrap_or(1);
        let keys = match args.word("keys") {
            Some(keys) => parse_modifiers(keys)?,
            None => Vec::new(),
        };
        if let Some(reference) = args.word("ref")
            && count == 1
            && keys.is_empty()
            && button != Button::Middle
        {
            let reference = reference.to_owned();
            let target = Target::Element(reference.clone());
            return self
                .act(peer, target, move |p| p.press(&reference, button))
                .await;
        }
        let (x, y) = self.position(args, "ref", "x", "y").await?;
        self.act(peer, Target::Point(x, y), move |p| {
            p.pointer(&Pointer::Click {
                x,
                y,
                button,
                count,
                keys,
            })
        })
        .await
    }

    async fn type_text(&self, args: &Args, peer: &Peer<RoleServer>) -> Result<Value, Fault> {
        let text = args.text("text")?.to_owned();
        self.guard_secure().await?;
        match args.word("ref") {
            Some(reference) => {
                let reference = reference.to_owned();
                self.act(peer, Target::Element(reference.clone()), move |p| {
                    p.set_text(&reference, &text)
                })
                .await
            }
            None => {
                self.act(peer, Target::Focus, move |p| p.type_text(&text))
                    .await
            }
        }
    }

    async fn open(&self, args: &Args) -> Result<Value, Fault> {
        let opening = match (args.word("app"), args.word("path"), args.word("url")) {
            (Some(app), None, None) => Opening::App(app.to_owned()),
            (None, Some(path), None) if path.starts_with('/') => Opening::Path(path.to_owned()),
            (None, Some(_), None) => return Err(bad("path must be absolute")),
            (None, None, Some(url)) if url.contains("://") || url.starts_with("mailto:") => {
                Opening::Url(url.to_owned())
            }
            (None, None, Some(_)) => return Err(bad("url needs its scheme, such as https://")),
            _ => return Err(bad("open takes exactly one of app, path or url")),
        };
        let app = self.on(move |p| p.open(&opening)).await?;
        Ok(json!({ "app": app.name }))
    }

    async fn set_window(&self, args: &Args, peer: &Peer<RoleServer>) -> Result<Value, Fault> {
        let window = args.text("window")?.to_owned();
        let state = match args.word("state") {
            None => None,
            Some("focused") => Some(WindowState::Focused),
            Some("minimized") => Some(WindowState::Minimized),
            Some("maximized") => Some(WindowState::Maximized),
            Some("normal") => Some(WindowState::Normal),
            Some("closed") => Some(WindowState::Closed),
            Some(other) => return Err(bad(&format!("{other} is not a window state"))),
        };
        let frame = match (
            args.number("x"),
            args.number("y"),
            args.number("width"),
            args.number("height"),
        ) {
            (None, None, None, None) => None,
            (Some(x), Some(y), Some(width), Some(height)) => Some(Rect {
                x,
                y,
                width,
                height,
            }),
            _ => return Err(bad("give x, y, width and height together")),
        };
        if state == Some(WindowState::Closed) {
            let title = window.clone();
            if !ask(peer, &format!("Close the window {title}"), false)
                .await?
                .0
            {
                return Err(Fault::Denied(format!(
                    "the user kept the window {title} open"
                )));
            }
        }
        let target = Target::Window(window.clone());
        self.act(peer, target, move |p| p.set_window(&window, state, frame))
            .await
    }

    async fn act(
        &self,
        peer: &Peer<RoleServer>,
        target: Target,
        work: impl FnOnce(&mut (dyn Platform + 'static)) -> Result<(), Fault> + Send + 'static,
    ) -> Result<Value, Fault> {
        self.hold()?;
        let app = self
            .on(move |p| match &target {
                Target::Element(reference) => p.owner(reference).map(|(app, _)| app),
                Target::Point(x, y) => p.app_at(*x, *y),
                Target::Window(window) => p.window_app(window),
                Target::Focus => p.focused_app(),
            })
            .await?;
        self.approve(&app, peer).await?;
        let watched = app.clone();
        let before = self.on(move |p| Ok(p.fingerprint(&watched))).await?;
        self.on(work).await?;
        let deadline = tokio::time::Instant::now() + SETTLE;
        let mut changed = false;
        while !changed && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(POLL).await;
            let watched = app.clone();
            changed = self.on(move |p| Ok(p.fingerprint(&watched))).await? != before;
        }
        Ok(json!({ "changed": changed, "app": app.name }))
    }

    async fn approve(&self, app: &App, peer: &Peer<RoleServer>) -> Result<(), Fault> {
        if self.on(|p| Ok(p.host_app())).await?.as_ref() == Some(app) {
            return Err(Fault::Denied(format!(
                "goat does not control {}, the terminal it runs in",
                app.name
            )));
        }
        if lock(&self.allowed).contains(&app.id) {
            return Ok(());
        }
        let key = format!("{GRANT}{}", app.id);
        if self
            .logins
            .get(&key)
            .map_err(|err| Fault::Failed(err.to_string()))?
            .is_some()
        {
            lock(&self.allowed).insert(app.id.clone());
            return Ok(());
        }
        let (is_allowed, is_always) =
            ask(peer, &format!("Allow goat to control {}", app.name), true).await?;
        if !is_allowed {
            return Err(Fault::Denied(format!(
                "the user did not allow controlling {}",
                app.name
            )));
        }
        if is_always {
            self.logins
                .save(&key, "{}")
                .map_err(|err| Fault::Failed(err.to_string()))?;
        }
        lock(&self.allowed).insert(app.id.clone());
        Ok(())
    }

    fn hold(&self) -> Result<(), Fault> {
        let mut held = lock(&self.held);
        if held.is_some() {
            return Ok(());
        }
        if let Some(parent) = self.lock_path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| Fault::Failed(err.to_string()))?;
        }
        let file = std::fs::File::create(&self.lock_path)
            .map_err(|err| Fault::Failed(format!("the computer lock: {err}")))?;
        match file.try_lock() {
            Ok(()) => {
                *held = Some(file);
                Ok(())
            }
            Err(std::fs::TryLockError::WouldBlock) => Err(Fault::Denied(
                "another goat session is using the computer; try again when it is done".to_owned(),
            )),
            Err(std::fs::TryLockError::Error(err)) => {
                Err(Fault::Failed(format!("the computer lock: {err}")))
            }
        }
    }

    async fn guard_secure(&self) -> Result<(), Fault> {
        if self.on(|p| Ok(p.is_secure_input())).await? {
            return Err(Fault::Denied(
                "a password field has secure input on; goat does not type into it".to_owned(),
            ));
        }
        Ok(())
    }

    async fn position(
        &self,
        args: &Args,
        reference: &str,
        x: &str,
        y: &str,
    ) -> Result<(f64, f64), Fault> {
        if let Some(reference) = args.word(reference) {
            let reference = reference.to_owned();
            let (_, frame) = self.on(move |p| p.owner(&reference)).await?;
            return Ok((frame.x + frame.width / 2.0, frame.y + frame.height / 2.0));
        }
        self.point(args, x, y)
    }

    fn point(&self, args: &Args, x: &str, y: &str) -> Result<(f64, f64), Fault> {
        let (Some(px), Some(py)) = (args.number(x), args.number(y)) else {
            return Err(bad(&format!("give a ref, or {x} and {y}")));
        };
        let Some(id) = args.word("shot") else {
            return Ok((px, py));
        };
        let shot = lock(&self.shots)
            .get(id)
            .copied()
            .ok_or_else(|| Fault::Missing(format!("no screenshot {id}; take one first")))?;
        Ok((
            shot.origin_x + px / shot.scale,
            shot.origin_y + py / shot.scale,
        ))
    }

    async fn on<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut (dyn Platform + 'static)) -> Result<T, Fault> + Send + 'static,
    ) -> Result<T, Fault> {
        self.desktop.run(work).await?
    }
}

impl ServerHandler for Computer {
    fn get_info(&self) -> InitializeResult {
        InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("computer", env!("CARGO_PKG_VERSION")))
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        std::future::ready(Ok(ListToolsResult::with_all_items(self.tools.to_vec())))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let args = Args(request.arguments.unwrap_or_default());
        let result = if request.name == "screenshot" {
            self.screenshot(&args).await
        } else {
            self.run(&request.name, &args, &context.peer)
                .await
                .map(CallToolResult::structured)
        };
        Ok(result.unwrap_or_else(|fault| failure(&fault)).into())
    }
}

enum Target {
    Element(String),
    Point(f64, f64),
    Window(String),
    Focus,
}

impl Args {
    fn word(&self, key: &str) -> Option<&str> {
        self.0
            .get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
    }

    fn text(&self, key: &str) -> Result<&str, Fault> {
        self.word(key)
            .ok_or_else(|| bad(&format!("{key} is required")))
    }

    fn number(&self, key: &str) -> Option<f64> {
        self.0.get(key).and_then(Value::as_f64)
    }

    fn count(&self, key: &str) -> Option<u32> {
        self.0
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|count| u32::try_from(count).ok())
    }

    fn flag(&self, key: &str) -> bool {
        self.0.get(key).and_then(Value::as_bool) == Some(true)
    }
}

pub(crate) fn tools() -> Vec<Tool> {
    serde_json::from_str(TOOLS).unwrap_or_else(|err| unreachable!("computer.json is valid: {err}"))
}

pub(crate) fn forget_grants(logins: &Logins) -> Result<(), crate::store::Error> {
    for key in logins.keys(GRANT)? {
        logins.remove(&key)?;
    }
    Ok(())
}

async fn ask(
    peer: &Peer<RoleServer>,
    message: &str,
    can_always: bool,
) -> Result<(bool, bool), Fault> {
    let schema = if can_always {
        json!({ "type": "object", "properties": { "always": { "type": "boolean" } } })
    } else {
        json!({ "type": "object", "properties": {} })
    };
    let requested_schema = serde_json::from_value(schema)
        .map_err(|err| Fault::Failed(format!("the question could not be built: {err}")))?;
    let request = ElicitRequestParams::FormElicitationParams {
        meta: None,
        message: message.to_owned(),
        requested_schema,
    };
    let answer = peer
        .create_elicitation(request)
        .await
        .map_err(|err| Fault::Denied(format!("the user could not be asked: {err}")))?;
    let is_allowed = answer.action == ElicitationAction::Accept;
    let is_always = answer
        .content
        .as_ref()
        .is_some_and(|content| content["always"] == true);
    Ok((is_allowed, is_always))
}

fn parse_chord(raw: &str) -> Result<Chord, Fault> {
    let parts: Vec<&str> = raw.split('+').map(str::trim).collect();
    let (key, modifiers) = parts
        .split_last()
        .filter(|(key, _)| !key.is_empty())
        .ok_or_else(|| bad("keys is empty"))?;
    Ok(Chord {
        modifiers: modifiers
            .iter()
            .map(|name| modifier(name))
            .collect::<Result<_, _>>()?,
        key: key.to_lowercase(),
    })
}

fn parse_modifiers(raw: &str) -> Result<Vec<Key>, Fault> {
    raw.split('+').map(|name| modifier(name.trim())).collect()
}

fn modifier(name: &str) -> Result<Key, Fault> {
    match name.to_lowercase().as_str() {
        "cmd" | "command" | "meta" | "super" | "win" => Ok(Key::Command),
        "ctrl" | "control" => Ok(Key::Control),
        "alt" | "option" | "opt" => Ok(Key::Option),
        "shift" => Ok(Key::Shift),
        other => Err(bad(&format!(
            "{other} is not a modifier; use cmd, ctrl, alt or shift"
        ))),
    }
}

fn bad(message: &str) -> Fault {
    Fault::Failed(format!("bad arguments: {message}"))
}

fn failure(fault: &Fault) -> CallToolResult {
    let next = match fault {
        Fault::Stale(_) => " Call list_elements again for a fresh ref.",
        Fault::Missing(_) => " Check list_windows or list_elements for what exists now.",
        Fault::Unsupported(_) => " Try a screenshot and click by position instead.",
        Fault::Denied(_) | Fault::Failed(_) => "",
    };
    CallToolResult::error(vec![ContentBlock::text(format!(
        "{}: {}.{next}",
        fault.code(),
        fault.message()
    ))])
}

fn to_value(value: impl serde::Serialize) -> Result<Value, Fault> {
    serde_json::to_value(value).map_err(|err| Fault::Failed(err.to_string()))
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Debug)]
pub struct Escape {
    pressed: Arc<tokio::sync::Notify>,
    #[cfg(target_os = "macos")]
    _tap: macos::Tap,
}

impl Escape {
    #[cfg(target_os = "macos")]
    pub fn watch() -> Option<Self> {
        let pressed = Arc::new(tokio::sync::Notify::new());
        let tap = macos::watch_escape(Arc::clone(&pressed))?;
        Some(Self { pressed, _tap: tap })
    }

    #[cfg(not(target_os = "macos"))]
    pub fn watch() -> Option<Self> {
        None
    }

    pub async fn pressed(&self) {
        self.pressed.notified().await;
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn start(goat_dir: &std::path::Path) -> Result<Desktop, Fault> {
    let scratch = goat_dir.join("computer");
    Desktop::start("goat-desktop", move || {
        macos::Mac::new(scratch).map(|mac| Box::new(mac) as Box<dyn Platform>)
    })
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn start(_goat_dir: &std::path::Path) -> Result<Desktop, Fault> {
    Err(unsupported())
}

#[cfg(target_os = "macos")]
pub fn missing_permissions(ask: bool) -> Result<Vec<&'static str>, String> {
    Ok(macos::missing(ask))
}

#[cfg(not(target_os = "macos"))]
pub fn missing_permissions(_ask: bool) -> Result<Vec<&'static str>, String> {
    Err(unsupported().message().to_owned())
}

#[cfg(not(target_os = "macos"))]
fn unsupported() -> Fault {
    Fault::Unsupported("computer use is available on macOS for now".to_owned())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use rmcp::handler::client::ClientHandler;
    use rmcp::model::{
        ClientCapabilities, ClientConfig, ElicitResult, ElicitationCapability,
        FormElicitationCapability,
    };
    use rmcp::service::{
        ClientLifecycleMode, ClientServiceExt, RoleClient, RunningService, ServiceExt,
    };

    use super::fake::Fake;
    use super::*;
    use crate::store::Store;

    #[derive(Clone, Default)]
    struct Answering {
        answers: Arc<Mutex<VecDeque<(ElicitationAction, bool)>>>,
        asked: Arc<Mutex<Vec<String>>>,
    }

    impl ClientHandler for Answering {
        fn get_info(&self) -> ClientConfig {
            let mut capabilities = ClientCapabilities::default();
            capabilities.elicitation =
                Some(ElicitationCapability::new().with_form(FormElicitationCapability::new()));
            ClientConfig::new(capabilities, Implementation::new("test", "0"))
        }

        fn create_elicitation(
            &self,
            request: ElicitRequestParams,
            _context: RequestContext<RoleClient>,
        ) -> impl Future<Output = Result<ElicitResult, ErrorData>> + Send + '_ {
            if let ElicitRequestParams::FormElicitationParams { message, .. } = request {
                lock(&self.asked).push(message);
            }
            let (action, is_always) = lock(&self.answers)
                .pop_front()
                .unwrap_or((ElicitationAction::Decline, false));
            std::future::ready(Ok(
                ElicitResult::new(action).with_content(json!({ "always": is_always }))
            ))
        }
    }

    struct Rig {
        client: RunningService<RoleClient, Answering>,
        _server: RunningService<RoleServer, Computer>,
        answering: Answering,
        log: Arc<Mutex<Vec<String>>>,
        logins: Arc<Logins>,
    }

    async fn rig_with(fake: Fake, logins: Arc<Logins>, home: &std::path::Path) -> Rig {
        let log = Arc::clone(&fake.log);
        let desktop =
            Desktop::start("goat-desktop", move || Ok::<_, Fault>(Box::new(fake))).unwrap();
        let computer = Computer::new(desktop, Arc::clone(&logins), home);
        let answering = Answering::default();
        let (near, far) = tokio::io::duplex(64 * 1024);
        let (server, client) = tokio::join!(
            computer.serve(far),
            answering
                .clone()
                .serve_with_lifecycle(near, ClientLifecycleMode::Initialize)
        );
        Rig {
            client: client.unwrap(),
            _server: server.unwrap(),
            answering,
            log,
            logins,
        }
    }

    async fn rig(home: &std::path::Path) -> Rig {
        let logins = Arc::new(Store::open_in_memory().unwrap().logins().unwrap());
        rig_with(Fake::new(Arc::default()), logins, home).await
    }

    impl Rig {
        fn answer(&self, action: ElicitationAction, is_always: bool) {
            lock(&self.answering.answers).push_back((action, is_always));
        }

        fn asked(&self) -> Vec<String> {
            lock(&self.answering.asked).clone()
        }

        fn log(&self) -> Vec<String> {
            lock(&self.log).clone()
        }

        async fn call(&self, name: &str, args: Value) -> Result<Value, String> {
            let mut params = CallToolRequestParams::new(name.to_owned());
            params.arguments = args.as_object().cloned();
            let result = self.client.peer().call_tool(params).await.unwrap();
            let value = serde_json::to_value(&result).unwrap();
            if value["isError"] == true {
                return Err(value["content"][0]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned());
            }
            Ok(value["structuredContent"].clone())
        }
    }

    #[tokio::test]
    async fn clicking_a_ref_asks_once_per_app_and_reports_the_change() {
        let dir = tempfile::tempdir().unwrap();
        let rig = rig(dir.path()).await;
        rig.answer(ElicitationAction::Accept, false);
        let listed = rig
            .call("list_elements", json!({ "find": "save" }))
            .await
            .unwrap();
        let save = listed["elements"][0]["ref"].as_str().unwrap().to_owned();

        let first = rig.call("click", json!({ "ref": save })).await.unwrap();
        let second = rig.call("click", json!({ "ref": save })).await.unwrap();

        assert_eq!(first, json!({ "changed": true, "app": "Notes" }));
        assert_eq!(second["changed"], true);
        assert_eq!(rig.asked(), ["Allow goat to control Notes"]);
        assert_eq!(rig.log(), ["press Save Left", "press Save Left"]);
    }

    #[tokio::test]
    async fn a_ref_from_an_earlier_listing_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let rig = rig(dir.path()).await;
        rig.answer(ElicitationAction::Accept, false);
        let old = rig.call("list_elements", json!({})).await.unwrap()["elements"][0]["ref"].clone();
        rig.call("list_elements", json!({})).await.unwrap();

        let err = rig.call("click", json!({ "ref": old })).await.unwrap_err();

        assert!(err.starts_with("stale_ref:"), "{err}");
        assert!(err.contains("Call list_elements again"), "{err}");
        assert_eq!(rig.log(), Vec::<String>::new());
    }

    #[tokio::test]
    async fn an_app_allowed_always_is_not_asked_about_again() {
        let dir = tempfile::tempdir().unwrap();
        let logins = Arc::new(Store::open_in_memory().unwrap().logins().unwrap());
        let first = rig_with(Fake::new(Arc::default()), Arc::clone(&logins), dir.path()).await;
        first.answer(ElicitationAction::Accept, true);
        first
            .call("press", json!({ "keys": "cmd+s" }))
            .await
            .unwrap();
        drop(first);
        let later = rig_with(Fake::new(Arc::default()), Arc::clone(&logins), dir.path()).await;

        later
            .call("press", json!({ "keys": "cmd+s" }))
            .await
            .unwrap();

        assert_eq!(later.asked(), Vec::<String>::new());
        assert_eq!(
            logins.keys(GRANT).unwrap(),
            ["grant:computer:com.apple.Notes"]
        );
        forget_grants(&later.logins).unwrap();
        assert_eq!(logins.keys(GRANT).unwrap(), Vec::<String>::new());
    }

    #[tokio::test]
    async fn a_refused_app_is_not_touched() {
        let dir = tempfile::tempdir().unwrap();
        let rig = rig(dir.path()).await;
        rig.answer(ElicitationAction::Decline, false);

        let err = rig.call("type", json!({ "text": "hi" })).await.unwrap_err();

        assert_eq!(err, "denied: the user did not allow controlling Notes.");
        assert_eq!(rig.log(), Vec::<String>::new());
    }

    #[tokio::test]
    async fn the_terminal_goat_runs_in_is_never_controlled() {
        let dir = tempfile::tempdir().unwrap();
        let rig = rig(dir.path()).await;

        let err = rig
            .call("click", json!({ "x": 1200, "y": 10 }))
            .await
            .unwrap_err();

        assert!(err.contains("the terminal it runs in"), "{err}");
        assert_eq!(rig.asked(), Vec::<String>::new());
    }

    #[tokio::test]
    async fn screenshot_pixels_are_turned_into_screen_points() {
        let dir = tempfile::tempdir().unwrap();
        let rig = rig(dir.path()).await;
        rig.answer(ElicitationAction::Accept, false);
        let mut params = CallToolRequestParams::new("screenshot".to_owned());
        params.arguments = json!({}).as_object().cloned();
        let shot = rig.client.peer().call_tool(params).await.unwrap();
        let shot = serde_json::to_value(&shot).unwrap();
        assert_eq!(shot["structuredContent"]["shot"], "s1");
        assert_eq!(shot["content"][1]["type"], "image");

        rig.call(
            "click",
            json!({ "x": 200, "y": 100, "shot": "s1", "count": 2 }),
        )
        .await
        .unwrap();

        assert_eq!(
            rig.log(),
            ["pointer Click { x: 100.0, y: 50.0, button: Left, count: 2, keys: [] }"]
        );
    }

    #[tokio::test]
    async fn typing_is_refused_while_secure_input_is_on() {
        let dir = tempfile::tempdir().unwrap();
        let logins = Arc::new(Store::open_in_memory().unwrap().logins().unwrap());
        let rig = rig_with(
            Fake::new(Arc::default()).with_secure_input(),
            logins,
            dir.path(),
        )
        .await;

        let err = rig
            .call("type", json!({ "text": "hunter2" }))
            .await
            .unwrap_err();

        assert!(err.starts_with("denied: a password field"), "{err}");
        assert_eq!(rig.asked(), Vec::<String>::new());
    }

    #[tokio::test]
    async fn closing_a_window_asks_even_for_an_allowed_app() {
        let dir = tempfile::tempdir().unwrap();
        let rig = rig(dir.path()).await;
        rig.answer(ElicitationAction::Decline, false);

        let err = rig
            .call("set_window", json!({ "window": "w1", "state": "closed" }))
            .await
            .unwrap_err();

        assert_eq!(rig.asked(), ["Close the window w1"]);
        assert!(err.starts_with("denied:"), "{err}");
        assert_eq!(rig.log(), Vec::<String>::new());
    }

    #[tokio::test]
    async fn a_second_session_waits_for_the_first_to_let_go() {
        let dir = tempfile::tempdir().unwrap();
        let first = rig(dir.path()).await;
        let second = rig(dir.path()).await;
        first.answer(ElicitationAction::Accept, false);
        second.answer(ElicitationAction::Accept, false);
        first.call("press", json!({ "keys": "tab" })).await.unwrap();

        let err = second
            .call("press", json!({ "keys": "tab" }))
            .await
            .unwrap_err();

        assert!(err.contains("another goat session"), "{err}");
    }

    #[tokio::test]
    async fn shortcuts_and_openings_are_checked_before_anything_runs() {
        let dir = tempfile::tempdir().unwrap();
        let rig = rig(dir.path()).await;
        rig.answer(ElicitationAction::Accept, false);

        rig.call("press", json!({ "keys": "cmd+shift+S" }))
            .await
            .unwrap();
        let bad_key = rig
            .call("press", json!({ "keys": "hyper+s" }))
            .await
            .unwrap_err();
        let two = rig
            .call("open", json!({ "app": "Notes", "url": "https://x.dev" }))
            .await
            .unwrap_err();
        let relative = rig
            .call("open", json!({ "path": "notes.txt" }))
            .await
            .unwrap_err();

        assert_eq!(rig.log(), ["keys [Command, Shift]+s"]);
        assert!(bad_key.contains("hyper is not a modifier"), "{bad_key}");
        assert!(two.contains("exactly one of app, path or url"), "{two}");
        assert!(relative.contains("path must be absolute"), "{relative}");
    }
}
