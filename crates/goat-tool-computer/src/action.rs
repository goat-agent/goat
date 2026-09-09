use goat_tool::ToolError;
use serde::Deserialize;
use serde_json::{Map, Value};

pub(crate) struct Request {
    pub action: Action,
    pub observe: bool,
    pub max_width: u32,
}

impl Request {
    pub fn parse(input: &str) -> Result<Self, ToolError> {
        let mut fields: Map<String, Value> = serde_json::from_str(input)?;
        let observe = fields
            .remove("observe")
            .map(serde_json::from_value)
            .transpose()?
            .unwrap_or(true);
        let max_width = fields
            .remove("max_width")
            .map(serde_json::from_value)
            .transpose()?
            .unwrap_or(1280);
        if max_width == 0 {
            return Err(ToolError::invalid_input(
                "max_width must be greater than zero",
            ));
        }
        let action = serde_json::from_value(Value::Object(fields))?;
        Ok(Self {
            action,
            observe,
            max_width,
        })
    }
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Action {
    Screenshot {},
    Zoom {
        region: [f64; 4],
    },
    Snapshot {
        app: Option<String>,
    },
    Apps {},
    FocusApp {
        app: String,
    },
    Click(Target),
    DoubleClick(Target),
    TripleClick(Target),
    RightClick(Target),
    Move {
        x: f64,
        y: f64,
    },
    Drag {
        x: f64,
        y: f64,
        to_x: f64,
        to_y: f64,
    },
    Scroll {
        x: f64,
        y: f64,
        dx: i32,
        dy: i32,
    },
    Type {
        text: String,
    },
    Key {
        keys: String,
    },
    Press {
        #[serde(rename = "ref")]
        reference: String,
        snapshot_id: Option<String>,
    },
    SetValue {
        #[serde(rename = "ref")]
        reference: String,
        snapshot_id: Option<String>,
        value: String,
    },
    Wait {
        ms: u64,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Target {
    pub x: Option<f64>,
    pub y: Option<f64>,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub snapshot_id: Option<String>,
}

pub(crate) fn validate_app(app: &str) -> Result<(), ToolError> {
    if app.trim().is_empty() {
        return Err(ToolError::invalid_input("app must not be empty"));
    }
    Ok(())
}

pub(crate) fn validate_chord(chord: &str) -> Result<(), ToolError> {
    let mut modifiers = 0_u8;
    let mut has_key = false;
    for token in chord.split('+') {
        let modifier = match token {
            "cmd" => 1,
            "ctrl" => 2,
            "alt" => 4,
            "shift" => 8,
            "fn" => 16,
            _ => 0,
        };
        if modifier == 0 && !is_key(token) {
            return Err(ToolError::invalid_input(format!(
                "unknown key token {token:?}"
            )));
        }
        if has_key || modifiers & modifier != 0 {
            return Err(ToolError::invalid_input(
                "key chord must contain distinct modifiers followed by exactly one key",
            ));
        }
        has_key = modifier == 0;
        modifiers |= modifier;
    }
    if !has_key {
        return Err(ToolError::invalid_input(
            "key chord must contain a non-modifier key",
        ));
    }
    Ok(())
}

fn is_key(token: &str) -> bool {
    if matches!(
        token,
        "enter"
            | "return"
            | "tab"
            | "escape"
            | "esc"
            | "space"
            | "backspace"
            | "delete"
            | "up"
            | "down"
            | "left"
            | "right"
            | "home"
            | "end"
            | "pageup"
            | "pagedown"
            | "f1"
            | "f2"
            | "f3"
            | "f4"
            | "f5"
            | "f6"
            | "f7"
            | "f8"
            | "f9"
            | "f10"
            | "f11"
            | "f12"
    ) {
        return true;
    }
    let mut characters = token.chars();
    matches!(
        (characters.next(), characters.next()),
        (Some(character), None)
            if !character.is_control() && !character.is_whitespace() && !character.is_uppercase()
    )
}
