#[cfg(test)]
#[path = "support/server.rs"]
mod server;

use std::path::Path;

use goat_core::account::{Account, Credential, Kind, codex};
use goat_core::message::{Part, Role};
use goat_core::provider::Endpoints;
use goat_core::session::{Environment, Event, Session, Sink};
use goat_core::store::Store;

const OPENAI_TOOL_CALL: &str = include_str!("fixtures/openai/tool_call.sse");
const OPENAI_ANSWER: &str = include_str!("fixtures/openai/answer.sse");
const ANTHROPIC_TOOL_USE: &str = include_str!("fixtures/anthropic/tool_use.sse");
const ANTHROPIC_ANSWER: &str = include_str!("fixtures/anthropic/answer.sse");

#[derive(Default)]
struct Recorder {
    text: String,
    reasoning: String,
    tools: Vec<String>,
}

impl Sink for Recorder {
    fn emit(&mut self, event: Event<'_>) {
        match event {
            Event::Text(text) => self.text.push_str(text),
            Event::Reasoning(text) => self.reasoning.push_str(text),
            Event::RunStarted { description } => self.tools.push(description.to_owned()),
            Event::Call { summary } => self.tools.push(summary.to_owned()),
            _ => {}
        }
    }
}

fn environment(dir: &Path, url: &str) -> Environment {
    Environment {
        workspace: dir.to_owned(),
        home: dir.to_owned(),
        spill: dir.join(".spill"),
        endpoints: Endpoints {
            openai: url.into(),
            codex: url.into(),
            anthropic: url.into(),
        },
    }
}

fn account(kind: Kind) -> Account {
    Account::new(kind, "test", Credential::ApiKey { key: "key".into() })
}

#[tokio::test]
async fn openai_turn_runs_the_tool_and_replays_reasoning() {
    let server = server::replay(vec![OPENAI_TOOL_CALL, OPENAI_ANSWER]).await;
    let dir = tempfile::tempdir().unwrap();
    let env = environment(dir.path(), &server.url);
    let store = Store::open_in_memory().unwrap();
    let mut session = Session::create(
        store,
        reqwest::Client::new(),
        &env,
        account(Kind::OpenAi),
        "gpt-6.2-astra",
    )
    .unwrap();
    let mut recorder = Recorder::default();

    session.turn("say hi", &mut recorder).await.unwrap();

    assert_eq!(recorder.text, "Done.");
    assert_eq!(recorder.reasoning, "\nRun a command.\nThen answer.");
    assert_eq!(recorder.tools, ["Say hi", "bash echo hi"]);
    let roles: Vec<Role> = session.messages().iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        [Role::User, Role::Assistant, Role::Tool, Role::Assistant]
    );
    let Part::ToolResult {
        output, is_error, ..
    } = &session.messages()[2].parts[0]
    else {
        panic!("expected a tool result");
    };
    assert!(output.starts_with("hi\n"));
    assert!(!is_error);

    let requests = server.requests.lock().unwrap();
    assert!(requests[0].contains(r#""tools":[{"type":"function","name":"run""#));
    assert!(requests[0].contains(r#""parallel_tool_calls":false"#));
    assert!(requests[1].contains(r#""encrypted_content":"enc-1""#));
    assert!(requests[1].contains(r#""type":"function_call_output","call_id":"call_1""#));
    assert!(requests[1].contains(r#""store":false"#));
}

#[tokio::test]
async fn anthropic_turn_echoes_thinking_with_its_signature() {
    let server = server::replay(vec![ANTHROPIC_TOOL_USE, ANTHROPIC_ANSWER]).await;
    let dir = tempfile::tempdir().unwrap();
    let env = environment(dir.path(), &server.url);
    let store = Store::open_in_memory().unwrap();
    let mut session = Session::create(
        store,
        reqwest::Client::new(),
        &env,
        account(Kind::Anthropic),
        "claude-opus-5-5",
    )
    .unwrap();
    let mut recorder = Recorder::default();

    session.turn("say hi", &mut recorder).await.unwrap();

    assert_eq!(recorder.text, "Done.");
    assert_eq!(recorder.reasoning, "Run a command.");
    let requests = server.requests.lock().unwrap();
    assert!(
        requests[0].contains(r#""tool_choice":{"type":"auto","disable_parallel_tool_use":true}"#)
    );
    assert!(requests[1].contains(r#""signature":"sig-1""#));
    assert!(requests[1].contains(r#""type":"tool_result","tool_use_id":"toolu_1""#));
    assert!(requests[1].contains(
        r#""input":{"code": "(await bash('echo hi')).output", "description": "Say hi"}"#
    ));
}

#[tokio::test]
async fn resumed_session_continues_with_stored_messages() {
    let server = server::replay(vec![OPENAI_TOOL_CALL, OPENAI_ANSWER]).await;
    let dir = tempfile::tempdir().unwrap();
    let env = environment(dir.path(), &server.url);
    let db = dir.path().join("goat.db");
    let account = account(Kind::OpenAi);

    let mut session = Session::create(
        Store::open(&db).unwrap(),
        reqwest::Client::new(),
        &env,
        account.clone(),
        "gpt-6.2-astra",
    )
    .unwrap();
    session
        .turn("first", &mut Recorder::default())
        .await
        .unwrap();
    let id = session.id();
    drop(session);

    let store = Store::open(&db).unwrap();
    assert_eq!(store.latest_session(dir.path()).unwrap().unwrap().id, id);
    let resumed = Session::resume(
        store,
        reqwest::Client::new(),
        &env,
        id,
        account,
        "gpt-6.2-astra",
    )
    .unwrap();
    assert_eq!(resumed.messages().len(), 4);
}

fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

#[tokio::test]
async fn large_context_is_compacted_at_the_start_of_the_next_turn() {
    let big_answer =
        leak(OPENAI_ANSWER.replace(r#""input_tokens":1300"#, r#""input_tokens":250000"#));
    let summary = leak(OPENAI_ANSWER.replace("Done.", "Summary text."));
    let server = server::replay(vec![big_answer, summary, OPENAI_ANSWER]).await;
    let dir = tempfile::tempdir().unwrap();
    let env = environment(dir.path(), &server.url);
    let db = dir.path().join("goat.db");
    let mut session = Session::create(
        Store::open(&db).unwrap(),
        reqwest::Client::new(),
        &env,
        account(Kind::OpenAi),
        "gpt-6.2-astra",
    )
    .unwrap();

    session
        .turn("first", &mut Recorder::default())
        .await
        .unwrap();
    let mut recorder = Recorder::default();
    session.turn("second", &mut recorder).await.unwrap();

    assert_eq!(recorder.text, "Done.");
    let roles: Vec<Role> = session.messages().iter().map(|m| m.role).collect();
    assert_eq!(roles, [Role::Summary, Role::User, Role::Assistant]);
    let requests = server.requests.lock().unwrap();
    assert!(requests[1].contains("Write a summary of this conversation"));
    assert!(requests[1].contains(r#""tool_choice":"none""#));
    assert!(requests[2].contains("Summary text."));
    assert!(!requests[2].contains(r#""content":"first""#));
    let stored: Vec<Role> = Store::open(&db)
        .unwrap()
        .context(session.id())
        .unwrap()
        .iter()
        .map(|m| m.role)
        .collect();
    assert_eq!(stored, roles);
}

#[tokio::test]
async fn cancelled_tool_call_is_closed_before_the_next_turn() {
    let slow_tool_call = leak(OPENAI_TOOL_CALL.replace("echo hi", "sleep 5"));
    let server = server::replay(vec![slow_tool_call, OPENAI_ANSWER]).await;
    let dir = tempfile::tempdir().unwrap();
    let env = environment(dir.path(), &server.url);
    let mut session = Session::create(
        Store::open_in_memory().unwrap(),
        reqwest::Client::new(),
        &env,
        account(Kind::OpenAi),
        "gpt-6.2-astra",
    )
    .unwrap();

    let cancelled = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        session.turn("slow", &mut Recorder::default()),
    )
    .await;
    assert!(cancelled.is_err());

    session
        .turn("again", &mut Recorder::default())
        .await
        .unwrap();
    let requests = server.requests.lock().unwrap();
    assert!(requests[1].contains(
        "interrupted by the user before this run finished. Calls it had started, which may have changed files:\\n- bash sleep 5"
    ));
    let roles: Vec<Role> = session.messages().iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        [
            Role::User,
            Role::Assistant,
            Role::Tool,
            Role::User,
            Role::Assistant
        ]
    );
}

#[tokio::test]
async fn finished_results_survive_a_cancel_in_a_later_call() {
    let second_call = r#"event: response.output_item.done
data: {"type":"response.output_item.done","sequence_number":3,"output_index":2,"item":{"type":"function_call","id":"fc_2","call_id":"call_2","name":"run","arguments":"{\"code\":\"await bash('sleep 5')\",\"description\":\"Wait\"}","status":"completed"}}

event: response.completed"#;
    let two_calls = leak(OPENAI_TOOL_CALL.replacen("event: response.completed", second_call, 1));
    let server = server::replay(vec![two_calls, OPENAI_ANSWER]).await;
    let dir = tempfile::tempdir().unwrap();
    let env = environment(dir.path(), &server.url);
    let mut session = Session::create(
        Store::open_in_memory().unwrap(),
        reqwest::Client::new(),
        &env,
        account(Kind::OpenAi),
        "gpt-6.2-astra",
    )
    .unwrap();

    let cancelled = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        session.turn("two", &mut Recorder::default()),
    )
    .await;
    assert!(cancelled.is_err());
    session
        .turn("again", &mut Recorder::default())
        .await
        .unwrap();

    let Part::ToolResult { output, .. } = &session.messages()[2].parts[0] else {
        panic!("expected the first result");
    };
    assert!(output.starts_with("hi\n"));
    let Part::ToolResult {
        output, is_error, ..
    } = &session.messages()[2].parts[1]
    else {
        panic!("expected the second result");
    };
    assert!(output.contains("interrupted"));
    assert!(is_error);
}

#[tokio::test]
async fn codex_login_sends_account_and_originator_headers() {
    let server = server::replay(vec![OPENAI_ANSWER]).await;
    let dir = tempfile::tempdir().unwrap();
    let env = environment(dir.path(), &server.url);
    let token = codex::Token {
        account_id: "acc-1".into(),
        plan: Some("pro".into()),
        email: None,
        id_token: "id".into(),
        access_token: "access".into(),
        refresh_token: "refresh".into(),
        expires_at: i64::MAX / 2,
    };
    let account = Account::new(Kind::OpenAi, "codex", Credential::Codex(token));
    let mut session = Session::create(
        Store::open_in_memory().unwrap(),
        reqwest::Client::new(),
        &env,
        account,
        "gpt-6.2-sol",
    )
    .unwrap();

    session.turn("hi", &mut Recorder::default()).await.unwrap();

    let request = server.requests.lock().unwrap()[0].to_lowercase();
    assert!(request.starts_with("post /responses "));
    assert!(request.contains("authorization: bearer access"));
    assert!(request.contains("chatgpt-account-id: acc-1"));
    assert!(request.contains("originator: goat"));
    assert!(request.contains(&format!("session-id: {}", session.id())));
    assert!(request.contains(r#""include":["reasoning.encrypted_content"]"#));
}
