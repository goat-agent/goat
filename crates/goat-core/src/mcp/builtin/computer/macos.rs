use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::process::Command;
use std::ptr::NonNull;
use std::sync::Arc;
use std::time::Duration;

use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication, NSWorkspace};
use objc2_application_services::{
    AXError, AXIsProcessTrusted, AXIsProcessTrustedWithOptions, AXUIElement, AXValue, AXValueType,
    kAXTrustedCheckOptionPrompt,
};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFDictionary, CFMachPort, CFNumber, CFRetained, CFRunLoop, CFString,
    CFType, CGPoint, CGRect, CGSize, kCFRunLoopCommonModes,
};
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayBounds, CGDisplayPixelsWide, CGEvent, CGEventField, CGEventFlags,
    CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventTapProxy, CGEventType,
    CGGetActiveDisplayList, CGMouseButton, CGPreflightListenEventAccess,
    CGPreflightScreenCaptureAccess, CGRectMakeWithDictionaryRepresentation,
    CGRequestListenEventAccess, CGRequestScreenCaptureAccess, CGScrollEventUnit,
    CGWindowListCopyWindowInfo, CGWindowListOption, kCGNullWindowID, kCGWindowBounds,
    kCGWindowLayer, kCGWindowName, kCGWindowNumber, kCGWindowOwnerPID,
};

use super::desktop::{
    App, Button, Capture, Chord, Direction, Display, Element, Fault, Key, Opening, Platform,
    Pointer, Rect, Running, Screen, Window, WindowState,
};

const LONG_EDGE: u32 = 1568;
const VALUE_CHARS: usize = 200;
const DEPTH: usize = 30;
const VISITS: usize = 4000;
const MENU_DEPTH: usize = 3;
const TYPE_CHUNK: usize = 16;
const DRAG_STEPS: u32 = 12;
const LAUNCH_WAIT: Duration = Duration::from_secs(5);
const STEP: Duration = Duration::from_millis(15);
const SKIPPED: &[&str] = &[
    "AXGroup",
    "AXScrollArea",
    "AXSplitGroup",
    "AXLayoutArea",
    "AXUnknown",
    "AXSplitter",
    "AXMenuBar",
    "AXMenu",
];
const PRESS_ACTIONS: &[&str] = &["AXPress", "AXConfirm", "AXPick", "AXOpen"];
const ESCAPE_KEY: i64 = 53;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn _AXUIElementGetWindow(element: &AXUIElement, window: *mut u32) -> AXError;
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn IsSecureEventInputEnabled() -> bool;
}

pub(crate) struct Mac {
    scratch: PathBuf,
    host: Option<App>,
    refs: HashMap<String, Held>,
    windows: HashMap<u32, u32>,
    generation: u32,
}

struct Held {
    element: CFRetained<AXUIElement>,
    pid: i32,
    role: String,
    window: u32,
}

struct Listed {
    number: u32,
    pid: i32,
    title: String,
    frame: Rect,
}

impl Mac {
    pub(crate) fn new(scratch: PathBuf) -> Result<Self, Fault> {
        let host = host_app();
        if !unsafe { AXIsProcessTrusted() } {
            return Err(Fault::Denied(format!(
                "{} needs Accessibility access: System Settings > Privacy & Security > Accessibility",
                host.as_ref()
                    .map_or("The terminal goat runs in", |app| app.name.as_str())
            )));
        }
        Ok(Self {
            scratch,
            host,
            refs: HashMap::new(),
            windows: HashMap::new(),
            generation: 0,
        })
    }

    fn held(&self, reference: &str) -> Result<&Held, Fault> {
        let held = self.refs.get(reference).ok_or_else(|| {
            if reference.starts_with('e') {
                Fault::Stale(format!("{reference} is stale"))
            } else {
                Fault::Missing(format!("{reference} is not a ref"))
            }
        })?;
        if string(&held.element, "AXRole").as_deref() != Some(held.role.as_str()) {
            return Err(Fault::Stale(format!("{reference} is gone")));
        }
        Ok(held)
    }

    fn remember(&mut self, element: CFRetained<AXUIElement>, window: u32, index: usize) -> Element {
        let generation = *self.windows.get(&window).unwrap_or(&self.generation);
        let reference = format!("e{generation}.{index}");
        let pid = pid_of(&element);
        let role = string(&element, "AXRole").unwrap_or_default();
        let described = describe(&element, &reference);
        self.refs.insert(
            reference,
            Held {
                element,
                pid,
                role,
                window,
            },
        );
        described
    }

    fn renew(&mut self, window: u32) {
        self.generation += 1;
        self.refs.retain(|_, held| held.window != window);
        self.windows.insert(window, self.generation);
    }

    fn window_element(number: u32) -> Result<(CFRetained<AXUIElement>, i32), Fault> {
        let listed = list_windows()
            .into_iter()
            .find(|listed| listed.number == number)
            .ok_or_else(|| Fault::Missing(format!("no window w{number}")))?;
        let app = unsafe { AXUIElement::new_application(listed.pid) };
        let windows = elements(&app, "AXWindows");
        windows
            .into_iter()
            .find(|window| window_number(window) == Some(number))
            .map(|window| (window, listed.pid))
            .ok_or_else(|| Fault::Missing(format!("w{number} has no accessible window")))
    }

    fn focused_window() -> Result<(CFRetained<AXUIElement>, i32), Fault> {
        let front = NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .ok_or_else(|| Fault::Missing("no app is in front".to_owned()))?;
        let pid = front.processIdentifier();
        let app = unsafe { AXUIElement::new_application(pid) };
        element(&app, "AXFocusedWindow")
            .map(|window| (window, pid))
            .ok_or_else(|| Fault::Missing("the app in front has no focused window".to_owned()))
    }
}

impl Platform for Mac {
    fn screen(&mut self) -> Result<Screen, Fault> {
        let mut apps: Vec<Running> = Vec::new();
        for listed in list_windows() {
            let window = Window {
                id: format!("w{}", listed.number),
                title: listed.title,
                frame: listed.frame,
                minimized: false,
            };
            match apps
                .iter_mut()
                .find(|running| app_pid(&running.app) == Some(listed.pid))
            {
                Some(running) => running.windows.push(window),
                None => {
                    if let Some(app) = app_of(listed.pid) {
                        apps.push(Running {
                            app,
                            windows: vec![window],
                        });
                    }
                }
            }
        }
        let front = NSWorkspace::sharedWorkspace().frontmostApplication();
        let focused_app = front
            .as_ref()
            .and_then(|front| app_of(front.processIdentifier()));
        let focused_window = Self::focused_window()
            .ok()
            .and_then(|(window, _)| window_number(&window))
            .map(|number| format!("w{number}"));
        Ok(Screen {
            displays: displays(),
            focused_app,
            focused_window,
            apps,
        })
    }

    fn elements(
        &mut self,
        window: Option<&str>,
        find: Option<&str>,
    ) -> Result<Vec<Element>, Fault> {
        let (root, pid) = match window {
            Some(window) => Self::window_element(parse_window(window)?)?,
            None => Self::focused_window()?,
        };
        let number = window_number(&root).unwrap_or_default();
        self.renew(number);
        let wanted = find.map(str::to_lowercase);
        let app = unsafe { AXUIElement::new_application(pid) };
        let menu = element(&app, "AXMenuBar");
        let mut found = Vec::new();
        let mut visits = 0;
        let mut stack: Vec<(CFRetained<AXUIElement>, usize, usize)> = vec![(root, 0, DEPTH)];
        if let Some(menu) = menu {
            stack.insert(0, (menu, 0, MENU_DEPTH));
        }
        while let Some((node, depth, limit)) = stack.pop() {
            visits += 1;
            if visits > VISITS {
                break;
            }
            let role = string(&node, "AXRole").unwrap_or_default();
            let name = name_of(&node);
            let value = value_of(&node);
            let is_shown = !SKIPPED.contains(&role.as_str())
                || !name.is_empty()
                || value.as_ref().is_some_and(|value| !value.is_empty());
            let is_wanted = wanted.as_ref().is_none_or(|wanted| {
                name.to_lowercase().contains(wanted)
                    || role.to_lowercase().contains(wanted)
                    || value
                        .as_ref()
                        .is_some_and(|value| value.to_lowercase().contains(wanted))
            });
            if depth < limit {
                for child in elements(&node, "AXChildren").into_iter().rev() {
                    stack.push((child, depth + 1, limit));
                }
            }
            if is_shown && is_wanted && depth > 0 {
                let index = found.len();
                found.push(self.remember(node, number, index));
            }
        }
        Ok(found)
    }

    fn element_at(&mut self, x: f64, y: f64) -> Result<Option<Element>, Fault> {
        let system = unsafe { AXUIElement::new_system_wide() };
        let mut out: *const AXUIElement = std::ptr::null();
        let err = unsafe {
            system.copy_element_at_position(narrow(x), narrow(y), NonNull::from(&mut out))
        };
        if err != AXError::Success || out.is_null() {
            return Ok(None);
        }
        let found = unsafe { CFRetained::from_raw(NonNull::new_unchecked(out.cast_mut())) };
        let window = element(&found, "AXWindow")
            .and_then(|window| window_number(&window))
            .unwrap_or_default();
        self.renew(window);
        Ok(Some(self.remember(found, window, 0)))
    }

    fn capture(&mut self, window: Option<&str>, region: Option<Rect>) -> Result<Capture, Fault> {
        if !CGPreflightScreenCaptureAccess() {
            return Err(Fault::Denied(format!(
                "{} needs Screen Recording access: System Settings > Privacy & Security > Screen Recording",
                self.host
                    .as_ref()
                    .map_or("The terminal goat runs in", |app| app.name.as_str())
            )));
        }
        let file = self.scratch.join("screenshot.png");
        let _ = std::fs::create_dir_all(&self.scratch);
        let mut command = Command::new("/usr/sbin/screencapture");
        command.args(["-x", "-t", "png"]);
        let (origin, width_points) = match (window, region) {
            (_, Some(region)) => {
                command.arg(format!(
                    "-R{},{},{},{}",
                    region.x, region.y, region.width, region.height
                ));
                ((region.x, region.y), region.width)
            }
            (Some(window), None) => {
                let number = parse_window(window)?;
                let listed = list_windows()
                    .into_iter()
                    .find(|listed| listed.number == number)
                    .ok_or_else(|| Fault::Missing(format!("no window {window}")))?;
                command.args(["-o", &format!("-l{number}")]);
                ((listed.frame.x, listed.frame.y), listed.frame.width)
            }
            (None, None) => {
                command.arg("-m");
                let main = displays()
                    .into_iter()
                    .next()
                    .ok_or_else(|| Fault::Missing("no display is attached".to_owned()))?;
                ((main.frame.x, main.frame.y), main.frame.width)
            }
        };
        let status = command
            .arg(&file)
            .status()
            .map_err(|err| Fault::Failed(format!("screencapture did not run: {err}")))?;
        if !status.success() {
            return Err(Fault::Failed("screencapture failed".to_owned()));
        }
        let bytes = std::fs::read(&file).map_err(|err| Fault::Failed(err.to_string()))?;
        let _ = std::fs::remove_file(&file);
        let mut image = image::load_from_memory(&bytes)
            .map_err(|err| Fault::Failed(format!("the screenshot could not be read: {err}")))?
            .to_rgba8();
        let mut scale = f64::from(image.width()) / width_points.max(1.0);
        if window.is_none()
            && let Some(host) = self.host.as_ref().and_then(app_pid)
        {
            for listed in list_windows()
                .into_iter()
                .filter(|listed| listed.pid == host)
            {
                cover(&mut image, listed.frame, origin, scale);
            }
        }
        let long = image.width().max(image.height());
        if long > LONG_EDGE {
            let ratio = f64::from(LONG_EDGE) / f64::from(long);
            let width = pixel(f64::from(image.width()) * ratio);
            let height = pixel(f64::from(image.height()) * ratio);
            image = image::imageops::resize(
                &image,
                width,
                height,
                image::imageops::FilterType::Triangle,
            );
            scale *= ratio;
        }
        let mut png = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|err| Fault::Failed(err.to_string()))?;
        Ok(Capture {
            width: image.width(),
            height: image.height(),
            png,
            origin_x: origin.0,
            origin_y: origin.1,
            scale,
        })
    }

    fn owner(&mut self, reference: &str) -> Result<(App, Rect), Fault> {
        let held = self.held(reference)?;
        let app = app_of(held.pid).ok_or_else(|| Fault::Stale(format!("{reference} is gone")))?;
        Ok((app, frame_of(&held.element)))
    }

    fn app_at(&mut self, x: f64, y: f64) -> Result<App, Fault> {
        let system = unsafe { AXUIElement::new_system_wide() };
        let mut out: *const AXUIElement = std::ptr::null();
        let err = unsafe {
            system.copy_element_at_position(narrow(x), narrow(y), NonNull::from(&mut out))
        };
        if err != AXError::Success || out.is_null() {
            return Err(Fault::Missing(format!("nothing is at {x},{y}")));
        }
        let found: CFRetained<AXUIElement> =
            unsafe { CFRetained::from_raw(NonNull::new_unchecked(out.cast_mut())) };
        app_of(pid_of(&found)).ok_or_else(|| Fault::Missing(format!("no app is at {x},{y}")))
    }

    fn window_app(&mut self, window: &str) -> Result<App, Fault> {
        let number = parse_window(window)?;
        list_windows()
            .into_iter()
            .find(|listed| listed.number == number)
            .and_then(|listed| app_of(listed.pid))
            .ok_or_else(|| Fault::Missing(format!("no window {window}")))
    }

    fn focused_app(&mut self) -> Result<App, Fault> {
        NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .and_then(|front| app_of(front.processIdentifier()))
            .ok_or_else(|| Fault::Missing("no app is in front".to_owned()))
    }

    fn press(&mut self, reference: &str, button: Button) -> Result<(), Fault> {
        let held = self.held(reference)?;
        let actions = actions_of(&held.element);
        let wanted: &[&str] = match button {
            Button::Right => &["AXShowMenu"],
            Button::Left | Button::Middle => PRESS_ACTIONS,
        };
        let action = wanted
            .iter()
            .find(|action| actions.iter().any(|known| known == *action))
            .ok_or_else(|| {
                Fault::Unsupported(format!("{} {reference} has no such action", held.role))
            })?;
        let err = unsafe { held.element.perform_action(&CFString::from_str(action)) };
        if err != AXError::Success {
            return Err(Fault::Failed(format!(
                "{action} on {reference} failed ({})",
                err.0
            )));
        }
        Ok(())
    }

    fn set_text(&mut self, reference: &str, text: &str) -> Result<(), Fault> {
        let element = CFRetained::clone(&self.held(reference)?.element);
        let value = CFString::from_str(text);
        let err = unsafe { element.set_attribute_value(&CFString::from_str("AXValue"), &value) };
        if err == AXError::Success && value_of(&element).as_deref() == Some(text) {
            return Ok(());
        }
        set_bool(&element, "AXFocused", true);
        std::thread::sleep(STEP * 4);
        self.press_keys(&Chord {
            modifiers: vec![Key::Command],
            key: "a".to_owned(),
        })?;
        self.type_text(text)?;
        if value_of(&element).as_deref() == Some(text) {
            Ok(())
        } else {
            Err(Fault::Failed(format!("{reference} did not take the text")))
        }
    }

    fn pointer(&mut self, input: &Pointer) -> Result<(), Fault> {
        match input {
            Pointer::Move { x, y } => move_to(*x, *y, &[]),
            Pointer::Click {
                x,
                y,
                button,
                count,
                keys,
            } => click(*x, *y, *button, *count, keys),
            Pointer::Drag { from, to } => drag(*from, *to),
            Pointer::Scroll {
                x,
                y,
                direction,
                amount,
            } => scroll(*x, *y, *direction, *amount),
        }
    }

    fn type_text(&mut self, text: &str) -> Result<(), Fault> {
        let units: Vec<u16> = text.encode_utf16().collect();
        for chunk in units.chunks(TYPE_CHUNK) {
            for is_down in [true, false] {
                let event = CGEvent::new_keyboard_event(None, 0, is_down)
                    .ok_or_else(|| Fault::Failed("the key event could not be made".to_owned()))?;
                unsafe {
                    CGEvent::keyboard_set_unicode_string(
                        Some(&event),
                        chunk.len() as _,
                        chunk.as_ptr(),
                    );
                }
                CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
            }
            std::thread::sleep(STEP);
        }
        Ok(())
    }

    fn press_keys(&mut self, chord: &Chord) -> Result<(), Fault> {
        let code = key_code(&chord.key)
            .ok_or_else(|| Fault::Unsupported(format!("{} is not a key goat knows", chord.key)))?;
        let flags = flags_of(&chord.modifiers);
        for is_down in [true, false] {
            let event = CGEvent::new_keyboard_event(None, code, is_down)
                .ok_or_else(|| Fault::Failed("the key event could not be made".to_owned()))?;
            CGEvent::set_flags(Some(&event), flags);
            CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
            std::thread::sleep(STEP);
        }
        Ok(())
    }

    fn open(&mut self, opening: &Opening) -> Result<App, Fault> {
        let mut command = Command::new("/usr/bin/open");
        match opening {
            Opening::App(name) => command.args(["-a", name]),
            Opening::Path(path) => command.arg(path),
            Opening::Url(url) => command.arg(url),
        };
        let output = command
            .output()
            .map_err(|err| Fault::Failed(format!("open did not run: {err}")))?;
        if !output.status.success() {
            return Err(Fault::Missing(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ));
        }
        let deadline = std::time::Instant::now() + LAUNCH_WAIT;
        loop {
            std::thread::sleep(STEP * 10);
            let front = self.focused_app();
            let is_ready = match opening {
                Opening::App(name) => front
                    .as_ref()
                    .is_ok_and(|app| app.name.eq_ignore_ascii_case(name)),
                Opening::Path(_) | Opening::Url(_) => front.is_ok(),
            };
            if is_ready || std::time::Instant::now() >= deadline {
                return front;
            }
        }
    }

    fn quit(&mut self, app: &App) -> Result<(), Fault> {
        let running = app_pid(app)
            .and_then(NSRunningApplication::runningApplicationWithProcessIdentifier)
            .ok_or_else(|| Fault::Missing(format!("{} is not running", app.name)))?;
        if running.terminate() {
            Ok(())
        } else {
            Err(Fault::Failed(format!("{} did not quit", app.name)))
        }
    }

    fn set_window(
        &mut self,
        window: &str,
        state: Option<WindowState>,
        frame: Option<Rect>,
    ) -> Result<(), Fault> {
        let (element, pid) = Self::window_element(parse_window(window)?)?;
        match state {
            None => {}
            Some(WindowState::Focused) => {
                if let Some(running) =
                    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
                {
                    running.activateWithOptions(NSApplicationActivationOptions::empty());
                }
                unsafe { element.perform_action(&CFString::from_str("AXRaise")) };
            }
            Some(WindowState::Minimized) => set_bool(&element, "AXMinimized", true),
            Some(WindowState::Normal) => {
                set_bool(&element, "AXFullScreen", false);
                set_bool(&element, "AXMinimized", false);
            }
            Some(WindowState::Maximized) => press_child(&element, "AXZoomButton")?,
            Some(WindowState::Closed) => press_child(&element, "AXCloseButton")?,
        }
        if let Some(frame) = frame {
            set_point(&element, "AXPosition", frame.x, frame.y);
            set_size(&element, "AXSize", frame.width, frame.height);
        }
        Ok(())
    }

    fn read_clipboard(&mut self) -> Result<String, Fault> {
        let output = Command::new("/usr/bin/pbpaste")
            .output()
            .map_err(|err| Fault::Failed(format!("pbpaste did not run: {err}")))?;
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn write_clipboard(&mut self, text: &str) -> Result<(), Fault> {
        use std::io::Write as _;
        let mut child = Command::new("/usr/bin/pbcopy")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|err| Fault::Failed(format!("pbcopy did not run: {err}")))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(text.as_bytes())
                .map_err(|err| Fault::Failed(err.to_string()))?;
        }
        child.wait().map_err(|err| Fault::Failed(err.to_string()))?;
        Ok(())
    }

    fn fingerprint(&mut self, app: &App) -> u64 {
        let mut hasher = DefaultHasher::new();
        if let Some(pid) = app_pid(app) {
            let root = unsafe { AXUIElement::new_application(pid) };
            let windows = elements(&root, "AXWindows");
            windows.len().hash(&mut hasher);
            if let Some(window) = element(&root, "AXFocusedWindow") {
                string(&window, "AXTitle").hash(&mut hasher);
                elements(&window, "AXChildren").len().hash(&mut hasher);
            }
            if let Some(focus) = element(&root, "AXFocusedUIElement") {
                string(&focus, "AXRole").hash(&mut hasher);
                value_of(&focus).hash(&mut hasher);
                string(&focus, "AXSelectedText").hash(&mut hasher);
            }
            if let Some(menu) =
                element(&root, "AXFocusedUIElement").and_then(|focus| string(&focus, "AXExpanded"))
            {
                menu.hash(&mut hasher);
            }
        }
        hasher.finish()
    }

    fn is_secure_input(&mut self) -> bool {
        unsafe { IsSecureEventInputEnabled() }
    }

    fn host_app(&mut self) -> Option<App> {
        self.host.clone()
    }
}

fn attribute(element: &AXUIElement, name: &str) -> Option<CFRetained<CFType>> {
    let mut value: *const CFType = std::ptr::null();
    let err = unsafe {
        element.copy_attribute_value(&CFString::from_str(name), NonNull::from(&mut value))
    };
    if err != AXError::Success || value.is_null() {
        return None;
    }
    Some(unsafe { CFRetained::from_raw(NonNull::new_unchecked(value.cast_mut())) })
}

fn string(element: &AXUIElement, name: &str) -> Option<String> {
    attribute(element, name)?
        .downcast::<CFString>()
        .ok()
        .map(|text| text.to_string())
}

fn element(of: &AXUIElement, name: &str) -> Option<CFRetained<AXUIElement>> {
    attribute(of, name)?.downcast::<AXUIElement>().ok()
}

fn elements(of: &AXUIElement, name: &str) -> Vec<CFRetained<AXUIElement>> {
    let Some(array) = attribute(of, name).and_then(|value| value.downcast::<CFArray>().ok()) else {
        return Vec::new();
    };
    let array: &CFArray<CFType> = unsafe { array.cast_unchecked() };
    array
        .iter()
        .filter_map(|item| item.downcast::<AXUIElement>().ok())
        .collect()
}

fn name_of(element: &AXUIElement) -> String {
    [
        "AXTitle",
        "AXDescription",
        "AXLabel",
        "AXPlaceholderValue",
        "AXHelp",
    ]
    .iter()
    .find_map(|name| string(element, name).filter(|text| !text.is_empty()))
    .unwrap_or_default()
}

fn value_of(element: &AXUIElement) -> Option<String> {
    let value = attribute(element, "AXValue")?;
    let text = match value.downcast::<CFString>() {
        Ok(text) => text.to_string(),
        Err(value) => match value.downcast::<CFNumber>() {
            Ok(number) => number.as_f64()?.to_string(),
            Err(value) => value.downcast::<CFBoolean>().ok()?.as_bool().to_string(),
        },
    };
    Some(text.chars().take(VALUE_CHARS).collect())
}

fn actions_of(element: &AXUIElement) -> Vec<String> {
    let mut names: *const CFArray = std::ptr::null();
    let err = unsafe { element.copy_action_names(NonNull::from(&mut names)) };
    if err != AXError::Success || names.is_null() {
        return Vec::new();
    }
    let names: CFRetained<CFArray> =
        unsafe { CFRetained::from_raw(NonNull::new_unchecked(names.cast_mut())) };
    let names: &CFArray<CFString> = unsafe { names.cast_unchecked() };
    names.iter().map(|name| name.to_string()).collect()
}

fn frame_of(element: &AXUIElement) -> Rect {
    let mut point = CGPoint::new(0.0, 0.0);
    let mut size = CGSize::new(0.0, 0.0);
    if let Some(value) =
        attribute(element, "AXPosition").and_then(|value| value.downcast::<AXValue>().ok())
    {
        unsafe { value.value(AXValueType::CGPoint, NonNull::from(&mut point).cast()) };
    }
    if let Some(value) =
        attribute(element, "AXSize").and_then(|value| value.downcast::<AXValue>().ok())
    {
        unsafe { value.value(AXValueType::CGSize, NonNull::from(&mut size).cast()) };
    }
    Rect {
        x: point.x,
        y: point.y,
        width: size.width,
        height: size.height,
    }
}

fn describe(element: &AXUIElement, reference: &str) -> Element {
    let role = string(element, "AXRole").unwrap_or_default();
    Element {
        reference: reference.to_owned(),
        role: readable(&role),
        name: name_of(element),
        value: value_of(element),
        frame: frame_of(element),
        enabled: flag(element, "AXEnabled").unwrap_or(true),
        focused: flag(element, "AXFocused").unwrap_or(false),
    }
}

fn readable(role: &str) -> String {
    let bare = role.strip_prefix("AX").unwrap_or(role);
    let mut out = String::with_capacity(bare.len() + 4);
    for (index, c) in bare.chars().enumerate() {
        if c.is_uppercase() && index > 0 {
            out.push(' ');
        }
        out.extend(c.to_lowercase());
    }
    out
}

fn flag(element: &AXUIElement, name: &str) -> Option<bool> {
    attribute(element, name)?
        .downcast::<CFBoolean>()
        .ok()
        .map(|value| value.as_bool())
}

fn set_bool(element: &AXUIElement, name: &str, value: bool) {
    unsafe { element.set_attribute_value(&CFString::from_str(name), CFBoolean::new(value)) };
}

fn set_point(element: &AXUIElement, name: &str, x: f64, y: f64) {
    let point = CGPoint::new(x, y);
    if let Some(value) = unsafe { AXValue::new(AXValueType::CGPoint, NonNull::from(&point).cast()) }
    {
        unsafe { element.set_attribute_value(&CFString::from_str(name), &value) };
    }
}

fn set_size(element: &AXUIElement, name: &str, width: f64, height: f64) {
    let size = CGSize::new(width, height);
    if let Some(value) = unsafe { AXValue::new(AXValueType::CGSize, NonNull::from(&size).cast()) } {
        unsafe { element.set_attribute_value(&CFString::from_str(name), &value) };
    }
}

fn press_child(window: &AXUIElement, button: &str) -> Result<(), Fault> {
    let child = element(window, button)
        .ok_or_else(|| Fault::Unsupported(format!("the window has no {}", readable(button))))?;
    let err = unsafe { child.perform_action(&CFString::from_str("AXPress")) };
    if err == AXError::Success {
        Ok(())
    } else {
        Err(Fault::Failed(format!(
            "pressing the {} failed",
            readable(button)
        )))
    }
}

fn pid_of(element: &AXUIElement) -> i32 {
    let mut pid = 0;
    unsafe { element.pid(NonNull::from(&mut pid)) };
    pid
}

fn window_number(window: &AXUIElement) -> Option<u32> {
    let mut number = 0;
    let err = unsafe { _AXUIElementGetWindow(window, &raw mut number) };
    (err == AXError::Success && number != 0).then_some(number)
}

fn parse_window(window: &str) -> Result<u32, Fault> {
    window
        .strip_prefix('w')
        .and_then(|number| number.parse().ok())
        .ok_or_else(|| Fault::Missing(format!("{window} is not a window id from list_windows")))
}

fn app_of(pid: i32) -> Option<App> {
    let running = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)?;
    let name = running.localizedName()?.to_string();
    let id = running
        .bundleIdentifier()
        .map_or_else(|| format!("pid:{pid}"), |id| id.to_string());
    Some(App { name, id })
}

fn app_pid(app: &App) -> Option<i32> {
    if let Some(pid) = app.id.strip_prefix("pid:") {
        return pid.parse().ok();
    }
    NSWorkspace::sharedWorkspace()
        .runningApplications()
        .iter()
        .find(|running| {
            running
                .bundleIdentifier()
                .is_some_and(|id| id.to_string() == app.id)
        })
        .map(|running| running.processIdentifier())
}

fn host_app() -> Option<App> {
    let mut pid = i32::try_from(std::os::unix::process::parent_id()).ok()?;
    for _ in 0..16 {
        if let Some(app) = app_of(pid).filter(|app| !app.id.starts_with("pid:")) {
            return Some(app);
        }
        let output = Command::new("/bin/ps")
            .args(["-o", "ppid=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        pid = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .ok()?;
        if pid <= 1 {
            return None;
        }
    }
    None
}

fn list_windows() -> Vec<Listed> {
    let option =
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
    let Some(array) = CGWindowListCopyWindowInfo(option, kCGNullWindowID) else {
        return Vec::new();
    };
    let array: &CFArray<CFDictionary> = unsafe { array.cast_unchecked() };
    array
        .iter()
        .filter_map(|info| {
            let info: &CFDictionary<CFString, CFType> = unsafe { info.cast_unchecked() };
            let number_of = |key: &CFString| {
                info.get(key)
                    .and_then(|value| value.downcast::<CFNumber>().ok())
                    .and_then(|number| number.as_i64())
            };
            if number_of(unsafe { kCGWindowLayer }) != Some(0) {
                return None;
            }
            let number = u32::try_from(number_of(unsafe { kCGWindowNumber })?).ok()?;
            let pid = i32::try_from(number_of(unsafe { kCGWindowOwnerPID })?).ok()?;
            let title = info
                .get(unsafe { kCGWindowName })
                .and_then(|value| value.downcast::<CFString>().ok())
                .map_or_else(String::new, |title| title.to_string());
            let bounds = info
                .get(unsafe { kCGWindowBounds })
                .and_then(|value| value.downcast::<CFDictionary>().ok())?;
            let mut rect = CGRect::default();
            if !unsafe { CGRectMakeWithDictionaryRepresentation(Some(&bounds), &raw mut rect) } {
                return None;
            }
            Some(Listed {
                number,
                pid,
                title,
                frame: Rect {
                    x: rect.origin.x,
                    y: rect.origin.y,
                    width: rect.size.width,
                    height: rect.size.height,
                },
            })
        })
        .collect()
}

fn displays() -> Vec<Display> {
    let mut ids: [CGDirectDisplayID; 16] = [0; 16];
    let mut count = 0_u32;
    let err = unsafe { CGGetActiveDisplayList(16, ids.as_mut_ptr(), &raw mut count) };
    if err.0 != 0 {
        return Vec::new();
    }
    ids.iter()
        .take(count as usize)
        .map(|id| {
            let bounds = CGDisplayBounds(*id);
            let pixels = f64::from(u32::try_from(CGDisplayPixelsWide(*id)).unwrap_or(u32::MAX));
            Display {
                id: id.to_string(),
                frame: Rect {
                    x: bounds.origin.x,
                    y: bounds.origin.y,
                    width: bounds.size.width,
                    height: bounds.size.height,
                },
                scale: if bounds.size.width > 0.0 {
                    pixels / bounds.size.width
                } else {
                    1.0
                },
            }
        })
        .collect()
}

fn cover(image: &mut image::RgbaImage, frame: Rect, origin: (f64, f64), scale: f64) {
    let left = pixel((frame.x - origin.0) * scale);
    let top = pixel((frame.y - origin.1) * scale);
    let right = pixel((frame.x + frame.width - origin.0) * scale).min(image.width());
    let bottom = pixel((frame.y + frame.height - origin.1) * scale).min(image.height());
    for y in top..bottom {
        for x in left..right {
            image.put_pixel(x, y, image::Rgba([40, 40, 40, 255]));
        }
    }
}

fn move_to(x: f64, y: f64, keys: &[Key]) -> Result<(), Fault> {
    post_mouse(CGEventType::MouseMoved, x, y, CGMouseButton::Left, 0, keys)
}

fn click(x: f64, y: f64, button: Button, count: u8, keys: &[Key]) -> Result<(), Fault> {
    let (down, up, which) = match button {
        Button::Left => (
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseUp,
            CGMouseButton::Left,
        ),
        Button::Right => (
            CGEventType::RightMouseDown,
            CGEventType::RightMouseUp,
            CGMouseButton::Right,
        ),
        Button::Middle => (
            CGEventType::OtherMouseDown,
            CGEventType::OtherMouseUp,
            CGMouseButton::Center,
        ),
    };
    move_to(x, y, keys)?;
    for clicks in 1..=i64::from(count) {
        post_mouse(down, x, y, which, clicks, keys)?;
        post_mouse(up, x, y, which, clicks, keys)?;
    }
    Ok(())
}

fn drag(from: (f64, f64), to: (f64, f64)) -> Result<(), Fault> {
    let left = CGMouseButton::Left;
    move_to(from.0, from.1, &[])?;
    post_mouse(CGEventType::LeftMouseDown, from.0, from.1, left, 1, &[])?;
    for step in 1..=DRAG_STEPS {
        let t = f64::from(step) / f64::from(DRAG_STEPS);
        let x = from.0 + (to.0 - from.0) * t;
        let y = from.1 + (to.1 - from.1) * t;
        post_mouse(CGEventType::LeftMouseDragged, x, y, left, 1, &[])?;
    }
    post_mouse(CGEventType::LeftMouseUp, to.0, to.1, left, 1, &[])
}

fn scroll(x: f64, y: f64, direction: Direction, amount: u32) -> Result<(), Fault> {
    move_to(x, y, &[])?;
    let lines = i32::try_from(amount).unwrap_or(i32::MAX);
    let (vertical, horizontal) = match direction {
        Direction::Up => (lines, 0),
        Direction::Down => (-lines, 0),
        Direction::Left => (0, lines),
        Direction::Right => (0, -lines),
    };
    let event =
        CGEvent::new_scroll_wheel_event2(None, CGScrollEventUnit::Line, 2, vertical, horizontal, 0)
            .ok_or_else(|| Fault::Failed("the scroll event could not be made".to_owned()))?;
    CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
    Ok(())
}

fn post_mouse(
    kind: CGEventType,
    x: f64,
    y: f64,
    button: CGMouseButton,
    clicks: i64,
    keys: &[Key],
) -> Result<(), Fault> {
    let event = CGEvent::new_mouse_event(None, kind, CGPoint::new(x, y), button)
        .ok_or_else(|| Fault::Failed("the mouse event could not be made".to_owned()))?;
    if clicks > 0 {
        CGEvent::set_integer_value_field(Some(&event), CGEventField::MouseEventClickState, clicks);
    }
    if !keys.is_empty() {
        CGEvent::set_flags(Some(&event), flags_of(keys));
    }
    CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
    std::thread::sleep(STEP);
    Ok(())
}

fn flags_of(keys: &[Key]) -> CGEventFlags {
    keys.iter().fold(CGEventFlags::empty(), |flags, key| {
        flags
            | match key {
                Key::Command => CGEventFlags::MaskCommand,
                Key::Control => CGEventFlags::MaskControl,
                Key::Option => CGEventFlags::MaskAlternate,
                Key::Shift => CGEventFlags::MaskShift,
            }
    })
}

fn key_code(key: &str) -> Option<u16> {
    let named = match key {
        "enter" | "return" => 36,
        "tab" => 48,
        "space" => 49,
        "backspace" | "delete" => 51,
        "escape" | "esc" => 53,
        "forwarddelete" => 117,
        "home" => 115,
        "end" => 119,
        "pageup" => 116,
        "pagedown" => 121,
        "left" => 123,
        "right" => 124,
        "down" => 125,
        "up" => 126,
        "f1" => 122,
        "f2" => 120,
        "f3" => 99,
        "f4" => 118,
        "f5" => 96,
        "f6" => 97,
        "f7" => 98,
        "f8" => 100,
        "f9" => 101,
        "f10" => 109,
        "f11" => 103,
        "f12" => 111,
        _ => return printable(key),
    };
    Some(named)
}

fn printable(key: &str) -> Option<u16> {
    let mut chars = key.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    let code = match c {
        'a' => 0,
        's' => 1,
        'd' => 2,
        'f' => 3,
        'h' => 4,
        'g' => 5,
        'z' => 6,
        'x' => 7,
        'c' => 8,
        'v' => 9,
        'b' => 11,
        'q' => 12,
        'w' => 13,
        'e' => 14,
        'r' => 15,
        'y' => 16,
        't' => 17,
        '1' => 18,
        '2' => 19,
        '3' => 20,
        '4' => 21,
        '6' => 22,
        '5' => 23,
        '=' => 24,
        '9' => 25,
        '7' => 26,
        '-' => 27,
        '8' => 28,
        '0' => 29,
        ']' => 30,
        'o' => 31,
        'u' => 32,
        '[' => 33,
        'i' => 34,
        'p' => 35,
        'l' => 37,
        'j' => 38,
        '\'' => 39,
        'k' => 40,
        ';' => 41,
        '\\' => 42,
        ',' => 43,
        '/' => 44,
        'n' => 45,
        'm' => 46,
        '.' => 47,
        '`' => 50,
        _ => return None,
    };
    Some(code)
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "screen coordinates are far inside f32's range"
)]
fn narrow(value: f64) -> f32 {
    value as f32
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is rounded and clamped to the u32 range first"
)]
fn pixel(value: f64) -> u32 {
    value.round().clamp(0.0, f64::from(u32::MAX)) as u32
}

pub(crate) fn missing(ask: bool) -> Vec<&'static str> {
    let mut missing = Vec::new();
    let is_trusted = if ask {
        let prompt = CFDictionary::from_slices(
            &[unsafe { kAXTrustedCheckOptionPrompt }],
            &[CFBoolean::new(true)],
        );
        unsafe { AXIsProcessTrustedWithOptions(Some(prompt.as_opaque())) }
    } else {
        unsafe { AXIsProcessTrusted() }
    };
    if !is_trusted {
        missing.push("Accessibility");
    }
    let can_capture = if ask {
        CGRequestScreenCaptureAccess()
    } else {
        CGPreflightScreenCaptureAccess()
    };
    if !can_capture {
        missing.push("Screen Recording");
    }
    let can_listen = if ask {
        CGRequestListenEventAccess()
    } else {
        CGPreflightListenEventAccess()
    };
    if !can_listen {
        missing.push("Input Monitoring");
    }
    missing
}

#[derive(Debug)]
pub(crate) struct Tap(Looping);

#[derive(Debug)]
struct Looping(CFRetained<CFRunLoop>);

unsafe impl Send for Looping {}
unsafe impl Sync for Looping {}

impl Drop for Tap {
    fn drop(&mut self) {
        self.0.0.stop();
    }
}

pub(crate) fn watch_escape(pressed: Arc<tokio::sync::Notify>) -> Option<Tap> {
    let (ready, started) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("goat-escape".to_owned())
        .spawn(move || {
            let watch = Box::into_raw(Box::new(pressed));
            let port = unsafe {
                CGEvent::tap_create(
                    CGEventTapLocation::SessionEventTap,
                    CGEventTapPlacement::HeadInsertEventTap,
                    CGEventTapOptions::ListenOnly,
                    1 << CGEventType::KeyDown.0,
                    Some(on_key),
                    watch.cast(),
                )
            };
            let looping = port.as_ref().and_then(|port| {
                let source = CFMachPort::new_run_loop_source(None, Some(port), 0)?;
                let run_loop = CFRunLoop::current()?;
                run_loop.add_source(Some(&source), unsafe { kCFRunLoopCommonModes });
                Some(run_loop)
            });
            let is_running = looping.is_some();
            ready.send(looping.map(Looping)).ok();
            if is_running {
                CFRunLoop::run();
            }
            if let Some(port) = port {
                CGEvent::tap_enable(&port, false);
            }
            drop(unsafe { Box::from_raw(watch) });
        })
        .ok()?;
    started.recv().ok().flatten().map(Tap)
}

unsafe extern "C-unwind" fn on_key(
    _proxy: CGEventTapProxy,
    kind: CGEventType,
    event: NonNull<CGEvent>,
    watch: *mut std::ffi::c_void,
) -> *mut CGEvent {
    if kind == CGEventType::KeyDown {
        let held = unsafe { event.as_ref() };
        let key = CGEvent::integer_value_field(Some(held), CGEventField::KeyboardEventKeycode);
        let source =
            CGEvent::integer_value_field(Some(held), CGEventField::EventSourceUnixProcessID);
        if key == ESCAPE_KEY && source != i64::from(std::process::id()) {
            unsafe { &*watch.cast::<Arc<tokio::sync::Notify>>() }.notify_waiters();
        }
    }
    event.as_ptr()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "needs a real macOS desktop with Accessibility access"]
    fn the_real_desktop_lists_windows_and_elements() {
        let dir = tempfile::tempdir().unwrap();
        let mut mac = Mac::new(dir.path().to_owned()).unwrap();

        let screen = mac.screen().unwrap();
        let window = screen
            .apps
            .iter()
            .find_map(|running| running.windows.first())
            .unwrap();
        let elements = mac.elements(Some(&window.id), None).unwrap();

        assert_ne!(screen.displays.len(), 0);
        assert_ne!(screen.apps.len(), 0);
        eprintln!(
            "apps: {:?}",
            screen
                .apps
                .iter()
                .map(|running| (&running.app.name, running.windows.len()))
                .collect::<Vec<_>>()
        );
        eprintln!("host: {:?}", mac.host_app());
        eprintln!(
            "focused: {:?} {:?}",
            screen.focused_app, screen.focused_window
        );
        eprintln!(
            "elements: {} e.g. {:?}",
            elements.len(),
            elements
                .iter()
                .take(8)
                .map(|e| (&e.reference, &e.role, &e.name))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    #[ignore = "needs a real macOS desktop with Screen Recording access"]
    fn the_real_desktop_takes_a_screenshot() {
        let dir = tempfile::tempdir().unwrap();
        let mut mac = Mac::new(dir.path().to_owned()).unwrap();

        let capture = mac.capture(None, None).unwrap();

        assert!(capture.width.max(capture.height) <= LONG_EDGE);
        assert!(capture.png.starts_with(b"\x89PNG"));
        eprintln!(
            "capture {}x{} scale {}",
            capture.width, capture.height, capture.scale
        );
    }

    #[test]
    #[ignore = "opens and quits Calculator on the real desktop"]
    fn the_real_calculator_takes_a_press() {
        let dir = tempfile::tempdir().unwrap();
        let mut mac = Mac::new(dir.path().to_owned()).unwrap();

        let app = mac.open(&Opening::App("Calculator".to_owned())).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(800));
        let window = mac
            .screen()
            .unwrap()
            .apps
            .into_iter()
            .find(|running| running.app == app)
            .and_then(|running| running.windows.into_iter().next())
            .unwrap();
        let before = mac.fingerprint(&app);
        let seven = mac
            .elements(Some(&window.id), Some("7"))
            .unwrap()
            .into_iter()
            .find(|element| element.role == "button")
            .unwrap();
        mac.press(&seven.reference, Button::Left).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let after = mac.fingerprint(&app);
        let shown = mac
            .elements(Some(&window.id), None)
            .unwrap()
            .into_iter()
            .filter_map(|element| element.value)
            .collect::<Vec<_>>();
        mac.quit(&app).unwrap();

        eprintln!(
            "app {app:?} window {} changed {} shown {shown:?}",
            window.id,
            before != after
        );
        assert!(shown.iter().any(|value| value.contains('7')));
    }

    #[tokio::test]
    #[ignore = "presses Esc in Calculator on the real desktop"]
    async fn the_real_escape_key_is_heard() {
        let dir = tempfile::tempdir().unwrap();
        let mut mac = Mac::new(dir.path().to_owned()).unwrap();
        let pressed = Arc::new(tokio::sync::Notify::new());
        let _tap = watch_escape(Arc::clone(&pressed)).unwrap();
        let app = mac.open(&Opening::App("Calculator".to_owned())).unwrap();
        assert_eq!(mac.focused_app().unwrap(), app);
        let heard = pressed.notified();

        mac.press_keys(&Chord {
            modifiers: Vec::new(),
            key: "escape".to_owned(),
        })
        .unwrap();
        let ignored_own =
            tokio::time::timeout(Duration::from_millis(300), pressed.notified()).await;
        std::process::Command::new("/usr/bin/osascript")
            .args(["-e", "tell application \"System Events\" to key code 53"])
            .status()
            .unwrap();
        let outcome = tokio::time::timeout(Duration::from_secs(2), heard).await;
        mac.quit(&app).unwrap();

        assert!(ignored_own.is_err());
        assert!(outcome.is_ok());
    }
}
