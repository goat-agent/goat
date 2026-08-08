use std::{fmt::Write as _, sync::Arc};

use goat_code_store::CodeStore as Store;
use goat_tool_shell::{BackgroundFuture, BackgroundProcessService, ProcessChunk, ProcessStart};
use tokio_util::sync::CancellationToken;

use crate::background;

pub(crate) struct EngineBackgroundProcessService {
    runs: Arc<background::Runs>,
    store: Store,
}

impl EngineBackgroundProcessService {
    pub(crate) fn new(runs: Arc<background::Runs>, store: Store) -> Self {
        Self { runs, store }
    }
}

impl BackgroundProcessService for EngineBackgroundProcessService {
    fn start<'a>(
        &'a self,
        request: ProcessStart,
        cancellation: &'a CancellationToken,
    ) -> BackgroundFuture<'a, goat_protocol::RunId> {
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Err("interrupted".to_owned());
            }
            let started = self
                .runs
                .spawn_labeled(
                    &request.command,
                    request.name.as_deref(),
                    &request.cwd,
                    request.watch,
                    request.label,
                )
                .await
                .map_err(|error| error.to_string())?;
            if let Some(pgid) = started.pgid {
                let db_id = self
                    .store
                    .create_process(goat_code_store::NewProcess {
                        pgid: i64::from(pgid),
                        command: request.command,
                        cwd: request.cwd.display().to_string(),
                        started_at: crate::persist::now_ms(),
                    })
                    .await
                    .ok();
                if let Some(db_id) = db_id {
                    self.runs.set_db_id(started.id, db_id).await;
                }
            }
            Ok(started.id)
        })
    }

    fn output(&self, run: goat_protocol::RunId) -> BackgroundFuture<'_, ProcessChunk> {
        Box::pin(async move {
            let chunk = self
                .runs
                .read_new(run)
                .await
                .ok_or_else(|| format!("no run #{run}"))?;
            Ok(ProcessChunk {
                text: chunk.text,
                state: chunk.state,
                exit_code: chunk.exit_code,
            })
        })
    }

    fn input(&self, run: goat_protocol::RunId, text: String) -> BackgroundFuture<'_, ()> {
        Box::pin(async move { self.runs.write_stdin(run, &text).await })
    }

    fn kill(&self, run: goat_protocol::RunId) -> BackgroundFuture<'_, ()> {
        Box::pin(async move { self.runs.kill(run, Some(background::Kind::Process)).await })
    }
}

pub(crate) async fn roster_message(ctx: &crate::Ctx) -> Option<goat_provider::Message> {
    let running = ctx.background.roster().await;
    if running.is_empty() {
        return None;
    }
    Some(goat_provider::Message::text(
        goat_provider::MessageRole::User,
        roster_text(&running),
    ))
}

fn roster_text(running: &[background::RunInfo]) -> String {
    let mut text = String::from(
        "<environment-status>\nAutomated status snapshot, not a user message — background work going now:\n",
    );
    for run in running {
        let watched = if run.watched { " watched" } else { "" };
        let _ = writeln!(
            text,
            "  #{}{watched} — {}: {}",
            run.id, run.label, run.title
        );
    }
    text.push_str("</environment-status>");
    text
}
