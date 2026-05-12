use std::sync::Arc;

use async_trait::async_trait;
use goat_skills::{ActivatedSkill, SkillIndex};
use goat_tool::{ToolCall, ToolContext, ToolFactory, ToolHandler, ToolName, ToolOutput, ToolSpec};
use serde::Deserialize;
use serde_json::json;

pub const NAME: ToolName = ToolName::from_static("skill.activate");

pub struct SkillTool;

#[derive(Debug, Deserialize)]
struct SkillArgs {
    name: String,
}

#[async_trait]
impl ToolHandler for SkillTool {
    async fn call(&self, ctx: ToolContext, call: ToolCall) -> ToolOutput {
        let args = match serde_json::from_value::<SkillArgs>(call.arguments) {
            Ok(args) => args,
            Err(e) => return ToolOutput::error(format!("invalid skill.activate input: {e}")),
        };
        if args.name.trim().is_empty() {
            return ToolOutput::error("skill name must not be empty");
        }
        let idx = SkillIndex::discover_root(&ctx.goat_root);
        match idx.activate(ctx.persona, &args.name) {
            Ok(skill) => ToolOutput::text(format_activated_skill(&skill)),
            Err(e) => ToolOutput::error(e.to_string()),
        }
    }
}

fn spec() -> ToolSpec {
    let mut spec = ToolSpec::new(
        NAME,
        "Load the full instructions for an available Agent Skill by name. Use after a user request matches a skill listed in <available_skills>.",
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The exact skill name from <available_skills>."
                }
            },
            "required": ["name"],
            "additionalProperties": false
        }),
    );
    spec.output_schema = Some(json!({
        "type": "object",
        "properties": {
            "content": { "type": "string" }
        },
        "required": ["content"],
        "additionalProperties": false
    }));
    spec
}

fn ctor() -> Arc<dyn ToolHandler> {
    Arc::new(SkillTool)
}

inventory::submit! {
    ToolFactory { name: NAME, default_enabled: true, spec, ctor }
}

fn format_activated_skill(skill: &ActivatedSkill) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "<skill_content name=\"{}\">\n",
        escape_attr(&skill.name)
    ));
    out.push_str(skill.body.trim());
    out.push_str("\n\nSkill directory: ");
    out.push_str(&skill.skill_dir.display().to_string());
    out.push_str("\nRelative paths in this skill are relative to the skill directory.\n");
    if !skill.resources.is_empty() {
        out.push_str("<skill_resources>\n");
        for resource in &skill.resources {
            out.push_str(&format!(
                "  <file kind=\"{}\">{}</file>\n",
                escape_attr(&resource.kind),
                escape_text(&resource.path.to_string_lossy())
            ));
        }
        out.push_str("</skill_resources>\n");
    }
    out.push_str("</skill_content>");
    out
}

fn escape_attr(s: &str) -> String {
    escape_text(s).replace('"', "&quot;")
}

fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use goat_tool::{ToolCall, ToolContext};
    use goat_types::{ChannelId, ConversationId, InstanceId, PersonaId};

    fn temp_root(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "goat-tool-skill-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn ctx(root: std::path::PathBuf) -> ToolContext {
        ToolContext {
            persona: PersonaId::from_slug("dev"),
            conversation: ConversationId {
                channel: ChannelId::from_static("test"),
                instance: InstanceId::new(),
                external: "c1".into(),
            },
            goat_root: root,
        }
    }

    #[test]
    fn spec_exposes_skill_activate_model_name() {
        let spec = spec();
        assert_eq!(spec.name.as_str(), "skill.activate");
        assert_eq!(spec.model_name(), "skill_activate");
        assert_eq!(spec.input_schema["required"][0], "name");
    }

    #[tokio::test]
    async fn activation_returns_wrapped_skill_content() {
        let root = temp_root("activate");
        let skill = root.join("skills/daily-operator/SKILL.md");
        std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
        std::fs::write(
            &skill,
            "---\nname: daily-operator\ndescription: Plan a day\n---\n# Daily\nDo it.",
        )
        .unwrap();

        let out = SkillTool
            .call(
                ctx(root),
                ToolCall {
                    call_id: "call_1".into(),
                    name: NAME,
                    arguments: json!({ "name": "daily-operator" }),
                },
            )
            .await;
        assert!(!out.is_error);
        let text = out.text_for_model();
        assert!(text.contains("<skill_content name=\"daily-operator\">"));
        assert!(text.contains("# Daily"));
    }

    #[tokio::test]
    async fn unknown_skill_is_error() {
        let out = SkillTool
            .call(
                ctx(temp_root("missing")),
                ToolCall {
                    call_id: "call_1".into(),
                    name: NAME,
                    arguments: json!({ "name": "missing" }),
                },
            )
            .await;
        assert!(out.is_error);
        assert!(out.text_for_model().contains("skill not found"));
    }
}
