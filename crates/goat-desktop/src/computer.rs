use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use goat_client::Link;
use goat_computer_host::{
    CAPABILITY, CAPABILITY_VERSION, ComputerHost, advertise, advertisement, withdrawal,
};
use goat_wire::envelope::{Hello, Role};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

pub struct ComputerState {
    inner: Arc<Inner>,
}

struct Inner {
    link: Arc<Link>,
    host: Option<Arc<ComputerHost>>,
    advertised: AtomicBool,
    changed: Notify,
    cancel: CancellationToken,
    instance: String,
    boot_epoch: u64,
}

impl ComputerState {
    pub fn new(link: Arc<Link>) -> Result<Self, String> {
        #[cfg(target_os = "macos")]
        let host = Some(Arc::new(ComputerHost::new(Arc::new(
            goat_computer_host::macos::MacDesktop::new().map_err(|error| error.to_string())?,
        ))));
        #[cfg(not(target_os = "macos"))]
        let host = None;
        let hostname = hostname::get().map_err(|error| error.to_string())?;
        Ok(Self {
            inner: Arc::new(Inner {
                link,
                host,
                advertised: AtomicBool::new(false),
                changed: Notify::new(),
                cancel: CancellationToken::new(),
                instance: format!("{}/desktop", hostname.to_string_lossy()),
                boot_epoch: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|error| error.to_string())?
                    .as_secs(),
            }),
        })
    }
}

#[derive(Clone, Serialize)]
pub struct ComputerStatus {
    accessibility: bool,
    screen: bool,
    advertised: bool,
    halted: bool,
    busy: bool,
}

#[cfg(target_os = "macos")]
async fn permissions() -> (bool, bool) {
    (
        tauri_plugin_macos_permissions::check_accessibility_permission().await,
        tauri_plugin_macos_permissions::check_screen_recording_permission().await,
    )
}

#[cfg(not(target_os = "macos"))]
fn permissions() -> std::future::Ready<(bool, bool)> {
    std::future::ready((false, false))
}

#[tauri::command]
pub async fn computer_status(state: State<'_, ComputerState>) -> Result<ComputerStatus, String> {
    let (accessibility, screen) = permissions().await;
    Ok(ComputerStatus {
        accessibility,
        screen,
        advertised: state.inner.advertised.load(Ordering::SeqCst),
        halted: state
            .inner
            .host
            .as_ref()
            .is_some_and(|host| host.is_halted()),
        busy: state.inner.host.as_ref().is_some_and(|host| host.is_busy()),
    })
}

#[tauri::command]
pub async fn computer_request_permission(
    kind: String,
    state: State<'_, ComputerState>,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    match kind.as_str() {
        "accessibility" => tauri_plugin_macos_permissions::request_accessibility_permission().await,
        "screen" => tauri_plugin_macos_permissions::request_screen_recording_permission().await,
        _ => return Err("unknown computer permission".into()),
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (kind, state);
        Err("computer use is available on macOS".into())
    }
    #[cfg(target_os = "macos")]
    {
        state.inner.changed.notify_one();
        Ok(())
    }
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn computer_resume(state: State<'_, ComputerState>) -> Result<(), String> {
    let host = state
        .inner
        .host
        .as_ref()
        .ok_or("computer use is available on macOS")?;
    host.resume();
    state.inner.changed.notify_one();
    Ok(())
}

pub fn setup(app: &AppHandle) -> Result<(), String> {
    let inner = app.state::<ComputerState>().inner.clone();
    if inner.host.is_none() {
        return Ok(());
    }
    let overlay = WebviewWindowBuilder::new(
        app,
        "overlay",
        WebviewUrl::App("index.html?overlay=1".into()),
    )
    .title("goat computer use")
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(false)
    .visible(false)
    .build()
    .map_err(|error| error.to_string())?;
    overlay
        .set_ignore_cursor_events(true)
        .map_err(|error| error.to_string())?;
    if let Some(monitor) = overlay
        .primary_monitor()
        .map_err(|error| error.to_string())?
    {
        overlay
            .set_position(*monitor.position())
            .map_err(|error| error.to_string())?;
        overlay
            .set_size(*monitor.size())
            .map_err(|error| error.to_string())?;
    }
    let shortcut_inner = inner.clone();
    app.global_shortcut()
        .on_shortcut(
            "CommandOrControl+Shift+Escape",
            move |app, _shortcut, event| {
                if event.state == ShortcutState::Pressed {
                    if let Some(host) = &shortcut_inner.host {
                        host.halt();
                    }
                    shortcut_inner.changed.notify_one();
                    let _ = app.emit("computer:halted", ());
                }
            },
        )
        .map_err(|error| error.to_string())?;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        provider_loop(inner, app).await;
    });
    Ok(())
}

pub fn stop(app: &AppHandle) {
    let state = app.state::<ComputerState>();
    if let Some(host) = &state.inner.host {
        host.halt();
    }
    state.inner.cancel.cancel();
}

async fn provider_loop(inner: Arc<Inner>, app: AppHandle) {
    let Some(host) = &inner.host else {
        return;
    };
    let mut connection: Option<goat_client::ApiSession> = None;
    let mut next_connect = tokio::time::Instant::now();
    let mut next_permissions = tokio::time::Instant::now();
    let mut permitted = false;
    let mut visible = false;
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    loop {
        let changed = tokio::select! {
            biased;
            () = inner.cancel.cancelled() => break,
            () = inner.changed.notified() => true,
            _ = tick.tick() => false,
        };
        let now = tokio::time::Instant::now();
        if changed || now >= next_permissions {
            let (accessibility, screen) = permissions().await;
            permitted = accessibility && screen;
            next_permissions = now + Duration::from_secs(5);
        }
        if connection
            .as_ref()
            .is_some_and(|session| session.closed.is_cancelled())
        {
            connection = None;
            inner.advertised.store(false, Ordering::SeqCst);
            next_connect = now + Duration::from_secs(3);
        }
        if permitted && !host.is_halted() {
            if connection.is_none() && now >= next_connect {
                let hello = Hello::new(
                    Role::Client,
                    concat!("goat-desktop/", env!("CARGO_PKG_VERSION")),
                )
                .with_method(CAPABILITY, vec![CAPABILITY_VERSION]);
                match goat_client::open_serving(&inner.link, "goat-desktop", host.clone(), hello)
                    .await
                {
                    Ok(session) => connection = Some(session),
                    Err(error) => {
                        tracing::warn!(%error, "computer provider could not connect");
                        next_connect = now + Duration::from_secs(3);
                    }
                }
            }
            if !inner.advertised.load(Ordering::SeqCst)
                && let Some(session) = &connection
            {
                match advertise(
                    &session.api,
                    advertisement(&inner.instance, "This Mac", inner.boot_epoch),
                )
                .await
                {
                    Ok(_) => {
                        inner.advertised.store(true, Ordering::SeqCst);
                    }
                    Err(error) => {
                        tracing::warn!(%error, "computer advertisement failed");
                        connection = None;
                        next_connect = now + Duration::from_secs(3);
                    }
                }
            }
        } else if inner.advertised.swap(false, Ordering::SeqCst)
            && let Some(session) = &connection
            && let Err(error) =
                advertise(&session.api, withdrawal(&inner.instance, inner.boot_epoch)).await
        {
            tracing::warn!(%error, "computer withdrawal failed; closing provider connection");
            connection = None;
        }
        let busy = host.is_busy();
        if busy != visible
            && let Some(overlay) = app.get_webview_window("overlay")
        {
            let result = if busy { overlay.show() } else { overlay.hide() };
            if let Err(error) = result {
                tracing::warn!(%error, "computer overlay could not update");
            } else {
                visible = busy;
            }
        }
    }
    inner.advertised.store(false, Ordering::SeqCst);
    if let Some(session) = connection {
        session.shutdown();
    }
    if let Some(overlay) = app.get_webview_window("overlay") {
        let _ = overlay.hide();
    }
}
