use goat_protocol::{Event, ToolCall, ToolCallId, ToolDisplay, ToolImageData, ToolOutcome};
use goat_provider::{ContentBlock, Provider, ToolDefinition};
use goat_tool::{
    ToolContent, ToolContext, ToolDefinitionContext, ToolInvocation, ToolOutput, ToolRegistry,
};
use tokio_util::sync::CancellationToken;

use crate::{
    Ctx, LoopEnv, Run,
    persist::{create_tool_call_record, finish_tool_db},
    subagent::ToolSelection,
};

pub(crate) struct ToolExecResult {
    result_content: ContentBlock,
    cancelled: bool,
}

pub(crate) struct ToolBatchResult {
    pub(crate) tool_results: Vec<ContentBlock>,
    pub(crate) cancelled: bool,
}

struct Prepared<'a> {
    vendor_id: &'a str,
    name: &'a str,
    input_json: &'a str,
    tui_id: u64,
    db_id: Option<i64>,
}

pub(crate) fn tool_outcome(result: &Result<ToolOutput, String>) -> ToolOutcome {
    match result {
        Ok(output) => {
            let mut outcome = ToolOutcome {
                ok: true,
                summary: output.summary.clone(),
                body: output.body.clone(),
                image: outcome_image(&output.content),
                git: None,
            };
            output.extend_outcome(&mut outcome);
            outcome
        }
        Err(message) => ToolOutcome {
            ok: false,
            summary: Some(message.clone()),
            body: None,
            image: None,
            git: None,
        },
    }
}

const MAX_OUTCOME_IMAGE_BYTES: usize = 8 * 1024 * 1024;

fn outcome_image(content: &ToolContent) -> Option<ToolImageData> {
    match content {
        ToolContent::Image(img) if img.data.len() <= MAX_OUTCOME_IMAGE_BYTES => {
            Some(ToolImageData {
                media_type: img.media_type.clone(),
                data: img.data.clone(),
            })
        }
        _ => None,
    }
}

pub(crate) fn call_display(tools: &ToolRegistry, name: &str, input: &str) -> ToolDisplay {
    tools.get(name).map_or_else(
        || goat_tool::display::generic_named(name, input),
        |tool| tool.display_input(input),
    )
}

pub(crate) fn summarize_line(text: &str) -> Option<String> {
    let line = text.lines().find(|line| !line.trim().is_empty())?;
    Some(goat_tool::display::truncate_chars(
        &goat_tool::display::flatten(line),
        80,
    ))
}

async fn run_regular_tool(
    ctx: &Ctx,
    task: goat_protocol::TaskId,
    call: ToolCallId,
    definition_context: ToolDefinitionContext,
    host: &(dyn std::any::Any + Send + Sync),
    name: &str,
    input_json: &str,
    tool_ctx: &ToolContext,
    token: &CancellationToken,
) -> Option<Result<ToolOutput, String>> {
    let Some(tool) = ctx
        .tools
        .get(name)
        .filter(|tool| tool.enabled(definition_context))
    else {
        return Some(Err(format!("unknown tool: {name}")));
    };
    let invocation = ToolInvocation {
        task,
        call,
        cancellation: token,
        definition_context,
        host: Some(host),
    };
    let future = tool.invoke(input_json, tool_ctx, invocation);
    if tool.handles_cancellation() {
        return Some(future.await.map_err(|error| error.to_string()));
    }
    let mut future = std::pin::pin!(future);
    tokio::select! {
        biased;
        () = token.cancelled() => None,
        result = &mut future => Some(result.map_err(|error| error.to_string())),
    }
}

const MAX_TOOL_RESULT_BYTES: usize = 64 * 1024;

pub(crate) fn cap_tool_result(mut content: String) -> String {
    if content.len() > MAX_TOOL_RESULT_BYTES {
        let boundary = content.floor_char_boundary(MAX_TOOL_RESULT_BYTES);
        content.truncate(boundary);
        content.push_str("\n[output truncated]\n");
    }
    content
}

async fn execute_tool(
    ctx: &Ctx,
    run: &Run<'_>,
    env: &LoopEnv,
    prep: &Prepared<'_>,
    tool_ctx: &ToolContext,
    token: &CancellationToken,
) -> ToolExecResult {
    let mutation_path = ctx
        .tools
        .get(prep.name)
        .and_then(|tool| tool.mutation_path(prep.input_json));
    let step: Option<Result<ToolOutput, String>> = if let Some(path) = mutation_path
        && let Err(error) = ctx.checkpoints.capture_path(&path, tool_ctx).await
    {
        Some(Err(format!(
            "could not checkpoint file before mutation: {error}"
        )))
    } else {
        run_regular_tool(
            ctx,
            run.id,
            ToolCallId(prep.tui_id),
            ToolDefinitionContext {
                interactive: env.interactive,
                top_level: env.allow_delegate,
                planning: env.plan,
            },
            env,
            prep.name,
            prep.input_json,
            tool_ctx,
            token,
        )
        .await
    };
    let Some(result) = step else {
        let outcome = ToolOutcome {
            ok: false,
            summary: Some("interrupted".to_owned()),
            body: None,
            image: None,
            git: None,
        };
        finish_tool_db(ctx, prep.db_id, &outcome).await;
        let _ = ctx
            .events
            .send(Event::ToolDone {
                id: run.id,
                call: ToolCallId(prep.tui_id),
                outcome,
            })
            .await;
        return ToolExecResult {
            result_content: ContentBlock::text_result(prep.vendor_id, "interrupted", true),
            cancelled: true,
        };
    };
    let outcome = tool_outcome(&result);
    let is_error = !outcome.ok;
    finish_tool_db(ctx, prep.db_id, &outcome).await;
    let _ = ctx
        .events
        .send(Event::ToolDone {
            id: run.id,
            call: ToolCallId(prep.tui_id),
            outcome,
        })
        .await;
    let content = match result {
        Ok(output) => match output.content {
            ToolContent::Text(text) => {
                vec![ContentBlock::Text {
                    text: cap_tool_result(text),
                }]
            }
            ToolContent::Image(img) => {
                vec![ContentBlock::Image {
                    media_type: img.media_type,
                    data: img.data,
                }]
            }
        },
        Err(msg) => vec![ContentBlock::Text { text: msg }],
    };
    ToolExecResult {
        result_content: ContentBlock::ToolResult {
            tool_use_id: prep.vendor_id.to_owned(),
            content,
            is_error,
        },
        cancelled: false,
    }
}

fn groups_into_one_row(prepared: &[Prepared<'_>]) -> bool {
    prepared.len() > 1
        && prepared
            .iter()
            .all(|prep| prep.name == goat_tool_delegate::DELEGATE_TOOL_NAME)
}

pub(crate) async fn run_tool_batch(
    ctx: &Ctx,
    run: &Run<'_>,
    env: &LoopEnv,
    pending_calls: &[(String, String, String)],
    call_seq: &mut u64,
    tool_ctx: &ToolContext,
    token: &CancellationToken,
) -> ToolBatchResult {
    let mut prepared: Vec<Prepared> = Vec::with_capacity(pending_calls.len());
    for (vendor_id, name, input_json) in pending_calls {
        *call_seq += 1;
        prepared.push(Prepared {
            vendor_id: vendor_id.as_str(),
            name: name.as_str(),
            input_json: input_json.as_str(),
            tui_id: *call_seq,
            db_id: None,
        });
    }
    if env.allow_delegate
        && groups_into_one_row(&prepared)
        && let Some(first) = prepared.first()
    {
        let members = prepared
            .iter()
            .map(|prep| goat_tool_delegate::group_member(ToolCallId(prep.tui_id), prep.input_json))
            .collect();
        let _ = ctx
            .events
            .send(Event::SubagentGroupStarted {
                id: run.id,
                group: ToolCallId(first.tui_id),
                members,
            })
            .await;
    }
    for prep in &mut prepared {
        let _ = ctx
            .events
            .send(Event::ToolStarted {
                id: run.id,
                call: ToolCall {
                    id: ToolCallId(prep.tui_id),
                    name: prep.name.to_owned(),
                    display: call_display(&ctx.tools, prep.name, prep.input_json),
                },
            })
            .await;
        prep.db_id = match run.ids() {
            Some(ids) => {
                create_tool_call_record(ctx, ids, prep.vendor_id, prep.name, prep.input_json).await
            }
            None => None,
        };
    }
    let results = futures::future::join_all(
        prepared
            .iter()
            .map(|prep| execute_tool(ctx, run, env, prep, tool_ctx, token)),
    )
    .await;
    let mut tool_results = Vec::with_capacity(results.len());
    let mut cancelled = false;
    for result in results {
        if result.cancelled {
            cancelled = true;
        }
        tool_results.push(result.result_content);
    }
    ToolBatchResult {
        tool_results,
        cancelled,
    }
}

pub(crate) fn build_tool_defs(
    ctx: &Ctx,
    provider: &dyn Provider,
    selection: Option<&ToolSelection>,
    allow_delegate: bool,
    allow_ask: bool,
    plan: bool,
) -> Vec<ToolDefinition> {
    if !provider.capabilities().tools {
        return Vec::new();
    }
    let defs: Vec<ToolDefinition> = ctx
        .tools
        .specs_for(ToolDefinitionContext {
            interactive: allow_ask,
            top_level: allow_delegate,
            planning: plan,
        })
        .into_iter()
        .filter(|spec| selection.is_none_or(|sel| sel.allows(spec.name)))
        .map(|spec| ToolDefinition {
            name: spec.name.to_owned(),
            description: spec.description.to_owned(),
            input_schema: spec.parameters,
        })
        .collect();
    defs
}

#[cfg(test)]
mod tests {
    use goat_tool::{ToolImage, ToolOutput};

    use super::tool_outcome;

    #[test]
    fn image_output_populates_outcome_image() {
        let output = ToolOutput::image(ToolImage {
            media_type: "image/png".to_owned(),
            data: "AAAA".to_owned(),
        });
        let outcome = tool_outcome(&Ok(output));
        assert!(outcome.ok);
        let image = outcome.image.expect("image attached");
        assert_eq!(image.media_type, "image/png");
        assert_eq!(image.data, "AAAA");
    }

    #[test]
    fn text_output_has_no_image() {
        let outcome = tool_outcome(&Ok(ToolOutput::text("hi")));
        assert!(outcome.image.is_none());
    }

    #[test]
    fn error_output_has_no_image() {
        let outcome = tool_outcome(&Err("boom".to_owned()));
        assert!(!outcome.ok);
        assert!(outcome.image.is_none());
    }

    fn prep<'a>(name: &'a str, input: &'a str) -> super::Prepared<'a> {
        super::Prepared {
            vendor_id: "v",
            name,
            input_json: input,
            tui_id: 1,
            db_id: None,
        }
    }

    const BLOCKING: &str = r#"{"subagent_type":"explore","name":"n","prompt":"p"}"#;

    #[test]
    fn a_parallel_blocking_batch_collapses_into_one_row() {
        let batch = vec![prep("Subagent", BLOCKING), prep("Subagent", BLOCKING)];
        assert!(super::groups_into_one_row(&batch));
    }

    #[test]
    fn a_lone_call_and_a_foreign_tool_never_group() {
        assert!(!super::groups_into_one_row(&[prep("Subagent", BLOCKING)]));
        let mixed = vec![prep("Subagent", BLOCKING), prep("Bash", "{}")];
        assert!(!super::groups_into_one_row(&mixed));
    }
}
