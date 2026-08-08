use std::sync::Arc;
use std::time::Duration;

use goat_protocol::Event;
use goat_tool::{Tool, ToolSandbox};
use goat_tool_shell::BashTool;
use tokio::sync::{Notify, mpsc};

#[tokio::test]
async fn foreground_and_background_bash_receive_the_same_environment() {
    let cwd = tempfile::tempdir().unwrap();
    let context = ToolSandbox::new(cwd.path()).unwrap();
    let foreground = BashTool
        .run(r#"{"command":"env | sort | cksum"}"#, &context)
        .await
        .unwrap()
        .as_text()
        .unwrap()
        .trim()
        .to_owned();

    let (events, mut event_rx) = mpsc::channel(64);
    let runs = crate::background::Runs::new(events, Arc::new(Notify::new()), None);
    let started = runs
        .spawn(
            "env | sort | cksum",
            None,
            cwd.path(),
            crate::background::WatchMode::CompletionOnly,
        )
        .await
        .unwrap();

    let background = tokio::time::timeout(Duration::from_secs(5), async {
        let mut output = None;
        let mut exited = false;
        while output.is_none() || !exited {
            match event_rx.recv().await.unwrap() {
                Event::ProcessOutput { process, chunk } if process == started.id => {
                    output = Some(chunk);
                }
                Event::ProcessExited { process, .. } if process == started.id => exited = true,
                _ => {}
            }
        }
        output.unwrap()
    })
    .await
    .unwrap();

    assert_eq!(background.trim(), foreground);
}
