use std::time::{Duration, Instant};

use chromiumoxide::Page;
use chromiumoxide::cdp::browser_protocol::page::StopLoadingParams;
use chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;
use tokio::time::sleep;

use crate::error::BrowserError;
use crate::resilience::{OP_META, with_timeout};

const NAV_READY_SOFTCAP: Duration = Duration::from_secs(10);
const READY_POLL_INTERVAL: Duration = Duration::from_millis(150);
const NAV_QUIET_GRACE: Duration = Duration::from_millis(300);
const POST_ACTION_PRE_DELAY: Duration = Duration::from_millis(120);
const MUTATION_SETTLE_CAP: Duration = Duration::from_secs(3);

const MUTATION_QUIET_JS: &str = "new Promise(r => { const Q=300,C=2500,F=1200; let done=false,qt=null; const fin=(x)=>{ if(done)return; done=true; try{o.disconnect()}catch(e){} clearTimeout(ft); clearTimeout(ct); if(qt)clearTimeout(qt); r(x); }; let o; try{ o=new MutationObserver(()=>{ clearTimeout(ft); if(qt)clearTimeout(qt); qt=setTimeout(()=>fin('quiet'),Q); }); o.observe(document.documentElement,{subtree:true,childList:true,attributes:true,characterData:true}); }catch(e){ r('no_root'); return; } const ft=setTimeout(()=>fin('idle'),F); const ct=setTimeout(()=>fin('cap'),C); })";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadyState {
    Loading,
    Interactive,
    Complete,
}

#[derive(Debug, PartialEq, Eq)]
enum ReadyDecision {
    Done(&'static str),
    Capped,
    Continue,
}

fn readiness_decision(
    state: ReadyState,
    interactive_elapsed: Option<Duration>,
    past_deadline: bool,
) -> ReadyDecision {
    match state {
        ReadyState::Complete => ReadyDecision::Done("complete"),
        ReadyState::Interactive => {
            if interactive_elapsed.is_some_and(|elapsed| elapsed >= NAV_QUIET_GRACE) {
                ReadyDecision::Done("interactive")
            } else if past_deadline {
                ReadyDecision::Capped
            } else {
                ReadyDecision::Continue
            }
        }
        ReadyState::Loading => {
            if past_deadline {
                ReadyDecision::Capped
            } else {
                ReadyDecision::Continue
            }
        }
    }
}

async fn read_state(page: &Page) -> ReadyState {
    match with_timeout(OP_META, "read_state", page.evaluate("document.readyState")).await {
        Ok(result) => match result.value().and_then(serde_json::Value::as_str) {
            Some("complete") => ReadyState::Complete,
            Some("interactive") => ReadyState::Interactive,
            _ => ReadyState::Loading,
        },
        Err(_) => ReadyState::Loading,
    }
}

pub async fn await_navigation_ready(page: &Page) -> &'static str {
    let deadline = Instant::now() + NAV_READY_SOFTCAP;
    let mut interactive_since: Option<Instant> = None;
    loop {
        let state = read_state(page).await;
        if state == ReadyState::Interactive {
            interactive_since.get_or_insert_with(Instant::now);
        } else {
            interactive_since = None;
        }
        let elapsed = interactive_since.map(|since| since.elapsed());
        match readiness_decision(state, elapsed, Instant::now() >= deadline) {
            ReadyDecision::Done(label) => return label,
            ReadyDecision::Capped => {
                let _ = with_timeout(
                    OP_META,
                    "stop_loading",
                    page.execute(StopLoadingParams::default()),
                )
                .await;
                return "stopped_capped";
            }
            ReadyDecision::Continue => sleep(READY_POLL_INTERVAL).await,
        }
    }
}

pub async fn settle_after_action(page: &Page) -> &'static str {
    sleep(POST_ACTION_PRE_DELAY).await;
    let load = await_navigation_ready(page).await;
    let _ = mutation_quiet(page).await;
    load
}

async fn mutation_quiet(page: &Page) -> Result<(), BrowserError> {
    let params = EvaluateParams::builder()
        .expression(MUTATION_QUIET_JS)
        .await_promise(true)
        .return_by_value(true)
        .build()
        .map_err(|err| BrowserError::Message(format!("settle eval build: {err}")))?;
    with_timeout(MUTATION_SETTLE_CAP, "mutation_quiet", page.evaluate(params)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{ReadyDecision, ReadyState, readiness_decision};

    #[test]
    fn complete_is_done_immediately() {
        assert_eq!(
            readiness_decision(ReadyState::Complete, None, false),
            ReadyDecision::Done("complete")
        );
        assert_eq!(
            readiness_decision(ReadyState::Complete, None, true),
            ReadyDecision::Done("complete")
        );
    }

    #[test]
    fn interactive_waits_for_grace_then_done() {
        assert_eq!(
            readiness_decision(ReadyState::Interactive, None, false),
            ReadyDecision::Continue
        );
        assert_eq!(
            readiness_decision(
                ReadyState::Interactive,
                Some(Duration::from_millis(100)),
                false
            ),
            ReadyDecision::Continue
        );
        assert_eq!(
            readiness_decision(
                ReadyState::Interactive,
                Some(Duration::from_millis(400)),
                false
            ),
            ReadyDecision::Done("interactive")
        );
    }

    #[test]
    fn loading_continues_until_deadline() {
        assert_eq!(
            readiness_decision(ReadyState::Loading, None, false),
            ReadyDecision::Continue
        );
        assert_eq!(
            readiness_decision(ReadyState::Loading, None, true),
            ReadyDecision::Capped
        );
    }

    #[test]
    fn deadline_caps_interactive_before_grace() {
        assert_eq!(
            readiness_decision(
                ReadyState::Interactive,
                Some(Duration::from_millis(50)),
                true
            ),
            ReadyDecision::Capped
        );
    }
}
