use goat_protocol::Op;

use crate::Screen;

pub enum CommandEffect {
    Show(Box<dyn Screen>),
    Dispatch(Vec<Op>),
    EditConfig(Vec<goat_api::ConfigEdit>),
    Submit { display: String, prompt: String },
    Noop,
    Quit,
}
