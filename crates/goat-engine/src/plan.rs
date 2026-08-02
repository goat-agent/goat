use std::path::{Path, PathBuf};

use goat_protocol::{Event, ToolCallId, ToolDisplay};
use goat_provider::ToolDefinition;
use goat_tool::ToolOutput;

use crate::{Ctx, Run};

pub(crate) const PROPOSE_PLAN_TOOL_NAME: &str = "ProposePlan";

const SLUG_MAX_LEN: usize = 48;

pub(crate) fn propose_plan_tool_def() -> ToolDefinition {
    ToolDefinition {
        name: PROPOSE_PLAN_TOOL_NAME.to_owned(),
        description: "Submit the plan you wrote for the user to approve. Call this only after the plan file is complete. The user reviews it and either approves — which leaves plan mode and starts implementation — or rejects it with feedback for you to revise. Takes no arguments; the plan is read from the plan file.".to_owned(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
    }
}

pub(crate) fn propose_plan_call_display(_input: &str) -> ToolDisplay {
    ToolDisplay::primary(goat_tool::display::call_sig(PROPOSE_PLAN_TOOL_NAME, &[]))
}

pub(crate) async fn run_propose_plan(
    ctx: &Ctx,
    run: &Run<'_>,
    plan_path: Option<&Path>,
    call_id: ToolCallId,
) -> Result<ToolOutput, String> {
    let path = plan_path
        .ok_or_else(|| "no plan file is bound to this session; write the plan first".to_owned())?;
    let plan = tokio::fs::read_to_string(path)
        .await
        .map_err(|err| format!("cannot read plan file {}: {err}", path.display()))?;
    if plan.trim().is_empty() {
        return Err(format!(
            "the plan file {} is empty; write the plan before proposing it",
            path.display()
        ));
    }
    let _ = ctx
        .events
        .send(Event::PlanProposed {
            id: run.id,
            call: call_id,
            plan,
            path: path.display().to_string(),
        })
        .await;
    Ok(
        ToolOutput::text("Plan submitted. The user is reviewing it — end your turn now and wait.")
            .with_summary("awaiting approval".to_owned()),
    )
}

pub(crate) fn approved_input(path: &Path) -> String {
    format!(
        "The plan at {} is approved. Implement it now. Re-read the file if you need the details.",
        path.display()
    )
}

pub(crate) fn rejected_input(feedback: &str) -> String {
    let trimmed = feedback.trim();
    if trimmed.is_empty() {
        "The user did not approve the plan. Revise it and call ProposePlan again.".to_owned()
    } else {
        format!(
            "The user did not approve the plan and asked for changes:\n\n{trimmed}\n\nRevise the plan file and call ProposePlan again."
        )
    }
}

pub(crate) fn resolve_path(dir: &Path, thread_id: i64, seed: &str) -> PathBuf {
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
        let dir = std::env::temp_dir().join("goat-plan-path-test-new");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = resolve_path(&dir, 42, "Add plan mode");
        assert_eq!(path, dir.join("42-add-plan-mode.md"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn path_reuses_existing_file_for_thread() {
        let dir = std::env::temp_dir().join("goat-plan-path-test-existing");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let existing = dir.join("7-earlier-title.md");
        std::fs::write(&existing, "plan").unwrap();
        let path = resolve_path(&dir, 7, "totally different now");
        assert_eq!(path, existing);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn path_falls_back_to_thread_id_when_slug_is_empty() {
        let dir = std::env::temp_dir().join("goat-plan-path-test-empty-slug");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(resolve_path(&dir, 9, "!!!"), dir.join("9.md"));
        let _ = std::fs::remove_dir_all(&dir);
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
