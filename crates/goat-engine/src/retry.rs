use std::time::{Duration, Instant};

use goat_protocol::Event;
use goat_provider::{Request, StreamError};
use rand::RngExt;
use tokio_util::sync::CancellationToken;

use crate::{
    Ctx, LoopEnv, Run,
    rounds::{RoundEnd, RoundResult, run_round},
};

pub(crate) const MAX_ATTEMPTS: u32 = 6;
const BASE_DELAY: Duration = Duration::from_secs(1);
const MAX_DELAY: Duration = Duration::from_secs(30);
const MIN_RETRY_AFTER: Duration = Duration::from_secs(1);
const MAX_RETRY_AFTER: Duration = Duration::from_mins(10);

pub(crate) fn retryable(error: &StreamError) -> bool {
    matches!(
        error,
        StreamError::RateLimited { .. }
            | StreamError::Overloaded { .. }
            | StreamError::Transport { .. }
    )
}

pub(crate) fn reason_label(error: &StreamError) -> &'static str {
    match error {
        StreamError::RateLimited { .. } => "rate limited",
        StreamError::Overloaded { .. } => "overloaded",
        StreamError::Transport { .. } => "connection lost",
        StreamError::ContextOverflow { .. } => "context window exceeded",
        StreamError::Auth { .. } => "authentication failed",
        StreamError::InvalidRequest { .. } => "invalid request",
        StreamError::Other { .. } => "provider error",
    }
}

pub(crate) fn failure_message(error: &StreamError, target: &goat_protocol::ModelTarget) -> String {
    match error {
        StreamError::Auth { message } => format!(
            "authentication failed ({}/{}): {message}",
            target.provider, target.account,
        ),
        other => other.to_string(),
    }
}

pub(crate) fn error_hint(error: &StreamError) -> Option<String> {
    match error {
        StreamError::Auth { .. } => {
            Some("/config to re-login — progress saved, send a message to continue".to_owned())
        }
        StreamError::ContextOverflow { .. } => {
            Some("/compact to free context, then resend".to_owned())
        }
        StreamError::RateLimited { .. } | StreamError::Overloaded { .. } => {
            Some("wait a moment and resend, or /model to switch".to_owned())
        }
        StreamError::Transport { .. } => Some("check your connection and resend".to_owned()),
        StreamError::InvalidRequest { .. } | StreamError::Other { .. } => None,
    }
}

fn format_elapsed(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

pub(crate) fn backoff_delay(error: &StreamError, attempt: u32) -> Duration {
    if let StreamError::RateLimited {
        retry_after: Some(after),
        ..
    } = error
    {
        return (*after).clamp(MIN_RETRY_AFTER, MAX_RETRY_AFTER);
    }
    let exponential = BASE_DELAY.saturating_mul(2u32.saturating_pow(attempt.saturating_sub(1)));
    exponential
        .min(MAX_DELAY)
        .mul_f64(rand::rng().random_range(0.5..=1.0))
}

pub(crate) fn resets_at(error: &StreamError) -> Option<i64> {
    if let StreamError::RateLimited { resets_at, .. } = error {
        *resets_at
    } else {
        None
    }
}

fn exhausted(mut result: RoundResult, attempts: u32, started: Instant) -> RoundResult {
    if let RoundEnd::Failed(
        StreamError::RateLimited { message, .. }
        | StreamError::Overloaded { message }
        | StreamError::Transport { message },
    ) = &mut result.end
    {
        *message = exhausted_message(message, attempts, started);
    }
    result
}

pub(crate) fn exhausted_message(message: &str, attempts: u32, started: Instant) -> String {
    format!(
        "gave up after {attempts} attempts ({}): {message}",
        format_elapsed(started.elapsed()),
    )
}

pub(crate) async fn run_round_with_retry(
    ctx: &Ctx<'_>,
    run: &Run<'_>,
    env: &LoopEnv<'_>,
    messages: &[goat_provider::Message],
    token: &CancellationToken,
) -> RoundResult {
    let started = Instant::now();
    let mut attempt = 1u32;
    loop {
        let request = Request {
            model: env.target.model.clone(),
            messages: messages.to_vec(),
            tools: env.tool_defs.to_vec(),
            effort: env.target.effort,
            tool_choice: goat_provider::ToolChoice::Auto,
            temperature: None,
            max_tokens: None,
            system: None,
        };
        let result = run_round(ctx, run, env.provider, request, token).await;
        let RoundEnd::Failed(error) = &result.end else {
            return result;
        };
        if !retryable(error) {
            return result;
        }
        if attempt >= MAX_ATTEMPTS {
            return exhausted(result, attempt, started);
        }
        let delay = backoff_delay(error, attempt);
        let _ = ctx
            .events
            .send(Event::Retrying {
                id: run.id,
                attempt,
                max_attempts: MAX_ATTEMPTS,
                delay_ms: u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                reason: reason_label(error).to_owned(),
                resets_at: resets_at(error),
            })
            .await;
        tokio::select! {
            biased;
            () = token.cancelled() => return RoundResult::ended(RoundEnd::Cancelled),
            () = tokio::time::sleep(delay) => {}
        }
        attempt += 1;
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use goat_provider::StreamError;

    #[test]
    fn retry_after_overrides_backoff_curve() {
        let error = StreamError::rate_limited("slow down", Some(Duration::from_secs(45)));
        assert_eq!(super::backoff_delay(&error, 1), Duration::from_secs(45));
        let huge = StreamError::rate_limited("slow down", Some(Duration::from_secs(10_000)));
        assert_eq!(super::backoff_delay(&huge, 1), Duration::from_mins(10));
    }

    #[test]
    fn backoff_grows_exponentially_with_jitter() {
        let error = StreamError::overloaded("busy");
        for attempt in 1..=super::MAX_ATTEMPTS {
            let delay = super::backoff_delay(&error, attempt);
            let ceiling =
                Duration::from_secs(2u64.saturating_pow(attempt - 1)).min(Duration::from_secs(30));
            assert!(
                delay <= ceiling,
                "attempt {attempt}: {delay:?} > {ceiling:?}"
            );
            assert!(
                delay >= ceiling.mul_f64(0.5),
                "attempt {attempt}: {delay:?} below jitter floor"
            );
        }
    }

    #[test]
    fn fatal_errors_are_not_retryable() {
        assert!(!super::retryable(&StreamError::auth("nope")));
        assert!(!super::retryable(&StreamError::invalid_request("bad")));
        assert!(!super::retryable(&StreamError::context_overflow("full")));
        assert!(!super::retryable(&StreamError::other("quota")));
        assert!(super::retryable(&StreamError::transport("reset")));
    }

    #[test]
    fn auth_failure_message_carries_remediation() {
        let target = goat_protocol::ModelTarget {
            provider: "anthropic".into(),
            model: "m".into(),
            account: "work".into(),
            effort: None,
        };
        let message = super::failure_message(&StreamError::auth("expired"), &target);
        assert!(message.contains("anthropic/work"));
        let hint = super::error_hint(&StreamError::auth("expired")).unwrap();
        assert!(hint.contains("/config to re-login"));
        assert!(hint.contains("progress saved"));
    }

    #[test]
    fn error_hint_covers_retryable_and_overflow() {
        assert!(super::error_hint(&StreamError::rate_limited("x", None)).is_some());
        assert!(super::error_hint(&StreamError::transport("x")).is_some());
        assert!(
            super::error_hint(&StreamError::ContextOverflow {
                message: "x".into()
            })
            .is_some()
        );
        assert!(super::error_hint(&StreamError::other("x")).is_none());
    }
}
