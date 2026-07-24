use std::path::{Component, Path, PathBuf};

use thiserror::Error;

use crate::scope::Scope;

#[derive(Debug, Error)]
pub enum FileError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("path escapes scope: {0}")]
    Traversal(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    Exists(String),
    #[error("text not found for replacement")]
    NoMatch,
    #[error("ambiguous replacement: {0} matches")]
    Ambiguous(usize),
    #[error("invalid line number: {0}")]
    BadLine(usize),
}

pub type FileResult<T> = Result<T, FileError>;

#[derive(Clone, Debug)]
pub struct MemoryFiles {
    root: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub chunk_no: usize,
    pub heading: String,
    pub text: String,
}

impl MemoryFiles {
    pub fn new(goat_root: &Path) -> Self {
        Self {
            root: goat_root.join("memory"),
        }
    }

    pub fn scope_dir(&self, scope: &Scope) -> PathBuf {
        self.root.join(scope.as_path_segment())
    }

    pub fn resolve(&self, scope: &Scope, rel: &str) -> FileResult<PathBuf> {
        let rel_path = Path::new(rel);
        for comp in rel_path.components() {
            match comp {
                Component::Normal(_) => {}
                _ => return Err(FileError::Traversal(rel.to_string())),
            }
        }
        Ok(self.scope_dir(scope).join(rel_path))
    }

    pub async fn view(
        &self,
        scope: &Scope,
        rel: &str,
        range: Option<(usize, usize)>,
    ) -> FileResult<String> {
        let path = self.resolve(scope, rel)?;
        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|_| FileError::NotFound(rel.to_string()))?;
        match range {
            None => Ok(content),
            Some((from, to)) => {
                let lines: Vec<&str> = content.lines().collect();
                if from == 0 || from > lines.len() {
                    return Err(FileError::BadLine(from));
                }
                let end = to.min(lines.len());
                Ok(lines[from - 1..end].join("\n"))
            }
        }
    }

    pub async fn list(&self, scope: &Scope) -> FileResult<Vec<String>> {
        let dir = self.scope_dir(scope);
        let mut out = Vec::new();
        collect_files(&dir, &dir, &mut out).await?;
        out.sort();
        Ok(out)
    }

    pub async fn create(&self, scope: &Scope, rel: &str, text: &str) -> FileResult<()> {
        let path = self.resolve(scope, rel)?;
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Err(FileError::Exists(rel.to_string()));
        }
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, text).await?;
        Ok(())
    }

    pub async fn write(&self, scope: &Scope, rel: &str, text: &str) -> FileResult<()> {
        let path = self.resolve(scope, rel)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, text).await?;
        Ok(())
    }

    pub async fn str_replace(
        &self,
        scope: &Scope,
        rel: &str,
        old: &str,
        new: &str,
    ) -> FileResult<()> {
        let path = self.resolve(scope, rel)?;
        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|_| FileError::NotFound(rel.to_string()))?;
        let count = content.matches(old).count();
        match count {
            0 => Err(FileError::NoMatch),
            1 => {
                let updated = content.replacen(old, new, 1);
                tokio::fs::write(&path, updated).await?;
                Ok(())
            }
            n => Err(FileError::Ambiguous(n)),
        }
    }

    pub async fn insert(
        &self,
        scope: &Scope,
        rel: &str,
        line: usize,
        text: &str,
    ) -> FileResult<()> {
        let path = self.resolve(scope, rel)?;
        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|_| FileError::NotFound(rel.to_string()))?;
        let mut lines: Vec<String> = content
            .lines()
            .map(std::string::ToString::to_string)
            .collect();
        if line > lines.len() {
            return Err(FileError::BadLine(line));
        }
        lines.insert(line, text.to_string());
        tokio::fs::write(&path, lines.join("\n")).await?;
        Ok(())
    }

    pub async fn delete(&self, scope: &Scope, rel: &str) -> FileResult<()> {
        let path = self.resolve(scope, rel)?;
        tokio::fs::remove_file(&path)
            .await
            .map_err(|_| FileError::NotFound(rel.to_string()))
    }

    pub async fn rename(&self, scope: &Scope, from: &str, to: &str) -> FileResult<()> {
        let src = self.resolve(scope, from)?;
        let dst = self.resolve(scope, to)?;
        if let Some(parent) = dst.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::rename(&src, &dst)
            .await
            .map_err(|_| FileError::NotFound(from.to_string()))
    }
}

pub fn chunk_markdown(content: &str) -> Vec<Chunk> {
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut cur_heading = String::new();
    let mut cur_lines: Vec<String> = Vec::new();

    let flush = |heading: &str, lines: &[String], out: &mut Vec<Chunk>| {
        let text = lines.join("\n");
        if text.trim().is_empty() {
            return;
        }
        out.push(Chunk {
            chunk_no: out.len(),
            heading: heading.to_string(),
            text,
        });
    };

    for line in content.lines() {
        if let Some(h) = line.strip_prefix('#') {
            flush(&cur_heading, &cur_lines, &mut chunks);
            cur_lines.clear();
            cur_heading = h.trim_start_matches('#').trim().to_string();
            cur_lines.push(line.to_string());
        } else {
            cur_lines.push(line.to_string());
        }
    }
    flush(&cur_heading, &cur_lines, &mut chunks);
    chunks
}

async fn collect_files(base: &Path, dir: &Path, out: &mut Vec<String>) -> FileResult<()> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&d).await else {
            continue;
        };
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(rel) = path.strip_prefix(base) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> (tempfile::TempDir, MemoryFiles) {
        let dir = tempfile::tempdir().unwrap();
        let mf = MemoryFiles::new(dir.path());
        (dir, mf)
    }

    #[tokio::test]
    async fn create_view_and_reject_duplicate() {
        let (_d, mf) = tmp();
        mf.create(&Scope::Owner, "core/profile.md", "hi")
            .await
            .unwrap();
        assert_eq!(
            mf.view(&Scope::Owner, "core/profile.md", None)
                .await
                .unwrap(),
            "hi"
        );
        assert!(
            mf.create(&Scope::Owner, "core/profile.md", "x")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn traversal_is_rejected() {
        let (_d, mf) = tmp();
        assert!(mf.resolve(&Scope::Owner, "../self/x.md").is_err());
        assert!(mf.resolve(&Scope::Owner, "a/../../b").is_err());
        assert!(mf.resolve(&Scope::Owner, "ok/nested.md").is_ok());
    }

    #[tokio::test]
    async fn str_replace_requires_unique_match() {
        let (_d, mf) = tmp();
        mf.write(&Scope::Self_, "a.md", "one two one")
            .await
            .unwrap();
        assert!(matches!(
            mf.str_replace(&Scope::Self_, "a.md", "one", "X").await,
            Err(FileError::Ambiguous(2))
        ));
        mf.str_replace(&Scope::Self_, "a.md", "two", "X")
            .await
            .unwrap();
        assert_eq!(
            mf.view(&Scope::Self_, "a.md", None).await.unwrap(),
            "one X one"
        );
    }

    #[tokio::test]
    async fn insert_delete_rename_list() {
        let (_d, mf) = tmp();
        let scope = Scope::domain("dev").unwrap();
        mf.write(&scope, "notes.md", "l1\nl3").await.unwrap();
        mf.insert(&scope, "notes.md", 1, "l2").await.unwrap();
        assert_eq!(
            mf.view(&scope, "notes.md", None).await.unwrap(),
            "l1\nl2\nl3"
        );
        mf.rename(&scope, "notes.md", "log/notes.md").await.unwrap();
        let list = mf.list(&scope).await.unwrap();
        assert_eq!(list, vec!["log/notes.md".to_string()]);
        mf.delete(&scope, "log/notes.md").await.unwrap();
        assert!(mf.list(&scope).await.unwrap().is_empty());
    }

    #[test]
    fn chunk_by_heading() {
        let md = "intro line\n\n## Decisions\nchose X\n\n## Open\ntodo Y";
        let chunks = chunk_markdown(md);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].heading, "");
        assert_eq!(chunks[1].heading, "Decisions");
        assert_eq!(chunks[2].heading, "Open");
        assert!(chunks[1].text.contains("chose X"));
        assert_eq!(chunks[2].chunk_no, 2);
    }

    #[test]
    fn chunk_empty_is_empty() {
        assert!(chunk_markdown("   \n\n").is_empty());
    }
}
