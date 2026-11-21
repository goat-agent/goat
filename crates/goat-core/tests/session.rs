#[cfg(test)]
#[path = "support/server.rs"]
mod server;

use std::path::Path;

use goat_core::account::{Account, Credential, Kind, codex};
use goat_core::image;
use goat_core::message::{Part, Role};
use goat_core::provider::Endpoints;
use goat_core::session::{self, Environment, Event, Prompt, Session, Sink};
use goat_core::store::Store;

const OPENAI_TOOL_CALL: &str = include_str!("fixtures/openai/tool_call.sse");
const OPENAI_ANSWER: &str = include_str!("fixtures/openai/answer.sse");
const ANTHROPIC_TOOL_USE: &str = include_str!("fixtures/anthropic/tool_use.sse");
const ANTHROPIC_ANSWER: &str = include_str!("fixtures/anthropic/answer.sse");
const ANTHROPIC_TRUNCATED_CALL: &str = include_str!("fixtures/anthropic/truncated_call.sse");
const ANTHROPIC_REFUSAL: &str = include_str!("fixtures/anthropic/refusal.sse");
const ANTHROPIC_OVERLOADED: &str = include_str!("fixtures/anthropic/overloaded.sse");
const ANTHROPIC_SEARCH: &str = include_str!("fixtures/anthropic/search.sse");
const OPENAI_SEARCH: &str = include_str!("fixtures/openai/search.sse");

#[derive(Default)]
struct Recorder {
    text: String,
    reasoning: String,
    tools: Vec<String>,
    shell: String,
    shell_ends: Vec<(Option<i32>, Option<String>)>,
    stops: Vec<&'static str>,
    order: Vec<String>,
}

impl Sink for Recorder {
    fn emit(&mut self, event: Event<'_>) {
        self.order.push(name(&event));
        match event {
            Event::Text(text) => self.text.push_str(text),
            Event::Reasoning(text) => self.reasoning.push_str(text),
            Event::RunStarted { description } => self.tools.push(description.to_owned()),
            Event::Call(call) => self
                .tools
                .push(format!("{} {}", call.kind.as_str(), call.target)),
            Event::ShellOutput(chunk) => self.shell.push_str(chunk),
            Event::Truncated => self.stops.push("truncated"),
            Event::Refused => self.stops.push("refused"),
            Event::ShellFinished { code, problem } => {
                self.shell_ends.push((code, problem.map(str::to_owned)));
            }
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

fn openai_session(env: &Environment) -> Result<Session, session::Error> {
    Session::create(
        Store::open_in_memory()?,
        reqwest::Client::new(),
        env,
        account(Kind::OpenAi),
        "gpt-6.2-astra",
    )
}

fn anthropic_session(env: &Environment) -> Result<Session, session::Error> {
    Session::create(
        Store::open_in_memory()?,
        reqwest::Client::new(),
        env,
        account(Kind::Anthropic),
        "claude-opus-5-5",
    )
}

fn text_of(session: &Session, index: usize) -> &str {
    let Part::Text { text, .. } = &session.messages()[index].parts[0] else {
        panic!("expected text");
    };
    text
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
    assert_eq!(recorder.reasoning, "Run a command.\nThen answer.\n");
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
    assert_eq!(recorder.reasoning, "Run a command.\n");
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

#[tokio::test]
async fn chosen_effort_is_sent_with_the_request() {
    let server = server::replay(vec![ANTHROPIC_ANSWER]).await;
    let dir = tempfile::tempdir().unwrap();
    let env = environment(dir.path(), &server.url);
    let mut session = Session::create(
        Store::open_in_memory().unwrap(),
        reqwest::Client::new(),
        &env,
        account(Kind::Anthropic),
        "claude-opus-5-5",
    )
    .unwrap();
    session.set_effort(Some("xhigh"));

    session.turn("hi", &mut Recorder::default()).await.unwrap();

    let requests = server.requests.lock().unwrap();
    assert!(requests[0].contains(r#""output_config":{"effort":"xhigh"}"#));
}

#[tokio::test]
async fn shell_output_streams_and_reaches_the_next_request() {
    let server = server::replay(vec![OPENAI_ANSWER]).await;
    let dir = tempfile::tempdir().unwrap();
    let env = environment(dir.path(), &server.url);
    let mut session = openai_session(&env).unwrap();
    let mut recorder = Recorder::default();

    session.shell("echo hi", &mut recorder).await.unwrap();
    session
        .turn("what did it print", &mut Recorder::default())
        .await
        .unwrap();

    assert_eq!(recorder.shell, "hi\n");
    assert_eq!(recorder.shell_ends, [(Some(0), None)]);
    let Part::Shell {
        command,
        output,
        code,
    } = &session.messages()[0].parts[0]
    else {
        panic!("expected a shell part");
    };
    assert_eq!((&**command, &**output, *code), ("echo hi", "hi\n", Some(0)));
    let requests = server.requests.lock().unwrap();
    assert!(requests[0].contains("$ echo hi\\nhi\\n[exit 0]"));
}

#[tokio::test]
async fn blocked_shell_command_is_neither_run_nor_kept() {
    let dir = tempfile::tempdir().unwrap();
    let env = environment(dir.path(), "http://127.0.0.1:9");
    let mut session = openai_session(&env).unwrap();
    let mut recorder = Recorder::default();

    session.shell("rm -rf ~", &mut recorder).await.unwrap();

    let [(None, Some(problem))] = recorder.shell_ends.as_slice() else {
        panic!("expected a blocked command");
    };
    assert!(problem.starts_with("blocked: "));
    assert!(session.messages().is_empty());
}

#[tokio::test]
async fn shell_after_a_cancelled_tool_call_follows_its_result() {
    let slow_tool_call = leak(OPENAI_TOOL_CALL.replace("echo hi", "sleep 5"));
    let server = server::replay(vec![slow_tool_call]).await;
    let dir = tempfile::tempdir().unwrap();
    let env = environment(dir.path(), &server.url);
    let mut session = openai_session(&env).unwrap();
    let cancelled = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        session.turn("slow", &mut Recorder::default()),
    )
    .await;
    assert!(cancelled.is_err());

    session
        .shell("echo hi", &mut Recorder::default())
        .await
        .unwrap();

    let roles: Vec<Role> = session.messages().iter().map(|m| m.role).collect();
    assert_eq!(roles, [Role::User, Role::Assistant, Role::Tool, Role::User]);
}

fn png(width: u32, height: u32) -> Result<Vec<u8>, ::image::ImageError> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    ::image::RgbImage::new(width, height).write_to(&mut bytes, ::image::ImageFormat::Png)?;
    Ok(bytes.into_inner())
}

#[tokio::test]
async fn an_image_the_model_reads_is_sent_with_its_result() {
    let read_call =
        leak(OPENAI_TOOL_CALL.replace("(await bash('echo hi')).output", "await read('shot.png')"));
    let server = server::replay(vec![read_call, OPENAI_ANSWER]).await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("shot.png"), png(6, 4).unwrap()).unwrap();
    let env = environment(dir.path(), &server.url);
    let mut session = openai_session(&env).unwrap();

    session
        .turn("look", &mut Recorder::default())
        .await
        .unwrap();

    let Part::ToolResult { images, .. } = &session.messages()[2].parts[0] else {
        panic!("expected a tool result");
    };
    assert_eq!(images.len(), 1);
    let requests = server.requests.lock().unwrap();
    assert!(requests[1].contains(r#""type":"input_image","image_url":"data:image/png;base64,"#));
}

#[tokio::test]
async fn attached_images_are_sent_again_after_resume() {
    let server = server::replay(vec![OPENAI_ANSWER, OPENAI_ANSWER]).await;
    let dir = tempfile::tempdir().unwrap();
    let env = environment(dir.path(), &server.url);
    let db = dir.path().join("goat.db");
    let account = account(Kind::OpenAi);
    let picture = image::prepare(png(6, 4).unwrap()).unwrap();
    let mut session = Session::create(
        Store::open(&db).unwrap(),
        reqwest::Client::new(),
        &env,
        account.clone(),
        "gpt-6.2-astra",
    )
    .unwrap();
    let prompt = Prompt {
        text: "what is this",
        images: std::slice::from_ref(&picture),
    };
    session
        .turn(prompt, &mut Recorder::default())
        .await
        .unwrap();
    let id = session.id();
    drop(session);

    let mut resumed = Session::resume(
        Store::open(&db).unwrap(),
        reqwest::Client::new(),
        &env,
        id,
        account,
        "gpt-6.2-astra",
    )
    .unwrap();
    resumed
        .turn("and now", &mut Recorder::default())
        .await
        .unwrap();

    let Part::Image(stored) = &resumed.messages()[0].parts[1] else {
        panic!("expected the attached image");
    };
    assert_eq!(stored, &picture.image);
    let requests = server.requests.lock().unwrap();
    assert!(requests[1].contains("data:image/png;base64,"));
}

#[tokio::test]
async fn a_reply_cut_off_in_a_tool_call_keeps_its_text_and_drops_the_call() {
    let server = server::replay(vec![ANTHROPIC_TRUNCATED_CALL]).await;
    let dir = tempfile::tempdir().unwrap();
    let env = environment(dir.path(), &server.url);
    let mut session = anthropic_session(&env).unwrap();
    let mut recorder = Recorder::default();

    session.turn("write it", &mut recorder).await.unwrap();

    assert_eq!(recorder.stops, ["truncated"]);
    let roles: Vec<Role> = session.messages().iter().map(|m| m.role).collect();
    assert_eq!(roles, [Role::User, Role::Assistant]);
    assert_eq!(text_of(&session, 1), "Writing the file.");
    assert_eq!(session.messages()[1].parts.len(), 1);
}

#[tokio::test]
async fn a_refused_reply_is_not_kept() {
    let server = server::replay(vec![ANTHROPIC_REFUSAL]).await;
    let dir = tempfile::tempdir().unwrap();
    let env = environment(dir.path(), &server.url);
    let mut session = anthropic_session(&env).unwrap();
    let mut recorder = Recorder::default();

    session.turn("do it", &mut recorder).await.unwrap();

    assert_eq!(recorder.stops, ["refused"]);
    let roles: Vec<Role> = session.messages().iter().map(|m| m.role).collect();
    assert_eq!(roles, [Role::User]);
}

#[tokio::test]
async fn an_overload_before_any_output_is_retried() {
    let server = server::replay(vec![ANTHROPIC_OVERLOADED, ANTHROPIC_ANSWER]).await;
    let dir = tempfile::tempdir().unwrap();
    let env = environment(dir.path(), &server.url);
    let mut session = anthropic_session(&env).unwrap();
    let mut recorder = Recorder::default();

    session.turn("hi", &mut recorder).await.unwrap();

    assert_eq!(recorder.text, "Done.");
    assert_eq!(server.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn an_overload_after_output_is_not_retried() {
    let shown_then_overloaded = leak(ANTHROPIC_ANSWER.replacen(
        "event: content_block_stop",
        "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\nevent: content_block_stop",
        1,
    ));
    let server = server::replay(vec![shown_then_overloaded, ANTHROPIC_ANSWER]).await;
    let dir = tempfile::tempdir().unwrap();
    let env = environment(dir.path(), &server.url);
    let mut session = anthropic_session(&env).unwrap();

    let result = session.turn("hi", &mut Recorder::default()).await;

    assert!(result.is_err());
    assert_eq!(server.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn an_openai_reply_cut_off_at_the_limit_is_kept() {
    let incomplete = leak(OPENAI_ANSWER.replace(
        r#"event: response.completed
data: {"type":"response.completed","sequence_number":4,"response":{"id":"resp_2","status":"completed""#,
        r#"event: response.incomplete
data: {"type":"response.incomplete","sequence_number":4,"response":{"id":"resp_2","status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}"#,
    ));
    let server = server::replay(vec![incomplete]).await;
    let dir = tempfile::tempdir().unwrap();
    let env = environment(dir.path(), &server.url);
    let mut session = openai_session(&env).unwrap();
    let mut recorder = Recorder::default();

    session.turn("go", &mut recorder).await.unwrap();

    assert_eq!(recorder.stops, ["truncated"]);
    assert_eq!(text_of(&session, 1), "Done.");
}

fn has_hits(output: &str) -> bool {
    let cited = output.find(r#""url": "https://tokio.rs"#);
    let listed = output.find(r#""url": "https://docs.rs/tokio""#);
    output.contains(r#""snippet": "Tokio is an async runtime for Rust.""#)
        && cited
            .zip(listed)
            .is_some_and(|(cited, listed)| cited < listed)
}

fn result_of(session: &Session) -> &str {
    let Part::ToolResult { output, .. } = &session.messages()[2].parts[0] else {
        panic!("expected a tool result");
    };
    output
}

#[tokio::test]
async fn openai_search_asks_the_provider_and_returns_its_citations() {
    let search_call = leak(
        OPENAI_TOOL_CALL.replace("(await bash('echo hi')).output", "await webSearch('tokio')"),
    );
    let server = server::replay(vec![search_call, OPENAI_SEARCH, OPENAI_ANSWER]).await;
    let dir = tempfile::tempdir().unwrap();
    let env = environment(dir.path(), &server.url);
    let mut session = openai_session(&env).unwrap();

    session
        .turn("find tokio", &mut Recorder::default())
        .await
        .unwrap();

    assert!(has_hits(result_of(&session)), "{}", result_of(&session));
    let requests = server.requests.lock().unwrap();
    assert!(requests[1].contains(r#""tools":[{"type":"web_search"}]"#));
    assert!(requests[1].contains(r#""content":"tokio""#));
}

#[tokio::test]
async fn anthropic_search_uses_the_web_search_server_tool() {
    let search_call = leak(
        ANTHROPIC_TOOL_USE.replace("(await bash('echo hi')).output", "await webSearch('tokio')"),
    );
    let server = server::replay(vec![search_call, ANTHROPIC_SEARCH, ANTHROPIC_ANSWER]).await;
    let dir = tempfile::tempdir().unwrap();
    let env = environment(dir.path(), &server.url);
    let mut session = anthropic_session(&env).unwrap();

    session
        .turn("find tokio", &mut Recorder::default())
        .await
        .unwrap();

    assert!(has_hits(result_of(&session)), "{}", result_of(&session));
    let requests = server.requests.lock().unwrap();
    assert!(requests[1].contains(r#""type":"web_search_20250305","name":"web_search""#));
}

fn name(event: &Event<'_>) -> String {
    match event {
        Event::Prompt(text) => format!("prompt {text}"),
        Event::ShellCommand(command) => format!("shell {command}"),
        Event::Text(text) => format!("text {text}"),
        Event::Reasoning(text) => format!("reasoning {}", text.trim_end()),
        Event::RunStarted { description } => format!("run {description}"),
        Event::Call(call) => format!("call {} {}", call.kind.as_str(), call.target),
        Event::CallFailed { problem } => format!("failed {problem}"),
        Event::RunFinished { outcome } => format!("finished {}", outcome.is_error),
        Event::ShellOutput(output) => format!("output {}", output.trim_end()),
        Event::ShellFinished { code, .. } => format!("exit {code:?}"),
        Event::Truncated => "truncated".to_owned(),
        Event::Refused => "refused".to_owned(),
        Event::Compacting => "compacting".to_owned(),
        Event::Usage(_) => "usage".to_owned(),
    }
}

fn shown(order: &[String]) -> Vec<String> {
    let mut merged: Vec<String> = Vec::new();
    for event in order {
        if event == "usage" || event.starts_with("reasoning") {
            continue;
        }
        match (merged.last_mut(), event.strip_prefix("text ")) {
            (Some(last), Some(more)) if last.starts_with("text ") => last.push_str(more),
            _ => merged.push(event.clone()),
        }
    }
    merged
}

#[tokio::test]
async fn a_resumed_session_replays_what_was_shown() {
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
    let mut live = Recorder::default();
    session.shell("echo hi", &mut live).await.unwrap();
    session.turn("say hi", &mut live).await.unwrap();
    let id = session.id();
    drop(session);

    let resumed = Session::resume(
        Store::open(&db).unwrap(),
        reqwest::Client::new(),
        &env,
        id,
        account,
        "gpt-6.2-astra",
    )
    .unwrap();
    let mut replayed = Recorder::default();
    resumed.replay(&mut replayed).unwrap();

    assert_eq!(
        shown(&replayed.order),
        [
            "shell echo hi",
            "output hi",
            "exit Some(0)",
            "prompt say hi",
            "run Say hi",
            "call bash echo hi",
            "finished false",
            "text Done.",
        ]
    );
    let live = shown(&live.order);
    assert_eq!(
        live,
        shown(&replayed.order)[1..]
            .iter()
            .filter(|event| !event.starts_with("prompt"))
            .cloned()
            .collect::<Vec<_>>()
    );
}
