use std::{
    any::Any,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use goat_protocol::{TaskId, ToolCallId, ToolDisplay};
use goat_tool::{
    Tool, ToolContext, ToolDefinitionContext, ToolError, ToolFuture, ToolInvocation, ToolOutput,
};

const SLUG_MAX_LEN: usize = 48;

pub struct PlanSubmission {
    pub task: TaskId,
    pub call: ToolCallId,
    pub plan: String,
    pub path: PathBuf,
}

pub type PlanFuture<'a> = Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

pub trait PlanService: Send + Sync {
    fn path(&self, host: Option<&(dyn Any + Send + Sync)>) -> Option<PathBuf>;
    fn submit<'a>(&'a self, submission: PlanSubmission) -> PlanFuture<'a>;
}

pub struct ProposePlanTool {
    service: Arc<dyn PlanService>,
}

impl ProposePlanTool {
    pub fn new(service: Arc<dyn PlanService>) -> Self {
        Self { service }
    }
}

impl Tool for ProposePlanTool {
    fn name(&self) -> &'static str {
        "ProposePlan"
    }

    fn description(&self) -> &'static str {
        "Submit the plan you wrote for the user to approve. Call this only after the plan file is complete. The user reviews it and either approves — which leaves plan mode and starts implementation — or rejects it with feedback for you to revise. Takes no arguments; the plan is read from the plan file."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    }

    fn run<'a>(&'a self, _input: &'a str, _ctx: &'a ToolContext) -> ToolFuture<'a> {
        Box::pin(async { Err(ToolError::execution("plan invocation is unavailable")) })
    }

    fn invoke<'a>(
        &'a self,
        _input: &'a str,
        _ctx: &'a ToolContext,
        invocation: ToolInvocation<'a>,
    ) -> ToolFuture<'a> {
        Box::pin(async move {
            let path = self.service.path(invocation.host).ok_or_else(|| {
                ToolError::execution("no plan file is bound to this session; write the plan first")
            })?;
            let plan = tokio::fs::read_to_string(&path).await.map_err(|err| {
                ToolError::execution(format!("cannot read plan file {}: {err}", path.display()))
            })?;
            if plan.trim().is_empty() {
                return Err(ToolError::execution(format!(
                    "the plan file {} is empty; write the plan before proposing it",
                    path.display()
                )));
            }
            self.service
                .submit(PlanSubmission {
                    task: invocation.task,
                    call: invocation.call,
                    plan,
                    path,
                })
                .await
                .map_err(ToolError::execution)?;
            Ok(ToolOutput::text(
                "Plan submitted. The user is reviewing it — end your turn now and wait.",
            )
            .with_summary("awaiting approval"))
        })
    }

    fn enabled(&self, context: ToolDefinitionContext) -> bool {
        context.planning
    }

    fn display_input(&self, _input: &str) -> ToolDisplay {
        ToolDisplay::primary(goat_tool::display::call_sig(self.name(), &[]))
    }
}

pub fn approved_input(path: &Path) -> String {
    format!(
        "The plan at {} is approved. Implement it now. Re-read the file if you need the details.",
        path.display()
    )
}

pub fn rejected_input(feedback: &str) -> String {
    let trimmed = feedback.trim();
    if trimmed.is_empty() {
        "The user did not approve the plan. Revise it and call ProposePlan again.".to_owned()
    } else {
        format!(
            "The user did not approve the plan and asked for changes:\n\n{trimmed}\n\nRevise the plan file and call ProposePlan again."
        )
    }
}

pub fn resolve_path(dir: &Path, thread_id: i64, seed: &str) -> PathBuf {
    if let Some(found) = existing_for_thread(dir, thread_id) {
        return found;
    }
    let slug = slugify(seed);
    if slug.is_empty() {
        dir.join(format!("{thread_id}.md"))
    } else {
        dir.join(format!("{thread_id}-{slug}.md"))
    }
}

fn existing_for_thread(dir: &Path, thread_id: i64) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let exact = format!("{thread_id}.md");
    let prefix = format!("{thread_id}-");
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.ends_with(".md") {
            continue;
        }
        if name == exact || name.starts_with(&prefix) {
            return Some(entry.path());
        }
    }
    None
}

fn slugify(text: &str) -> String {
    let mut slug = String::new();
    let mut pending_dash = false;
    for ch in text.chars() {
        if slug.chars().count() >= SLUG_MAX_LEN {
            break;
        }
        if ch.is_ascii_alphanumeric() {
            if pending_dash && !slug.is_empty() {
                slug.push('-');
            }
            pending_dash = false;
            slug.extend(ch.to_lowercase());
        } else if ch.is_alphanumeric() {
            if pending_dash && !slug.is_empty() {
                slug.push('-');
            }
            pending_dash = false;
            slug.push(ch);
        } else {
            pending_dash = true;
        }
    }
    slug
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{approved_input, rejected_input, resolve_path, slugify};

    #[test]
    fn slug_keeps_alphanumerics_and_collapses_separators() {
        assert_eq!(slugify("Add plan mode!"), "add-plan-mode");
        assert_eq!(slugify("  spaced   out  "), "spaced-out");
        assert_eq!(slugify("한글 제목"), "한글-제목");
        assert_eq!(slugify("!!!"), "");
    }

    #[test]
    fn slug_is_bounded() {
        let long = "a".repeat(200);
        assert!(slugify(&long).chars().count() <= super::SLUG_MAX_LEN);
    }

    #[test]
    fn path_uses_thread_id_and_slug() {
        let dir = tempfile::tempdir().unwrap();
        let path = resolve_path(dir.path(), 42, "Add plan mode");
        assert_eq!(path, dir.path().join("42-add-plan-mode.md"));
    }

    #[test]
    fn path_reuses_existing_file_for_thread() {
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("7-earlier-title.md");
        std::fs::write(&existing, "plan").unwrap();
        let path = resolve_path(dir.path(), 7, "totally different now");
        assert_eq!(path, existing);
    }

    #[test]
    fn path_falls_back_to_thread_id_when_slug_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(resolve_path(dir.path(), 9, "!!!"), dir.path().join("9.md"));
    }

    #[test]
    fn approval_input_names_the_plan_file() {
        let text = approved_input(Path::new("/plans/1-demo.md"));
        assert!(text.contains("/plans/1-demo.md"));
        assert!(text.contains("Implement it now"));
    }

    #[test]
    fn rejection_input_carries_feedback() {
        let text = rejected_input("  split step 2  ");
        assert!(text.contains("split step 2"));
        assert!(text.contains("ProposePlan"));
        assert!(!text.contains("  split"));
    }

    #[test]
    fn rejection_input_without_feedback_still_asks_for_a_revision() {
        let text = rejected_input("   ");
        assert!(text.contains("ProposePlan"));
    }
}
