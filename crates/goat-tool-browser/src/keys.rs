use crate::error::BrowserError;

pub struct KeyStroke {
    pub key: &'static str,
    pub code: &'static str,
    pub virtual_key: i64,
    pub text: String,
}

const NAMED: [(&str, &str, i64, &str); 14] = [
    ("Enter", "Enter", 13, "\r"),
    ("Tab", "Tab", 9, "\t"),
    ("Space", "Space", 32, " "),
    ("Escape", "Escape", 27, ""),
    ("Backspace", "Backspace", 8, "\u{8}"),
    ("Delete", "Delete", 46, ""),
    ("ArrowUp", "ArrowUp", 38, ""),
    ("ArrowDown", "ArrowDown", 40, ""),
    ("ArrowLeft", "ArrowLeft", 37, ""),
    ("ArrowRight", "ArrowRight", 39, ""),
    ("Home", "Home", 36, ""),
    ("End", "End", 35, ""),
    ("PageUp", "PageUp", 33, ""),
    ("PageDown", "PageDown", 34, ""),
];

pub fn stroke(name: &str) -> Result<KeyStroke, BrowserError> {
    if let Some((key, code, virtual_key, text)) = NAMED
        .iter()
        .find(|(key, ..)| key.eq_ignore_ascii_case(name))
    {
        return Ok(KeyStroke {
            key,
            code,
            virtual_key: *virtual_key,
            text: (*text).to_owned(),
        });
    }
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(single), None) if !single.is_control() => Ok(KeyStroke {
            key: "",
            code: "",
            virtual_key: i64::from(single.to_ascii_uppercase() as u32),
            text: single.to_string(),
        }),
        _ => Err(BrowserError::Input(format!(
            "unknown key '{name}'; use a single character or one of {}",
            NAMED
                .iter()
                .map(|(key, ..)| *key)
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

impl KeyStroke {
    pub fn key_name(&self) -> String {
        if self.key.is_empty() {
            self.text.clone()
        } else {
            self.key.to_owned()
        }
    }

    pub fn code_name(&self) -> String {
        self.code.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::stroke;

    #[test]
    fn named_keys_carry_their_virtual_code() {
        let enter = stroke("Enter").unwrap();
        assert_eq!(enter.virtual_key, 13);
        assert_eq!(enter.text, "\r");
        assert_eq!(enter.key_name(), "Enter");
    }

    #[test]
    fn named_keys_are_case_insensitive() {
        assert_eq!(stroke("escape").unwrap().virtual_key, 27);
    }

    #[test]
    fn a_single_character_types_itself() {
        let a = stroke("a").unwrap();
        assert_eq!(a.text, "a");
        assert_eq!(a.key_name(), "a");
        assert_eq!(a.virtual_key, i64::from(b'A'));
    }

    #[test]
    fn an_unknown_name_is_an_input_error() {
        assert!(stroke("Ctrl+Shift+P").is_err());
        assert!(stroke("").is_err());
    }
}
