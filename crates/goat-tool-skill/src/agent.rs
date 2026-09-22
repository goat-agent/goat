use std::borrow::Cow;
use std::collections::BTreeMap;

use goat_skill::{Call, Scopes, SkillSet};
use goat_tool::{
    Tool, ToolCall, ToolContext, ToolDefinitionContext, ToolFuture, ToolName, ToolOutput, ToolSpec,
};
use serde::Deserialize;
use serde_json::json;

pub const NAME: ToolName = ToolName::from_static("skill");

pub struct SkillTool;

#[derive(Debug, Deserialize)]
struct SkillArgs {
    skill: String,
    #[serde(default)]
    args: Option<String>,
    #[serde(default)]
    arguments: Option<BTreeMap<String, String>>,
}

impl Tool for SkillTool {
    fn name(&self) -> ToolName {
        NAME.clone()
    }

    fn description(&self) -> Cow<'static, str> {
        "Load the full instructions for an available Agent Skill by name. Use after a user request matches a skill listed in <available_skills>.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "skill": {
                    "type": "string",
                    "description": "The exact skill name from <available_skills>."
                },
                "args": {
                    "type": "string",
                    "description": "Optional raw argument string for the skill. Supports $ARGUMENTS, $ARGUMENTS[n], and $n placeholders in SKILL.md."
                },
                "arguments": {
                    "type": "object",
                    "additionalProperties": { "type": "string" },
                    "description": "Named values for skills that list <argument> entries in <available_skills>. Preferred over args when arguments are declared; unknown names and missing required arguments are errors."
                }
            },
            "required": ["skill"],
            "additionalProperties": false
        })
    }

    fn definition(&self, _ctx: ToolDefinitionContext) -> Option<ToolSpec> {
        Some(spec())
    }

    fn call<'a>(&'a self, call: &'a ToolCall, ctx: ToolContext<'a>) -> ToolFuture<'a> {
        Box::pin(async move {
            let args = match serde_json::from_value::<SkillArgs>(call.arguments.clone()) {
                Ok(args) => args,
                Err(e) => return Ok(ToolOutput::error(format!("invalid skill input: {e}"))),
            };
            if args.skill.trim().is_empty() {
                return Ok(ToolOutput::error("skill name must not be empty"));
            }
            let call = match (args.arguments, args.args) {
                (Some(named), _) => Some(Call::Named(named)),
                (None, Some(raw)) => Some(Call::Raw(raw)),
                (None, None) => None,
            };
            let ac = ctx.agent_context()?;
            let skills = SkillSet::load(&Scopes::agent(&ctx.sandbox.cwd, &ac.slug));
            let skill = match skills.activate(&args.skill) {
                Ok(skill) => skill,
                Err(e) => return Ok(ToolOutput::error(e.to_string())),
            };
            match goat_skill::resolve(&skill.arguments, call.as_ref()) {
                Ok(resolved) => Ok(ToolOutput::text(goat_skill::render(
                    skill,
                    resolved.as_ref(),
                ))),
                Err(e) => Ok(ToolOutput::error(e.to_string())),
            }
        })
    }
}

fn spec() -> ToolSpec {
    let tool = SkillTool;
    let mut spec = ToolSpec::new(tool.name(), tool.description(), tool.parameters());
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

pub fn register(registry: &mut goat_tool::ToolRegistry) {
    registry.insert(std::sync::Arc::new(SkillTool));
}

#[cfg(test)]
mod tests {
    use super::*;
    use goat_tool::{AgentContext, ToolReadState, ToolSandbox};
    use goat_types::{AgentId, ChannelId, ConversationId, InstanceId};
    use tokio_util::sync::CancellationToken;

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

    struct Fixture {
        sandbox: ToolSandbox,
        token: CancellationToken,
        agent: AgentContext,
    }

    impl Fixture {
        fn new(root: &std::path::Path) -> Self {
            Self {
                sandbox: ToolSandbox::rooted(root).unwrap(),
                token: CancellationToken::new(),
                agent: AgentContext {
                    id: AgentId::from_slug("dev"),
                    slug: "dev".into(),
                    conversation: ConversationId {
                        channel: ChannelId::from_static("test"),
                        instance: InstanceId::new(),
                        external: "c1".into(),
                    },
                    audience: None,
                    read_state: ToolReadState::default(),
                },
            }
        }

        fn ctx(&self) -> ToolContext<'_> {
            ToolContext::agent(&self.sandbox, &self.token, &self.agent)
        }
    }

    #[test]
    fn spec_exposes_skill_name() {
        let spec = spec();
        assert_eq!(spec.name.as_str(), "skill");
        assert_eq!(spec.input_schema["required"][0], "skill");
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

        let fixture = Fixture::new(&root);
        let out = SkillTool
            .call(
                &ToolCall {
                    call_id: "call_1".into(),
                    name: NAME,
                    arguments: json!({ "skill": "daily-operator" }),
                },
                fixture.ctx(),
            )
            .await
            .unwrap();
        assert!(!out.is_error);
        let text = out.text_for_model();
        assert!(text.contains("<skill_content name=\"daily-operator\">"));
        assert!(text.contains("# Daily"));
    }

    #[tokio::test]
    async fn activation_substitutes_skill_args() {
        let root = temp_root("args");
        let skill = root.join("skills/reminder/SKILL.md");
        std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
        std::fs::write(
            &skill,
            "---\nname: reminder\ndescription: Manage reminders\n---\n# Reminder\nSub: $0\nTask: $1\nRaw: $ARGUMENTS",
        )
        .unwrap();

        let fixture = Fixture::new(&root);
        let out = SkillTool
            .call(
                &ToolCall {
                    call_id: "call_1".into(),
                    name: NAME,
                    arguments: json!({ "skill": "reminder", "args": "add \"보고서 작성\"" }),
                },
                fixture.ctx(),
            )
            .await
            .unwrap();
        assert!(!out.is_error);
        let text = out.text_for_model();
        assert!(text.contains("Sub: add"));
        assert!(text.contains("Task: 보고서 작성"));
        assert!(
            text.contains("Raw: add &quot;보고서 작성&quot;")
                || text.contains("Raw: add \"보고서 작성\"")
        );
    }

    #[tokio::test]
    async fn named_arguments_resolve_against_declarations() {
        let root = temp_root("named");
        let skill = root.join("skills/remind/SKILL.md");
        std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
        std::fs::write(
            &skill,
            "---\nname: remind\ndescription: Remind me\narguments:\n  - name: task\n    description: what to do\n    required: true\n    value: text_tail\n---\nTask: $task",
        )
        .unwrap();

        let fixture = Fixture::new(&root);
        let out = SkillTool
            .call(
                &ToolCall {
                    call_id: "call_1".into(),
                    name: NAME,
                    arguments: json!({ "skill": "remind", "arguments": { "task": "ship" } }),
                },
                fixture.ctx(),
            )
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.text_for_model().contains("Task: ship"));

        let missing = SkillTool
            .call(
                &ToolCall {
                    call_id: "call_2".into(),
                    name: NAME,
                    arguments: json!({ "skill": "remind" }),
                },
                fixture.ctx(),
            )
            .await
            .unwrap();
        assert!(missing.is_error);
        assert!(missing.text_for_model().contains("required"));
    }

    #[tokio::test]
    async fn unknown_skill_is_error() {
        let fixture = Fixture::new(&temp_root("missing"));
        let out = SkillTool
            .call(
                &ToolCall {
                    call_id: "call_1".into(),
                    name: NAME,
                    arguments: json!({ "skill": "missing" }),
                },
                fixture.ctx(),
            )
            .await
            .unwrap();
        assert!(out.is_error);
        assert!(out.text_for_model().contains("no skill named `missing`"));
    }
}
