use goat_protocol::Op;

use crate::Screen;

pub enum CommandEffect {
    Show(Box<dyn Screen>),
    Dispatch(Vec<Op>),
    Submit { display: String, prompt: String },
    Noop,
    Quit,
}
