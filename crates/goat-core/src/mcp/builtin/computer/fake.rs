use std::sync::{Arc, Mutex};

use super::desktop::{
    App, Button, Capture, Chord, Display, Element, Fault, Opening, Platform, Pointer, Rect,
    Running, Screen, Window, WindowState,
};

#[derive(Debug, Default)]
pub(crate) struct Fake {
    pub(crate) log: Arc<Mutex<Vec<String>>>,
    generation: u32,
    version: u64,
    is_secure: bool,
    text: String,
}

impl Fake {
    pub(crate) fn new(log: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            log,
            ..Self::default()
        }
    }

    pub(crate) fn with_secure_input(mut self) -> Self {
        self.is_secure = true;
        self
    }

    fn record(&mut self, entry: String) {
        self.version += 1;
        self.log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(entry);
    }

    fn current(&self, reference: &str) -> Result<usize, Fault> {
        let (generation, index) = reference
            .strip_prefix('e')
            .and_then(|rest| rest.split_once('.'))
            .and_then(|(generation, index)| {
                Some((
                    generation.parse::<u32>().ok()?,
                    index.parse::<usize>().ok()?,
                ))
            })
            .ok_or_else(|| Fault::Missing(format!("{reference} is not a ref")))?;
        if generation != self.generation || index >= ELEMENTS.len() {
            return Err(Fault::Stale(format!("{reference} is stale")));
        }
        Ok(index)
    }
}

const ELEMENTS: &[(&str, &str, f64, f64)] = &[
    ("button", "Save", 100.0, 40.0),
    ("text field", "Title", 200.0, 80.0),
];

fn notes() -> App {
    App {
        name: "Notes".to_owned(),
        id: "com.apple.Notes".to_owned(),
    }
}

fn terminal() -> App {
    App {
        name: "Terminal".to_owned(),
        id: "com.apple.Terminal".to_owned(),
    }
}

impl Platform for Fake {
    fn screen(&mut self) -> Result<Screen, Fault> {
        Ok(Screen {
            displays: vec![Display {
                id: "1".to_owned(),
                frame: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 1440.0,
                    height: 900.0,
                },
                scale: 2.0,
            }],
            focused_app: Some(notes()),
            focused_window: Some("w1".to_owned()),
            apps: vec![Running {
                app: notes(),
                windows: vec![Window {
                    id: "w1".to_owned(),
                    title: "Groceries".to_owned(),
                    frame: Rect {
                        x: 0.0,
                        y: 0.0,
                        width: 800.0,
                        height: 600.0,
                    },
                    minimized: false,
                }],
            }],
        })
    }

    fn elements(
        &mut self,
        _window: Option<&str>,
        find: Option<&str>,
    ) -> Result<Vec<Element>, Fault> {
        self.generation += 1;
        let generation = self.generation;
        Ok(ELEMENTS
            .iter()
            .enumerate()
            .filter(|(_, (role, name, _, _))| {
                find.is_none_or(|find| {
                    let find = find.to_lowercase();
                    name.to_lowercase().contains(&find) || role.contains(&find)
                })
            })
            .map(|(index, (role, name, x, y))| Element {
                reference: format!("e{generation}.{index}"),
                role: (*role).to_owned(),
                name: (*name).to_owned(),
                value: (*role == "text field").then(|| self.text.clone()),
                frame: Rect {
                    x: *x,
                    y: *y,
                    width: 80.0,
                    height: 20.0,
                },
                enabled: true,
                focused: false,
            })
            .collect())
    }

    fn element_at(&mut self, x: f64, y: f64) -> Result<Option<Element>, Fault> {
        let found = self.elements(None, None)?;
        Ok(found.into_iter().find(|element| {
            x >= element.frame.x
                && x <= element.frame.x + element.frame.width
                && y >= element.frame.y
                && y <= element.frame.y + element.frame.height
        }))
    }

    fn capture(&mut self, _window: Option<&str>, _region: Option<Rect>) -> Result<Capture, Fault> {
        Ok(Capture {
            png: b"\x89PNG".to_vec(),
            width: 1440,
            height: 900,
            origin_x: 0.0,
            origin_y: 0.0,
            scale: 2.0,
        })
    }

    fn owner(&mut self, reference: &str) -> Result<(App, Rect), Fault> {
        let index = self.current(reference)?;
        let (_, _, x, y) = ELEMENTS[index];
        Ok((
            notes(),
            Rect {
                x,
                y,
                width: 80.0,
                height: 20.0,
            },
        ))
    }

    fn app_at(&mut self, x: f64, _y: f64) -> Result<App, Fault> {
        Ok(if x > 1000.0 { terminal() } else { notes() })
    }

    fn window_app(&mut self, window: &str) -> Result<App, Fault> {
        if window == "w1" {
            Ok(notes())
        } else {
            Err(Fault::Missing(format!("no window {window}")))
        }
    }

    fn focused_app(&mut self) -> Result<App, Fault> {
        Ok(notes())
    }

    fn press(&mut self, reference: &str, button: Button) -> Result<(), Fault> {
        let index = self.current(reference)?;
        self.record(format!("press {} {button:?}", ELEMENTS[index].1));
        Ok(())
    }

    fn set_text(&mut self, reference: &str, text: &str) -> Result<(), Fault> {
        self.current(reference)?;
        self.text = text.to_owned();
        self.record(format!("set {text}"));
        Ok(())
    }

    fn pointer(&mut self, input: &Pointer) -> Result<(), Fault> {
        self.record(format!("pointer {input:?}"));
        Ok(())
    }

    fn type_text(&mut self, text: &str) -> Result<(), Fault> {
        self.record(format!("type {text}"));
        Ok(())
    }

    fn press_keys(&mut self, chord: &Chord) -> Result<(), Fault> {
        self.record(format!("keys {:?}+{}", chord.modifiers, chord.key));
        Ok(())
    }

    fn open(&mut self, opening: &Opening) -> Result<App, Fault> {
        self.record(format!("open {opening:?}"));
        Ok(notes())
    }

    fn quit(&mut self, app: &App) -> Result<(), Fault> {
        self.record(format!("quit {}", app.name));
        Ok(())
    }

    fn set_window(
        &mut self,
        window: &str,
        state: Option<WindowState>,
        frame: Option<Rect>,
    ) -> Result<(), Fault> {
        self.record(format!("window {window} {state:?} {frame:?}"));
        Ok(())
    }

    fn read_clipboard(&mut self) -> Result<String, Fault> {
        Ok("copied".to_owned())
    }

    fn write_clipboard(&mut self, text: &str) -> Result<(), Fault> {
        self.record(format!("clipboard {text}"));
        Ok(())
    }

    fn fingerprint(&mut self, _app: &App) -> u64 {
        self.version
    }

    fn is_secure_input(&mut self) -> bool {
        self.is_secure
    }

    fn host_app(&mut self) -> Option<App> {
        Some(terminal())
    }
}
