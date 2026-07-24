use futures::StreamExt;
use goat_protocol::Event;
use goat_provider::{
    ContentBlock, Message, MessageRole, Provider, Request, StreamChunk, StreamError,
};
use goat_tool::ToolContext;
use tokio_util::sync::CancellationToken;

use crate::{
    Ctx, LoopEnv, Run,
    compaction::ContextTracker,
    conversation::Conversation,
    persist::{now_ms, persist_message},
    rate_limit_cache,
    tools_exec::run_tool_batch,
};

pub(crate) const ROUNDS_BACKSTOP: usize = 1000;

pub(crate) enum RoundEnd {
    Completed,
    Cancelled,
    Failed(StreamError),
}

pub(crate) struct RoundResult {
    pub(crate) end: RoundEnd,
    pub(crate) raw: String,
    pub(crate) thinking: Option<(String, String)>,
    pub(crate) redacted: Vec<String>,
    pub(crate) pending_calls: Vec<(String, String, String)>,
    pub(crate) usage: Option<goat_provider::Usage>,
    pub(crate) rate_limits: Option<goat_provider::RateLimitSnapshot>,
}

impl RoundResult {
    pub(crate) fn ended(end: RoundEnd) -> Self {
        Self {
            end,
            raw: String::new(),
            thinking: None,
            redacted: Vec::new(),
            pending_calls: Vec::new(),
            usage: None,
            rate_limits: None,
        }
    }
}

pub(crate) enum RoundOutcome {
    Done,
    Continue,
    Cancelled,
}

pub(crate) enum LoopOutcome {
    Completed,
    Cancelled,
    Failed(String, Option<String>),
}

async fn drain_steering(ctx: &Ctx<'_>, run: &Run<'_>, conversation: &mut Conversation) {
    let Some(queue) = run.steering() else {
        return;
    };
    loop {
        let entry = queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front();
        let Some(input) = entry else {
            break;
        };
        let message = crate::turn::user_message(&input.text, &input.attachments);
        let db_id = match run.ids() {
            Some(ids) => persist_message(ctx, ids, &message).await,
            None => None,
        };
        conversation.push(message, db_id);
        let _ = ctx
            .events
            .send(Event::UserMessage {
                id: input.id,
                text: input.text,
                display: input.display,
                attachments: input.attachments,
            })
            .await;
    }
}

fn normalize_tool_input(input: String, schema: Option<&serde_json::Value>) -> String {
    if input.trim().is_empty() {
        return "{}".to_owned();
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&input) else {
        return input;
    };
    let Some(object) = as_object_value(value) else {
        return input;
    };
    let object = unwrap_object_fields(object, schema);
    serde_json::Value::Object(object).to_string()
}

fn as_object_value(value: serde_json::Value) -> Option<serde_json::Map<String, serde_json::Value>> {
    match value {
        serde_json::Value::Object(map) => Some(map),
        serde_json::Value::String(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(serde_json::Value::Object(map)) => Some(map),
            _ => None,
        },
        _ => None,
    }
}

fn unwrap_object_fields(
    mut object: serde_json::Map<String, serde_json::Value>,
    schema: Option<&serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    let Some(properties) = schema
        .and_then(|schema| schema.get("properties"))
        .and_then(serde_json::Value::as_object)
    else {
        return object;
    };
    for (field, value) in &mut object {
        let serde_json::Value::String(text) = value else {
            continue;
        };
        let Some(expected) = properties
            .get(field)
            .and_then(|spec| spec.get("type"))
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        if expected != "array" && expected != "object" {
            continue;
        }
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(text) else {
            continue;
        };
        let matches = matches!(
            (expected, &parsed),
            ("array", serde_json::Value::Array(_)) | ("object", serde_json::Value::Object(_))
        );
        if matches {
            *value = parsed;
        }
    }
    object
}

fn tool_input_value(input: &str) -> serde_json::Value {
    serde_json::from_str(input)
        .ok()
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()))
}

pub(crate) async fn run_round(
    ctx: &Ctx<'_>,
    run: &Run<'_>,
    provider: &dyn Provider,
    request: Request,
    token: &CancellationToken,
) -> RoundResult {
    let mut stream = tokio::select! {
        biased;
        () = token.cancelled() => return RoundResult::ended(RoundEnd::Cancelled),
        opened = provider.stream(request) => match opened {
            Ok(stream) => stream,
            Err(error) => return RoundResult::ended(RoundEnd::Failed(error)),
        }
    };
    let mut raw = String::new();
    let mut thinking = String::new();
    let mut signature = String::new();
    let mut redacted: Vec<String> = Vec::new();
    let mut pending_calls: Vec<(String, String, String)> = Vec::new();
    let mut usage: Option<goat_provider::Usage> = None;
    let mut rate_limits: Option<goat_provider::RateLimitSnapshot> = None;
    let end = loop {
        tokio::select! {
            biased;
            () = token.cancelled() => {
                break RoundEnd::Cancelled;
            }
            maybe_chunk = stream.next() => match maybe_chunk {
                Some(Ok(StreamChunk::TextDelta { text })) => {
                    raw.push_str(&text);
                    let _ = ctx
                        .events
                        .send(Event::TextDelta { id: run.id, chunk: text })
                        .await;
                }
                Some(Ok(StreamChunk::ThinkingDelta { text })) => {
                    thinking.push_str(&text);
                    let _ = ctx
                        .events
                        .send(Event::ThinkingDelta { id: run.id, chunk: text })
                        .await;
                }
                Some(Ok(StreamChunk::ThinkingSignature { signature: sig })) => {
                    signature.push_str(&sig);
                }
                Some(Ok(StreamChunk::RedactedThinking { data })) => {
                    redacted.push(data);
                }
                Some(Ok(StreamChunk::ToolCall { id: vendor_id, name, input })) => {
                    pending_calls.push((vendor_id, name, input));
                }
                Some(Ok(StreamChunk::Usage { usage: u })) => {
                    usage = Some(u);
                }
                Some(Ok(StreamChunk::RateLimits { snapshot })) => {
                    rate_limits = Some(snapshot);
                }
                Some(Err(error)) => break RoundEnd::Failed(error),
                None => break RoundEnd::Completed,
            }
        }
    };
    let thinking = (!thinking.is_empty() || !signature.is_empty()).then_some((thinking, signature));
    RoundResult {
        end,
        raw,
        thinking,
        redacted,
        pending_calls,
        usage,
        rate_limits,
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) async fn process_round_output(
    ctx: &Ctx<'_>,
    run: &Run<'_>,
    env: &LoopEnv<'_>,
    round: RoundResult,
    conversation: &mut Conversation,
    tracker: &mut ContextTracker,
    rounds: usize,
    call_seq: &mut u64,
    tool_ctx: &ToolContext,
    token: &CancellationToken,
) -> RoundOutcome {
    if let Some(usage) = round.usage.clone()
        && run.is_top()
    {
        let context_window = env.provider.context_window(&env.target.model);
        let compaction_threshold = context_window.map(crate::compaction::proactive_limit);
        let _ = ctx
            .events
            .send(Event::Usage {
                id: run.id,
                provider: env.target.provider.clone(),
                account: env.target.account.clone(),
                usage,
                context_window,
                compaction_threshold,
            })
            .await;
    }
    if let Some(snapshot) = round.rate_limits
        && run.is_top()
    {
        let now = now_ms() / 1000;
        let serialized = {
            let mut cache = ctx
                .rl_cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            cache.upsert(
                &env.target.provider,
                &env.target.account,
                snapshot.windows.clone(),
                now,
            );
            cache.to_json()
        };
        if let (Some(path), Some(json)) = (ctx.rl_path, serialized) {
            let path = path.to_owned();
            tokio::task::spawn_blocking(move || rate_limit_cache::write(&path, &json));
        }
        let _ = ctx
            .events
            .send(Event::RateLimits {
                provider: env.target.provider.clone(),
                account: env.target.account.clone(),
                snapshot,
                cached_at: now,
            })
            .await;
    }
    let mut pending_calls: Vec<(String, String, String)> = round
        .pending_calls
        .into_iter()
        .map(|(vendor_id, name, input)| {
            let schema = env
                .tool_defs
                .iter()
                .find(|def| def.name == name)
                .map(|def| &def.input_schema);
            (vendor_id, name, normalize_tool_input(input, schema))
        })
        .collect();
    let (raw, recovered) =
        crate::tool_recovery::recover(&env.target.provider, &round.raw, env.tool_defs);
    for (idx, (name, raw_input)) in recovered.into_iter().enumerate() {
        let schema = env
            .tool_defs
            .iter()
            .find(|def| def.name == name)
            .map(|def| &def.input_schema);
        let input = normalize_tool_input(raw_input, schema);
        if pending_calls
            .iter()
            .any(|(_, n, i)| *n == name && crate::tool_recovery::input_equivalent(i, &input))
        {
            continue;
        }
        let vendor_id = format!("recovered-{rounds}-{idx}");
        tracing::warn!(
            name = %name,
            vendor_id = %vendor_id,
            "recovered a tool call the model emitted as assistant text"
        );
        pending_calls.push((vendor_id, name, input));
    }
    let shown_text = (!raw.trim().is_empty()).then(|| raw.clone());
    if !raw.trim().is_empty()
        || !pending_calls.is_empty()
        || round.thinking.is_some()
        || !round.redacted.is_empty()
    {
        let mut content = Vec::new();
        if let Some((text, signature)) = &round.thinking {
            content.push(ContentBlock::Thinking {
                text: text.clone(),
                signature: signature.clone(),
            });
        }
        for data in &round.redacted {
            content.push(ContentBlock::RedactedThinking { data: data.clone() });
        }
        if !raw.trim().is_empty() {
            content.push(ContentBlock::Text { text: raw.clone() });
        }
        for (vendor_id, name, input_json) in &pending_calls {
            content.push(ContentBlock::ToolUse {
                id: vendor_id.clone(),
                name: name.clone(),
                input: tool_input_value(input_json),
            });
        }
        let message = Message {
            role: MessageRole::Assistant,
            content,
        };
        let db_id = match run.ids() {
            Some(ids) => persist_message(ctx, ids, &message).await,
            None => None,
        };
        conversation.push(message, db_id);
    }
    if let Some(usage) = &round.usage {
        tracker.record(conversation.len(), usage);
    }
    if let Some(shown) = shown_text {
        let _ = ctx
            .events
            .send(Event::TextDone {
                id: run.id,
                text: shown,
            })
            .await;
    }
    if pending_calls.is_empty() {
        if run.steering_pending() {
            return RoundOutcome::Continue;
        }
        return RoundOutcome::Done;
    }
    if rounds >= ROUNDS_BACKSTOP {
        tracing::warn!(rounds, "tool round backstop reached; ending run");
        let synthetic: Vec<ContentBlock> = pending_calls
            .iter()
            .map(|(vendor_id, _, _)| {
                ContentBlock::text_result(vendor_id.clone(), "tool round limit reached", true)
            })
            .collect();
        let message = Message {
            role: MessageRole::User,
            content: crate::prompt::append_language_anchor(synthetic, run.is_top()),
        };
        let db_id = match run.ids() {
            Some(ids) => persist_message(ctx, ids, &message).await,
            None => None,
        };
        conversation.push(message, db_id);
        return RoundOutcome::Done;
    }
    let batch = run_tool_batch(ctx, run, env, &pending_calls, call_seq, tool_ctx, token).await;
    let message = Message {
        role: MessageRole::User,
        content: crate::prompt::append_language_anchor(batch.tool_results, run.is_top()),
    };
    let db_id = match run.ids() {
        Some(ids) => persist_message(ctx, ids, &message).await,
        None => None,
    };
    conversation.push(message, db_id);
    if batch.cancelled {
        RoundOutcome::Cancelled
    } else {
        RoundOutcome::Continue
    }
}

pub(crate) async fn core_loop(
    ctx: &Ctx<'_>,
    run: &Run<'_>,
    env: &LoopEnv<'_>,
    token: &CancellationToken,
    conversation: &mut Conversation,
    tracker: &mut ContextTracker,
) -> LoopOutcome {
    let mut tool_ctx = match ToolContext::new(env.cwd) {
        Ok(tool_ctx) => tool_ctx,
        Err(err) => return LoopOutcome::Failed(err.to_string(), None),
    };
    tool_ctx.exec_policy = env.exec_policy.clone();
    let mut rounds = 0usize;
    let mut call_seq = 0u64;
    let mut compacted_for_overflow = false;
    loop {
        rounds += 1;
        drain_steering(ctx, run, conversation).await;
        if let Some(window) = env.provider.context_window(&env.target.model)
            && tracker.estimate(conversation.messages(), env.tool_defs)
                > crate::compaction::proactive_limit(window)
        {
            match crate::compaction::compact(ctx, run, env, conversation, tracker, None, token)
                .await
            {
                Ok(_) => {}
                Err(crate::compaction::CompactionError::Cancelled) => {
                    return LoopOutcome::Cancelled;
                }
                Err(crate::compaction::CompactionError::Failed(message)) => {
                    tracing::warn!(%message, "proactive compaction failed");
                }
            }
        }
        let roster = if run.is_top() {
            crate::process_tools::roster_message(ctx).await
        } else {
            None
        };
        let round = if let Some(roster) = roster {
            let mut messages = conversation.messages().to_vec();
            messages.push(roster);
            crate::retry::run_round_with_retry(ctx, run, env, &messages, token).await
        } else {
            crate::retry::run_round_with_retry(ctx, run, env, conversation.messages(), token).await
        };
        match &round.end {
            RoundEnd::Cancelled => return LoopOutcome::Cancelled,
            RoundEnd::Failed(StreamError::ContextOverflow { .. }) if !compacted_for_overflow => {
                match crate::compaction::compact(ctx, run, env, conversation, tracker, None, token)
                    .await
                {
                    Ok(_) => {
                        compacted_for_overflow = true;
                        continue;
                    }
                    Err(crate::compaction::CompactionError::Cancelled) => {
                        return LoopOutcome::Cancelled;
                    }
                    Err(crate::compaction::CompactionError::Failed(message)) => {
                        return LoopOutcome::Failed(
                            message,
                            Some("/clear to reset the conversation".to_owned()),
                        );
                    }
                }
            }
            RoundEnd::Failed(error) => {
                return LoopOutcome::Failed(
                    crate::retry::failure_message(error, env.target),
                    crate::retry::error_hint(error),
                );
            }
            RoundEnd::Completed => {
                compacted_for_overflow = false;
            }
        }
        match process_round_output(
            ctx,
            run,
            env,
            round,
            conversation,
            tracker,
            rounds,
            &mut call_seq,
            &tool_ctx,
            token,
        )
        .await
        {
            RoundOutcome::Done => return LoopOutcome::Completed,
            RoundOutcome::Cancelled => return LoopOutcome::Cancelled,
            RoundOutcome::Continue => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_tool_input, tool_input_value};

    fn norm(input: &str) -> String {
        normalize_tool_input(input.to_owned(), None)
    }

    fn norm_with(input: &str, schema: &serde_json::Value) -> String {
        normalize_tool_input(input.to_owned(), Some(schema))
    }

    fn value(input: &str) -> serde_json::Value {
        serde_json::from_str(input).expect("valid json")
    }

    #[test]
    fn empty_input_becomes_empty_object() {
        assert_eq!(norm(""), "{}");
        assert_eq!(norm("   "), "{}");
    }

    #[test]
    fn object_input_is_preserved() {
        assert_eq!(
            value(&norm("{\"path\":\"a.txt\"}")),
            value("{\"path\":\"a.txt\"}")
        );
    }

    #[test]
    fn normalization_is_idempotent() {
        let once = norm("{\"path\":\"a.txt\"}");
        let twice = normalize_tool_input(once.clone(), None);
        assert_eq!(once, twice);
    }

    #[test]
    fn whole_argument_stringify_is_unwrapped() {
        let input = "\"{\\\"path\\\":\\\"a.txt\\\"}\"";
        assert_eq!(value(&norm(input)), value("{\"path\":\"a.txt\"}"));
    }

    #[test]
    fn field_level_array_stringify_is_unwrapped() {
        let schema = value("{\"properties\":{\"questions\":{\"type\":\"array\"}}}");
        let input = "{\"questions\":\"[{\\\"question\\\":\\\"A?\\\"}]\"}";
        let out = value(&norm_with(input, &schema));
        assert_eq!(out, value("{\"questions\":[{\"question\":\"A?\"}]}"));
    }

    #[test]
    fn field_level_object_stringify_is_unwrapped() {
        let schema = value("{\"properties\":{\"filter\":{\"type\":\"object\"}}}");
        let input = "{\"filter\":\"{\\\"k\\\":1}\"}";
        let out = value(&norm_with(input, &schema));
        assert_eq!(out, value("{\"filter\":{\"k\":1}}"));
    }

    #[test]
    fn string_field_is_never_unwrapped() {
        let schema = value("{\"properties\":{\"body\":{\"type\":\"string\"}}}");
        let input = "{\"body\":\"{\\\"k\\\":1}\"}";
        let out = value(&norm_with(input, &schema));
        assert_eq!(out, value("{\"body\":\"{\\\"k\\\":1}\"}"));
    }

    #[test]
    fn multilingual_payloads_pass_through() {
        for payload in [
            "{\"text\":\"앱을 만들어줘\"}",
            "{\"text\":\"アプリを作って\"}",
            "{\"text\":\"اصنع تطبيقا\"}",
            "{\"text\":\"ship it 🚀🦊\"}",
        ] {
            assert_eq!(value(&norm(payload)), value(payload));
        }
    }

    #[test]
    fn non_object_input_is_preserved_not_discarded() {
        for input in ["5", "[1,2]", "null", "\"hi\""] {
            assert_eq!(norm(input), input);
        }
    }

    #[test]
    fn history_value_is_object_for_normalizable_input() {
        assert!(tool_input_value(&norm("{\"path\":\"a.txt\"}")).is_object());
    }

    #[test]
    fn history_value_falls_back_to_object_for_non_object() {
        let block = tool_input_value(&norm("[1,2]"));
        assert!(block.is_object());
        assert_eq!(block, value("{}"));
    }
}
