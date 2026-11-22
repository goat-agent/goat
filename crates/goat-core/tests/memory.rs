#[cfg(test)]
#[path = "support/server.rs"]
mod server;

use goat_core::account::{Account, Credential, Kind};
use goat_core::image;
use goat_core::provider::Endpoints;
use goat_core::session::{Environment, Event, Prompt, Session, Sink};
use goat_core::store::Store;

const TURNS: usize = 50;
const BUDGET_KIB: u64 = 40 * 1024;
const IMAGE_EDGE: u32 = 260;

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
            codex: server.url.as_str().into(),
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

    let mut requests = 0;
    for turn in 0..TURNS {
        let seed = u32::try_from(turn).unwrap();
        let picture = image::prepare(noise(seed).unwrap()).unwrap();
        let text = format!("turn {turn}");
        let prompt = Prompt {
            text: &text,
            images: std::slice::from_ref(&picture),
        };
        session.turn(prompt, &mut Discard).await.unwrap();
        let mut received = server.requests.lock().unwrap();
        requests += received.len();
        received.clear();
    }
    assert_eq!(session.messages().len(), TURNS * 4);
    assert_eq!(requests, TURNS * 2);

    if let Some(peak) = peak_data_kib() {
        eprintln!("peak memory outside code after {TURNS} turns: {peak} KiB");
        assert!(
            peak <= BUDGET_KIB,
            "peak memory outside code {peak} KiB is over {BUDGET_KIB} KiB"
        );
    }
}

fn noise(seed: u32) -> Result<Vec<u8>, ::image::ImageError> {
    let mut state = seed.wrapping_mul(2_654_435_761) | 1;
    let pixels = ::image::RgbImage::from_fn(IMAGE_EDGE, IMAGE_EDGE, |_, _| {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        let [r, g, b, _] = state.to_le_bytes();
        ::image::Rgb([r, g, b])
    });
    let mut bytes = std::io::Cursor::new(Vec::new());
    pixels.write_to(&mut bytes, ::image::ImageFormat::Png)?;
    Ok(bytes.into_inner())
}

fn peak_data_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let field = |name: &str| -> Option<u64> {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|value| value.trim().trim_end_matches("kB").trim().parse().ok())
    };
    Some(field("VmHWM:")?.saturating_sub(field("RssFile:")?))
}
