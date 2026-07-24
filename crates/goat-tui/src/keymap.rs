use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub(crate) fn ctrl_key(key: &KeyEvent) -> Option<char> {
    if !key.modifiers.contains(KeyModifiers::CONTROL) {
        return None;
    }
    match key.code {
        KeyCode::Char(c) => Some(base_key(c)),
        _ => None,
    }
}

pub(crate) fn super_char(key: &KeyEvent) -> Option<char> {
    if !key
        .modifiers
        .intersects(KeyModifiers::SUPER | KeyModifiers::META)
    {
        return None;
    }
    match key.code {
        KeyCode::Char(c) => Some(base_key(c)),
        _ => None,
    }
}

fn base_key(c: char) -> char {
    dubeolsik(c).unwrap_or_else(|| c.to_ascii_lowercase())
}

fn dubeolsik(c: char) -> Option<char> {
    Some(match c {
        'ㅂ' | 'ㅃ' => 'q',
        'ㅈ' | 'ㅉ' => 'w',
        'ㄷ' | 'ㄸ' => 'e',
        'ㄱ' | 'ㄲ' => 'r',
        'ㅅ' | 'ㅆ' => 't',
        'ㅛ' => 'y',
        'ㅕ' => 'u',
        'ㅑ' => 'i',
        'ㅐ' | 'ㅒ' => 'o',
        'ㅔ' | 'ㅖ' => 'p',
        'ㅁ' => 'a',
        'ㄴ' => 's',
        'ㅇ' => 'd',
        'ㄹ' => 'f',
        'ㅎ' => 'g',
        'ㅗ' => 'h',
        'ㅓ' => 'j',
        'ㅏ' => 'k',
        'ㅣ' => 'l',
        'ㅋ' => 'z',
        'ㅌ' => 'x',
        'ㅊ' => 'c',
        'ㅍ' => 'v',
        'ㅠ' => 'b',
        'ㅜ' => 'n',
        'ㅡ' => 'm',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    use super::{base_key, ctrl_key, super_char};

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn ctrl_c_latin() {
        let k = key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(ctrl_key(&k), Some('c'));
    }

    #[test]
    fn ctrl_c_dubeolsik() {
        let k = key(KeyCode::Char('ㅊ'), KeyModifiers::CONTROL);
        assert_eq!(ctrl_key(&k), Some('c'));
    }

    #[test]
    fn ctrl_uppercase_normalizes() {
        let k = key(KeyCode::Char('C'), KeyModifiers::CONTROL);
        assert_eq!(ctrl_key(&k), Some('c'));
    }

    #[test]
    fn no_modifier_returns_none() {
        let k = key(KeyCode::Char('ㅊ'), KeyModifiers::NONE);
        assert_eq!(ctrl_key(&k), None);
    }

    #[test]
    fn ctrl_enter_returns_none() {
        let k = key(KeyCode::Enter, KeyModifiers::CONTROL);
        assert_eq!(ctrl_key(&k), None);
    }

    #[test]
    fn super_c_latin_and_dubeolsik() {
        assert_eq!(
            super_char(&key(KeyCode::Char('c'), KeyModifiers::SUPER)),
            Some('c')
        );
        assert_eq!(
            super_char(&key(KeyCode::Char('ㅊ'), KeyModifiers::SUPER)),
            Some('c')
        );
        assert_eq!(
            super_char(&key(KeyCode::Char('C'), KeyModifiers::META)),
            Some('c')
        );
    }

    #[test]
    fn super_v_dubeolsik() {
        assert_eq!(
            super_char(&key(KeyCode::Char('ㅍ'), KeyModifiers::SUPER)),
            Some('v')
        );
    }

    #[test]
    fn super_char_requires_super_or_meta() {
        assert_eq!(
            super_char(&key(KeyCode::Char('c'), KeyModifiers::NONE)),
            None
        );
        assert_eq!(
            super_char(&key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            None
        );
    }

    #[test]
    fn base_key_spot_check() {
        assert_eq!(base_key('ㅊ'), 'c');
        assert_eq!(base_key('ㅁ'), 'a');
        assert_eq!(base_key('ㄹ'), 'f');
        assert_eq!(base_key('ㅡ'), 'm');
        assert_eq!(base_key('ㅜ'), 'n');
        assert_eq!(base_key('c'), 'c');
        assert_eq!(base_key('C'), 'c');
    }
}
