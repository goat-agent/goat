use std::fmt::Write as _;

use crate::args::Resolved;
use crate::manifest::ArgumentValue;
use crate::scan::Skill;

#[must_use]
pub fn render(skill: &Skill, resolved: Option<&Resolved>) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "<skill_content name=\"{}\">", escape(&skill.name));
    let body = skill.body.trim();
    out.push_str(&resolved.map_or_else(|| body.to_owned(), |resolved| resolved.apply(body)));
    out.push('\n');
    if let Some(dir) = &skill.dir {
        let _ = write!(out, "\nSkill directory: {}", dir.display());
        out.push_str("\nRelative paths in this skill are relative to the skill directory.\n");
    }
    let resources = skill.resources();
    if !resources.is_empty() {
        out.push_str("<skill_resources>\n");
        for resource in resources {
            let _ = writeln!(
                out,
                "  <file kind=\"{}\">{}</file>",
                escape(resource.kind),
                escape(&resource.path.to_string_lossy())
            );
        }
        out.push_str("</skill_resources>\n");
    }
    out.push_str("</skill_content>");
    out
}

pub(crate) fn catalog<'a>(skills: impl Iterator<Item = &'a Skill>) -> Option<String> {
    let mut body = String::new();
    let mut any_arguments = false;
    for skill in skills {
        body.push_str("  <skill>\n");
        let _ = writeln!(body, "    <name>{}</name>", escape(&skill.name));
        let _ = writeln!(
            body,
            "    <description>{}</description>",
            escape(&skill.description)
        );
        for argument in &skill.arguments {
            any_arguments = true;
            let _ = write!(
                body,
                "    <argument name=\"{}\" required=\"{}\" takes=\"{}\"",
                escape(&argument.name),
                argument.required,
                argument.value.label()
            );
            if let ArgumentValue::Choice(options) = &argument.value {
                let _ = write!(
                    body,
                    " options=\"{}\"",
                    escape(
                        &options
                            .iter()
                            .map(|option| option.value.as_str())
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                );
            }
            let _ = writeln!(body, ">{}</argument>", escape(&argument.description));
        }
        body.push_str("  </skill>\n");
    }
    if body.is_empty() {
        return None;
    }
    let mut out = String::from(
        "The following skills provide specialized instructions for specific tasks.\n\
When a task matches a skill's description, call the `Skill` tool with the skill name before proceeding.\n\
Do not load skill resources eagerly; use listed resource paths only when needed.\n\
<available_skills>\n",
    );
    out.push_str(&body);
    out.push_str("</available_skills>");
    if any_arguments {
        out.push_str(
            "\nWhen a skill lists <argument> entries, pass values by name in the `Skill` tool's `arguments` object.",
        );
    }
    Some(out)
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use crate::args::{Call, resolve};
    use crate::manifest::{Argument, ArgumentValue};
    use crate::scan::{Scope, Skill};

    fn skill(arguments: Vec<Argument>) -> Skill {
        Skill {
            name: "audit".to_owned(),
            description: "audits <things>".to_owned(),
            arguments,
            body: "Audit $target now.".to_owned(),
            dir: None,
            scope: Scope::Common,
        }
    }

    #[test]
    fn a_rendered_skill_carries_its_substituted_body() {
        let declared = vec![Argument {
            name: "target".to_owned(),
            description: "what to audit".to_owned(),
            required: true,
            value: ArgumentValue::Word,
        }];
        let skill = skill(declared.clone());
        let resolved = resolve(&declared, Some(&Call::Raw("payments".to_owned())))
            .unwrap()
            .unwrap();
        let rendered = super::render(&skill, Some(&resolved));
        assert!(rendered.starts_with("<skill_content name=\"audit\">\n"));
        assert!(rendered.contains("Audit payments now."));
        assert!(rendered.ends_with("</skill_content>"));
    }

    #[test]
    fn a_catalog_declares_what_each_argument_takes() {
        let listed = skill(vec![Argument {
            name: "target".to_owned(),
            description: "what to audit".to_owned(),
            required: true,
            value: ArgumentValue::Word,
        }]);
        let catalog = super::catalog([&listed].into_iter()).expect("a non-empty catalog renders");
        assert!(catalog.contains("<description>audits &lt;things&gt;</description>"));
        assert!(catalog.contains(
            "<argument name=\"target\" required=\"true\" takes=\"word\">what to audit</argument>"
        ));
        assert!(catalog.contains("`arguments` object"));
    }

    #[test]
    fn an_empty_catalog_is_absent_rather_than_blank() {
        assert!(super::catalog(std::iter::empty()).is_none());
    }
}
