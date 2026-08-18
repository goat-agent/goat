use goat_protocol::Op;

use crate::Screen;

pub enum CommandEffect {
    Show(Box<dyn Screen>),
    Dispatch(Vec<Op>),
    Admin(Vec<goat_client::AdminRequest>),
    Submit { display: String, prompt: String },
    Noop,
    Quit,
}
