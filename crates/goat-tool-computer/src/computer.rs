use std::fmt::Write;
use std::sync::Arc;
use std::time::Duration;

use goat_api::{AxNode, ComputerCommand, HostComputerOutput, Rect, RunningApp};
use goat_tool::{ToolError, ToolOutput};
use tokio::sync::Mutex;

use crate::action::{Action, Request};
use crate::state::{Shot, State};
use crate::transport::Transport;

pub struct Computer {
    transport: Arc<dyn Transport>,
    state: Mutex<State>,
}

impl Computer {
    pub fn new(transport: Arc<dyn Transport>) -> Self {
        Self {
            transport,
            state: Mutex::new(State::default()),
        }
    }

    pub async fn run(&self, input: &str) -> Result<ToolOutput, ToolError> {
        let Request {
            action,
            observe,
            max_width,
        } = Request::parse(input)?;
        let waiting = matches!(action, Action::Wait { .. });
        let delay = match &action {
            Action::Wait { ms } => Duration::from_millis((*ms).min(10_000)),
            _ => Duration::from_millis(300),
        };
        let mut state = self.state.lock().await;
        if let Some(command) = state.command(action, max_width)? {
            match self.call(command).await? {
                output @ HostComputerOutput::Screenshot { .. } => {
                    return image_output(&mut state, output, false);
                }
                HostComputerOutput::Snapshot {
                    snapshot_id,
                    app,
                    window_title,
                    window,
                    nodes,
                } => {
                    let text =
                        snapshot_text(&snapshot_id, &app, window_title.as_deref(), &window, &nodes);
                    state.snapshot_id = Some(snapshot_id);
                    return Ok(ToolOutput::text(text));
                }
                HostComputerOutput::Apps { apps } => return Ok(ToolOutput::text(apps_text(&apps))),
                HostComputerOutput::Done {} => {}
                HostComputerOutput::Point { .. } => return Err(unexpected_reply()),
            }
        }
        if waiting || observe {
            tokio::time::sleep(delay).await;
        }
        if !observe {
            return Ok(ToolOutput::text("done"));
        }
        let output = self.call(ComputerCommand::Screenshot { max_width }).await?;
        image_output(&mut state, output, true)
    }

    async fn call(&self, command: ComputerCommand) -> Result<HostComputerOutput, ToolError> {
        let expected = match &command {
            ComputerCommand::Screenshot { .. } | ComputerCommand::Zoom { .. } => Reply::Screenshot,
            ComputerCommand::Snapshot { .. } => Reply::Snapshot,
            ComputerCommand::Apps {} => Reply::Apps,
            _ => Reply::Done,
        };
        let output = self
            .transport
            .call(command)
            .await
            .map_err(|error| ToolError::execution(error.to_string()))?;
        if !matches!(
            (expected, &output),
            (Reply::Screenshot, HostComputerOutput::Screenshot { .. })
                | (Reply::Snapshot, HostComputerOutput::Snapshot { .. })
                | (Reply::Apps, HostComputerOutput::Apps { .. })
                | (Reply::Done, HostComputerOutput::Done {})
        ) {
            return Err(unexpected_reply());
        }
        Ok(output)
    }
}

enum Reply {
    Screenshot,
    Snapshot,
    Apps,
    Done,
}

fn unexpected_reply() -> ToolError {
    ToolError::execution("computer host returned an unexpected reply")
}

fn image_output(
    state: &mut State,
    output: HostComputerOutput,
    done: bool,
) -> Result<ToolOutput, ToolError> {
    let HostComputerOutput::Screenshot {
        media_type,
        data,
        width,
        height,
        display,
    } = output
    else {
        return Err(unexpected_reply());
    };
    if media_type != "image/png"
        || width == 0
        || height == 0
        || ![
            display.x,
            display.y,
            display.width,
            display.height,
            display.x + display.width,
            display.y + display.height,
        ]
        .into_iter()
        .all(f64::is_finite)
        || display.width <= 0.0
        || display.height <= 0.0
    {
        return Err(ToolError::execution(
            "computer host returned an invalid PNG screenshot or display bounds",
        ));
    }
    state.last_shot = Some(Shot {
        width,
        height,
        display,
    });
    let prefix = if done { "done; " } else { "" };
    Ok(ToolOutput::png(data).with_summary(format!("{prefix}screenshot {width}x{height}")))
}

fn snapshot_text(
    snapshot_id: &str,
    app: &RunningApp,
    window_title: Option<&str>,
    window: &Rect,
    nodes: &[AxNode],
) -> String {
    let mut text = format!(
        "snapshot_id: {snapshot_id}\napp: {} (pid {})\nwindow: {:?} ",
        app.name,
        app.pid,
        window_title.unwrap_or_default()
    );
    frame(&mut text, window);
    text.push('\n');
    for node in nodes {
        for _ in 0..node.depth {
            text.push_str("  ");
        }
        write!(text, "- {} {}", node.reference, node.role).unwrap();
        if let Some(title) = &node.title {
            write!(text, " {title:?}").unwrap();
        }
        if let Some(value) = &node.value {
            let end = value
                .char_indices()
                .nth(120)
                .map_or(value.len(), |(index, _)| index);
            write!(text, " value={:?}", &value[..end]).unwrap();
            if end < value.len() {
                text.push('…');
            }
        }
        text.push(' ');
        frame(&mut text, &node.frame);
        if node.focused {
            text.push_str(" focused");
        }
        if !node.enabled {
            text.push_str(" disabled");
        }
        if !node.actions.is_empty() {
            text.push_str(" actions=");
            for (index, action) in node.actions.iter().enumerate() {
                if index > 0 {
                    text.push(',');
                }
                text.push_str(action);
            }
        }
        text.push('\n');
    }
    text
}

fn frame(text: &mut String, rect: &Rect) {
    write!(
        text,
        "[{:.0},{:.0} {:.0}x{:.0}]",
        rect.x, rect.y, rect.width, rect.height
    )
    .unwrap();
}

fn apps_text(apps: &[RunningApp]) -> String {
    let mut text = String::new();
    for app in apps {
        let marker = if app.frontmost { '*' } else { ' ' };
        write!(text, "{marker} {} (pid {}", app.name, app.pid).unwrap();
        if let Some(bundle) = &app.bundle_id {
            write!(text, ", {bundle}").unwrap();
        }
        text.push_str(")\n");
    }
    text
}
