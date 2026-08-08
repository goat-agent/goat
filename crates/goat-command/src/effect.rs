use goat_protocol::{NotifyKind, Op};

use crate::Screen;

pub enum CommandEffect {
    Show(Box<dyn Screen>),
    Dispatch(Vec<Op>),
    Submit { display: String, prompt: String },
    Notify(NotifyKind, String),
    OpenConfig,
    ShowHelp,
    ClearConversation,
    CompactConversation(Option<String>),
    RenameConversation(String),
    SubmitText(String),
    SubmitCommand { display: String, prompt: String },
    Notice(String),
    Error(String),
    OpenUsage,
    OpenStatus,
    Noop,
    Quit,
}
