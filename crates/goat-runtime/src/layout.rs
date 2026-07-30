use std::path::Path;

use goat_config::GoatPaths;
use tracing::{info, warn};

pub(crate) fn migrate(paths: &GoatPaths) {
    move_loose_subagents(&paths.agents_dir, &paths.subagents_dir);
    absorb_profile_skills(&paths.root.join("profiles"), &paths.agents_dir);
}

fn move_loose_subagents(agents: &Path, subagents: &Path) {
    let Ok(read) = std::fs::read_dir(agents) else {
        return;
    };
    for entry in read.flatten() {
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Some(name) = path.file_name() else {
            continue;
        };
        if std::fs::create_dir_all(subagents).is_err() {
            return;
        }
        let dest = subagents.join(name);
        if dest.exists() {
            warn!(
                from = %path.display(),
                to = %dest.display(),
                "a subagent with this name already exists; leaving the old file",
            );
            continue;
        }
        match std::fs::rename(&path, &dest) {
            Ok(()) => info!(
                from = %path.display(),
                to = %dest.display(),
                "moved a subagent definition out of agents/",
            ),
            Err(e) => warn!(
                from = %path.display(),
                error = %e,
                "could not move a subagent definition",
            ),
        }
    }
}

fn absorb_profile_skills(profiles: &Path, agents: &Path) {
    let Ok(read) = std::fs::read_dir(profiles) else {
        return;
    };
    for entry in read.flatten() {
        let slug_dir = entry.path();
        if !slug_dir.is_dir() {
            continue;
        }
        let Some(slug) = slug_dir.file_name() else {
            continue;
        };
        let skills = slug_dir.join("skills");
        if skills.is_dir() {
            let dest_parent = agents.join(slug);
            let dest = dest_parent.join("skills");
            if dest.exists() {
                warn!(
                    from = %skills.display(),
                    to = %dest.display(),
                    "agent skills already exist; leaving the profiles copy in place",
                );
                continue;
            }
            if std::fs::create_dir_all(&dest_parent).is_err() {
                continue;
            }
            match std::fs::rename(&skills, &dest) {
                Ok(()) => info!(
                    from = %skills.display(),
                    to = %dest.display(),
                    "moved agent skills out of profiles/",
                ),
                Err(e) => {
                    warn!(
                        from = %skills.display(),
                        error = %e,
                        "could not move agent skills",
                    );
                    continue;
                }
            }
        }
        let _ = std::fs::remove_dir(&slug_dir);
    }
    let _ = std::fs::remove_dir(profiles);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn loose_definitions_move_into_subagents() {
        let dir = tempfile::tempdir().unwrap();
        let paths = GoatPaths::from_root(dir.path().to_path_buf());
        write(&paths.agents_dir.join("helper.md"), "---\n---");
        write(&paths.agents_dir.join("dev/agent.md"), "persona");
        migrate(&paths);
        assert!(paths.subagents_dir.join("helper.md").is_file());
        assert!(!paths.agents_dir.join("helper.md").exists());
        assert!(paths.agents_dir.join("dev/agent.md").is_file());
    }

    #[test]
    fn profile_skills_move_under_the_agent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let paths = GoatPaths::from_root(dir.path().to_path_buf());
        let old = dir.path().join("profiles/dev/skills/plan/SKILL.md");
        write(&old, "---\nname: plan\ndescription: d\n---\nbody");
        migrate(&paths);
        assert!(paths.agents_dir.join("dev/skills/plan/SKILL.md").is_file());
        assert!(!dir.path().join("profiles").exists());
    }

    #[test]
    fn existing_destinations_are_never_clobbered() {
        let dir = tempfile::tempdir().unwrap();
        let paths = GoatPaths::from_root(dir.path().to_path_buf());
        write(&paths.agents_dir.join("helper.md"), "old");
        write(&paths.subagents_dir.join("helper.md"), "new");
        write(&dir.path().join("profiles/dev/skills/a/SKILL.md"), "old");
        write(&paths.agents_dir.join("dev/skills/b/SKILL.md"), "new");
        migrate(&paths);
        assert_eq!(
            std::fs::read_to_string(paths.subagents_dir.join("helper.md")).unwrap(),
            "new"
        );
        assert!(paths.agents_dir.join("helper.md").is_file());
        assert!(dir.path().join("profiles/dev/skills/a/SKILL.md").is_file());
        assert!(paths.agents_dir.join("dev/skills/b/SKILL.md").is_file());
    }

    #[test]
    fn a_missing_tree_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let paths = GoatPaths::from_root(dir.path().to_path_buf());
        migrate(&paths);
        assert!(!paths.subagents_dir.exists());
    }
}
