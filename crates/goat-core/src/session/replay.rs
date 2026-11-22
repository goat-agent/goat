use super::{Event, Sink};
use crate::message::{Message, Part, Role, Trace};
use crate::tool::{self, Call, Outcome};

#[derive(Debug, Default)]
pub(super) struct Replay {
    descriptions: Vec<(Box<str>, Box<str>)>,
}

impl Replay {
    pub(super) fn message(&mut self, message: &Message, sink: &mut impl Sink) {
        match message.role {
            Role::User => user(message, sink),
            Role::Assistant => self.assistant(message, sink),
            Role::Tool => self.results(message, sink),
            Role::Summary => sink.emit(Event::Compacting),
        }
    }

    fn assistant(&mut self, message: &Message, sink: &mut impl Sink) {
        self.descriptions.clear();
        for part in &message.parts {
            match part {
                Part::Reasoning { summary, .. } if !summary.trim().is_empty() => {
                    sink.emit(Event::Reasoning(summary));
                    if !summary.ends_with('\n') {
                        sink.emit(Event::Reasoning("\n"));
                    }
                }
                Part::Text { text, .. } => sink.emit(Event::Text(text)),
                Part::ToolCall {
                    id, name, input, ..
                } => {
                    if let Ok(run) = tool::Run::parse(name, input) {
                        self.descriptions
                            .push((id.clone(), run.description().into()));
                    }
                }
                _ => {}
            }
        }
    }

    fn results(&self, message: &Message, sink: &mut impl Sink) {
        for part in &message.parts {
            let Part::ToolResult {
                call_id,
                output,
                is_error,
                trace,
                ..
            } = part
            else {
                continue;
            };
            let description = self
                .descriptions
                .iter()
                .find(|(id, _)| id == call_id)
                .map(|(_, description)| description);
            if let Some(description) = description {
                sink.emit(Event::RunStarted { description });
            }
            for step in trace {
                match step {
                    Trace::Call { kind, target } => sink.emit(Event::Call(&Call {
                        kind: *kind,
                        target: target.clone(),
                    })),
                    Trace::Failed { problem } => sink.emit(Event::CallFailed { problem }),
                    Trace::Changed { diff } => sink.emit(Event::Changed(diff)),
                    Trace::Loaded { path } => sink.emit(Event::Loaded { path }),
                }
            }
            let outcome = Outcome {
                output: output.clone(),
                is_error: *is_error,
                images: Vec::new(),
            };
            sink.emit(Event::RunFinished { outcome: &outcome });
        }
    }
}

fn user(message: &Message, sink: &mut impl Sink) {
    for part in &message.parts {
        match part {
            Part::Text { text, .. } => sink.emit(Event::Prompt(text)),
            Part::Shell {
                command,
                output,
                code,
            } => {
                sink.emit(Event::ShellCommand(command));
                sink.emit(Event::ShellOutput(output));
                sink.emit(Event::ShellFinished {
                    code: *code,
                    problem: None,
                });
            }
            _ => {}
        }
    }
}
