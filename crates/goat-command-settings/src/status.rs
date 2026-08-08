mod screen;

use goat_command::{Command, CommandEffect, CommandInvocation, Session, SessionSnapshot};
use goat_worktree::WorkspaceKind;

pub use screen::StatusScreen;
use screen::{StatusRow, daemon_label, uptime};

pub struct Status;

impl Command for Status {
    fn name(&self) -> &'static str {
        "status"
    }

    fn description(&self) -> &'static str {
        "show session, conversation, and daemon status"
    }

    fn run(&self, _invocation: CommandInvocation, session: &mut dyn Session) -> CommandEffect {
        let snapshot = session.snapshot();
        let accounts = session.accounts();
        CommandEffect::Show(Box::new(StatusScreen::new(status_rows(
            &snapshot, accounts,
        ))))
    }
}

fn status_rows(
    snapshot: &SessionSnapshot,
    accounts: &[goat_protocol::AccountEntry],
) -> Vec<StatusRow> {
    let mut rows = vec![StatusRow {
        label: "session",
        value: match (snapshot.session_id, snapshot.client_id) {
            (Some(session), Some(client)) => format!("#{session} · client {client}"),
            (Some(session), None) => format!("#{session}"),
            _ => "—".to_owned(),
        },
    }];
    rows.push(StatusRow {
        label: "conversation",
        value: snapshot
            .conversation_id
            .map_or_else(|| "—".to_owned(), |id| id.to_string()),
    });
    if let Some(daemon) = &snapshot.daemon {
        rows.push(StatusRow {
            label: "daemon",
            value: daemon_label(daemon),
        });
    }
    rows.push(StatusRow {
        label: "model",
        value: snapshot.model.as_ref().map_or_else(
            || "—".to_owned(),
            |model| {
                let multiple = accounts
                    .iter()
                    .find(|entry| entry.provider == model.provider)
                    .is_some_and(|entry| entry.accounts.len() > 1);
                model_label(model, multiple)
            },
        ),
    });
    if snapshot.mode.is_plan() {
        rows.push(StatusRow {
            label: "mode",
            value: snapshot
                .plan_path
                .as_deref()
                .map_or_else(|| "plan".to_owned(), |path| format!("plan · {path}")),
        });
    }
    rows.push(StatusRow {
        label: "cwd",
        value: snapshot.cwd.clone(),
    });
    rows.push(StatusRow {
        label: "target",
        value: snapshot
            .remote
            .as_ref()
            .map_or_else(|| "local".to_owned(), |name| format!("remote: {name}")),
    });
    if let Some(workspace) = &snapshot.workspace {
        rows.push(StatusRow {
            label: "worktree",
            value: workspace_label(workspace),
        });
    }
    if let Some(pr) = &snapshot.pull_request {
        rows.push(StatusRow {
            label: "pr",
            value: format!("#{} {}", pr.number, pr_state_label(pr.state)),
        });
    }
    rows.push(StatusRow {
        label: "windows",
        value: snapshot.window_count.to_string(),
    });
    rows.push(StatusRow {
        label: "queued",
        value: format!(
            "{} · processes {} · skills {}",
            snapshot.queued_count, snapshot.process_count, snapshot.skill_count
        ),
    });
    rows.push(StatusRow {
        label: "transcript",
        value: format!("{} entries", snapshot.transcript_entries),
    });
    rows.push(StatusRow {
        label: "toggles",
        value: format!(
            "mouse {} · computer {} · browser {}",
            toggle_label(snapshot.mouse_capture),
            toggle_label(snapshot.computer_use),
            toggle_label(snapshot.browser)
        ),
    });
    rows.push(StatusRow {
        label: "theme",
        value: if snapshot.dark_theme {
            "dark".to_owned()
        } else {
            "light".to_owned()
        },
    });
    if let Some(path) = &snapshot.log_path {
        rows.push(StatusRow {
            label: "log",
            value: path.clone(),
        });
    }
    rows.push(StatusRow {
        label: "uptime",
        value: uptime(snapshot.started),
    });
    if let Some(conversation) = snapshot.conversation_id {
        rows.push(StatusRow {
            label: "resume",
            value: format!("goat code --resume {conversation}"),
        });
    }
    rows
}

fn model_label(model: &goat_protocol::ModelTarget, multiple_accounts: bool) -> String {
    let mut label = if multiple_accounts {
        format!("{}:{}/{}", model.provider, model.account, model.model)
    } else {
        format!("{}/{}", model.provider, model.model)
    };
    if let Some(effort) = model.effort {
        label.push(':');
        label.push_str(effort.as_str());
    }
    label
}

fn workspace_label(workspace: &goat_worktree::Workspace) -> String {
    let repo = workspace.owner_root.file_name().map_or_else(
        || shorten_home(&workspace.owner_root),
        |name| name.to_string_lossy().into_owned(),
    );
    match &workspace.kind {
        WorkspaceKind::Managed { label } => format!("{repo}@{label}"),
        WorkspaceKind::Main | WorkspaceKind::OtherWorktree => {
            if workspace.git_branch.is_empty() {
                repo
            } else {
                format!("{repo}:{}", workspace.git_branch)
            }
        }
    }
}

fn shorten_home(path: &std::path::Path) -> String {
    let display = path.display().to_string();
    if let Some(home) = std::env::var_os("HOME") {
        let home = home.to_string_lossy();
        if let Some(rest) = display.strip_prefix(home.as_ref()) {
            return format!("~{rest}");
        }
    }
    display
}

fn toggle_label(enabled: bool) -> &'static str {
    if enabled { "✓" } else { "✗" }
}

fn pr_state_label(state: goat_github::PrState) -> &'static str {
    match state {
        goat_github::PrState::Open => "open",
        goat_github::PrState::Merged => "merged",
        goat_github::PrState::Closed => "closed",
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use goat_command::{Command, CommandEffect, CommandInvocation, SessionSnapshot};
    use goat_protocol::Mode;

    use super::{Status, status_rows};

    fn snapshot() -> SessionSnapshot {
        SessionSnapshot {
            session_id: None,
            client_id: None,
            conversation_id: None,
            daemon: None,
            model: None,
            models_loaded: false,
            mode: Mode::Normal,
            plan_path: None,
            cwd: "/tmp/work".to_owned(),
            remote: None,
            workspace: None,
            pull_request: None,
            window_count: 1,
            queued_count: 0,
            process_count: 0,
            skill_count: 0,
            transcript_entries: 0,
            mouse_capture: false,
            computer_use: false,
            browser: false,
            dark_theme: true,
            log_path: None,
            started: Instant::now(),
        }
    }

    #[test]
    fn conversation_rows_include_resume_command() {
        let mut snapshot = snapshot();
        snapshot.conversation_id = Some(87);
        let rows = status_rows(&snapshot, &[]);
        assert_eq!(
            rows.iter()
                .find(|row| row.label == "conversation")
                .unwrap()
                .value,
            "87"
        );
        assert_eq!(
            rows.iter().find(|row| row.label == "resume").unwrap().value,
            "goat code --resume 87"
        );
    }

    #[test]
    fn command_opens_status_screen() {
        let effect = Status.run(
            CommandInvocation {
                name: "status".to_owned(),
                subcommand: None,
                raw: "/status".to_owned(),
                raw_args: String::new(),
                parameters: Vec::new(),
            },
            &mut goat_command::EmptySession::default(),
        );
        assert!(matches!(effect, CommandEffect::Show(_)));
    }

    #[test]
    fn session_and_target_rows_use_snapshot() {
        let mut snapshot = snapshot();
        snapshot.session_id = Some(12);
        snapshot.client_id = Some(7);
        let rows = status_rows(&snapshot, &[]);
        assert_eq!(
            rows.iter()
                .find(|row| row.label == "session")
                .unwrap()
                .value,
            "#12 · client 7"
        );
        assert_eq!(
            rows.iter().find(|row| row.label == "target").unwrap().value,
            "local"
        );
    }
}
