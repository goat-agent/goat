use std::path::{Component, Path, PathBuf};

use crate::ToolError;

pub fn resolve_in_root(root: &Path, raw: &Path) -> Result<PathBuf, ToolError> {
    let raw_display = raw.display().to_string();
    let root = root
        .canonicalize()
        .map_err(|source| ToolError::PathResolution {
            path: root.display().to_string(),
            source,
        })?;
    let joined = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        root.join(raw)
    };
    let normalized = lexical_normalize(&joined);
    if !normalized.starts_with(&root) {
        return Err(ToolError::PathEscape { path: raw_display });
    }
    if normalized.exists() {
        let canonical = normalized
            .canonicalize()
            .map_err(|source| ToolError::PathResolution {
                path: raw_display.clone(),
                source,
            })?;
        if !canonical.starts_with(&root) {
            return Err(ToolError::PathEscape { path: raw_display });
        }
    } else {
        let real = real_ancestor_path(&normalized).ok_or_else(|| ToolError::PathEscape {
            path: raw_display.clone(),
        })?;
        if !real.starts_with(&root) {
            return Err(ToolError::PathEscape { path: raw_display });
        }
    }
    Ok(normalized)
}

fn real_ancestor_path(normalized: &Path) -> Option<PathBuf> {
    let mut ancestor = normalized;
    let mut remaining: Vec<Component> = Vec::new();
    loop {
        if std::fs::symlink_metadata(ancestor).is_ok() {
            break;
        }
        let component = ancestor.components().next_back()?;
        remaining.push(component);
        ancestor = ancestor.parent()?;
    }
    let mut real = ancestor.canonicalize().ok()?;
    for component in remaining.into_iter().rev() {
        real.push(component);
    }
    Some(lexical_normalize(&real))
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}
