use goat_protocol::{NotifyKind, Op};

use crate::Screen;

pub enum CommandEffect {
    Show(Box<dyn Screen>),
    Dispatch(Vec<Op>),
    Submit { display: String, prompt: String },
    Notify(NotifyKind, String),
    OpenModelPicker,
    SelectModelNamed(String),
    OpenEffortPicker,
    SelectEffort(String),
    OpenThreadPicker,
    ResumeIndex(usize),
    OpenRewind,
    OpenConfig,
    ShowHelp,
    ClearConversation,
    CompactConversation(Option<String>),
    TogglePlanMode,
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
