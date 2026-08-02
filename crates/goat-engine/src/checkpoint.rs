use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicI64, Ordering},
};

use goat_store::{
    CheckpointFileVersion, CodeCheckpoint, CodeStore as Store, CreatedMessage, NewCheckpointFile,
    NewCodeCheckpoint,
};
use goat_tool::ToolContext;

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileImage {
    content: Option<Vec<u8>>,
    mode: Option<u32>,
    supported: bool,
}

pub(crate) struct RestoreReport {
    pub(crate) restored: usize,
    pub(crate) skipped: usize,
}

pub(crate) struct CheckpointTracker {
    store: Store,
    active: AtomicI64,
}

impl CheckpointTracker {
    pub(crate) fn new(store: Store) -> Self {
        Self {
            store,
            active: AtomicI64::new(0),
        }
    }

    pub(crate) fn clear(&self) {
        self.active.store(0, Ordering::Release);
    }

    pub(crate) async fn begin(
        &self,
        thread_id: i64,
        message: &CreatedMessage,
        draft: String,
        attachments: &[goat_protocol::InputAttachment],
        root: &Path,
    ) -> Result<i64, String> {
        self.clear();
        let attachments = serde_json::to_string(attachments).map_err(|err| err.to_string())?;
        let checkpoint_id = self
            .store
            .create_code_checkpoint(NewCodeCheckpoint {
                thread_id,
                prompt_message_id: message.id,
                parent_message_id: message.parent_message_id,
                draft,
                attachments,
                created_at: crate::persist::now_ms(),
            })
            .await
            .map_err(|err| err.to_string())?;
        let root = root.canonicalize().map_err(|err| err.to_string())?;
        let paths = self
            .store
            .tracked_checkpoint_paths(thread_id)
            .await
            .map_err(|err| err.to_string())?;
        for raw in paths {
            let path = PathBuf::from(&raw);
            if !path.starts_with(&root) {
                continue;
            }
            let image = snapshot(&path)
                .await
                .unwrap_or_else(|_| unsupported_image());
            self.record(checkpoint_id, path, image, false).await?;
        }
        self.active.store(checkpoint_id, Ordering::Release);
        Ok(checkpoint_id)
    }

    pub(crate) async fn capture_tool_path(
        &self,
        input: &str,
        tool_ctx: &ToolContext,
    ) -> Result<(), String> {
        let checkpoint_id = self.active.load(Ordering::Acquire);
        if checkpoint_id == 0 {
            return Ok(());
        }
        let value: serde_json::Value =
            serde_json::from_str(input).map_err(|err| err.to_string())?;
        let Some(raw) = value.get("path").and_then(serde_json::Value::as_str) else {
            return Ok(());
        };
        let path = tool_ctx.resolve(raw).map_err(|err| err.to_string())?;
        tool_ctx
            .ensure_writable(&path, raw)
            .map_err(|err| err.to_string())?;
        if !path.starts_with(&tool_ctx.cwd) {
            return Ok(());
        }
        let image = snapshot(&path).await?;
        self.record(checkpoint_id, path, image, true).await
    }

    async fn record(
        &self,
        checkpoint_id: i64,
        path: PathBuf,
        image: FileImage,
        touched: bool,
    ) -> Result<(), String> {
        self.store
            .record_checkpoint_file(NewCheckpointFile {
                checkpoint_id,
                path: path.to_string_lossy().into_owned(),
                content: image.content,
                mode: image.mode,
                supported: image.supported,
                touched,
            })
            .await
            .map_err(|err| err.to_string())
    }

    pub(crate) async fn points(&self, thread_id: i64) -> Result<Vec<CodeCheckpoint>, String> {
        self.store
            .active_code_checkpoints(thread_id)
            .await
            .map_err(|err| err.to_string())
    }

    pub(crate) async fn restore(
        &self,
        thread_id: i64,
        checkpoint_id: i64,
        root: &Path,
    ) -> Result<RestoreReport, String> {
        let checkpoints = self.points(thread_id).await?;
        let checkpoint = checkpoints
            .iter()
            .find(|checkpoint| checkpoint.id == checkpoint_id)
            .ok_or_else(|| "checkpoint is no longer part of this conversation".to_owned())?;
        if !checkpoint.files_available {
            return Err("file snapshots for that checkpoint are no longer available".to_owned());
        }
        let versions = self
            .store
            .active_checkpoint_file_versions(thread_id)
            .await
            .map_err(|err| err.to_string())?;
        let targets = target_images(checkpoint_id, versions);
        let root = root.canonicalize().map_err(|err| err.to_string())?;
        apply_targets(&root, targets).await
    }
}

fn target_images(
    checkpoint_id: i64,
    versions: Vec<CheckpointFileVersion>,
) -> BTreeMap<PathBuf, FileImage> {
    let mut targets = BTreeMap::new();
    for version in versions {
        if version.checkpoint_id < checkpoint_id {
            continue;
        }
        targets
            .entry(PathBuf::from(version.path))
            .or_insert(FileImage {
                content: version.content,
                mode: version.mode,
                supported: version.supported,
            });
    }
    targets
}

async fn apply_targets(
    root: &Path,
    targets: BTreeMap<PathBuf, FileImage>,
) -> Result<RestoreReport, String> {
    let mut restored = 0usize;
    let mut skipped = 0usize;
    let mut rollback: Vec<(PathBuf, FileImage)> = Vec::new();
    for (path, target) in targets {
        if !path.starts_with(root) || !target.supported {
            skipped += 1;
            continue;
        }
        let current = snapshot(&path).await?;
        if !current.supported {
            skipped += 1;
            continue;
        }
        if current == target {
            continue;
        }
        rollback.push((path.clone(), current));
        if let Err(err) = write_image(&path, &target).await {
            for (rollback_path, rollback_image) in rollback.into_iter().rev() {
                let _ = write_image(&rollback_path, &rollback_image).await;
            }
            return Err(err);
        }
        restored += 1;
    }
    Ok(RestoreReport { restored, skipped })
}

async fn snapshot(path: &Path) -> Result<FileImage, String> {
    let metadata = match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FileImage {
                content: None,
                mode: None,
                supported: true,
            });
        }
        Err(err) => return Err(format!("{}: {err}", path.display())),
    };
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() || linked(&metadata) {
        return Ok(unsupported_image());
    }
    let content = tokio::fs::read(path)
        .await
        .map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(FileImage {
        content: Some(content),
        mode: file_mode(&metadata),
        supported: true,
    })
}

fn unsupported_image() -> FileImage {
    FileImage {
        content: None,
        mode: None,
        supported: false,
    }
}

async fn write_image(path: &Path, image: &FileImage) -> Result<(), String> {
    match &image.content {
        Some(content) => {
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|err| format!("{}: {err}", parent.display()))?;
            }
            tokio::fs::write(path, content)
                .await
                .map_err(|err| format!("{}: {err}", path.display()))?;
            set_file_mode(path, image.mode).await?;
        }
        None => match tokio::fs::remove_file(path).await {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(format!("{}: {err}", path.display())),
        },
    }
    Ok(())
}

#[cfg(unix)]
fn linked(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    metadata.nlink() > 1
}

#[cfg(not(unix))]
fn linked(_metadata: &std::fs::Metadata) -> bool {
    false
}

#[cfg(unix)]
fn file_mode(metadata: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;

    Some(metadata.permissions().mode())
}

#[cfg(not(unix))]
fn file_mode(_metadata: &std::fs::Metadata) -> Option<u32> {
    None
}

#[cfg(unix)]
async fn set_file_mode(path: &Path, mode: Option<u32>) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let Some(mode) = mode else {
        return Ok(());
    };
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .await
        .map_err(|err| format!("{}: {err}", path.display()))
}

#[cfg(not(unix))]
async fn set_file_mode(_path: &Path, _mode: Option<u32>) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use goat_store::{CodeStore, NewMessage, NewThread};
    use goat_tool::ToolContext;

    use super::CheckpointTracker;

    async fn setup(root: &std::path::Path) -> (CheckpointTracker, i64, goat_store::CreatedMessage) {
        let store = CodeStore::open_in_memory().await.unwrap();
        let thread_id = store
            .create_thread(NewThread {
                cwd: root.display().to_string(),
                title: None,
                provider: "openai".into(),
                model: "gpt".into(),
                account: "default".into(),
                effort: None,
                created_at: 1,
                updated_at: 1,
            })
            .await
            .unwrap();
        let message = store
            .create_message(NewMessage {
                thread_id,
                turn_id: None,
                role: "user".into(),
                body: "change files".into(),
                created_at: 2,
            })
            .await
            .unwrap();
        (CheckpointTracker::new(store), thread_id, message)
    }

    #[tokio::test]
    async fn restores_existing_and_new_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let existing = root.join("existing.txt");
        let created = root.join("created.txt");
        tokio::fs::write(&existing, b"before").await.unwrap();
        let (tracker, thread_id, message) = setup(&root).await;
        let checkpoint_id = tracker
            .begin(thread_id, &message, "change files".into(), &[], &root)
            .await
            .unwrap();
        let context = ToolContext::new(&root).unwrap();
        tracker
            .capture_tool_path(r#"{"path":"existing.txt"}"#, &context)
            .await
            .unwrap();
        tracker
            .capture_tool_path(r#"{"path":"created.txt"}"#, &context)
            .await
            .unwrap();
        tokio::fs::write(&existing, b"after").await.unwrap();
        tokio::fs::write(&created, b"new").await.unwrap();

        let report = tracker
            .restore(thread_id, checkpoint_id, &root)
            .await
            .unwrap();

        assert_eq!(report.restored, 2);
        assert_eq!(report.skipped, 0);
        assert_eq!(tokio::fs::read(&existing).await.unwrap(), b"before");
        assert!(!created.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn restores_file_mode() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let path = root.join("script");
        tokio::fs::write(&path, b"before").await.unwrap();
        tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o744))
            .await
            .unwrap();
        let (tracker, thread_id, message) = setup(&root).await;
        let checkpoint_id = tracker
            .begin(thread_id, &message, "change mode".into(), &[], &root)
            .await
            .unwrap();
        let context = ToolContext::new(&root).unwrap();
        tracker
            .capture_tool_path(r#"{"path":"script"}"#, &context)
            .await
            .unwrap();
        tokio::fs::write(&path, b"after").await.unwrap();
        tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .await
            .unwrap();

        tracker
            .restore(thread_id, checkpoint_id, &root)
            .await
            .unwrap();

        let mode = tokio::fs::metadata(&path)
            .await
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o744);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn skips_hard_linked_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let path = root.join("linked");
        let other = root.join("other");
        tokio::fs::write(&path, b"before").await.unwrap();
        std::fs::hard_link(&path, &other).unwrap();
        let (tracker, thread_id, message) = setup(&root).await;
        let checkpoint_id = tracker
            .begin(thread_id, &message, "change link".into(), &[], &root)
            .await
            .unwrap();
        let context = ToolContext::new(&root).unwrap();
        tracker
            .capture_tool_path(r#"{"path":"linked"}"#, &context)
            .await
            .unwrap();
        tokio::fs::write(&path, b"after").await.unwrap();

        let report = tracker
            .restore(thread_id, checkpoint_id, &root)
            .await
            .unwrap();

        assert_eq!(report.restored, 0);
        assert_eq!(report.skipped, 1);
        assert_eq!(tokio::fs::read(&path).await.unwrap(), b"after");
    }
}
