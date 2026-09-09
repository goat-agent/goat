#[cfg(target_os = "macos")]
mod mouse;
#[cfg(target_os = "macos")]
pub use mouse::NativeMouse;

#[derive(Debug, thiserror::Error)]
pub enum NativeError {
    #[error("could not create native mouse event source")]
    EventSource,
    #[error("could not read native cursor position")]
    Cursor,
    #[error("could not create native mouse event")]
    MouseEvent,
    #[error("application {0} is no longer running")]
    AppGone(i32),
    #[error("macOS refused to activate application {0}")]
    FocusDenied(i32),
}

#[cfg(target_os = "macos")]
use goat_api::RunningApp;
#[cfg(target_os = "macos")]
use objc2::rc::autoreleasepool;
#[cfg(target_os = "macos")]
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication, NSWorkspace};

#[cfg(target_os = "macos")]
#[must_use]
pub fn running_apps() -> Vec<RunningApp> {
    autoreleasepool(|_| {
        let workspace = NSWorkspace::sharedWorkspace();
        let frontmost = workspace
            .frontmostApplication()
            .map(|app| app.processIdentifier());
        workspace
            .runningApplications()
            .iter()
            .filter(|app| !app.isTerminated())
            .map(|app| {
                let pid = app.processIdentifier();
                let bundle_id = app.bundleIdentifier().map(|value| value.to_string());
                let name = app.localizedName().map_or_else(
                    || bundle_id.clone().unwrap_or_else(|| format!("pid {pid}")),
                    |value| value.to_string(),
                );
                RunningApp {
                    name,
                    pid,
                    bundle_id,
                    frontmost: Some(pid) == frontmost,
                }
            })
            .collect()
    })
}

#[cfg(target_os = "macos")]
pub fn focus_app(pid: i32) -> Result<(), NativeError> {
    autoreleasepool(|_| {
        let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
            .ok_or(NativeError::AppGone(pid))?;
        if app.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows) {
            Ok(())
        } else {
            Err(NativeError::FocusDenied(pid))
        }
    })
}
