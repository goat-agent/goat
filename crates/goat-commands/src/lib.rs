use std::fmt::Write as _;

pub use goat_command::{
    BranchSpec, ChoiceSpec, Command, CommandEffect, CommandShape, CommandSpec, ParameterSpec,
    ParameterValue,
};
use goat_command::{CommandInvocation, ParsedValue, parse_line};
pub use goat_command_app::PlanScreen;
pub use goat_command_conversation::RewindScreen;
pub use goat_command_settings::AccountScreen;
use goat_protocol::{SkillArgument, SkillArgumentValue, SkillInfo};

pub struct CommandRegistry {
    builtins: Vec<Box<dyn Command>>,
    skills: Vec<SkillInfo>,
}

impl CommandRegistry {
    pub fn builtin() -> Self {
        Self {
            builtins: builtin_commands(),
            skills: Vec::new(),
        }
    }

    pub fn set_skills(&mut self, skills: &[SkillInfo]) {
        self.skills = skills.to_vec();
    }

    pub fn contains(&self, name: &str) -> bool {
        self.builtins
            .iter()
            .any(|command| command.name() == name || command.aliases().contains(&name))
            || self.skills.iter().any(|skill| skill.name == name)
    }

    pub fn resolve_line(
        &self,
        raw: &str,
        session: &mut dyn goat_command::Session,
    ) -> CommandEffect {
        let line = match parse_line(raw) {
            Ok(line) => line,
            Err(error) => return command_error(session, error.message()),
        };
        if let Some(command) = self.builtins.iter().find(|command| {
            command.name() == line.name || command.aliases().contains(&line.name.as_str())
        }) {
            let spec = command.spec();
            return match spec.parse(raw, &line.args) {
                Ok(invocation) => command.run(invocation, session),
                Err(error) => command_error(session, error.message()),
            };
        }
        if let Some(skill) = self.skills.iter().find(|skill| skill.name == line.name) {
            return resolve_skill(skill, raw, &line.args, session);
        }
        command_error(session, format!("unknown command: /{}", line.name))
    }

    pub fn resolve(
        &self,
        name: &str,
        args: &str,
        session: &mut dyn goat_command::Session,
    ) -> CommandEffect {
        let suffix = if args.trim().is_empty() {
            String::new()
        } else {
            format!(" {args}")
        };
        self.resolve_line(&format!("/{name}{suffix}"), session)
    }

    pub fn spec(&self, name: &str) -> Option<CommandSpec> {
        if let Some(command) = self
            .builtins
            .iter()
            .find(|command| command.name() == name || command.aliases().contains(&name))
        {
            return Some(command.spec());
        }
        self.skills
            .iter()
            .find(|skill| skill.name == name)
            .map(skill_spec)
    }

    pub fn specs(&self) -> Vec<CommandSpec> {
        let builtins = self.builtins.iter().map(|command| command.spec());
        let skills = self
            .skills
            .iter()
            .filter(|skill| {
                !self.builtins.iter().any(|command| {
                    command.name() == skill.name || command.aliases().contains(&skill.name.as_str())
                })
            })
            .map(skill_spec);
        builtins.chain(skills).collect()
    }
}

fn command_error(session: &mut dyn goat_command::Session, message: String) -> CommandEffect {
    session.notify(goat_protocol::NotifyKind::Error, message);
    CommandEffect::Noop
}

fn resolve_skill(
    skill: &SkillInfo,
    raw: &str,
    args: &str,
    session: &mut dyn goat_command::Session,
) -> CommandEffect {
    if skill.arguments.is_empty() {
        return CommandEffect::Submit {
            display: skill_display(&skill.name, args),
            prompt: skill_invocation(&skill.name, args),
        };
    }
    let spec = skill_spec(skill);
    match spec.parse(raw, args) {
        Ok(invocation) => CommandEffect::Submit {
            display: invocation.raw.clone(),
            prompt: structured_skill_invocation(&skill.name, invocation),
        },
        Err(error) => command_error(session, error.message()),
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::builtin()
    }
}

fn builtin_commands() -> Vec<Box<dyn Command>> {
    let mut commands = goat_command_settings::all();
    commands.extend(goat_command_conversation::all());
    commands.extend(goat_command_help::all());
    commands.extend(goat_command_app::all());
    commands
}

fn skill_spec(skill: &SkillInfo) -> CommandSpec {
    CommandSpec {
        name: skill.name.clone(),
        description: skill.description.clone(),
        aliases: Vec::new(),
        shape: if skill.arguments.is_empty() {
            free_form_shape()
        } else {
            CommandShape::Parameters(skill.arguments.iter().map(skill_parameter).collect())
        },
    }
}

fn free_form_shape() -> CommandShape {
    CommandShape::Parameters(vec![ParameterSpec {
        name: "instructions".to_owned(),
        description: "instructions for the skill".to_owned(),
        required: false,
        value: ParameterValue::TextTail,
    }])
}

fn skill_parameter(argument: &SkillArgument) -> ParameterSpec {
    ParameterSpec {
        name: argument.name.clone(),
        description: argument.description.clone(),
        required: argument.required,
        value: skill_value(&argument.value),
    }
}

fn skill_value(value: &SkillArgumentValue) -> ParameterValue {
    match value {
        SkillArgumentValue::Word {} => ParameterValue::Word,
        SkillArgumentValue::Integer {} => ParameterValue::Integer,
        SkillArgumentValue::Choice { options: choices } => ParameterValue::Choice(
            choices
                .iter()
                .map(|choice| ChoiceSpec {
                    value: choice.value.clone(),
                    description: choice.description.clone(),
                })
                .collect(),
        ),
        SkillArgumentValue::TextTail {} => ParameterValue::TextTail,
    }
}

fn skill_display(name: &str, args: &str) -> String {
    let args = args.trim();
    if args.is_empty() {
        format!("/{name}")
    } else {
        format!("/{name} {args}")
    }
}

fn skill_invocation(name: &str, args: &str) -> String {
    skill_display(name, args)
}

fn structured_skill_invocation(_name: &str, invocation: CommandInvocation) -> String {
    let mut text = invocation.raw.clone();
    if !invocation.parameters.is_empty() {
        text.push_str("\n\nArguments:");
        for parameter in invocation.parameters {
            let _ = write!(
                text,
                "\n{}: {}",
                parameter.name,
                parsed_value(parameter.value)
            );
        }
    }
    text
}

fn parsed_value(value: ParsedValue) -> String {
    match value {
        ParsedValue::Word(value) | ParsedValue::Choice(value) | ParsedValue::Text(value) => value,
        ParsedValue::Integer(value) => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{CommandEffect, CommandRegistry};
    use goat_command::{CommandShape, EmptySession, ParameterValue};
    use goat_protocol::{SkillArgument, SkillArgumentValue, SkillInfo};

    fn resolve(registry: &CommandRegistry, raw: &str) -> CommandEffect {
        registry.resolve_line(raw, &mut EmptySession::default())
    }

    fn skill(name: &str) -> SkillInfo {
        SkillInfo {
            name: name.to_owned(),
            description: "a demo".to_owned(),
            arguments: Vec::new(),
        }
    }

    #[test]
    fn builtin_commands_resolve_to_effects() {
        let registry = CommandRegistry::builtin();
        assert!(matches!(
            resolve(&registry, "/model"),
            CommandEffect::Show(_)
        ));
        assert!(matches!(
            resolve(&registry, "/config"),
            CommandEffect::Show(_)
        ));
        assert!(matches!(
            resolve(&registry, "/provider"),
            CommandEffect::Show(_)
        ));
        assert!(matches!(
            resolve(&registry, "/clear"),
            CommandEffect::Dispatch(_)
        ));
        assert!(matches!(
            resolve(&registry, "/usage"),
            CommandEffect::Show(_)
        ));
        assert!(matches!(
            resolve(&registry, "/status"),
            CommandEffect::Show(_)
        ));
        assert!(matches!(
            resolve(&registry, "/help"),
            CommandEffect::Show(_)
        ));
        assert!(matches!(resolve(&registry, "/exit"), CommandEffect::Quit));
    }

    #[test]
    fn exit_alias_quit_resolves_to_quit() {
        let registry = CommandRegistry::builtin();
        assert!(matches!(resolve(&registry, "/quit"), CommandEffect::Quit));
    }

    #[test]
    fn unknown_command_is_error() {
        assert!(matches!(
            resolve(&CommandRegistry::builtin(), "/nope"),
            CommandEffect::Noop
        ));
    }

    #[test]
    fn skills_resolve_to_submit() {
        let mut registry = CommandRegistry::builtin();
        registry.set_skills(&[skill("demo")]);
        match resolve(&registry, "/demo with args") {
            CommandEffect::Submit { display, prompt } => {
                assert_eq!(display, "/demo with args");
                assert_eq!(prompt, "/demo with args");
            }
            _ => panic!("expected submit command effect"),
        }
    }

    #[test]
    fn set_skills_replaces_previous() {
        let mut registry = CommandRegistry::builtin();
        registry.set_skills(&[SkillInfo {
            name: "old".to_owned(),
            description: "x".to_owned(),
            arguments: Vec::new(),
        }]);
        registry.set_skills(&[SkillInfo {
            name: "new".to_owned(),
            description: "y".to_owned(),
            arguments: Vec::new(),
        }]);
        assert!(matches!(resolve(&registry, "/old"), CommandEffect::Noop));
        assert!(matches!(
            resolve(&registry, "/new"),
            CommandEffect::Submit { .. }
        ));
    }

    #[test]
    fn specs_list_builtins_and_skills() {
        let mut registry = CommandRegistry::builtin();
        registry.set_skills(&[skill("demo")]);
        let names: Vec<_> = registry.specs().into_iter().map(|spec| spec.name).collect();
        assert!(names.iter().any(|name| name == "model"));
        assert!(names.iter().any(|name| name == "provider"));
        assert!(names.iter().any(|name| name == "demo"));
    }

    #[test]
    fn exit_spec_carries_quit_alias() {
        let registry = CommandRegistry::builtin();
        let exit = registry
            .specs()
            .into_iter()
            .find(|spec| spec.name == "exit")
            .unwrap();
        assert!(exit.aliases.iter().any(|alias| alias == "quit"));
    }

    #[test]
    fn builtin_specs_include_shapes() {
        let registry = CommandRegistry::builtin();
        let effort = registry.spec("effort").unwrap();
        let CommandShape::Parameters(parameters) = effort.shape else {
            panic!("expected parameters");
        };
        assert!(matches!(parameters[0].value, ParameterValue::Word));
    }

    #[test]
    fn skill_default_spec_is_instructions_text_tail() {
        let mut registry = CommandRegistry::builtin();
        registry.set_skills(&[skill("demo")]);
        let spec = registry.spec("demo").unwrap();
        let CommandShape::Parameters(parameters) = spec.shape else {
            panic!("expected parameters");
        };
        assert_eq!(parameters[0].name, "instructions");
        assert!(matches!(parameters[0].value, ParameterValue::TextTail));
    }

    #[test]
    fn structured_skill_invocation_formats_arguments() {
        let mut registry = CommandRegistry::builtin();
        registry.set_skills(&[SkillInfo {
            name: "review".to_owned(),
            description: "review".to_owned(),
            arguments: vec![SkillArgument {
                name: "target".to_owned(),
                description: "target".to_owned(),
                required: true,
                value: SkillArgumentValue::Word {},
            }],
        }]);
        let CommandEffect::Submit { display, prompt } = resolve(&registry, "/review src/lib.rs")
        else {
            panic!("expected submit command");
        };
        assert_eq!(display, "/review src/lib.rs");
        assert_eq!(
            prompt,
            "/review src/lib.rs\n\nArguments:\ntarget: src/lib.rs"
        );
    }

    #[test]
    fn a_declared_choice_refuses_a_value_it_does_not_offer() {
        let mut registry = CommandRegistry::builtin();
        registry.set_skills(&[SkillInfo {
            name: "deploy".to_owned(),
            description: "deploy".to_owned(),
            arguments: vec![SkillArgument {
                name: "env".to_owned(),
                description: "env".to_owned(),
                required: true,
                value: SkillArgumentValue::Choice {
                    options: vec![goat_protocol::SkillChoice {
                        value: "prod".to_owned(),
                        description: None,
                    }],
                },
            }],
        }]);
        assert!(matches!(
            resolve(&registry, "/deploy prod"),
            CommandEffect::Submit { .. }
        ));
        assert!(matches!(
            resolve(&registry, "/deploy staging"),
            CommandEffect::Noop
        ));
    }
}
