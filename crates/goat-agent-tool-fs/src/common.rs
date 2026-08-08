use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use goat_agent_tool::{ToolCaller, ToolOutput, ToolReadSnapshot};

pub const MAX_READ_BYTES: u64 = 2 * 1024 * 1024;
pub const DEFAULT_READ_LINES: usize = 2_000;
pub const DEFAULT_LIST_LIMIT: usize = 100;
pub const MAX_LIST_LIMIT: usize = 1_000;
pub const MAX_MATCHES: usize = 1_000;

pub fn existing_path(ctx: &ToolCaller, path: &Path) -> Result<PathBuf, String> {
    let resolved = ctx.resolve_path(path).map_err(|e| e.to_string())?;
    fs::canonicalize(&resolved).map_err(|e| format!("invalid path {}: {e}", resolved.display()))
}

pub fn writable_path(ctx: &ToolCaller, path: &Path) -> Result<PathBuf, String> {
    ctx.resolve_path(path).map_err(|e| e.to_string())
}

pub fn read_text(path: &Path) -> Result<String, String> {
    let meta = fs::metadata(path).map_err(|e| format!("cannot stat {}: {e}", path.display()))?;
    if !meta.is_file() {
        return Err(format!("not a file: {}", path.display()));
    }
    if meta.len() > MAX_READ_BYTES {
        return Err(format!(
            "file too large: {} bytes (limit {MAX_READ_BYTES})",
            meta.len()
        ));
    }
    fs::read_to_string(path)
        .map_err(|e| format!("cannot read {} as UTF-8 text: {e}", path.display()))
}

pub fn snapshot(path: &Path, content: &str, complete: bool) -> Result<ToolReadSnapshot, String> {
    let meta = fs::metadata(path).map_err(|e| format!("cannot stat {}: {e}", path.display()))?;
    Ok(ToolReadSnapshot {
        size: meta.len(),
        modified_ms: modified_ms(&meta),
        hash: stable_hash(content),
        complete,
    })
}

pub fn store_snapshot(
    ctx: &ToolCaller,
    path: PathBuf,
    snap: ToolReadSnapshot,
) -> Result<(), String> {
    let mut state = ctx
        .read_state
        .lock()
        .map_err(|_| "read-state lock poisoned".to_string())?;
    state.insert(path, snap);
    Ok(())
}

pub fn require_fresh_complete_read(
    ctx: &ToolCaller,
    path: &Path,
    content: &str,
) -> Result<(), String> {
    let current = snapshot(path, content, true)?;
    let state = ctx
        .read_state
        .lock()
        .map_err(|_| "read-state lock poisoned".to_string())?;
    let Some(previous) = state.get(path) else {
        return Err(format!("read the complete file first: {}", path.display()));
    };
    if !previous.complete {
        return Err(format!("read the complete file first: {}", path.display()));
    }
    if previous.size != current.size
        || previous.modified_ms != current.modified_ms
        || previous.hash != current.hash
    {
        return Err(format!("file changed since last read: {}", path.display()));
    }
    Ok(())
}

pub fn update_complete_snapshot(
    ctx: &ToolCaller,
    path: &Path,
    content: &str,
) -> Result<(), String> {
    let snap = snapshot(path, content, true)?;
    store_snapshot(ctx, path.to_path_buf(), snap)
}

pub fn modified_ms(meta: &fs::Metadata) -> Option<u128> {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis())
}

pub fn stable_hash(content: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

pub fn error(text: impl Into<String>) -> ToolOutput {
    ToolOutput::error(text.into())
}

pub fn limit(value: Option<usize>, default: usize) -> usize {
    value.unwrap_or(default).clamp(1, MAX_LIST_LIMIT)
}

pub fn is_hidden_component(path: &Path) -> bool {
    path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|s| s.starts_with('.') && s != ".")
    })
}
