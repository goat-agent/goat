use goat_auth::{CredentialKey, CredentialValue};
use goat_client::{AdminRequest, LoginMethod};
use goat_command::{Composer, Settings, Theme, Viewport};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AdminInput {
    ConfigEdit {
        edits: Vec<goat_api::ConfigEdit>,
    },
    CredentialSet {
        key: CredentialKey,
        value: CredentialValue,
    },
    CredentialRemove {
        key: CredentialKey,
    },
    ProviderLogin {
        provider: String,
        account: String,
        method: LoginInput,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum LoginInput {
    ApiKey { secret: String },
    OAuth,
}

impl From<AdminInput> for AdminRequest {
    fn from(input: AdminInput) -> Self {
        match input {
            AdminInput::ConfigEdit { edits } => Self::ConfigEdit(edits),
            AdminInput::CredentialSet { key, value } => Self::CredentialSet { key, value },
            AdminInput::CredentialRemove { key } => Self::CredentialRemove { key },
            AdminInput::ProviderLogin {
                provider,
                account,
                method,
            } => Self::ProviderLogin {
                provider,
                account,
                method: match method {
                    LoginInput::ApiKey { secret } => LoginMethod::ApiKey(secret),
                    LoginInput::OAuth => LoginMethod::OAuth,
                },
            },
        }
    }
}

pub struct WebviewControls;

impl Settings for WebviewControls {
    fn theme(&self) -> Theme {
        Theme::dark()
    }

    fn set_theme(&mut self, _theme: Theme) {}

    fn mouse_capture(&self) -> bool {
        false
    }

    fn set_mouse_capture(&mut self, _enabled: bool) {}
}

impl Composer for WebviewControls {
    fn text(&self) -> String {
        String::new()
    }

    fn is_empty(&self) -> bool {
        true
    }

    fn shell(&self) -> bool {
        false
    }

    fn set_plain_text(&mut self, _text: &str) {}

    fn replace_at_query(&mut self, _replacement: &str) {}

    fn insert_str(&mut self, _text: &str) {}

    fn insert_char(&mut self, _ch: char) {}

    fn backspace(&mut self) {}

    fn delete_forward(&mut self) {}

    fn move_left(&mut self) -> bool {
        false
    }

    fn move_right(&mut self) -> bool {
        false
    }

    fn move_word_left(&mut self) -> bool {
        false
    }

    fn move_word_right(&mut self) -> bool {
        false
    }

    fn move_home(&mut self) -> bool {
        false
    }

    fn move_end(&mut self) -> bool {
        false
    }

    fn move_up(&mut self) -> bool {
        false
    }

    fn move_down(&mut self) -> bool {
        false
    }

    fn newline(&mut self) {}

    fn at_query(&self) -> Option<String> {
        None
    }
}

impl Viewport for WebviewControls {
    fn scroll(&self) -> usize {
        0
    }

    fn set_scroll(&mut self, _scroll: usize) {}

    fn follow(&self) -> bool {
        true
    }

    fn set_follow(&mut self, _follow: bool) {}

    fn page_rows(&self) -> usize {
        0
    }

    fn run_cursor(&self) -> Option<usize> {
        None
    }

    fn run_count(&self) -> usize {
        0
    }

    fn move_run_cursor(&mut self, _cursor: usize) {}

    fn open_run(&mut self, _cursor: usize) {}

    fn close_run_selector(&mut self) {}
}
