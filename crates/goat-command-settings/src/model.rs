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
        if let Ok(parsed) = goat_model::Model::parse(query.trim()) {
            let provider = parsed.provider.to_string();
            let known = session
                .models()
                .iter()
                .any(|entry| entry.provider == provider)
                || session
                    .accounts()
                    .iter()
                    .any(|entry| entry.provider == provider);
            if known {
                let model = parsed.id;
                if let Some(account) = parsed.account {
                    return CommandEffect::Dispatch(vec![Op::SelectModel {
                        target: goat_protocol::ModelTarget {
                            provider,
                            model,
                            account,
                            effort: None,
                        },
                    }]);
                }
                let choices: Vec<goat_protocol::AccountChoice> = session
                    .accounts()
                    .iter()
                    .find(|entry| entry.provider == provider)
                    .map(|entry| {
                        entry
                            .accounts
                            .iter()
                            .map(|account| goat_protocol::AccountChoice {
                                id: account.name.clone(),
                                display: account.name.clone(),
                                target: goat_protocol::ModelTarget {
                                    provider: provider.clone(),
                                    model: model.clone(),
                                    account: account.name.clone(),
                                    effort: None,
                                },
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                return match choices.as_slice() {
                    [choice] => CommandEffect::Dispatch(vec![Op::SelectModel {
                        target: choice.target.clone(),
                    }]),
                    [] => CommandEffect::Dispatch(vec![Op::SelectModel {
                        target: goat_protocol::ModelTarget {
                            provider,
                            model,
                            account: goat_providers::DEFAULT_ACCOUNT.to_owned(),
                            effort: None,
                        },
                    }]),
                    _ => CommandEffect::Show(Box::new(AccountScreen::new(choices))),
                };
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
