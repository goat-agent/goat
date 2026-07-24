#[cfg(not(target_os = "linux"))]
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
#[cfg(not(target_os = "linux"))]
use image::{DynamicImage, ImageFormat};

use crate::{action::Action, error::ComputerError};

pub struct Image {
    pub media_type: String,
    pub data: String,
}

pub trait ComputerBackend: Send + Sync {
    fn display_size(&self) -> (u32, u32);
    fn screenshot(&self) -> Result<Image, ComputerError>;
    fn screenshot_region(&self, x1: i32, y1: i32, x2: i32, y2: i32)
    -> Result<Image, ComputerError>;
    fn execute(&self, action: &Action) -> Result<(), ComputerError>;
}

pub struct DesktopBackend {
    inner: desktop::DesktopBackendImpl,
}

impl DesktopBackend {
    pub fn new() -> Result<Self, ComputerError> {
        Ok(Self {
            inner: desktop::DesktopBackendImpl::new()?,
        })
    }
}

impl ComputerBackend for DesktopBackend {
    fn display_size(&self) -> (u32, u32) {
        self.inner.display_size()
    }

    fn screenshot(&self) -> Result<Image, ComputerError> {
        self.inner.screenshot()
    }

    fn screenshot_region(
        &self,
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
    ) -> Result<Image, ComputerError> {
        self.inner.screenshot_region(x1, y1, x2, y2)
    }

    fn execute(&self, action: &Action) -> Result<(), ComputerError> {
        self.inner.execute(action)
    }
}

#[cfg(not(target_os = "linux"))]
fn encode_png(img: &DynamicImage) -> Result<Image, ComputerError> {
    let mut buf = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut buf), ImageFormat::Png)?;
    Ok(Image {
        media_type: "image/png".to_owned(),
        data: BASE64.encode(&buf),
    })
}

#[cfg(target_os = "linux")]
mod desktop {
    use crate::{action::Action, backend::Image, error::ComputerError};

    pub struct DesktopBackendImpl {
        message: &'static str,
    }

    impl DesktopBackendImpl {
        pub fn new() -> Result<Self, ComputerError> {
            Err(ComputerError::Unavailable(Self::message().to_owned()))
        }

        pub fn display_size(&self) -> (u32, u32) {
            if self.message.is_empty() {
                (1, 1)
            } else {
                (0, 0)
            }
        }

        pub fn screenshot(&self) -> Result<Image, ComputerError> {
            Err(self.unavailable())
        }

        pub fn screenshot_region(
            &self,
            _x1: i32,
            _y1: i32,
            _x2: i32,
            _y2: i32,
        ) -> Result<Image, ComputerError> {
            Err(self.unavailable())
        }

        pub fn execute(&self, _action: &Action) -> Result<(), ComputerError> {
            Err(self.unavailable())
        }

        fn unavailable(&self) -> ComputerError {
            ComputerError::Unavailable(self.message.to_owned())
        }

        fn message() -> &'static str {
            "Linux computer use is unavailable in this build"
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod desktop {
    use std::thread::sleep;
    use std::time::Duration;

    use enigo::{
        Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard as _, Mouse as _, Settings,
    };
    use image::{DynamicImage, imageops::FilterType};
    use xcap::Monitor;

    use crate::{
        action::{Action, Modifiers},
        backend::{Image, encode_png},
        error::ComputerError,
    };

    const TARGET_WIDTH: u32 = 1366;
    const TARGET_HEIGHT: u32 = 768;
    const MAX_WAIT_MS: u64 = 60_000;

    fn clamp_wait(ms: u64) -> Duration {
        Duration::from_millis(ms.min(MAX_WAIT_MS))
    }

    pub struct DesktopBackendImpl {
        target_w: u32,
        target_h: u32,
        scale: f64,
        enigo: std::sync::Mutex<Option<Enigo>>,
    }

    impl DesktopBackendImpl {
        pub fn new() -> Result<Self, ComputerError> {
            let mon = primary_monitor()?;
            let real_w = mon.width()?;
            let real_h = mon.height()?;

            let scale = f64::max(
                f64::from(real_w) / f64::from(TARGET_WIDTH),
                f64::from(real_h) / f64::from(TARGET_HEIGHT),
            )
            .max(1.0);

            let target_w = (f64::from(real_w) / scale).round() as u32;
            let target_h = (f64::from(real_h) / scale).round() as u32;

            Ok(Self {
                target_w,
                target_h,
                scale,
                enigo: std::sync::Mutex::new(None),
            })
        }

        pub fn display_size(&self) -> (u32, u32) {
            (self.target_w, self.target_h)
        }

        pub fn screenshot(&self) -> Result<Image, ComputerError> {
            self.capture_and_encode()
        }

        pub fn screenshot_region(
            &self,
            x1: i32,
            y1: i32,
            x2: i32,
            y2: i32,
        ) -> Result<Image, ComputerError> {
            self.capture_region_encode(x1, y1, x2, y2)
        }

        pub fn execute(&self, action: &Action) -> Result<(), ComputerError> {
            match action {
                Action::Screenshot | Action::Zoom { .. } => Ok(()),
                Action::Wait { duration_ms } => {
                    sleep(clamp_wait(*duration_ms));
                    Ok(())
                }
                _ => self.execute_input(action),
            }
        }

        fn real_coord(&self, x: i32, y: i32) -> (i32, i32) {
            (
                (f64::from(x) * self.scale).round() as i32,
                (f64::from(y) * self.scale).round() as i32,
            )
        }

        fn capture_and_encode(&self) -> Result<Image, ComputerError> {
            let raw = primary_monitor()?.capture_image()?;
            let full = DynamicImage::ImageRgba8(raw);
            let scaled = if full.width() > self.target_w || full.height() > self.target_h {
                full.resize(self.target_w, self.target_h, FilterType::Lanczos3)
            } else {
                full
            };
            encode_png(&scaled)
        }

        fn capture_region_encode(
            &self,
            x1: i32,
            y1: i32,
            x2: i32,
            y2: i32,
        ) -> Result<Image, ComputerError> {
            let (rx1, ry1) = self.real_coord(x1.min(x2).max(0), y1.min(y2).max(0));
            let (rx2, ry2) = self.real_coord(x1.max(x2).max(0), y1.max(y2).max(0));
            let w = (rx2 - rx1).max(1) as u32;
            let h = (ry2 - ry1).max(1) as u32;

            let raw = primary_monitor()?.capture_region(rx1 as u32, ry1 as u32, w, h)?;
            encode_png(&DynamicImage::ImageRgba8(raw))
        }

        fn execute_input(&self, action: &Action) -> Result<(), ComputerError> {
            let mut guard = self
                .enigo
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let enigo = match guard.as_mut() {
                Some(enigo) => enigo,
                None => guard.insert(Enigo::new(&Settings::default())?),
            };
            match action {
                Action::Screenshot | Action::Zoom { .. } | Action::Wait { .. } => {}

                Action::MouseMove { x, y } => {
                    let (rx, ry) = self.real_coord(*x, *y);
                    enigo.move_mouse(rx, ry, Coordinate::Abs)?;
                }

                Action::LeftClick { x, y, modifiers } => {
                    self.click(&mut *enigo, Button::Left, *x, *y, modifiers, 1)?;
                }
                Action::RightClick { x, y, modifiers } => {
                    self.click(&mut *enigo, Button::Right, *x, *y, modifiers, 1)?;
                }
                Action::MiddleClick { x, y, modifiers } => {
                    self.click(&mut *enigo, Button::Middle, *x, *y, modifiers, 1)?;
                }
                Action::DoubleClick { x, y, modifiers } => {
                    self.click(&mut *enigo, Button::Left, *x, *y, modifiers, 2)?;
                }
                Action::TripleClick { x, y, modifiers } => {
                    self.click(&mut *enigo, Button::Left, *x, *y, modifiers, 3)?;
                }

                Action::MouseDown { x, y } => {
                    let (rx, ry) = self.real_coord(*x, *y);
                    enigo.move_mouse(rx, ry, Coordinate::Abs)?;
                    enigo.button(Button::Left, Direction::Press)?;
                }
                Action::MouseUp { x, y } => {
                    let (rx, ry) = self.real_coord(*x, *y);
                    enigo.move_mouse(rx, ry, Coordinate::Abs)?;
                    enigo.button(Button::Left, Direction::Release)?;
                }

                Action::Drag { path, modifiers } => {
                    let Some(&(fx, fy)) = path.first() else {
                        return Ok(());
                    };
                    press_modifiers(&mut *enigo, modifiers)?;
                    let (sx, sy) = self.real_coord(fx, fy);
                    enigo.move_mouse(sx, sy, Coordinate::Abs)?;
                    enigo.button(Button::Left, Direction::Press)?;
                    for &(px, py) in &path[1..] {
                        let (rx, ry) = self.real_coord(px, py);
                        enigo.move_mouse(rx, ry, Coordinate::Abs)?;
                    }
                    enigo.button(Button::Left, Direction::Release)?;
                    release_modifiers(&mut *enigo, modifiers)?;
                }

                Action::Scroll {
                    x,
                    y,
                    dx,
                    dy,
                    modifiers,
                } => {
                    let (rx, ry) = self.real_coord(*x, *y);
                    press_modifiers(&mut *enigo, modifiers)?;
                    enigo.move_mouse(rx, ry, Coordinate::Abs)?;
                    if *dy != 0 {
                        enigo.scroll(*dy, Axis::Vertical)?;
                    }
                    if *dx != 0 {
                        enigo.scroll(*dx, Axis::Horizontal)?;
                    }
                    release_modifiers(&mut *enigo, modifiers)?;
                }

                Action::Type { text } => enigo.text(text)?,

                Action::Key { combo } => execute_key_combo(&mut *enigo, combo)?,

                Action::HoldKey { key, duration_ms } => {
                    let k = parse_key_name(key)
                        .ok_or_else(|| ComputerError::UnknownKey(key.clone()))?;
                    enigo.key(k, Direction::Press)?;
                    sleep(clamp_wait(*duration_ms));
                    enigo.key(k, Direction::Release)?;
                }
            }
            Ok(())
        }

        fn click(
            &self,
            enigo: &mut Enigo,
            button: Button,
            x: i32,
            y: i32,
            modifiers: &Modifiers,
            times: u32,
        ) -> Result<(), ComputerError> {
            let (rx, ry) = self.real_coord(x, y);
            press_modifiers(enigo, modifiers)?;
            enigo.move_mouse(rx, ry, Coordinate::Abs)?;
            for _ in 0..times {
                enigo.button(button, Direction::Click)?;
            }
            release_modifiers(enigo, modifiers)?;
            Ok(())
        }
    }

    fn primary_monitor() -> Result<Monitor, ComputerError> {
        Monitor::all()?
            .into_iter()
            .next()
            .ok_or(ComputerError::NoMonitor)
    }

    fn press_modifiers(enigo: &mut Enigo, m: &Modifiers) -> Result<(), ComputerError> {
        for key in modifier_keys(m) {
            enigo.key(key, Direction::Press)?;
        }
        Ok(())
    }

    fn release_modifiers(enigo: &mut Enigo, m: &Modifiers) -> Result<(), ComputerError> {
        for key in modifier_keys(m).into_iter().rev() {
            enigo.key(key, Direction::Release)?;
        }
        Ok(())
    }

    fn modifier_keys(m: &Modifiers) -> Vec<Key> {
        let mut keys = Vec::new();
        if m.shift {
            keys.push(Key::Shift);
        }
        if m.ctrl {
            keys.push(Key::Control);
        }
        if m.alt {
            keys.push(Key::Alt);
        }
        if m.meta {
            keys.push(Key::Meta);
        }
        keys
    }

    fn execute_key_combo(enigo: &mut Enigo, combo: &str) -> Result<(), ComputerError> {
        let parts: Vec<&str> = combo.split('+').collect();
        let (modifiers, main) = parts.split_at(parts.len() - 1);

        let mod_keys: Vec<Key> = modifiers
            .iter()
            .map(|s| parse_key_name(s).ok_or_else(|| ComputerError::UnknownKey((*s).to_owned())))
            .collect::<Result<_, _>>()?;
        let main_key =
            parse_key_name(main[0]).ok_or_else(|| ComputerError::UnknownKey(main[0].to_owned()))?;

        for &k in &mod_keys {
            enigo.key(k, Direction::Press)?;
        }
        enigo.key(main_key, Direction::Click)?;
        for &k in mod_keys.iter().rev() {
            enigo.key(k, Direction::Release)?;
        }
        Ok(())
    }

    fn parse_key_name(s: &str) -> Option<Key> {
        match s.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => Some(Key::Control),
            "shift" => Some(Key::Shift),
            "alt" | "option" => Some(Key::Alt),
            "super" | "meta" | "cmd" | "command" | "win" | "windows" => Some(Key::Meta),
            "return" | "enter" => Some(Key::Return),
            "tab" => Some(Key::Tab),
            "escape" | "esc" => Some(Key::Escape),
            "space" => Some(Key::Space),
            "backspace" | "back" => Some(Key::Backspace),
            "delete" | "del" => Some(Key::Delete),
            "up" | "uparrow" => Some(Key::UpArrow),
            "down" | "downarrow" => Some(Key::DownArrow),
            "left" | "leftarrow" => Some(Key::LeftArrow),
            "right" | "rightarrow" => Some(Key::RightArrow),
            "home" => Some(Key::Home),
            "end" => Some(Key::End),
            "pageup" | "prior" | "page_up" => Some(Key::PageUp),
            "pagedown" | "next" | "page_down" => Some(Key::PageDown),
            "f1" => Some(Key::F1),
            "f2" => Some(Key::F2),
            "f3" => Some(Key::F3),
            "f4" => Some(Key::F4),
            "f5" => Some(Key::F5),
            "f6" => Some(Key::F6),
            "f7" => Some(Key::F7),
            "f8" => Some(Key::F8),
            "f9" => Some(Key::F9),
            "f10" => Some(Key::F10),
            "f11" => Some(Key::F11),
            "f12" => Some(Key::F12),
            other => {
                let mut chars = other.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => Some(Key::Unicode(c)),
                    _ => None,
                }
            }
        }
    }
}
