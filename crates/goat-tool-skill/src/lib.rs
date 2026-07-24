use goat_tool::{Tool, ToolContext, ToolError, ToolFuture, ToolOutput};
use serde::Deserialize;

pub struct SkillTool;

#[derive(Deserialize)]
struct Input {
    name: String,
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
                "name": {"type": "string"}
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

    fn run<'a>(&'a self, input: &'a str, ctx: &'a ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let args: Input = serde_json::from_str(input)?;
            let skills = goat_skill::load(&ctx.cwd);
            match skills.get(&args.name) {
                Some(skill) => Ok(ToolOutput::text(skill.body.clone())),
                None => Err(ToolError::UnknownSkill { name: args.name }),
            }
        })
    }
}

pub fn all() -> Vec<Box<dyn Tool>> {
    vec![Box::new(SkillTool)]
}

#[cfg(test)]
mod tests {
    use super::SkillTool;
    use goat_tool::{Tool, ToolContext, ToolError};

    fn write_project_skill(dir: &std::path::Path, name: &str, contents: &str) {
        let skill_dir = dir.join(goat_config::PROJECT_SKILLS_SUBDIR).join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), contents).unwrap();
    }

    #[tokio::test]
    async fn loads_project_skill_body() {
        let dir = tempfile::tempdir().unwrap();
        write_project_skill(
            dir.path(),
            "demo",
            "---\ndescription: a demo\n---\nThe full instructions.",
        );
        let ctx = ToolContext::new(dir.path()).unwrap();
        let out = SkillTool.run(r#"{"name":"demo"}"#, &ctx).await.unwrap();
        assert_eq!(out.as_text().unwrap(), "The full instructions.");
    }

    #[tokio::test]
    async fn unknown_skill_errors() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path()).unwrap();
        let result = SkillTool.run(r#"{"name":"missing"}"#, &ctx).await;
        assert!(matches!(result, Err(ToolError::UnknownSkill { .. })));
    }
}
