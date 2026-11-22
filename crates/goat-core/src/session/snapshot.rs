use std::cell::{Cell, RefCell};
use std::fmt::Write as _;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::Id;

const LARGEST_FILE: u64 = 10 * 1024 * 1024;
const FIRST_BYTES: u64 = 200 * 1024 * 1024;
const FIRST_FILES: usize = 20_000;
const KEY_CHARS: usize = 16;
const CLEARED: [&str; 7] = [
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_CEILING_DIRECTORIES",
    "GIT_DISCOVERY_ACROSS_FILESYSTEM",
    "GIT_EXTERNAL_DIFF",
];

#[derive(Debug)]
pub(super) struct Snapshots {
    workspace: PathBuf,
    dir: PathBuf,
    index: PathBuf,
    session: Id,
    off: RefCell<Option<String>>,
    is_announced: Cell<bool>,
}

impl Snapshots {
    pub(super) fn new(root: &Path, workspace: &Path, home: &Path, session: Id) -> Self {
        let digest = Sha256::digest(workspace.as_os_str().as_encoded_bytes());
        let key = digest.iter().fold(String::new(), |mut key, byte| {
            write!(key, "{byte:02x}").ok();
            key
        });
        let dir = root.join(&key[..KEY_CHARS]);
        let off = if home.starts_with(workspace) {
            Some("the working directory holds the home directory".to_owned())
        } else if root.starts_with(workspace) {
            Some("the working directory holds goat's own data".to_owned())
        } else {
            None
        };
        Self {
            workspace: workspace.to_owned(),
            index: dir.join(format!("index-{session}")),
            dir,
            session,
            off: RefCell::new(off),
            is_announced: Cell::new(false),
        }
    }

    pub(super) async fn take(&self) -> Option<String> {
        if self.off.borrow().is_some() {
            return None;
        }
        match self.snapshot().await {
            Ok(tree) => Some(tree),
            Err(problem) => {
                self.off.replace(Some(problem));
                None
            }
        }
    }

    pub(super) fn off(&self) -> Option<String> {
        self.off.borrow().clone()
    }

    pub(super) fn notice(&self) -> Option<String> {
        if self.is_announced.get() {
            return None;
        }
        let off = self.off.borrow().clone()?;
        self.is_announced.set(true);
        Some(off)
    }

    pub(super) async fn restore(&self, tree: &str) -> Result<Vec<PathBuf>, String> {
        if let Some(problem) = self.off.borrow().clone() {
            return Err(problem);
        }
        self.git(
            &["cat-file", "-e", &format!("{tree}^{{tree}}")],
            None,
            &self.index,
        )
        .await
        .map_err(|_| "the snapshot of that prompt is gone".to_owned())?;
        let current = self.snapshot().await?;
        let listed = self
            .git(
                &[
                    "diff-tree",
                    "-r",
                    "-z",
                    "--no-renames",
                    "--name-status",
                    &current,
                    tree,
                ],
                None,
                &self.index,
            )
            .await?;
        let mut fields = listed.split('\0').filter(|field| !field.is_empty());
        let mut written = Vec::new();
        let mut removed = Vec::new();
        while let (Some(status), Some(path)) = (fields.next(), fields.next()) {
            if status == "D" {
                removed.push(path);
            } else {
                written.push(path);
            }
        }
        for path in &removed {
            match std::fs::remove_file(self.workspace.join(path)) {
                Err(err) if err.kind() != ErrorKind::NotFound => {
                    return Err(format!("could not remove {path}: {err}"));
                }
                _ => {}
            }
        }
        if !written.is_empty() {
            let scratch = self.dir.join(format!("restore-{}", self.session));
            let paths: Vec<u8> = written
                .iter()
                .flat_map(|path| path.bytes().chain([0]))
                .collect();
            let checked_out = async {
                self.git(&["read-tree", tree], None, &scratch).await?;
                self.git(
                    &["checkout-index", "-f", "-z", "--stdin"],
                    Some(&paths),
                    &scratch,
                )
                .await
            }
            .await;
            std::fs::remove_file(&scratch).ok();
            checked_out?;
        }
        self.snapshot().await?;
        let mut paths: Vec<PathBuf> = removed
            .into_iter()
            .chain(written)
            .map(PathBuf::from)
            .collect();
        paths.sort();
        Ok(paths)
    }

    async fn snapshot(&self) -> Result<String, String> {
        let is_first = self.prepare().await?;
        let listed = self
            .git(
                &[
                    "ls-files",
                    "-z",
                    "--others",
                    "--modified",
                    "--exclude-standard",
                ],
                None,
                &self.index,
            )
            .await?;
        let mut large = Vec::new();
        let mut bytes = 0;
        let mut files = 0;
        for path in listed.split('\0').filter(|path| !path.is_empty()) {
            let size = std::fs::symlink_metadata(self.workspace.join(path))
                .map_or(0, |meta| if meta.is_file() { meta.len() } else { 0 });
            if size > LARGEST_FILE {
                large.push(path);
            } else {
                bytes += size;
                files += 1;
            }
        }
        if is_first && (bytes > FIRST_BYTES || files > FIRST_FILES) {
            return Err(format!(
                "the working directory has over {} MB or {FIRST_FILES} files to snapshot",
                FIRST_BYTES / 1024 / 1024
            ));
        }
        if !large.is_empty() {
            let literal: Vec<u8> = large
                .iter()
                .flat_map(|path| format!(":(literal){path}\0").into_bytes())
                .collect();
            self.git(
                &[
                    "rm",
                    "--cached",
                    "-q",
                    "--ignore-unmatch",
                    "--pathspec-from-file=-",
                    "--pathspec-file-nul",
                ],
                Some(&literal),
                &self.index,
            )
            .await?;
        }
        let mut spec = b".\0".to_vec();
        for path in &large {
            spec.extend_from_slice(format!(":(exclude,literal){path}\0").as_bytes());
        }
        self.git(
            &["add", "-A", "--pathspec-from-file=-", "--pathspec-file-nul"],
            Some(&spec),
            &self.index,
        )
        .await?;
        let tree = self.git(&["write-tree"], None, &self.index).await?;
        let shared = self.dir.join(format!("index.{}", self.session));
        if std::fs::copy(&self.index, &shared).is_ok() {
            std::fs::rename(&shared, self.dir.join("index")).ok();
        }
        Ok(tree.trim().to_owned())
    }

    async fn prepare(&self) -> Result<bool, String> {
        let repo = self.dir.join("repo");
        if !repo.join("HEAD").exists() {
            std::fs::create_dir_all(&self.dir)
                .map_err(|err| format!("could not create {}: {err}", self.dir.display()))?;
            let status = isolated(Command::new("git"))
                .args(["init", "-q", "--bare"])
                .arg(&repo)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await
                .map_err(|err| not_run(&err))?;
            if !status.success() {
                return Err("git could not create the snapshot store".to_owned());
            }
        }
        std::fs::remove_file(self.index.with_extension("lock")).ok();
        if self.index.exists() {
            return Ok(false);
        }
        let shared = self.dir.join("index");
        Ok(std::fs::copy(&shared, &self.index).is_err())
    }

    async fn git(
        &self,
        args: &[&str],
        input: Option<&[u8]>,
        index: &Path,
    ) -> Result<String, String> {
        let mut command = isolated(Command::new("git"));
        command
            .args(args)
            .current_dir(&self.workspace)
            .env("GIT_DIR", self.dir.join("repo"))
            .env("GIT_WORK_TREE", &self.workspace)
            .env("GIT_INDEX_FILE", index)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|err| not_run(&err))?;
        if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
            stdin
                .write_all(input)
                .await
                .map_err(|err| format!("could not talk to git: {err}"))?;
        }
        let output = child
            .wait_with_output()
            .await
            .map_err(|err| format!("git {} did not finish: {err}", args[0]))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let problem = stderr.lines().next().unwrap_or("no message");
            return Err(format!("git {} failed: {problem}", args[0]));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

fn isolated(mut command: Command) -> Command {
    command
        .args([
            "-c",
            "core.autocrlf=false",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.quotepath=false",
        ])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0");
    for name in CLEARED {
        command.env_remove(name);
    }
    command
}

fn not_run(err: &std::io::Error) -> String {
    if err.kind() == ErrorKind::NotFound {
        "git is not installed".to_owned()
    } else {
        format!("could not run git: {err}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Place {
        _root: tempfile::TempDir,
        workspace: PathBuf,
        snapshots: Snapshots,
    }

    fn place() -> Place {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("work");
        std::fs::create_dir_all(&workspace).unwrap();
        let snapshots = Snapshots::new(
            &root.path().join("snapshots"),
            &workspace,
            &root.path().join("home"),
            Id::generate(),
        );
        Place {
            _root: root,
            workspace,
            snapshots,
        }
    }

    fn read(place: &Place, name: &str) -> Option<String> {
        std::fs::read_to_string(place.workspace.join(name)).ok()
    }

    #[tokio::test]
    async fn restoring_undoes_changes_made_any_way() {
        let place = place();
        std::fs::write(place.workspace.join("a.txt"), "one").unwrap();
        std::fs::write(place.workspace.join("gone.txt"), "kept").unwrap();
        let tree = place.snapshots.take().await.unwrap();

        std::fs::write(place.workspace.join("a.txt"), "two").unwrap();
        std::fs::remove_file(place.workspace.join("gone.txt")).unwrap();
        std::fs::create_dir_all(place.workspace.join("new")).unwrap();
        std::fs::write(place.workspace.join("new/b.txt"), "made").unwrap();
        let restored = place.snapshots.restore(&tree).await.unwrap();

        assert_eq!(read(&place, "a.txt").as_deref(), Some("one"));
        assert_eq!(read(&place, "gone.txt").as_deref(), Some("kept"));
        assert_eq!(read(&place, "new/b.txt"), None);
        assert_eq!(
            restored,
            [
                PathBuf::from("a.txt"),
                PathBuf::from("gone.txt"),
                PathBuf::from("new/b.txt")
            ]
        );
    }

    #[tokio::test]
    async fn ignored_and_large_files_are_left_alone() {
        let place = place();
        std::fs::write(place.workspace.join(".gitignore"), "target/\n").unwrap();
        std::fs::create_dir_all(place.workspace.join("target")).unwrap();
        std::fs::write(place.workspace.join("target/out"), "built").unwrap();
        let big = vec![b'x'; usize::try_from(LARGEST_FILE).unwrap() + 1];
        std::fs::write(place.workspace.join("big.bin"), &big).unwrap();
        let tree = place.snapshots.take().await.unwrap();

        std::fs::write(place.workspace.join("target/out"), "rebuilt").unwrap();
        std::fs::write(place.workspace.join("big.bin"), "small now").unwrap();
        std::fs::write(place.workspace.join("big.bin"), &big).unwrap();
        let restored = place.snapshots.restore(&tree).await.unwrap();

        assert_eq!(restored, Vec::<PathBuf>::new());
        assert_eq!(read(&place, "target/out").as_deref(), Some("rebuilt"));
    }

    #[tokio::test]
    async fn an_unchanged_workspace_restores_nothing() {
        let place = place();
        std::fs::write(place.workspace.join("a.txt"), "one").unwrap();
        let tree = place.snapshots.take().await.unwrap();

        assert_eq!(
            place.snapshots.restore(&tree).await.unwrap(),
            Vec::<PathBuf>::new()
        );
        assert_eq!(place.snapshots.take().await, Some(tree));
    }

    #[tokio::test]
    async fn the_home_directory_is_never_snapshotted() {
        let root = tempfile::tempdir().unwrap();
        let snapshots = Snapshots::new(
            &root.path().join("snapshots"),
            root.path(),
            &root.path().join("me"),
            Id::generate(),
        );

        assert_eq!(snapshots.take().await, None);
        assert_eq!(
            snapshots.notice().as_deref(),
            Some("the working directory holds the home directory")
        );
        assert_eq!(snapshots.notice(), None);
        assert!(!root.path().join("snapshots").exists());
    }

    #[tokio::test]
    async fn the_snapshot_store_is_never_inside_what_it_snapshots() {
        let root = tempfile::tempdir().unwrap();
        let snapshots = Snapshots::new(
            &root.path().join(".goat/snapshots"),
            root.path(),
            Path::new("/home/someone"),
            Id::generate(),
        );

        assert_eq!(snapshots.take().await, None);
        assert_eq!(
            snapshots.notice().as_deref(),
            Some("the working directory holds goat's own data")
        );
    }

    #[tokio::test]
    async fn a_second_session_starts_from_the_shared_index() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("work");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("a.txt"), "one").unwrap();
        let open = || {
            Snapshots::new(
                &root.path().join("snapshots"),
                &workspace,
                &root.path().join("home"),
                Id::generate(),
            )
        };
        let first = open().take().await.unwrap();

        let second = open();
        assert!(!second.prepare().await.unwrap());
        assert_eq!(second.take().await, Some(first));
    }
}
