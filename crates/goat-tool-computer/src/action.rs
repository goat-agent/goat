use serde_json::Value;

use crate::error::ComputerError;

#[derive(Debug, Clone)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

impl Modifiers {
    pub fn none() -> Self {
        Self {
            shift: false,
            ctrl: false,
            alt: false,
            meta: false,
        }
    }

    fn set(&mut self, token: &str) {
        match token.trim().to_ascii_lowercase().as_str() {
            "shift" => self.shift = true,
            "ctrl" | "control" => self.ctrl = true,
            "alt" | "option" => self.alt = true,
            "meta" | "super" | "cmd" | "command" | "win" => self.meta = true,
            _ => {}
        }
    }

    pub fn from_keys(keys: &[Value]) -> Self {
        let mut m = Self::none();
        for k in keys {
            if let Some(s) = k.as_str() {
                m.set(s);
            }
        }
        m
    }
}

#[derive(Debug, Clone)]
pub enum Action {
    Screenshot,
    MouseMove {
        x: i32,
        y: i32,
    },
    LeftClick {
        x: i32,
        y: i32,
        modifiers: Modifiers,
    },
    RightClick {
        x: i32,
        y: i32,
        modifiers: Modifiers,
    },
    MiddleClick {
        x: i32,
        y: i32,
        modifiers: Modifiers,
    },
    DoubleClick {
        x: i32,
        y: i32,
        modifiers: Modifiers,
    },
    TripleClick {
        x: i32,
        y: i32,
        modifiers: Modifiers,
    },
    MouseDown {
        x: i32,
        y: i32,
    },
    MouseUp {
        x: i32,
        y: i32,
    },
    Drag {
        path: Vec<(i32, i32)>,
        modifiers: Modifiers,
    },
    Scroll {
        x: i32,
        y: i32,
        dx: i32,
        dy: i32,
        modifiers: Modifiers,
    },
    Type {
        text: String,
    },
    Key {
        combo: String,
    },
    HoldKey {
        key: String,
        duration_ms: u64,
    },
    Wait {
        duration_ms: u64,
    },
    Zoom {
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
    },
}

fn xy(v: &Value) -> (i32, i32) {
    (
        v["x"].as_i64().unwrap_or(0) as i32,
        v["y"].as_i64().unwrap_or(0) as i32,
    )
}

fn mods(v: &Value) -> Modifiers {
    v.get("modifiers")
        .and_then(Value::as_array)
        .map_or_else(Modifiers::none, |arr| Modifiers::from_keys(arr))
}

fn missing(field: &str) -> ComputerError {
    ComputerError::InvalidInput(format!("missing field: {field}"))
}

pub fn parse(input: &str) -> Result<Action, ComputerError> {
    let v: Value =
        serde_json::from_str(input).map_err(|e| ComputerError::InvalidInput(e.to_string()))?;
    let action = v["action"].as_str().ok_or_else(|| missing("action"))?;
    let m = mods(&v);

    match action {
        "screenshot" => Ok(Action::Screenshot),
        "move" => {
            let (x, y) = xy(&v);
            Ok(Action::MouseMove { x, y })
        }
        "click" => {
            let (x, y) = xy(&v);
            match v["button"].as_str().unwrap_or("left") {
                "right" => Ok(Action::RightClick { x, y, modifiers: m }),
                "middle" => Ok(Action::MiddleClick { x, y, modifiers: m }),
                _ => Ok(Action::LeftClick { x, y, modifiers: m }),
            }
        }
        "double_click" => {
            let (x, y) = xy(&v);
            Ok(Action::DoubleClick { x, y, modifiers: m })
        }
        "triple_click" => {
            let (x, y) = xy(&v);
            Ok(Action::TripleClick { x, y, modifiers: m })
        }
        "mouse_down" => {
            let (x, y) = xy(&v);
            Ok(Action::MouseDown { x, y })
        }
        "mouse_up" => {
            let (x, y) = xy(&v);
            Ok(Action::MouseUp { x, y })
        }
        "drag" => {
            let path = v["path"]
                .as_array()
                .ok_or_else(|| missing("path"))?
                .iter()
                .filter_map(|pt| Some((pt["x"].as_i64()? as i32, pt["y"].as_i64()? as i32)))
                .collect();
            Ok(Action::Drag { path, modifiers: m })
        }
        "scroll" => {
            let (x, y) = xy(&v);
            let dx = v["dx"].as_i64().unwrap_or(0) as i32;
            let dy = v["dy"].as_i64().unwrap_or(0) as i32;
            Ok(Action::Scroll {
                x,
                y,
                dx,
                dy,
                modifiers: m,
            })
        }
        "type" => {
            let text = v["text"]
                .as_str()
                .ok_or_else(|| missing("text"))?
                .to_owned();
            Ok(Action::Type { text })
        }
        "key" => {
            let combo = match v["keys"].as_array() {
                Some(keys) => keys
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("+"),
                None => v["combo"]
                    .as_str()
                    .ok_or_else(|| missing("keys"))?
                    .to_owned(),
            };
            Ok(Action::Key { combo })
        }
        "hold_key" => {
            let key = v["key"].as_str().ok_or_else(|| missing("key"))?.to_owned();
            let duration_ms = v["ms"].as_u64().unwrap_or(500);
            Ok(Action::HoldKey { key, duration_ms })
        }
        "wait" => Ok(Action::Wait {
            duration_ms: v["ms"].as_u64().unwrap_or(1000),
        }),
        "zoom" => Ok(Action::Zoom {
            x1: v["x1"].as_i64().unwrap_or(0) as i32,
            y1: v["y1"].as_i64().unwrap_or(0) as i32,
            x2: v["x2"].as_i64().unwrap_or(0) as i32,
            y2: v["y2"].as_i64().unwrap_or(0) as i32,
        }),
        other => Err(ComputerError::InvalidInput(format!(
            "unknown action: {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, Modifiers, parse};

    #[test]
    fn screenshot_parses() {
        assert!(matches!(
            parse(r#"{"action":"screenshot"}"#).unwrap(),
            Action::Screenshot
        ));
    }

    #[test]
    fn click_routes_button() {
        assert!(matches!(
            parse(r#"{"action":"click","x":1,"y":2}"#).unwrap(),
            Action::LeftClick { x: 1, y: 2, .. }
        ));
        assert!(matches!(
            parse(r#"{"action":"click","x":1,"y":2,"button":"right"}"#).unwrap(),
            Action::RightClick { .. }
        ));
        assert!(matches!(
            parse(r#"{"action":"click","x":1,"y":2,"button":"middle"}"#).unwrap(),
            Action::MiddleClick { .. }
        ));
    }

    #[test]
    fn key_prefers_keys_array_then_combo() {
        let combo = match parse(r#"{"action":"key","keys":["ctrl","c"]}"#).unwrap() {
            Action::Key { combo } => combo,
            other => panic!("expected key, got {other:?}"),
        };
        assert_eq!(combo, "ctrl+c");
        let combo = match parse(r#"{"action":"key","combo":"cmd+v"}"#).unwrap() {
            Action::Key { combo } => combo,
            other => panic!("expected key, got {other:?}"),
        };
        assert_eq!(combo, "cmd+v");
    }

    #[test]
    fn missing_required_fields_error() {
        assert!(parse(r#"{"action":"type"}"#).is_err());
        assert!(parse(r#"{"action":"drag"}"#).is_err());
        assert!(parse(r#"{"action":"hold_key"}"#).is_err());
        assert!(parse(r#"{"action":"key"}"#).is_err());
        assert!(parse("{}").is_err());
        assert!(parse(r#"{"action":"nope"}"#).is_err());
    }

    #[test]
    fn wait_and_hold_defaults() {
        assert!(matches!(
            parse(r#"{"action":"wait"}"#).unwrap(),
            Action::Wait { duration_ms: 1000 }
        ));
        assert!(matches!(
            parse(r#"{"action":"hold_key","key":"a"}"#).unwrap(),
            Action::HoldKey {
                duration_ms: 500,
                ..
            }
        ));
    }

    #[test]
    fn modifier_aliases_map_to_meta() {
        let arr: Vec<serde_json::Value> = ["cmd", "super", "win", "command"]
            .iter()
            .map(|s| serde_json::Value::from(*s))
            .collect();
        let m = Modifiers::from_keys(&arr);
        assert!(m.meta);
        assert!(!m.ctrl);
    }

    #[test]
    fn drag_collects_path_points() {
        let path = match parse(r#"{"action":"drag","path":[{"x":1,"y":2},{"x":3,"y":4}]}"#).unwrap()
        {
            Action::Drag { path, .. } => path,
            other => panic!("expected drag, got {other:?}"),
        };
        assert_eq!(path, vec![(1, 2), (3, 4)]);
    }
}
