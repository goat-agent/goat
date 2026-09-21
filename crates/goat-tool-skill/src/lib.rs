use std::collections::BTreeMap;

use goat_skill::{Call, Shared, SkillError};
use goat_tool::{Tool, ToolError, ToolErrorClass, ToolFuture, ToolOutput, ToolSandbox};
use serde::Deserialize;

pub struct SkillTool {
    skills: Shared,
}

impl SkillTool {
    pub fn new(skills: Shared) -> Self {
        Self { skills }
    }
}

fn class(error: &SkillError) -> ToolErrorClass {
    match error {
        SkillError::NotFound(_) => ToolErrorClass::NotFound,
        _ => ToolErrorClass::InvalidInput,
    }
}

#[derive(Deserialize)]
struct Input {
    name: String,
    #[serde(default)]
    args: Option<String>,
    #[serde(default)]
    arguments: Option<BTreeMap<String, String>>,
}

impl Tool for SkillTool {
    fn name(&self) -> &'static str {
        "Skill"
    }

    fn description(&self) -> &'static str {
        "Load a skill's instructions by name. Available skills are listed in the system prompt; call this to read the full instructions for one before following it."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The exact skill name from the system prompt's skill catalog."
                },
                "args": {
                    "type": "string",
                    "description": "Optional raw argument line. Use for a skill that declares no arguments but substitutes $ARGUMENTS, $ARGUMENTS[n], or $n."
                },
                "arguments": {
                    "type": "object",
                    "additionalProperties": {"type": "string"},
                    "description": "Named values for a skill that lists <argument> entries. Unknown names and missing required arguments are errors."
                }
            },
            "required": ["name"]
        })
    }

    fn display_input(&self, input: &str) -> goat_protocol::ToolDisplay {
        match serde_json::from_str::<Input>(input) {
            Ok(args) => goat_protocol::ToolDisplay::primary(args.name),
            Err(_) => goat_tool::display::generic(input),
        }
    }

    fn run<'a>(&'a self, input: &'a str, _ctx: &'a ToolSandbox) -> ToolFuture<'a> {
        Box::pin(async move {
            let args: Input = serde_json::from_str(input)?;
            let skills = self.skills.get();
            let skill = skills
                .activate(&args.name)
                .map_err(|error| ToolError::new(class(&error), error.to_string()))?;
            let call = match (args.arguments, args.args) {
                (Some(named), _) => Some(Call::Named(named)),
                (None, Some(raw)) => Some(Call::Raw(raw)),
                (None, None) => None,
            };
            let resolved = goat_skill::resolve(&skill.arguments, call.as_ref())
                .map_err(|error| ToolError::new(class(&error), error.to_string()))?;
            Ok(ToolOutput::text(goat_skill::render(
                skill,
                resolved.as_ref(),
            )))
        })
    }
}

pub fn all_with(skills: Shared) -> Vec<Box<dyn Tool>> {
    vec![Box::new(SkillTool::new(skills))]
}

#[cfg(test)]
mod tests {
    use goat_skill::{Scopes, Shared, SkillSet};
    use goat_tool::{Tool, ToolErrorClass, ToolSandbox};

    use super::SkillTool;

    fn write_project_skill(dir: &std::path::Path, name: &str, contents: &str) {
        let skill_dir = dir.join(goat_skill::PROJECT_SUBDIR).join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), contents).unwrap();
    }

    fn tool_at(dir: &std::path::Path) -> SkillTool {
        let set = SkillSet::load(&Scopes::code(dir, dir).with_agents_user(None));
        SkillTool::new(Shared::new(set))
    }

    #[tokio::test]
    async fn loads_project_skill_body() {
        let dir = tempfile::tempdir().unwrap();
        write_project_skill(
            dir.path(),
            "demo",
            "---\ndescription: a demo\n---\nThe full instructions.",
        );
        let ctx = ToolSandbox::new(dir.path()).unwrap();
        let out = tool_at(dir.path())
            .run(r#"{"name":"demo"}"#, &ctx)
            .await
            .unwrap();
        let text = out.as_text().unwrap();
        assert!(text.contains("<skill_content name=\"demo\">"));
        assert!(text.contains("The full instructions."));
    }

    #[tokio::test]
    async fn declared_arguments_substitute_into_the_body() {
        let dir = tempfile::tempdir().unwrap();
        write_project_skill(
            dir.path(),
            "audit",
            "---\ndescription: audits\narguments:\n  - name: target\n    description: what to audit\n    required: true\n    value: text_tail\n---\nAudit $target now.",
        );
        let ctx = ToolSandbox::new(dir.path()).unwrap();
        let out = tool_at(dir.path())
            .run(
                r#"{"name":"audit","arguments":{"target":"the payments service"}}"#,
                &ctx,
            )
            .await
            .unwrap();
        assert!(
            out.as_text()
                .unwrap()
                .contains("Audit the payments service now.")
        );
    }

    #[tokio::test]
    async fn a_missing_required_argument_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        write_project_skill(
            dir.path(),
            "audit",
            "---\ndescription: audits\narguments:\n  - name: target\n    description: what to audit\n    required: true\n    value: word\n---\nAudit $target now.",
        );
        let ctx = ToolSandbox::new(dir.path()).unwrap();
        let result = tool_at(dir.path()).run(r#"{"name":"audit"}"#, &ctx).await;
        assert!(matches!(
            result,
            Err(error) if error.class() == ToolErrorClass::InvalidInput
        ));
    }

    #[tokio::test]
    async fn unknown_skill_errors() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolSandbox::new(dir.path()).unwrap();
        let result = tool_at(dir.path()).run(r#"{"name":"missing"}"#, &ctx).await;
        assert!(matches!(
            result,
            Err(error) if error.class() == ToolErrorClass::NotFound
        ));
    }

    #[tokio::test]
    async fn a_skill_written_after_load_is_not_seen() {
        let dir = tempfile::tempdir().unwrap();
        let tool = tool_at(dir.path());
        write_project_skill(
            dir.path(),
            "late",
            "---\ndescription: added later\n---\nlate body.",
        );
        let ctx = ToolSandbox::new(dir.path()).unwrap();
        let result = tool.run(r#"{"name":"late"}"#, &ctx).await;
        assert!(
            matches!(
                result,
                Err(error) if error.class() == ToolErrorClass::NotFound
            ),
            "the session's skill set is pinned until an explicit reload"
        );
    }
}
