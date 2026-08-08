mod account;
mod screen;

use goat_command::{
    Command, CommandEffect, CommandInvocation, CommandShape, ParameterSpec, ParameterValue, Session,
};
use goat_protocol::Op;

pub use account::AccountScreen;
pub use screen::ModelScreen;

pub struct Model;

impl Command for Model {
    fn name(&self) -> &'static str {
        "model"
    }

    fn description(&self) -> &'static str {
        "switch model"
    }

    fn shape(&self) -> CommandShape {
        CommandShape::Parameters(vec![ParameterSpec {
            name: "name".to_owned(),
            description: "model name".to_owned(),
            required: false,
            value: ParameterValue::TextTail,
        }])
    }

    fn run(&self, invocation: CommandInvocation, session: &mut dyn Session) -> CommandEffect {
        let snapshot = session.snapshot();
        let loading = session.models().is_empty() && !snapshot.models_loaded;
        let Some(query) = invocation.text("name") else {
            return CommandEffect::Show(Box::new(ModelScreen::new(
                session.models().to_vec(),
                session.current_model().cloned(),
                loading,
            )));
        };
        let needle = query.trim().to_lowercase();
        let exact: Vec<_> = session
            .models()
            .iter()
            .filter(|entry| {
                entry.model.to_lowercase() == needle
                    || format!("{}/{}", entry.provider, entry.model).to_lowercase() == needle
            })
            .collect();
        if let [entry] = exact.as_slice() {
            match entry.accounts.as_slice() {
                [account] => {
                    return CommandEffect::Dispatch(vec![Op::SelectModel {
                        target: account.target.clone(),
                    }]);
                }
                [] => {}
                accounts => {
                    return CommandEffect::Show(Box::new(AccountScreen::new(accounts.to_vec())));
                }
            }
        }
        let mut screen = ModelScreen::new(
            session.models().to_vec(),
            session.current_model().cloned(),
            loading,
        );
        for ch in query.trim().chars() {
            screen.on_char(ch);
        }
        CommandEffect::Show(Box::new(screen))
    }
}
