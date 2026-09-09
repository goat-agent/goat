use std::{thread, time::Duration};

use enigo::{Axis, Direction, Enigo, InputResult, Key, Keyboard, Mouse, Settings};
use goat_api::{MouseButton, Point};
use goat_computer_host_apps::NativeMouse;

use crate::DesktopError;

pub(super) struct Devices {
    enigo: Option<Enigo>,
    pub(super) mouse: NativeMouse,
}

impl Devices {
    pub(super) fn new() -> Result<Self, DesktopError> {
        let mouse =
            NativeMouse::new().map_err(|error| DesktopError::NotStarted(error.to_string()))?;
        Ok(Self { enigo: None, mouse })
    }
}

pub(super) fn coordinates(point: &Point) -> Result<(i32, i32), DesktopError> {
    fn coordinate(value: f64) -> Result<i32, DesktopError> {
        if !value.is_finite() || value < f64::from(i32::MIN) || value > f64::from(i32::MAX) {
            return Err(DesktopError::NotStarted(
                "coordinates must be finite display points in the supported range".into(),
            ));
        }
        #[allow(clippy::cast_possible_truncation)]
        Ok(value.round() as i32)
    }
    Ok((coordinate(point.x)?, coordinate(point.y)?))
}

pub(super) fn click(
    devices: &mut Devices,
    at: (i32, i32),
    button: &MouseButton,
    count: u8,
) -> Result<(), DesktopError> {
    if !(1..=3).contains(&count) {
        return Err(DesktopError::NotStarted(
            "click count must be 1, 2, or 3".into(),
        ));
    }
    perform(devices, |input| {
        input.move_to(at)?;
        for index in 0..count {
            if index > 0 {
                thread::sleep(Duration::from_millis(25));
            }
            input.mouse_event(|mouse| mouse.press(button, index + 1))?;
            input.button_up()?;
        }
        Ok(())
    })
}

pub(super) fn move_mouse(devices: &mut Devices, to: (i32, i32)) -> Result<(), DesktopError> {
    perform(devices, |input| input.move_to(to))
}

pub(super) fn drag(
    devices: &mut Devices,
    from: (i32, i32),
    to: (i32, i32),
) -> Result<(), DesktopError> {
    perform(devices, |input| {
        input.move_to(from)?;
        input.mouse_event(|mouse| mouse.press(&MouseButton::Left, 1))?;
        for step in 1..=8 {
            thread::sleep(Duration::from_millis(25));
            let x = i64::from(from.0) + (i64::from(to.0) - i64::from(from.0)) * step / 8;
            let y = i64::from(from.1) + (i64::from(to.1) - i64::from(from.1)) * step / 8;
            let x = i32::try_from(x)
                .map_err(|error| DesktopError::OutcomeUnknown(error.to_string()))?;
            let y = i32::try_from(y)
                .map_err(|error| DesktopError::OutcomeUnknown(error.to_string()))?;
            input.move_to((x, y))?;
        }
        input.button_up()
    })
}

pub(super) fn scroll(
    devices: &mut Devices,
    at: (i32, i32),
    dx: i32,
    dy: i32,
) -> Result<(), DesktopError> {
    if dx == i32::MIN || dy == i32::MIN {
        return Err(DesktopError::NotStarted(
            "scroll distances must be greater than i32::MIN".into(),
        ));
    }
    perform(devices, |input| {
        input.move_to(at)?;
        if dy != 0 {
            input.event(|enigo| enigo.scroll(dy, Axis::Vertical))?;
        }
        if dx != 0 {
            input.event(|enigo| enigo.scroll(dx, Axis::Horizontal))?;
        }
        Ok(())
    })
}

pub(super) fn text(devices: &mut Devices, text: &str) -> Result<(), DesktopError> {
    if text.is_empty() {
        return Ok(());
    }
    perform(devices, |input| {
        for (index, segment) in text.split('\t').enumerate() {
            if index > 0 {
                input.event(|enigo| enigo.key(Key::Tab, Direction::Press))?;
                input.keys.push(Key::Tab);
                input.event(|enigo| enigo.key(Key::Tab, Direction::Release))?;
                input.keys.pop();
            }
            if !segment.is_empty() {
                input.started = true;
                input.event(|enigo| enigo.text(segment))?;
            }
        }
        Ok(())
    })
}

pub(super) fn chord(devices: &mut Devices, keys: &[Key]) -> Result<(), DesktopError> {
    perform(devices, |input| {
        for &key in keys {
            input.event(|enigo| enigo.key(key, Direction::Press))?;
            input.keys.push(key);
        }
        Ok(())
    })
}

pub(super) fn parse_chord(chord: &str) -> Result<Vec<Key>, DesktopError> {
    let mut keys = Vec::with_capacity(6);
    let mut has_key = false;
    for token in chord.split('+') {
        let modifier = match token {
            "cmd" => Some(Key::Meta),
            "ctrl" => Some(Key::Control),
            "alt" => Some(Key::Alt),
            "shift" => Some(Key::Shift),
            "fn" => Some(Key::Function),
            _ => None,
        };
        let key = if let Some(modifier) = modifier {
            modifier
        } else {
            match token {
                "enter" | "return" => Key::Return,
                "tab" => Key::Tab,
                "escape" | "esc" => Key::Escape,
                "space" => Key::Space,
                "backspace" => Key::Backspace,
                "delete" => Key::Delete,
                "up" => Key::UpArrow,
                "down" => Key::DownArrow,
                "left" => Key::LeftArrow,
                "right" => Key::RightArrow,
                "home" => Key::Home,
                "end" => Key::End,
                "pageup" => Key::PageUp,
                "pagedown" => Key::PageDown,
                "f1" => Key::F1,
                "f2" => Key::F2,
                "f3" => Key::F3,
                "f4" => Key::F4,
                "f5" => Key::F5,
                "f6" => Key::F6,
                "f7" => Key::F7,
                "f8" => Key::F8,
                "f9" => Key::F9,
                "f10" => Key::F10,
                "f11" => Key::F11,
                "f12" => Key::F12,
                _ => {
                    let mut chars = token.chars();
                    match (chars.next(), chars.next()) {
                        (Some(character), None)
                            if !character.is_control()
                                && !character.is_whitespace()
                                && !character.is_uppercase() =>
                        {
                            Key::Unicode(character)
                        }
                        _ => {
                            return Err(DesktopError::NotStarted(format!(
                                "unknown key token {token:?}"
                            )));
                        }
                    }
                }
            }
        };
        if has_key || keys.contains(&key) {
            return Err(DesktopError::NotStarted(
                "key chord must contain distinct modifiers followed by exactly one key".into(),
            ));
        }
        has_key = modifier.is_none();
        keys.push(key);
    }
    if !has_key {
        return Err(DesktopError::NotStarted(
            "key chord must contain a non-modifier key".into(),
        ));
    }
    Ok(keys)
}

struct Input<'a> {
    enigo: &'a mut Enigo,
    mouse: &'a mut NativeMouse,
    started: bool,
    keys: Vec<Key>,
}

impl Input<'_> {
    fn event(
        &mut self,
        action: impl FnOnce(&mut Enigo) -> InputResult<()>,
    ) -> Result<(), DesktopError> {
        action(self.enigo).map_err(|error| {
            if self.started {
                DesktopError::OutcomeUnknown(error.to_string())
            } else {
                DesktopError::NotStarted(error.to_string())
            }
        })?;
        self.started = true;
        Ok(())
    }

    fn mouse_event(
        &mut self,
        action: impl FnOnce(&mut NativeMouse) -> Result<(), goat_computer_host_apps::NativeError>,
    ) -> Result<(), DesktopError> {
        action(self.mouse).map_err(|error| {
            if self.started {
                DesktopError::OutcomeUnknown(error.to_string())
            } else {
                DesktopError::NotStarted(error.to_string())
            }
        })?;
        self.started = true;
        Ok(())
    }

    fn move_to(&mut self, at: (i32, i32)) -> Result<(), DesktopError> {
        self.mouse_event(|mouse| mouse.move_to(at.0, at.1))
    }

    fn button_up(&mut self) -> Result<(), DesktopError> {
        self.mouse_event(NativeMouse::release)
    }

    fn release_all(&mut self) -> Result<(), DesktopError> {
        let mut errors = Vec::new();
        if let Err(error) = self.mouse.release() {
            errors.push(error.to_string());
        }
        for key in self.keys.drain(..).rev() {
            if let Err(error) = self.enigo.key(key, Direction::Release) {
                errors.push(error.to_string());
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(DesktopError::OutcomeUnknown(format!(
                "could not release native input: {}",
                errors.join("; ")
            )))
        }
    }
}

impl Drop for Input<'_> {
    fn drop(&mut self) {
        let _ = self.release_all();
    }
}

fn perform(
    devices: &mut Devices,
    action: impl FnOnce(&mut Input<'_>) -> Result<(), DesktopError>,
) -> Result<(), DesktopError> {
    if devices.enigo.is_none() {
        let settings = Settings {
            open_prompt_to_get_permissions: false,
            ..Settings::default()
        };
        devices.enigo = Some(
            Enigo::new(&settings).map_err(|error| DesktopError::NotStarted(error.to_string()))?,
        );
    }
    let mut input = Input {
        enigo: devices.enigo.as_mut().expect("input initialized"),
        mouse: &mut devices.mouse,
        started: false,
        keys: Vec::new(),
    };
    let result = action(&mut input);
    match (result, input.release_all()) {
        (Ok(()), cleanup) => cleanup,
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup)) => {
            Err(DesktopError::OutcomeUnknown(format!("{error}; {cleanup}")))
        }
    }
}
