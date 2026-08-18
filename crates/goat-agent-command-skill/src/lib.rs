use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use std::collections::BTreeMap;

use goat_agent_command::{
    CommandArgSpec, CommandError, CommandFactory, CommandHandler, CommandOutput,
    CommandProviderContext, CommandRegistry, CommandSpec,
};
use goat_skill::{Argument, ArgumentValue, Call, Scopes, SkillSet};
use goat_types::{CommandCall, CommandName};
use tracing::warn;

pub const ID: &str = "skill";

fn register_from_context(registry: &mut CommandRegistry, ctx: &CommandProviderContext) {
    register(registry, &ctx.goat_root, &ctx.agent_slug);
}

inventory::submit! {
    CommandFactory { id: ID, register: register_from_context }
}

pub fn register(registry: &mut CommandRegistry, goat_root: &Path, agent_slug: &str) {
    for skill in SkillSet::load(&Scopes::agent(goat_root, agent_slug)).iter() {
        let name = match CommandName::new(skill.name.clone()) {
            Ok(name) => name,
            Err(e) => {
                warn!(skill = %skill.name, error = ?e, "skipping skill command");
                continue;
            }
        };
        let spec = if skill.arguments.is_empty() {
            CommandSpec::raw_string(name, skill.description.clone())
        } else {
            CommandSpec::named(
                name,
                skill.description.clone(),
                skill.arguments.iter().map(arg_spec).collect(),
            )
        };
        let handler = Arc::new(SkillCommand {
            goat_root: goat_root.to_path_buf(),
            agent_slug: agent_slug.to_owned(),
            skill: skill.name.clone(),
        });
        if let Err(e) = registry.insert(spec, handler) {
            warn!(skill = %skill.name, error = ?e, "skipping duplicate skill command");
        }
    }
}

fn arg_spec(argument: &Argument) -> CommandArgSpec {
    let description = match &argument.value {
        ArgumentValue::Choice(options) => format!(
            "{} (one of {})",
            argument.description,
            options
                .iter()
                .map(|option| option.value.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => argument.description.clone(),
    };
    CommandArgSpec::new(argument.name.clone(), description, argument.required)
}

struct SkillCommand {
    goat_root: PathBuf,
    agent_slug: String,
    skill: String,
}

#[async_trait]
impl CommandHandler for SkillCommand {
    async fn call(&self, call: CommandCall) -> Result<CommandOutput, CommandError> {
        let skills = SkillSet::load(&Scopes::agent(&self.goat_root, &self.agent_slug));
        let skill = skills
            .activate(&self.skill)
            .map_err(|e| CommandError::Failed(e.to_string()))?;
        let args =
            named_values(&call.raw).map_or_else(|| Call::Raw(call.args.clone()), Call::Named);
        let resolved = goat_skill::resolve(&skill.arguments, Some(&args))
            .map_err(|e| CommandError::Failed(e.to_string()))?;
        Ok(CommandOutput::Query {
            content: goat_skill::render(skill, resolved.as_ref()),
        })
    }
}

fn named_values(raw: &serde_json::Value) -> Option<BTreeMap<String, String>> {
    let object = raw.get("arguments")?.as_object()?;
    Some(
        object
            .iter()
            .filter_map(|(name, value)| {
                value
                    .as_str()
                    .map(|value| (name.clone(), value.to_string()))
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_root(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "goat-agent-command-skill-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[tokio::test]
    async fn registers_skill_as_command_and_expands_args() {
        let root = temp_root("register");
        let skill = root.join("skills/reminder/SKILL.md");
        std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
        std::fs::write(
            &skill,
            "---\nname: reminder\ndescription: Manage reminders\n---\n# Reminder\nTask: $1\nRaw: $ARGUMENTS",
        )
        .unwrap();

        let mut registry = CommandRegistry::new();
        register(&mut registry, &root, "dev");
        assert!(
            registry
                .specs()
                .iter()
                .any(|spec| spec.name.as_str() == "reminder")
        );

        let output = registry
            .call(CommandCall::new(
                "call_1",
                CommandName::new("reminder").unwrap(),
                "add \"보고서 작성\"",
                json!({}),
            ))
            .await
            .unwrap();
        let CommandOutput::Query { content } = output else {
            panic!("expected query output");
        };
        assert!(content.contains("Task: 보고서 작성"));
        assert!(content.contains("Raw: add \"보고서 작성\""));
    }

    #[tokio::test]
    async fn declared_arguments_register_named_specs_and_resolve() {
        let root = temp_root("named");
        let skill = root.join("skills/remind/SKILL.md");
        std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
        std::fs::write(
            &skill,
            "---\nname: remind\ndescription: Remind me\narguments:\n  - name: task\n    description: what to do\n    required: true\n    value: text_tail\n---\nTask: $task",
        )
        .unwrap();

        let mut registry = CommandRegistry::new();
        register(&mut registry, &root, "dev");
        let spec = registry
            .specs()
            .into_iter()
            .find(|spec| spec.name.as_str() == "remind")
            .unwrap();
        assert!(matches!(
            spec.args,
            goat_agent_command::CommandArgs::Named(ref args) if args.len() == 1 && args[0].required
        ));

        let output = registry
            .call(CommandCall::new(
                "call_1",
                CommandName::new("remind").unwrap(),
                "",
                json!({ "arguments": { "task": "ship" } }),
            ))
            .await
            .unwrap();
        let CommandOutput::Query { content } = output else {
            panic!("expected query output");
        };
        assert!(content.contains("Task: ship"));

        let missing = registry
            .call(CommandCall::new(
                "call_2",
                CommandName::new("remind").unwrap(),
                "",
                json!({}),
            ))
            .await;
        assert!(missing.is_err());
    }
}
