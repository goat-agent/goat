use std::{future::Future, pin::Pin, sync::Arc};

use goat_protocol::{AskQuestion, TaskId, ToolCallId, ToolDisplay};
use goat_tool::{
    Tool, ToolContext, ToolDefinitionContext, ToolError, ToolFuture, ToolInvocation, ToolOutput,
    ToolSummaryKind, display,
};
use tokio_util::sync::CancellationToken;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub type QuestionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + 'a>>;

pub trait QuestionBroker: Send + Sync {
    fn ask<'a>(
        &'a self,
        task: TaskId,
        call: ToolCallId,
        questions: Vec<AskQuestion>,
        cancellation: &'a CancellationToken,
    ) -> QuestionFuture<'a>;
}

pub struct AskTool {
    broker: Arc<dyn QuestionBroker>,
}

impl AskTool {
    pub fn new(broker: Arc<dyn QuestionBroker>) -> Self {
        Self { broker }
    }
}

#[derive(serde::Deserialize)]
struct Input {
    questions: Vec<AskQuestion>,
}

impl Tool for AskTool {
    fn name(&self) -> &'static str {
        "Ask"
    }

    fn description(&self) -> &'static str {
        "Pause execution and ask the user one or more questions, each with optional choice options. Returns the user's answers as a JSON array of strings in the same order as the questions. Use when you need the user's input or a decision before proceeding."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": { "type": "string" },
                            "multiple": {
                                "type": "boolean",
                                "description": "If true, the user may select several options for this question; selected labels are returned joined by ', '. Defaults to single-select."
                            },
                            "options": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": { "type": "string" },
                                        "description": { "type": "string" }
                                    },
                                    "required": ["label"]
                                }
                            }
                        },
                        "required": ["question"]
                    }
                }
            },
            "required": ["questions"]
        })
    }

    fn display_input(&self, input: &str) -> ToolDisplay {
        let Ok(args) = serde_json::from_str::<Input>(input) else {
            return display::generic_named(self.name(), input);
        };
        let Some(first) = args.questions.first() else {
            return display::generic_named(self.name(), input);
        };
        let question = display::flatten(&first.question);
        if args.questions.len() > 1 {
            let more = format!("+{} more", args.questions.len() - 1);
            ToolDisplay::primary(display::call_sig(
                self.name(),
                &[question.as_str(), more.as_str()],
            ))
        } else {
            ToolDisplay::primary(display::call_sig(self.name(), &[question.as_str()]))
        }
    }

    fn enabled(&self, context: ToolDefinitionContext) -> bool {
        context.interactive
    }

    fn handles_cancellation(&self) -> bool {
        true
    }

    fn summary_kind(&self) -> ToolSummaryKind {
        ToolSummaryKind::Body
    }

    fn run<'a>(&'a self, _input: &'a str, _ctx: &'a ToolContext) -> ToolFuture<'a> {
        Box::pin(async {
            Err(ToolError::execution(
                "question invocation context is unavailable",
            ))
        })
    }

    fn invoke<'a>(
        &'a self,
        input: &'a str,
        _ctx: &'a ToolContext,
        invocation: ToolInvocation<'a>,
    ) -> ToolFuture<'a> {
        Box::pin(async move {
            let args: Input = serde_json::from_str(input)?;
            if args.questions.is_empty() {
                return Err(ToolError::invalid_input("questions must not be empty"));
            }
            let answers = self
                .broker
                .ask(
                    invocation.task,
                    invocation.call,
                    args.questions.clone(),
                    invocation.cancellation,
                )
                .await
                .map_err(ToolError::execution)?;
            let summary = answer_summary(&args.questions, &answers);
            let body = answer_body(&args.questions, &answers);
            let json = serde_json::to_string(&answers)
                .map_err(|error| ToolError::execution(format!("serialize error: {error}")))?;
            Ok(ToolOutput::text(json).with_summary(summary).with_body(body))
        })
    }
}

const SUMMARY_ROWS: usize = 5;
const SUMMARY_FIELD_WIDTH: usize = 96;

fn answer_summary(questions: &[AskQuestion], answers: &[String]) -> String {
    if questions.len() == 1 {
        let answer = answers.first().map_or("", String::as_str);
        return format!("Answer: {}", display_answer(answer));
    }
    let shown = questions.len().min(SUMMARY_ROWS);
    let empty = String::new();
    let mut lines: Vec<String> = questions
        .iter()
        .enumerate()
        .take(shown)
        .map(|(index, question)| {
            let answer = answers.get(index).unwrap_or(&empty);
            format!(
                "{} → {}",
                truncate_display(&display::flatten(&question.question), SUMMARY_FIELD_WIDTH),
                display_answer(answer)
            )
        })
        .collect();
    if questions.len() > shown {
        lines.push(format!("… {} more", questions.len() - shown));
    }
    lines.join("\n")
}

fn answer_body(questions: &[AskQuestion], answers: &[String]) -> String {
    let empty = String::new();
    questions
        .iter()
        .enumerate()
        .map(|(index, question)| {
            let answer = answers.get(index).unwrap_or(&empty);
            format!(
                "{} → {}",
                display::flatten(&question.question),
                display_answer(answer)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn display_answer(answer: &str) -> String {
    let flattened = display::flatten(answer);
    if flattened.is_empty() {
        "—".to_owned()
    } else {
        truncate_display(&flattened, SUMMARY_FIELD_WIDTH)
    }
}

fn truncate_display(text: &str, max_width: usize) -> String {
    if text.width() <= max_width {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut width = 0usize;
    for character in text.chars() {
        let character_width = character.width().unwrap_or(0);
        if width + character_width + 1 > max_width {
            break;
        }
        out.push(character);
        width += character_width;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use goat_protocol::AskQuestion;

    use super::{answer_body, answer_summary, truncate_display};

    fn question(text: &str) -> AskQuestion {
        AskQuestion {
            question: text.to_owned(),
            options: Vec::new(),
            multiple: false,
        }
    }

    #[test]
    fn single_answer_summary() {
        let summary = answer_summary(&[question("Deploy target?")], &["production".to_owned()]);
        assert_eq!(summary, "Answer: production");
    }

    #[test]
    fn empty_answer_summary() {
        let summary = answer_summary(&[question("Deploy target?")], &[String::new()]);
        assert_eq!(summary, "Answer: —");
    }

    #[test]
    fn multi_answer_summary() {
        let summary = answer_summary(
            &[question("Deploy target?"), question("Run migrations?")],
            &["production".to_owned(), String::new()],
        );
        assert_eq!(summary, "Deploy target? → production\nRun migrations? → —");
    }

    #[test]
    fn truncates_by_display_width() {
        assert_eq!(truncate_display("abcdef", 4), "abc…");
        assert_eq!(truncate_display("한글테스트", 5), "한글…");
    }

    #[test]
    fn caps_summary_rows() {
        let questions: Vec<AskQuestion> = (0..7).map(|i| question(&format!("Q{i}"))).collect();
        let answers: Vec<String> = (0..7).map(|i| format!("A{i}")).collect();
        let summary = answer_summary(&questions, &answers);
        assert!(summary.contains("Q0 → A0"));
        assert!(summary.contains("Q4 → A4"));
        assert!(summary.contains("… 2 more"));
        assert!(!summary.contains("Q5 → A5"));
    }

    #[test]
    fn single_answer_body() {
        let body = answer_body(&[question("Deploy target?")], &["production".to_owned()]);
        assert_eq!(body, "Deploy target? → production");
    }

    #[test]
    fn multi_answer_body_lists_every_question() {
        let questions: Vec<AskQuestion> = (0..7).map(|i| question(&format!("Q{i}"))).collect();
        let mut answers: Vec<String> = (0..7).map(|i| format!("A{i}")).collect();
        answers[3] = String::new();
        let body = answer_body(&questions, &answers);
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 7);
        assert_eq!(lines[0], "Q0 → A0");
        assert_eq!(lines[3], "Q3 → —");
        assert_eq!(lines[6], "Q6 → A6");
    }
}
