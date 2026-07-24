use std::path::Path;

use goat_tool::{Tool, ToolContext};

#[tokio::test]
#[ignore = "requires a real Chrome install and drives the persistent profile"]
async fn navigates_clicks_debug_evals_and_closes() {
    let tool = goat_tool_browser::browser_tool();
    let ctx = ToolContext::new(Path::new(".")).unwrap();

    let nav = r#"{"action":"navigate","url":"data:text/html,<button onclick='window.__c=1'>Go</button>"}"#;
    let snapshot = tool.run(nav, &ctx).await.unwrap();
    let snapshot = snapshot.as_text().expect("navigate returns text");
    assert!(
        snapshot.contains("[ref=s1:e1]"),
        "snapshot should tag the button: {snapshot}"
    );

    tool.run(r#"{"action":"click","ref":"e1"}"#, &ctx)
        .await
        .unwrap();

    let value = tool
        .run(r#"{"action":"debug_eval","js":"window.__c"}"#, &ctx)
        .await
        .unwrap();
    assert_eq!(value.as_text().unwrap().trim(), "1");

    let closed = tool.run(r#"{"action":"close"}"#, &ctx).await.unwrap();
    assert!(closed.as_text().unwrap().contains("closed"));
}

#[tokio::test]
#[ignore = "requires a real Chrome install and drives the persistent profile"]
async fn snapshot_pierces_shadow_dom() {
    let tool = goat_tool_browser::browser_tool();
    let ctx = ToolContext::new(Path::new(".")).unwrap();

    let nav = r#"{"action":"navigate","url":"data:text/html,<my-el></my-el><script>customElements.define('my-el',class extends HTMLElement{connectedCallback(){this.attachShadow({mode:'open'}).innerHTML='<button>Shadow</button>'}})</script>"}"#;
    let snapshot = tool.run(nav, &ctx).await.unwrap();
    let snapshot = snapshot.as_text().unwrap();
    assert!(
        snapshot.contains("button \"Shadow\""),
        "shadow-root button should appear: {snapshot}"
    );

    tool.run(r#"{"action":"close"}"#, &ctx).await.unwrap();
}

#[tokio::test]
#[ignore = "requires a real Chrome install and drives the persistent profile"]
async fn click_reports_covering_element() {
    let tool = goat_tool_browser::browser_tool();
    let ctx = ToolContext::new(Path::new(".")).unwrap();

    let nav = r#"{"action":"navigate","url":"data:text/html,<button>Real</button><div style='position:fixed;inset:0;z-index:9999;background:red'>Cover</div>"}"#;
    tool.run(nav, &ctx).await.unwrap();

    let err = match tool.run(r#"{"action":"click","ref":"e1"}"#, &ctx).await {
        Ok(_) => panic!("covered click should error"),
        Err(err) => err.to_string(),
    };
    assert!(
        err.contains("covered") || err.contains("blocked"),
        "covered click should surface the blocker: {err}"
    );

    tool.run(r#"{"action":"close"}"#, &ctx).await.unwrap();
}

#[tokio::test]
#[ignore = "requires a real Chrome install and drives the persistent profile"]
async fn dialog_guard_prevents_hang() {
    let tool = goat_tool_browser::browser_tool();
    let ctx = ToolContext::new(Path::new(".")).unwrap();

    let nav = r#"{"action":"navigate","url":"data:text/html,<button onclick=\"alert('boom')\">Go</button>"}"#;
    tool.run(nav, &ctx).await.unwrap();

    let after = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        tool.run(r#"{"action":"click","ref":"e1"}"#, &ctx),
    )
    .await
    .expect("click must not hang on an alert dialog")
    .unwrap();
    assert!(after.as_text().is_some());

    tool.run(r#"{"action":"close"}"#, &ctx).await.unwrap();
}

#[tokio::test]
#[ignore = "requires a real Chrome install and drives the persistent profile"]
async fn read_and_storage_actions_smoke() {
    let tool = goat_tool_browser::browser_tool();
    let ctx = ToolContext::new(Path::new(".")).unwrap();

    let nav = r#"{"action":"navigate","url":"data:text/html,<main><h1>Title</h1><p>Body text here.</p></main>"}"#;
    tool.run(nav, &ctx).await.unwrap();

    let content = tool
        .run(r#"{"action":"read_content"}"#, &ctx)
        .await
        .unwrap();
    assert!(content.as_text().unwrap().contains("Body text here"));

    tool.run(
        r#"{"action":"storage","op":"set_local","name":"k","value":"v"}"#,
        &ctx,
    )
    .await
    .unwrap();
    let got = tool
        .run(r#"{"action":"storage","op":"get_local","name":"k"}"#, &ctx)
        .await
        .unwrap();
    assert!(got.as_text().unwrap().contains('v'));

    tool.run(r#"{"action":"read_console"}"#, &ctx)
        .await
        .unwrap();
    tool.run(r#"{"action":"read_network"}"#, &ctx)
        .await
        .unwrap();
    tool.run(r#"{"action":"tab","op":"list"}"#, &ctx)
        .await
        .unwrap();

    tool.run(r#"{"action":"close"}"#, &ctx).await.unwrap();
}
