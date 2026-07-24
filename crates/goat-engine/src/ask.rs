use goat_protocol::{AskQuestion, Event, ToolCallId, ToolDisplay};
use goat_provider::ToolDefinition;
use goat_tool::ToolOutput;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{Ctx, Run};

pub(crate) const ASK_TOOL_NAME: &str = "Ask";

pub(crate) fn ask_tool_def() -> ToolDefinition {
    ToolDefinition {
        name: ASK_TOOL_NAME.to_owned(),
        description: "Pause execution and ask the user one or more questions, each with optional choice options. Returns the user's answers as a JSON array of strings in the same order as the questions. Use when you need the user's input or a decision before proceeding.".to_owned(),
        input_schema: serde_json::json!({
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
        }),
    }
}

pub(crate) fn ask_call_display(input: &str) -> ToolDisplay {
    #[derive(serde::Deserialize)]
    struct Input {
        questions: Vec<AskQuestion>,
    }
    let Ok(args) = serde_json::from_str::<Input>(input) else {
        return goat_tool::display::generic_named(ASK_TOOL_NAME, input);
    };
    let Some(first) = args.questions.first() else {
        return goat_tool::display::generic_named(ASK_TOOL_NAME, input);
    };
    let q = goat_tool::display::flatten(&first.question);
    if args.questions.len() > 1 {
        let more = format!("+{} more", args.questions.len() - 1);
        ToolDisplay::primary(goat_tool::display::call_sig(
            ASK_TOOL_NAME,
            &[q.as_str(), more.as_str()],
        ))
    } else {
        ToolDisplay::primary(goat_tool::display::call_sig(ASK_TOOL_NAME, &[q.as_str()]))
    }
}

pub(crate) async fn run_ask(
    ctx: &Ctx<'_>,
    run: &Run<'_>,
    input_json: &str,
    call_id: ToolCallId,
    token: &CancellationToken,
) -> Result<ToolOutput, String> {
    #[derive(serde::Deserialize)]
    struct Input {
        questions: Vec<AskQuestion>,
    }
    let args: Input =
        serde_json::from_str(input_json).map_err(|err| format!("invalid Ask input: {err}"))?;
    if args.questions.is_empty() {
        return Err("questions must not be empty".to_owned());
    }
    let (tx, rx) = oneshot::channel::<Vec<String>>();
    ctx.asks.lock().await.insert(call_id, tx);
    let _ = ctx
        .events
        .send(Event::AskStarted {
            id: run.id,
            call: call_id,
            questions: args.questions.clone(),
        })
        .await;
    let result = tokio::select! {
        biased;
        () = token.cancelled() => {
            ctx.asks.lock().await.remove(&call_id);
            let _ = ctx
                .events
                .send(Event::AskDismissed { id: run.id, call: call_id })
                .await;
            return Err("interrupted".to_owned());
        }
        res = rx => res,
    };
    match result {
        Ok(answers) => {
            let summary = answer_summary(&args.questions, &answers);
            serde_json::to_string(&answers)
                .map(|json| ToolOutput::text(json).with_summary(summary))
                .map_err(|err| format!("serialize error: {err}"))
        }
        Err(_) => Err("answer channel closed".to_owned()),
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
        .map(|(i, question)| {
            let answer = answers.get(i).unwrap_or(&empty);
            format!(
                "{} → {}",
                truncate_display(
                    &goat_tool::display::flatten(&question.question),
                    SUMMARY_FIELD_WIDTH
                ),
                display_answer(answer)
            )
        })
        .collect();
    if questions.len() > shown {
        lines.push(format!("… {} more", questions.len() - shown));
    }
    lines.join("\n")
}

fn display_answer(answer: &str) -> String {
    let flattened = goat_tool::display::flatten(answer);
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
    for ch in text.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if width + ch_width + 1 > max_width {
            break;
        }
        out.push(ch);
        width += ch_width;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use goat_protocol::AskQuestion;

    use super::{answer_summary, truncate_display};

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
        assert_eq!(
            summary,
            "Deploy target? → production
Run migrations? → —"
        );
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
}
