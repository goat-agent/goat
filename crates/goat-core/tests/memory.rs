#[cfg(test)]
#[path = "support/server.rs"]
mod server;

use goat_core::account::{Account, Credential, Kind};
use goat_core::provider::Endpoints;
use goat_core::session::{Environment, Event, Session, Sink};
use goat_core::store::Store;

const TURNS: usize = 50;
const BUDGET_KIB: u64 = 50 * 1024;

struct Discard;

impl Sink for Discard {
    fn emit(&mut self, _: Event<'_>) {}
}

#[tokio::test]
async fn fifty_turns_stay_within_the_memory_budget() {
    let server = server::replay(vec![
        include_str!("fixtures/openai/tool_call.sse"),
        include_str!("fixtures/openai/answer.sse"),
    ])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let env = Environment {
        workspace: dir.path().to_owned(),
        home: dir.path().to_owned(),
        spill: dir.path().join(".spill"),
        endpoints: Endpoints {
            openai: server.url.as_str().into(),
            anthropic: server.url.as_str().into(),
        },
    };
    let store = Store::open(&dir.path().join("goat.db")).unwrap();
    let account = Account::new(
        Kind::OpenAi,
        "test",
        Credential::ApiKey { key: "key".into() },
    );
    let mut session = Session::create(
        store,
        reqwest::Client::new(),
        &env,
        account,
        "gpt-6.2-astra",
    )
    .unwrap();

    for turn in 0..TURNS {
        session
            .turn(&format!("turn {turn}"), &mut Discard)
            .await
            .unwrap();
    }
    assert_eq!(session.messages().len(), TURNS * 4);
    assert_eq!(server.requests.lock().unwrap().len(), TURNS * 2);

    if let Some(peak) = peak_rss_kib() {
        eprintln!("peak rss after {TURNS} turns: {peak} KiB");
        assert!(
            peak <= BUDGET_KIB,
            "peak rss {peak} KiB is over {BUDGET_KIB} KiB"
        );
    }
}

fn peak_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))
        .and_then(|value| value.trim().trim_end_matches("kB").trim().parse().ok())
}
