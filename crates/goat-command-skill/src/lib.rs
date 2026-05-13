use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use goat_command::{
    CommandCall, CommandError, CommandFactory, CommandHandler, CommandName, CommandOutput,
    CommandProviderContext, CommandRegistry, CommandSpec,
};
use goat_skills::{format_activated_skill, SkillIndex};
use goat_types::PersonaId;
use tracing::warn;

pub const ID: &str = "skill";

fn register_from_context(registry: &mut CommandRegistry, ctx: CommandProviderContext) {
    register(registry, ctx.goat_root, PersonaId(ctx.persona));
}

inventory::submit! {
    CommandFactory { id: ID, register: register_from_context }
}

pub fn register(registry: &mut CommandRegistry, goat_root: PathBuf, persona: PersonaId) {
    let index = SkillIndex::discover_root(&goat_root);
    for entry in index.effective_entries(persona) {
        let name = match CommandName::new(entry.name.clone()) {
            Ok(name) => name,
            Err(e) => {
                warn!(skill = %entry.name, error = ?e, "skipping skill command");
                continue;
            }
        };
        let spec = CommandSpec::raw_string(name, entry.description.clone());
        let handler = Arc::new(SkillCommand {
            goat_root: goat_root.clone(),
            persona,
            skill: entry.name.clone(),
        });
        if let Err(e) = registry.insert(spec, handler) {
            warn!(skill = %entry.name, error = ?e, "skipping duplicate skill command");
        }
    }
}

struct SkillCommand {
    goat_root: PathBuf,
    persona: PersonaId,
    skill: String,
}

#[async_trait]
impl CommandHandler for SkillCommand {
    async fn call(&self, call: CommandCall) -> Result<CommandOutput, CommandError> {
        let index = SkillIndex::discover_root(&self.goat_root);
        let skill = index
            .activate(self.persona, &self.skill)
            .map_err(|e| CommandError::Failed(e.to_string()))?;
        Ok(CommandOutput::Query {
            content: format_activated_skill(&skill, Some(&call.args)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_root(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "goat-command-skill-{name}-{}-{}",
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
        register(&mut registry, root, PersonaId::from_slug("dev"));
        assert!(registry
            .specs()
            .iter()
            .any(|spec| spec.name.as_str() == "reminder"));

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
}
