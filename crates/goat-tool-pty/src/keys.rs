use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum PtyInputItem {
    Text { text: String },
    Key { key: String },
}

impl PtyInputItem {
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            PtyInputItem::Text { text } => text.as_bytes().to_vec(),
            PtyInputItem::Key { key } => named_key_bytes(key.as_str())
                .unwrap_or(key.as_bytes())
                .to_vec(),
        }
    }
}

fn named_key_bytes(key: &str) -> Option<&'static [u8]> {
    Some(match key {
        "enter" => b"\r",
        "tab" => b"\t",
        "esc" => b"\x1b",
        "backspace" => b"\x7f",
        "up" => b"\x1b[A",
        "down" => b"\x1b[B",
        "left" => b"\x1b[D",
        "right" => b"\x1b[C",
        "home" => b"\x1b[H",
        "end" => b"\x1b[F",
        "pageup" => b"\x1b[5~",
        "pagedown" => b"\x1b[6~",
        "ctrl-c" => b"\x03",
        "ctrl-d" => b"\x04",
        "ctrl-z" => b"\x1a",
        "ctrl-l" => b"\x0c",
        "ctrl-u" => b"\x15",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_named_keys() {
        let item = PtyInputItem::Key {
            key: "enter".into(),
        };
        assert_eq!(item.to_bytes(), b"\r");
        let item = PtyInputItem::Key {
            key: "ctrl-c".into(),
        };
        assert_eq!(item.to_bytes(), b"\x03");
        let item = PtyInputItem::Key { key: "up".into() };
        assert_eq!(item.to_bytes(), b"\x1b[A");
    }

    #[test]
    fn encodes_text() {
        let item = PtyInputItem::Text {
            text: "hello".into(),
        };
        assert_eq!(item.to_bytes(), b"hello");
    }

    #[test]
    fn unknown_key_falls_back_to_raw_bytes() {
        let item = PtyInputItem::Key {
            key: "unknown".into(),
        };
        assert_eq!(item.to_bytes(), b"unknown");
    }
}
