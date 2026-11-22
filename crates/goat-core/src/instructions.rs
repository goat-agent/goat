use std::io::Read;
use std::path::{Path, PathBuf};

pub(crate) const FILE: &str = "AGENTS.md";
pub(crate) const BYTES: usize = 32 * 1024;

pub(crate) fn sources(workspace: &Path, personal: &Path) -> Vec<PathBuf> {
    let root = workspace
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .unwrap_or(workspace);
    let mut project: Vec<PathBuf> = workspace
        .ancestors()
        .take_while(|dir| dir.starts_with(root))
        .map(|dir| dir.join(FILE))
        .collect();
    project.reverse();
    std::iter::once(personal.to_owned())
        .chain(project)
        .filter(|path| path.is_file())
        .collect()
}

pub(crate) fn read(path: &Path, limit: usize) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(u64::try_from(limit).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .ok()?;
    let text = String::from_utf8_lossy(&bytes).trim().to_owned();
    (!text.is_empty()).then_some(text)
}

pub(crate) fn block(shown: &str, text: &str) -> String {
    format!("Instructions from {shown}:\n\n{text}")
}

pub(crate) fn shown(path: &Path, workspace: &Path, home: &Path) -> String {
    if let Ok(inside) = path.strip_prefix(workspace) {
        return inside.display().to_string();
    }
    match path.strip_prefix(home) {
        Ok(inside) => format!("~/{}", inside.display()),
        Err(_) => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn personal_comes_first_then_the_project_from_its_root_down() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        let workspace = root.join("crates/core");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        let personal = dir.path().join("AGENTS.md");
        for path in [&personal, &root.join(FILE), &workspace.join(FILE)] {
            std::fs::write(path, "rules").unwrap();
        }
        std::fs::write(dir.path().join("repo/crates").join("other.md"), "").unwrap();

        assert_eq!(
            sources(&workspace, &personal),
            [personal, root.join(FILE), workspace.join(FILE)]
        );
    }

    #[test]
    fn outside_a_repository_only_the_working_directory_counts() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("work");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(dir.path().join(FILE), "above").unwrap();
        std::fs::write(workspace.join(FILE), "here").unwrap();

        assert_eq!(
            sources(&workspace, &dir.path().join("none.md")),
            [workspace.join(FILE)]
        );
    }

    #[test]
    fn paths_are_shown_from_the_workspace_or_home() {
        let home = Path::new("/home/me");
        let workspace = Path::new("/home/me/work");

        assert_eq!(
            shown(&workspace.join("src/AGENTS.md"), workspace, home),
            "src/AGENTS.md"
        );
        assert_eq!(
            shown(&home.join(".goat/AGENTS.md"), workspace, home),
            "~/.goat/AGENTS.md"
        );
        assert_eq!(
            shown(Path::new("/srv/AGENTS.md"), workspace, home),
            "/srv/AGENTS.md"
        );
    }
}
