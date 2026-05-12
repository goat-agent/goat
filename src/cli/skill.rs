use anyhow::{anyhow, Result};
use clap::Subcommand;
use goat_config::GoatPaths;
use goat_skills::SkillIndex;

use super::ui::{self, Footer, Style, Table};

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// List every discovered Agent Skill and persona-scoped override.
    #[command(visible_alias = "ls")]
    List,
    /// Print one skill's SKILL.md.
    Show { name: String },
}

pub async fn run(cmd: Cmd) -> Result<()> {
    let paths = GoatPaths::default_layout()?;
    match cmd {
        Cmd::List => list(&paths),
        Cmd::Show { name } => show(&paths, &name),
    }
}

fn list(paths: &GoatPaths) -> Result<()> {
    ui::cell("Skills", || {
        let idx = SkillIndex::discover_root(&paths.root);

        let mut table = Table::new(["name", "scope", "status", "description"]);
        let mut rows = 0usize;
        for e in idx.all_entries() {
            table.styled_row(vec![
                (e.name.clone(), Style::Plain),
                (e.scope.label().to_string(), Style::Plain),
                ("ok".into(), Style::Ok),
                (e.description.clone(), Style::Plain),
            ]);
            rows += 1;
        }
        for d in idx.diagnostics() {
            table.styled_row(vec![
                (display_skill_path(&d.path), Style::Dim),
                (d.scope.label().to_string(), Style::Plain),
                ("warn".into(), Style::Warn),
                (d.message.clone(), Style::Warn),
            ]);
            rows += 1;
        }
        if rows == 0 {
            ui::line(&ui::dim(
                "no skills under ~/.goat/skills/ or ~/.agents/skills/",
            ));
            return Ok(Footer::Hint(
                "None",
                "drop SKILL.md into ~/.goat/skills/<name>/".into(),
            ));
        }
        table.render();
        Ok(Footer::None)
    })
}

fn show(paths: &GoatPaths, name: &str) -> Result<()> {
    ui::cell(&format!("Skill {name}"), || {
        let idx = SkillIndex::discover_root(&paths.root);
        let entry = idx
            .all_entries()
            .into_iter()
            .find(|e| e.name == name)
            .ok_or_else(|| {
                anyhow!("no skill `{name}` under ~/.goat/skills/ or ~/.agents/skills/")
            })?;
        ui::pair("scope", entry.scope.label());
        ui::line(&ui::dim(&entry.path.display().to_string()));
        ui::blank();
        for raw_line in std::fs::read_to_string(&entry.path)?.lines() {
            ui::line(raw_line);
        }
        Ok(Footer::None)
    })
}

fn display_skill_path(path: &std::path::Path) -> String {
    path.parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("?")
        .to_string()
}
