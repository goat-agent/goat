use std::path::{Path, PathBuf};

pub fn load_project_instructions(cwd: &Path) -> Option<String> {
    let mut segments: Vec<String> = Vec::new();

    if let Some(global) = goat_config::global_instructions_file()
        && let Some(content) = read_capped(&global)
    {
        segments.push(format!(
            "# Project instructions (~/.goat/AGENTS.md)\n\n{content}"
        ));
    }

    let root = find_git_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    for dir in ancestor_chain(&root, cwd) {
        let Some(path) = pick_instructions_file(&dir) else {
            continue;
        };
        if let Some(content) = read_capped(&path) {
            let rel = path
                .strip_prefix(&root)
                .map_or_else(|_| path.display().to_string(), |p| p.display().to_string());
            segments.push(format!("# Project instructions ({rel})\n\n{content}"));
        }
    }

    if segments.is_empty() {
        None
    } else {
        Some(segments.join("\n\n"))
    }
}

fn find_git_root(cwd: &Path) -> Option<PathBuf> {
    let mut dir = cwd;
    loop {
        if dir.join(".git").exists() {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?;
    }
}

fn ancestor_chain(root: &Path, cwd: &Path) -> Vec<PathBuf> {
    let mut chain: Vec<PathBuf> = Vec::new();
    let mut dir = cwd;
    loop {
        chain.push(dir.to_path_buf());
        if dir == root {
            break;
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => break,
        }
    }
    chain.reverse();
    chain
}

fn pick_instructions_file(dir: &Path) -> Option<PathBuf> {
    let override_path = dir.join(goat_config::PROJECT_INSTRUCTIONS_OVERRIDE_FILE);
    if override_path.is_file() {
        return Some(override_path);
    }
    let standard_path = dir.join(goat_config::PROJECT_INSTRUCTIONS_FILE);
    if standard_path.is_file() {
        return Some(standard_path);
    }
    None
}

fn read_capped(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let cap = goat_config::INSTRUCTIONS_MAX_BYTES;
    if bytes.len() <= cap {
        return String::from_utf8(bytes).ok();
    }
    let truncated = &bytes[..cap];
    let text = match std::str::from_utf8(truncated) {
        Ok(s) => s.to_owned(),
        Err(e) => std::str::from_utf8(&truncated[..e.valid_up_to()])
            .ok()?
            .to_owned(),
    };
    Some(format!("{text}\n\n[...truncated at 32 KiB]"))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn no_files_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_project_instructions(dir.path()).is_none());
    }

    #[test]
    fn single_agents_md_included() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("AGENTS.md"), "rule: use snake_case").unwrap();
        let result = load_project_instructions(dir.path()).unwrap();
        assert!(result.contains("rule: use snake_case"));
    }

    #[test]
    fn override_file_takes_precedence() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("AGENTS.md"), "standard content").unwrap();
        fs::write(dir.path().join("AGENTS.override.md"), "override content").unwrap();
        let result = load_project_instructions(dir.path()).unwrap();
        assert!(result.contains("override content"));
        assert!(!result.contains("standard content"));
    }

    #[test]
    fn git_root_walk_orders_root_before_leaf() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join("AGENTS.md"), "root-rule").unwrap();
        let subdir = dir.path().join("sub");
        fs::create_dir_all(&subdir).unwrap();
        fs::write(subdir.join("AGENTS.md"), "leaf-rule").unwrap();
        let result = load_project_instructions(&subdir).unwrap();
        let root_pos = result.find("root-rule").unwrap();
        let leaf_pos = result.find("leaf-rule").unwrap();
        assert!(root_pos < leaf_pos);
    }

    #[test]
    fn no_git_uses_cwd_only() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("AGENTS.md"), "single-rule").unwrap();
        let result = load_project_instructions(dir.path()).unwrap();
        assert!(result.contains("single-rule"));
    }

    #[test]
    fn caps_content_at_32kib() {
        let dir = tempfile::tempdir().unwrap();
        let large = "x".repeat(40 * 1024);
        fs::write(dir.path().join("AGENTS.md"), &large).unwrap();
        let result = load_project_instructions(dir.path()).unwrap();
        assert!(result.contains("[...truncated at 32 KiB]"));
        assert!(result.len() < large.len() + 200);
    }
}
