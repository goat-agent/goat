use std::fmt::Write as _;

pub use goat_command::{
    BranchSpec, ChoiceSpec, Command, CommandEffect, CommandShape, CommandSpec, ParameterSpec,
    ParameterValue,
};
use goat_command::{CommandInvocation, ParsedValue, parse_line};
use goat_protocol::{
    SkillBranchInfo, SkillCommandShape, SkillInfo, SkillParameterInfo, SkillParameterValue,
};

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

    pub fn resolve_line(&self, raw: &str) -> CommandEffect {
        let line = match parse_line(raw) {
            Ok(line) => line,
            Err(error) => return CommandEffect::Error(error.message()),
        };
        if let Some(command) = self.builtins.iter().find(|command| {
            command.name() == line.name || command.aliases().contains(&line.name.as_str())
        }) {
            let spec = command.spec();
            return match spec.parse(raw, &line.args) {
                Ok(invocation) => command.run(invocation),
                Err(error) => CommandEffect::Error(error.message()),
            };
        }
        if let Some(skill) = self.skills.iter().find(|skill| skill.name == line.name) {
            return resolve_skill(skill, raw, &line.args);
        }
        CommandEffect::Error(format!("unknown command: /{}", line.name))
    }

    pub fn resolve(&self, name: &str, args: &str) -> CommandEffect {
        let suffix = if args.trim().is_empty() {
            String::new()
        } else {
            format!(" {args}")
        };
        self.resolve_line(&format!("/{name}{suffix}"))
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

fn resolve_skill(skill: &SkillInfo, raw: &str, args: &str) -> CommandEffect {
    if skill.command.is_none() {
        return CommandEffect::SubmitCommand {
            display: skill_display(&skill.name, args),
            prompt: skill_invocation(&skill.name, args),
        };
    }
    let spec = skill_spec(skill);
    match spec.parse(raw, args) {
        Ok(invocation) => CommandEffect::SubmitCommand {
            display: invocation.raw.clone(),
            prompt: structured_skill_invocation(&skill.name, invocation),
        },
        Err(error) => CommandEffect::Error(error.message()),
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
        shape: skill
            .command
            .as_ref()
            .map_or_else(default_skill_shape, skill_shape),
    }
}

fn default_skill_shape() -> CommandShape {
    CommandShape::Parameters(vec![ParameterSpec {
        name: "instructions".to_owned(),
        description: "instructions for the skill".to_owned(),
        required: false,
        value: ParameterValue::TextTail,
    }])
}

fn skill_shape(command: &SkillCommandShape) -> CommandShape {
    match command {
        SkillCommandShape::Arguments { items: arguments } => {
            CommandShape::Parameters(arguments.iter().map(skill_parameter).collect())
        }
        SkillCommandShape::Subcommands { items: subcommands } => {
            CommandShape::Branches(subcommands.iter().map(skill_branch).collect())
        }
    }
}

fn skill_branch(branch: &SkillBranchInfo) -> BranchSpec {
    BranchSpec {
        name: branch.name.clone(),
        description: branch.description.clone(),
        parameters: branch.arguments.iter().map(skill_parameter).collect(),
    }
}

fn skill_parameter(parameter: &SkillParameterInfo) -> ParameterSpec {
    ParameterSpec {
        name: parameter.name.clone(),
        description: parameter.description.clone(),
        required: parameter.required,
        value: skill_value(&parameter.value),
    }
}

fn skill_value(value: &SkillParameterValue) -> ParameterValue {
    match value {
        SkillParameterValue::Word {} => ParameterValue::Word,
        SkillParameterValue::Integer {} => ParameterValue::Integer,
        SkillParameterValue::Choice { options: choices } => ParameterValue::Choice(
            choices
                .iter()
                .map(|choice| ChoiceSpec {
                    value: choice.value.clone(),
                    description: choice.description.clone(),
                })
                .collect(),
        ),
        SkillParameterValue::TextTail {} => ParameterValue::TextTail,
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
    use goat_command::{CommandShape, ParameterValue};
    use goat_protocol::{
        SkillBranchInfo, SkillCommandShape, SkillInfo, SkillParameterInfo, SkillParameterValue,
    };

    fn skill(name: &str) -> SkillInfo {
        SkillInfo {
            name: name.to_owned(),
            description: "a demo".to_owned(),
            command: None,
        }
    }

    #[test]
    fn builtin_commands_resolve_to_effects() {
        let registry = CommandRegistry::builtin();
        assert!(matches!(
            registry.resolve_line("/model"),
            CommandEffect::OpenModelPicker
        ));
        assert!(matches!(
            registry.resolve_line("/config"),
            CommandEffect::OpenConfig
        ));
        assert!(matches!(
            registry.resolve_line("/provider"),
            CommandEffect::OpenConfig
        ));
        assert!(matches!(
            registry.resolve_line("/clear"),
            CommandEffect::ClearConversation
        ));
        assert!(matches!(
            registry.resolve_line("/help"),
            CommandEffect::ShowHelp
        ));
        assert!(matches!(
            registry.resolve_line("/exit"),
            CommandEffect::Quit
        ));
    }

    #[test]
    fn exit_alias_quit_resolves_to_quit() {
        let registry = CommandRegistry::builtin();
        assert!(matches!(
            registry.resolve_line("/quit"),
            CommandEffect::Quit
        ));
    }

    #[test]
    fn unknown_command_is_error() {
        assert!(matches!(
            CommandRegistry::builtin().resolve_line("/nope"),
            CommandEffect::Error(_)
        ));
    }

    #[test]
    fn skills_resolve_to_submit() {
        let mut registry = CommandRegistry::builtin();
        registry.set_skills(&[skill("demo")]);
        match registry.resolve_line("/demo with args") {
            CommandEffect::SubmitCommand { display, prompt } => {
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
            command: None,
        }]);
        registry.set_skills(&[SkillInfo {
            name: "new".to_owned(),
            description: "y".to_owned(),
            command: None,
        }]);
        assert!(matches!(
            registry.resolve_line("/old"),
            CommandEffect::Error(_)
        ));
        assert!(matches!(
            registry.resolve_line("/new"),
            CommandEffect::SubmitCommand { .. }
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
            command: Some(SkillCommandShape::Arguments {
                items: vec![SkillParameterInfo {
                    name: "target".to_owned(),
                    description: "target".to_owned(),
                    required: true,
                    value: SkillParameterValue::Word {},
                }],
            }),
        }]);
        let CommandEffect::SubmitCommand { display, prompt } =
            registry.resolve_line("/review src/lib.rs")
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
    fn structured_skill_invocation_formats_subcommands() {
        let mut registry = CommandRegistry::builtin();
        registry.set_skills(&[SkillInfo {
            name: "review".to_owned(),
            description: "review".to_owned(),
            command: Some(SkillCommandShape::Subcommands {
                items: vec![SkillBranchInfo {
                    name: "security".to_owned(),
                    description: "security".to_owned(),
                    arguments: vec![SkillParameterInfo {
                        name: "focus".to_owned(),
                        description: "focus".to_owned(),
                        required: false,
                        value: SkillParameterValue::TextTail {},
                    }],
                }],
            }),
        }]);
        let CommandEffect::SubmitCommand { display, prompt } =
            registry.resolve_line("/review security auth flow")
        else {
            panic!("expected submit command");
        };
        assert_eq!(display, "/review security auth flow");
        assert_eq!(
            prompt,
            "/review security auth flow\n\nArguments:\nfocus: auth flow"
        );
    }

    #[test]
    fn unknown_skill_subcommand_errors() {
        let mut registry = CommandRegistry::builtin();
        registry.set_skills(&[SkillInfo {
            name: "review".to_owned(),
            description: "review".to_owned(),
            command: Some(SkillCommandShape::Subcommands {
                items: vec![SkillBranchInfo {
                    name: "security".to_owned(),
                    description: "security".to_owned(),
                    arguments: Vec::new(),
                }],
            }),
        }]);
        assert!(matches!(
            registry.resolve_line("/review nope"),
            CommandEffect::Error(_)
        ));
    }
}
