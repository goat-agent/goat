use goat_provider::{ContentBlock, Message, MessageRole, ToolDefinition, Usage};

const IMAGE_TOKEN_ESTIMATE: u32 = 4_800;
const MESSAGE_OVERHEAD_TOKENS: u32 = 8;

struct Measured {
    history_len: usize,
    tokens: u32,
}

#[derive(Default)]
pub(crate) struct ContextTracker {
    measured: Option<Measured>,
    tool_def_cache: std::cell::Cell<Option<(usize, usize, usize, u32)>>,
}

impl ContextTracker {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn record(&mut self, history_len: usize, usage: &Usage) {
        self.measured = Some(Measured {
            history_len,
            tokens: usage.input_tokens.saturating_add(usage.output_tokens),
        });
    }

    pub(crate) fn invalidate(&mut self) {
        self.measured = None;
    }

    fn tool_def_tokens(&self, tool_defs: &[ToolDefinition]) -> u32 {
        let fingerprint: usize = tool_defs
            .iter()
            .map(|def| def.name.len().wrapping_add(def.description.len()))
            .fold(0usize, usize::wrapping_add);
        let key = (tool_defs.as_ptr() as usize, tool_defs.len(), fingerprint);
        if let Some((ptr, len, fp, tokens)) = self.tool_def_cache.get()
            && (ptr, len, fp) == key
        {
            return tokens;
        }
        let tokens = estimate_tool_defs(tool_defs);
        self.tool_def_cache.set(Some((key.0, key.1, key.2, tokens)));
        tokens
    }

    pub(crate) fn estimate(&self, messages: &[Message], tool_defs: &[ToolDefinition]) -> u32 {
        match &self.measured {
            Some(measured) if measured.history_len <= messages.len() => measured
                .tokens
                .saturating_add(estimate_messages(&messages[measured.history_len..])),
            _ => self
                .tool_def_tokens(tool_defs)
                .saturating_add(estimate_messages(messages)),
        }
    }
}

fn chars_to_tokens(chars: usize) -> u32 {
    u32::try_from(chars.div_ceil(4)).unwrap_or(u32::MAX)
}

fn estimate_block(block: &ContentBlock) -> u32 {
    match block {
        ContentBlock::Text { text } | ContentBlock::Thinking { text, .. } => {
            chars_to_tokens(text.chars().count())
        }
        ContentBlock::RedactedThinking { data } => chars_to_tokens(data.len()),
        ContentBlock::ToolUse { name, input, .. } => {
            chars_to_tokens(name.len() + input.to_string().chars().count())
        }
        ContentBlock::ToolResult { content, .. } => content
            .iter()
            .map(estimate_block)
            .fold(0u32, u32::saturating_add),
        ContentBlock::Image { .. } => IMAGE_TOKEN_ESTIMATE,
    }
}

pub(crate) fn estimate_messages(messages: &[Message]) -> u32 {
    messages
        .iter()
        .map(|message| {
            message
                .content
                .iter()
                .map(estimate_block)
                .fold(MESSAGE_OVERHEAD_TOKENS, u32::saturating_add)
        })
        .fold(0u32, u32::saturating_add)
}

fn estimate_tool_defs(tool_defs: &[ToolDefinition]) -> u32 {
    tool_defs
        .iter()
        .map(|def| {
            chars_to_tokens(
                def.name.len()
                    + def.description.chars().count()
                    + def.input_schema.to_string().chars().count(),
            )
        })
        .fold(0u32, u32::saturating_add)
}

pub(crate) fn reserve_for(window: u32) -> u32 {
    (window / 100 * 15).clamp(16_384, 40_000)
}

pub(crate) fn proactive_limit(window: u32) -> u32 {
    window.saturating_sub(reserve_for(window))
}

pub(crate) const KEEP_RECENT_TOKENS: u32 = 20_000;
const KEEP_RECENT_MIN_TOKENS: u32 = 4_096;
const MIN_SUMMARIZATION_BUDGET: u32 = 8_192;

const SUMMARIZATION_PROMPT: &str = "Your context window is nearly full. Stop working on the task. Your only job now is to write a checkpoint summary of this session so the work can continue seamlessly in a fresh context window that will contain only this summary and the most recent messages.

First, inside <analysis> tags, walk through the conversation chronologically: each user request, the approaches taken, key decisions, files touched, errors hit and how they were resolved, and exactly where the work stands now.

Then write the summary inside <summary> tags using exactly these sections:

## Task
Every explicit request the user made, in order, quoting the user's wording in its original language where it is load-bearing, and noting the language the user is writing in. State the overall goal and the current sub-goal.

## Constraints and preferences
Rules, conventions, and preferences from the user or project instructions, including anything the user said not to do.

## State
What is completed and verified, what is in progress right now, and what is broken or blocked. Quote exact error messages still under investigation.

## Key decisions and learnings
Choices made and why, approaches that failed and must not be retried, and important facts learned about the codebase, build, or environment.

## Files
Every file read, created, or modified, with why it matters. For files mid-edit, include the exact snippets, signatures, or line references needed to continue without rereading the conversation.

## Next steps
The concrete remaining work, in order, each step tied to the user request it serves. For work that was in progress, quote the most recent plan or instruction verbatim. Do not invent steps the user has not asked for.

Respond with the analysis and summary only. Do not address the user and do not continue the task.";

pub(crate) const SUMMARY_WRAPPER: &str = "This session is continuing from an earlier conversation that ran out of context. The summary below covers everything before the most recent messages. Continue the work from where it left off without asking the user to repeat anything. File contents may have changed since they appear in the summary; reread before editing. Reply to the user in the language of their original request recorded in the summary's Task section, and keep code, identifiers, paths, and commands unchanged.\n\n";

const PARTIAL_INPUT_NOTE: &str =
    "[The earliest part of this conversation was not available when this summary was written.]";

pub(crate) fn summary_message(summary: &str) -> Message {
    Message::text(MessageRole::User, format!("{SUMMARY_WRAPPER}{summary}"))
}

pub(crate) struct CompactionOutcome {
    pub(crate) tokens_after: u32,
    pub(crate) usage: Usage,
}

pub(crate) enum CompactionError {
    Cancelled,
    Failed(String),
}

fn is_user_text(message: &Message) -> bool {
    message.role == MessageRole::User
        && !message.content.is_empty()
        && message
            .content
            .iter()
            .all(|block| matches!(block, ContentBlock::Text { .. }))
}

fn has_tool_result(message: &Message) -> bool {
    message
        .content
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
}

fn has_tool_use(message: &Message) -> bool {
    message
        .content
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolUse { .. }))
}

fn group_start(messages: &[Message], end: usize) -> usize {
    if end > 0
        && messages[end].role == MessageRole::User
        && has_tool_result(&messages[end])
        && messages[end - 1].role == MessageRole::Assistant
        && has_tool_use(&messages[end - 1])
    {
        end - 1
    } else {
        end
    }
}

pub(crate) fn plan_tail(messages: &[Message], db_ids: &[Option<i64>], keep_budget: u32) -> usize {
    let mut start = messages.len();
    let mut used = 0u32;
    while start > 1 {
        let group = group_start(messages, start - 1);
        if group == 0 {
            break;
        }
        let cost = estimate_messages(&messages[group..start]);
        if used.saturating_add(cost) > keep_budget {
            break;
        }
        used = used.saturating_add(cost);
        start = group;
    }
    if db_ids[start..].iter().any(Option::is_some) {
        while start < messages.len() && db_ids[start].is_none() {
            start += 1;
        }
    }
    start
}

fn preserved_indices(
    messages: &[Message],
    db_ids: &[Option<i64>],
    anchor: Option<i64>,
    tail_start: usize,
) -> Vec<usize> {
    let from = anchor
        .and_then(|db| db_ids.iter().position(|id| *id == Some(db)))
        .or_else(|| messages[..tail_start].iter().rposition(is_user_text));
    let Some(from) = from else {
        return Vec::new();
    };
    (from..tail_start)
        .filter(|&i| is_user_text(&messages[i]))
        .collect()
}

fn summarization_input(messages: &[Message], budget: u32, prompt: &Message) -> Vec<Message> {
    let mut start = messages.len();
    let mut used = 0u32;
    while start > 1 {
        let group = group_start(messages, start - 1);
        if group == 0 {
            break;
        }
        let cost = estimate_messages(&messages[group..start]);
        if used.saturating_add(cost) > budget {
            break;
        }
        used = used.saturating_add(cost);
        start = group;
    }
    let mut input = Vec::new();
    if messages
        .first()
        .is_some_and(|message| message.role == MessageRole::System)
    {
        input.push(messages[0].clone());
    }
    if start > 1 {
        input.push(Message::text(MessageRole::User, PARTIAL_INPUT_NOTE));
    }
    input.extend(messages[start.min(messages.len())..].iter().cloned());
    input.push(prompt.clone());
    input
}

pub(crate) fn extract_summary(text: &str) -> String {
    if let Some(open) = text.find("<summary>")
        && let Some(close) = text.rfind("</summary>")
        && close > open
    {
        return text[open + "<summary>".len()..close].trim().to_owned();
    }
    let mut remainder = text.to_owned();
    if let Some(open) = remainder.find("<analysis>")
        && let Some(close) = remainder.find("</analysis>")
        && close > open
    {
        remainder.replace_range(open..close + "</analysis>".len(), "");
    }
    remainder.trim().to_owned()
}

enum CollectEnd {
    Cancelled,
    Failed(goat_provider::StreamError),
}

async fn collect_text(
    provider: &dyn goat_provider::Provider,
    request: goat_provider::Request,
    token: &tokio_util::sync::CancellationToken,
) -> Result<(String, Usage), CollectEnd> {
    use futures::StreamExt;
    let mut stream = tokio::select! {
        biased;
        () = token.cancelled() => return Err(CollectEnd::Cancelled),
        opened = provider.stream(request) => match opened {
            Ok(stream) => stream,
            Err(error) => return Err(CollectEnd::Failed(error)),
        }
    };
    let mut text = String::new();
    let mut usage = Usage::default();
    loop {
        tokio::select! {
            biased;
            () = token.cancelled() => {
                return Err(CollectEnd::Cancelled);
            }
            maybe_chunk = stream.next() => match maybe_chunk {
                Some(Ok(goat_provider::StreamChunk::TextDelta { text: chunk })) => text.push_str(&chunk),
                Some(Ok(goat_provider::StreamChunk::Usage { usage: collected })) => usage = collected,
                Some(Ok(_)) => {}
                Some(Err(error)) => return Err(CollectEnd::Failed(error)),
                None => return Ok((text, usage)),
            }
        }
    }
}

enum CollectFail {
    Cancelled,
    Overflow,
    Fatal(String),
}

async fn collect_with_retry(
    ctx: &crate::Ctx<'_>,
    run: &crate::Run<'_>,
    provider: &dyn goat_provider::Provider,
    request: &goat_provider::Request,
    token: &tokio_util::sync::CancellationToken,
) -> Result<(String, Usage), CollectFail> {
    let started = std::time::Instant::now();
    let mut attempt = 1u32;
    loop {
        let error = match collect_text(provider, request.clone(), token).await {
            Ok(collected) => return Ok(collected),
            Err(CollectEnd::Cancelled) => return Err(CollectFail::Cancelled),
            Err(CollectEnd::Failed(goat_provider::StreamError::ContextOverflow { .. })) => {
                return Err(CollectFail::Overflow);
            }
            Err(CollectEnd::Failed(error)) => error,
        };
        if !crate::retry::retryable(&error) {
            return Err(CollectFail::Fatal(error.to_string()));
        }
        if attempt >= crate::retry::MAX_ATTEMPTS {
            return Err(CollectFail::Fatal(crate::retry::exhausted_message(
                &error.to_string(),
                attempt,
                started,
            )));
        }
        let delay = crate::retry::backoff_delay(&error, attempt);
        let _ = ctx
            .events
            .send(goat_protocol::Event::Retrying {
                id: run.id,
                attempt,
                max_attempts: crate::retry::MAX_ATTEMPTS,
                delay_ms: u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                reason: crate::retry::reason_label(&error).to_owned(),
                resets_at: crate::retry::resets_at(&error),
            })
            .await;
        tokio::select! {
            biased;
            () = token.cancelled() => return Err(CollectFail::Cancelled),
            () = tokio::time::sleep(delay) => {}
        }
        attempt += 1;
    }
}

pub(crate) async fn compact(
    ctx: &crate::Ctx<'_>,
    run: &crate::Run<'_>,
    env: &crate::LoopEnv<'_>,
    conversation: &mut crate::conversation::Conversation,
    tracker: &mut ContextTracker,
    instructions: Option<&str>,
    token: &tokio_util::sync::CancellationToken,
) -> Result<CompactionOutcome, CompactionError> {
    let _ = ctx
        .events
        .send(goat_protocol::Event::CompactionStarted { id: run.id })
        .await;
    let tokens_before = tracker.estimate(conversation.messages(), env.tool_defs);
    let result = compact_inner(ctx, run, env, conversation, tracker, instructions, token).await;
    let (ok, tokens_after, usage) = match &result {
        Ok(outcome) => (true, outcome.tokens_after, outcome.usage.clone()),
        Err(_) => (false, tokens_before, Usage::default()),
    };
    let _ = ctx
        .events
        .send(goat_protocol::Event::CompactionDone {
            id: run.id,
            ok,
            tokens_before,
            tokens_after,
            usage,
        })
        .await;
    result
}

async fn compact_inner(
    ctx: &crate::Ctx<'_>,
    run: &crate::Run<'_>,
    env: &crate::LoopEnv<'_>,
    conversation: &mut crate::conversation::Conversation,
    tracker: &mut ContextTracker,
    instructions: Option<&str>,
    token: &tokio_util::sync::CancellationToken,
) -> Result<CompactionOutcome, CompactionError> {
    let messages = conversation.messages().to_vec();
    let db_ids = conversation.db_ids().to_vec();
    let tokens_before = tracker.estimate(&messages, env.tool_defs);
    let window = env.provider.context_window(&env.target.model);
    let mut budget = window.map_or_else(
        || (tokens_before / 2).max(MIN_SUMMARIZATION_BUDGET),
        proactive_limit,
    );
    let mut prompt_text = SUMMARIZATION_PROMPT.to_owned();
    if let Some(focus) = instructions {
        prompt_text.push_str("\n\nAdditional instructions for this summary:\n");
        prompt_text.push_str(focus);
    }
    let prompt = Message::text(MessageRole::User, prompt_text);
    let (raw, usage) = loop {
        let input = summarization_input(&messages, budget, &prompt);
        let request = goat_provider::Request {
            model: env.target.model.clone(),
            messages: input,
            tools: env.tool_defs.to_vec(),
            effort: None,
            tool_choice: goat_provider::ToolChoice::None,
            temperature: None,
            max_tokens: None,
            system: None,
        };
        match collect_with_retry(ctx, run, env.provider, &request, token).await {
            Ok(collected) => break collected,
            Err(CollectFail::Cancelled) => return Err(CompactionError::Cancelled),
            Err(CollectFail::Overflow) => {
                budget /= 2;
                if budget < MIN_SUMMARIZATION_BUDGET {
                    return Err(CompactionError::Failed(
                        "context window exhausted; the summarization input cannot fit".to_owned(),
                    ));
                }
            }
            Err(CollectFail::Fatal(message)) => return Err(CompactionError::Failed(message)),
        }
    };
    let summary = extract_summary(&raw);
    if summary.is_empty() {
        return Err(CompactionError::Failed(
            "summarization produced no summary".to_owned(),
        ));
    }
    let keep_budget = KEEP_RECENT_TOKENS
        .min(budget / 4)
        .max(KEEP_RECENT_MIN_TOKENS);
    let tail_start = plan_tail(&messages, &db_ids, keep_budget);
    let anchor = run.ids().and_then(|ids| ids.user_message_db_id);
    let preserved = preserved_indices(&messages, &db_ids, anchor, tail_start);
    let mut entries: Vec<(Message, Option<i64>)> = Vec::new();
    if messages
        .first()
        .is_some_and(|message| message.role == MessageRole::System)
    {
        entries.push((messages[0].clone(), None));
    }
    entries.push((summary_message(&summary), None));
    for &index in &preserved {
        entries.push((messages[index].clone(), db_ids[index]));
    }
    for index in tail_start..messages.len() {
        entries.push((messages[index].clone(), db_ids[index]));
    }
    let new_messages: Vec<Message> = entries.iter().map(|(message, _)| message.clone()).collect();
    let tokens_after =
        estimate_messages(&new_messages).saturating_add(estimate_tool_defs(env.tool_defs));
    if let Some(ids) = run.ids()
        && let Some(thread) = ids.stored_thread
    {
        let after_message_id = db_ids.iter().flatten().copied().max().unwrap_or(0);
        let tail_from_message_id = (tail_start < messages.len())
            .then(|| db_ids[tail_start])
            .flatten();
        let preserved_message_ids: Vec<i64> = preserved
            .iter()
            .filter_map(|&index| db_ids[index])
            .collect();
        if let Err(err) = ctx
            .store
            .create_compaction(goat_store::NewCompaction {
                thread_id: thread,
                summary: summary.clone(),
                after_message_id,
                tail_from_message_id,
                preserved_message_ids,
                tokens_before: i64::from(tokens_before),
                tokens_after: i64::from(tokens_after),
                created_at: crate::persist::now_ms(),
            })
            .await
        {
            tracing::warn!(%err, "failed to persist compaction");
        }
    }
    conversation.replace(entries);
    tracker.invalidate();
    Ok(CompactionOutcome {
        tokens_after,
        usage,
    })
}

#[cfg(test)]
mod tests {
    use goat_provider::{ContentBlock, Message, MessageRole, Usage};

    use super::{ContextTracker, proactive_limit, reserve_for};

    fn usage(input: u32, output: u32) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        }
    }

    #[test]
    fn unmeasured_estimate_uses_full_heuristic() {
        let tracker = ContextTracker::new();
        let messages = vec![Message::text(MessageRole::User, "a".repeat(400))];
        let estimate = tracker.estimate(&messages, &[]);
        assert!((100..=120).contains(&estimate), "got {estimate}");
    }

    #[test]
    fn tool_def_estimate_is_cached_and_consistent() {
        use goat_provider::ToolDefinition;
        let tracker = ContextTracker::new();
        let tool_defs = vec![ToolDefinition {
            name: "Read".to_owned(),
            description: "read a file".to_owned(),
            input_schema: serde_json::json!({"type":"object","properties":{"path":{"type":"string"}}}),
        }];
        let messages = vec![Message::text(MessageRole::User, "hi")];
        let first = tracker.estimate(&messages, &tool_defs);
        let second = tracker.estimate(&messages, &tool_defs);
        assert_eq!(first, second);
        assert!(first > tracker.estimate(&messages, &[]));
    }

    #[test]
    fn measured_estimate_adds_only_the_delta() {
        let mut tracker = ContextTracker::new();
        let mut messages = vec![
            Message::text(MessageRole::System, "s".repeat(100_000)),
            Message::text(MessageRole::User, "u".repeat(100_000)),
        ];
        tracker.record(messages.len(), &usage(50_000, 1_000));
        messages.push(Message::text(MessageRole::User, "d".repeat(400)));
        let estimate = tracker.estimate(&messages, &[]);
        assert!((51_100..=51_120).contains(&estimate), "got {estimate}");
    }

    #[test]
    fn invalidate_falls_back_to_heuristic() {
        let mut tracker = ContextTracker::new();
        let messages = vec![Message::text(MessageRole::User, "hi")];
        tracker.record(1, &usage(1_000_000, 0));
        tracker.invalidate();
        assert!(tracker.estimate(&messages, &[]) < 100);
    }

    #[test]
    fn shrunk_history_falls_back_to_heuristic() {
        let mut tracker = ContextTracker::new();
        tracker.record(10, &usage(500_000, 0));
        let messages = vec![Message::text(MessageRole::User, "hi")];
        assert!(tracker.estimate(&messages, &[]) < 100);
    }

    #[test]
    fn image_blocks_use_fixed_estimate() {
        let tracker = ContextTracker::new();
        let messages = vec![Message {
            role: MessageRole::User,
            content: vec![ContentBlock::Image {
                media_type: "image/png".into(),
                data: "xx".into(),
            }],
        }];
        let estimate = tracker.estimate(&messages, &[]);
        assert!((4_800..4_820).contains(&estimate), "got {estimate}");
    }

    #[test]
    fn reserve_scales_with_window() {
        assert_eq!(reserve_for(200_000), 30_000);
        assert_eq!(proactive_limit(200_000), 170_000);
        assert_eq!(reserve_for(1_000_000), 40_000);
        assert_eq!(proactive_limit(1_000_000), 960_000);
        assert_eq!(reserve_for(32_000), 16_384);
    }
}

#[cfg(test)]
mod compact_tests {
    use goat_provider::{ContentBlock, Message, MessageRole};

    use super::{extract_summary, plan_tail, summarization_input};

    fn text(role: MessageRole, body: impl Into<String>) -> Message {
        Message::text(role, body)
    }

    fn tool_pair(id: &str, size: usize) -> [Message; 2] {
        [
            Message {
                role: MessageRole::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: id.to_owned(),
                    name: "Read".to_owned(),
                    input: serde_json::json!({"path": "x"}),
                }],
            },
            Message {
                role: MessageRole::User,
                content: vec![ContentBlock::text_result(id, "r".repeat(size), false)],
            },
        ]
    }

    #[test]
    fn plan_tail_never_strands_a_tool_result_head() {
        for keep_budget in [1u32, 50, 200, 1_000, 100_000] {
            let mut messages = vec![text(MessageRole::System, "sys")];
            let mut db_ids: Vec<Option<i64>> = vec![None];
            messages.push(text(MessageRole::User, "do it"));
            db_ids.push(Some(1));
            let mut next_id = 2i64;
            for round in 0..6 {
                for message in tool_pair(&format!("call-{round}"), 400) {
                    messages.push(message);
                    db_ids.push(Some(next_id));
                    next_id += 1;
                }
            }
            let tail_start = plan_tail(&messages, &db_ids, keep_budget);
            assert!(
                tail_start >= 1,
                "tail must never include the system message"
            );
            if tail_start < messages.len() {
                let head = &messages[tail_start];
                assert!(
                    !(head.role == MessageRole::User && super::has_tool_result(head)),
                    "keep_budget {keep_budget}: tail starts with a dangling tool_result at {tail_start}"
                );
            }
        }
    }

    #[test]
    fn plan_tail_snaps_past_synthetic_messages() {
        let messages = vec![
            text(MessageRole::System, "sys"),
            text(MessageRole::User, "summary bridge"),
            text(MessageRole::User, "real prompt"),
        ];
        let db_ids = vec![None, None, Some(5)];
        let tail_start = plan_tail(&messages, &db_ids, 100_000);
        assert_eq!(tail_start, 2, "tail must snap to the first persisted row");
    }

    #[test]
    fn plan_tail_keeps_full_tail_for_unpersisted_child_run() {
        let messages = vec![
            text(MessageRole::System, "sys"),
            text(MessageRole::User, "prompt"),
            text(MessageRole::Assistant, "step 1"),
            text(MessageRole::User, "more"),
            text(MessageRole::Assistant, "step 2"),
        ];
        let db_ids = vec![None; messages.len()];
        let tail_start = plan_tail(&messages, &db_ids, 100_000);
        assert!(
            tail_start < messages.len(),
            "child run (all unpersisted) must not snap the tail away to empty"
        );
    }

    #[test]
    fn summarization_input_marks_dropped_prefix() {
        let mut messages = vec![text(MessageRole::System, "sys")];
        for i in 0..10 {
            messages.push(text(
                MessageRole::User,
                format!("m{i} {}", "x".repeat(4_000)),
            ));
        }
        let prompt = text(MessageRole::User, "summarize");
        let input = summarization_input(&messages, 2_000, &prompt);
        assert_eq!(input[0].role, MessageRole::System);
        assert!(input[1].text_content().contains("not available"));
        assert_eq!(input.last().unwrap().text_content(), "summarize");
        let full = summarization_input(&messages, u32::MAX, &prompt);
        assert!(
            !full[1].text_content().contains("not available"),
            "full-budget input must not carry the partial note"
        );
        assert_eq!(full.len(), messages.len() + 1);
    }

    #[test]
    fn extract_summary_prefers_tagged_span() {
        let raw = "<analysis>walkthrough</analysis>\n<summary>## Task\nbuild</summary>";
        assert_eq!(extract_summary(raw), "## Task\nbuild");
        let untagged = "<analysis>thinking</analysis>\nplain summary body";
        assert_eq!(extract_summary(untagged), "plain summary body");
        assert_eq!(extract_summary("just text"), "just text");
    }

    #[test]
    fn summarization_and_wrapper_preserve_language() {
        assert!(super::SUMMARIZATION_PROMPT.contains("original language"));
        assert!(super::SUMMARIZATION_PROMPT.contains("noting the language the user is writing in"));
        assert!(super::SUMMARY_WRAPPER.contains("language of their original request"));
        assert!(
            super::SUMMARY_WRAPPER
                .contains("keep code, identifiers, paths, and commands unchanged")
        );
    }
}
