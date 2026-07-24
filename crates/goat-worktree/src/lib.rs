mod error;
mod git;
mod metadata;
mod workspace;

pub use workspace::{Workspace, WorkspaceKind, workspace};

use std::{
    fs,
    io::ErrorKind,
    path::{Component, Path, PathBuf},
};

use ignore::gitignore::GitignoreBuilder;

pub use error::WorktreeError;
use git::{
    ExistingBase, GitWorktree, branch_exists, commit_oid, common_dir, git_output, git_path,
    git_worktrees, is_dirty, os, repo_root, resolve_base_ref, validate_branch_name,
};
use metadata::{metadata_path, read_metadata, write_metadata_open};

const GOAT_DIR: &str = ".goat";
const WORKTREES_DIR: &str = "worktrees";
const BRANCH_PREFIX: &str = "worktree-";
const EXCLUDE_ENTRY: &str = ".goat/worktrees/";

pub fn enter(label: &str) -> Result<PathBuf, WorktreeError> {
    let cwd = std::env::current_dir().map_err(|source| WorktreeError::CurrentDir { source })?;
    let launch = prepare_from_cwd(label, &cwd)?;
    std::env::set_current_dir(&launch.path).map_err(|source| WorktreeError::Enter {
        path: launch.path.clone(),
        source,
    })?;
    Ok(launch.path)
}

pub fn list() -> Result<(), WorktreeError> {
    let cwd = std::env::current_dir().map_err(|source| WorktreeError::CurrentDir { source })?;
    let repo = Repo::discover(&cwd)?;
    let entries = managed_worktrees(&repo)?;
    if entries.is_empty() {
        println!("no managed worktrees");
        return Ok(());
    }
    for entry in entries {
        let dirty = if is_dirty(&entry.path)? {
            "dirty"
        } else {
            "clean"
        };
        println!(
            "{}  {}  {}  {}",
            entry.label,
            entry.branch,
            dirty,
            entry.path.display()
        );
    }
    Ok(())
}

pub fn remove(label: &str) -> Result<(), WorktreeError> {
    let cwd = std::env::current_dir().map_err(|source| WorktreeError::CurrentDir { source })?;
    remove_from_cwd(label, &cwd)
}

fn remove_from_cwd(label: &str, cwd: &Path) -> Result<(), WorktreeError> {
    validate_label(label)?;
    let repo = Repo::discover(cwd)?;
    let branch = branch_name(label);
    let path = repo.bucket.join(label);
    if !path.exists() {
        return Err(WorktreeError::UnknownWorktree {
            label: label.to_owned(),
        });
    }
    verify_existing_worktree(&repo, &path)?;
    if is_dirty(&path)? {
        return Err(WorktreeError::DirtyWorktree {
            label: label.to_owned(),
        });
    }
    if has_unique_commits(&repo, label, &branch)? {
        return Err(WorktreeError::UniqueCommits {
            label: label.to_owned(),
            branch,
        });
    }
    git_output(
        &repo.owner_root,
        &[os("worktree"), os("remove"), git_path(&path)],
    )?;
    if branch_exists(&repo.owner_root, &branch)? {
        git_output(&repo.owner_root, &[os("branch"), os("-D"), os(&branch)])?;
    }
    let metadata_path = metadata_path(&repo.bucket, label);
    if metadata_path.exists() {
        fs::remove_file(&metadata_path).map_err(|source| WorktreeError::Io {
            path: metadata_path,
            source,
        })?;
    }
    Ok(())
}

struct Launch {
    path: PathBuf,
}

fn prepare_from_cwd(label: &str, cwd: &Path) -> Result<Launch, WorktreeError> {
    validate_label(label)?;
    let repo = Repo::discover(cwd)?;
    validate_branch_name(&repo.owner_root, &branch_name(label))?;
    ensure_local_exclude(&repo)?;
    fs::create_dir_all(&repo.bucket).map_err(|source| WorktreeError::Io {
        path: repo.bucket.clone(),
        source,
    })?;

    let path = repo.bucket.join(label);
    let branch = branch_name(label);
    let worktrees = git_worktrees(&repo.owner_root)?;

    if path.exists() {
        verify_existing_worktree(&repo, &path)?;
        write_metadata_open(&repo.bucket, label, &path, &branch, None)?;
        return Ok(Launch { path });
    }

    if let Some(existing) = worktrees.iter().find(|worktree| {
        worktree
            .branch
            .as_ref()
            .is_some_and(|existing_branch| existing_branch == &branch)
    }) {
        return Err(WorktreeError::BranchCheckedOut {
            branch,
            path: existing.path.clone(),
        });
    }

    if path.try_exists().map_err(|source| WorktreeError::Io {
        path: path.clone(),
        source,
    })? {
        return Err(WorktreeError::PathCollision { path });
    }

    let base = if branch_exists(&repo.owner_root, &branch)? {
        ExistingBase::Branch(branch.clone())
    } else {
        ExistingBase::Ref(resolve_base_ref(&repo.owner_root)?)
    };

    match &base {
        ExistingBase::Branch(existing) => {
            git_output(
                &repo.owner_root,
                &[os("worktree"), os("add"), git_path(&path), os(existing)],
            )?;
        }
        ExistingBase::Ref(base_ref) => {
            git_output(
                &repo.owner_root,
                &[
                    os("worktree"),
                    os("add"),
                    os("-b"),
                    os(&branch),
                    git_path(&path),
                    os(&base_ref.name),
                ],
            )?;
        }
    }

    copy_worktree_include(cwd, &path)?;
    match &base {
        ExistingBase::Branch(_) => write_metadata_open(&repo.bucket, label, &path, &branch, None)?,
        ExistingBase::Ref(base_ref) => write_metadata_open(
            &repo.bucket,
            label,
            &path,
            &branch,
            Some((base_ref.kind.clone(), base_ref.oid.clone())),
        )?,
    }
    Ok(Launch { path })
}

#[derive(Clone)]
struct Repo {
    owner_root: PathBuf,
    common_dir: PathBuf,
    bucket: PathBuf,
}

impl Repo {
    fn discover(cwd: &Path) -> Result<Self, WorktreeError> {
        let current_root = repo_root(cwd)?;
        let common_dir = common_dir(&current_root)?;
        let worktrees = git_worktrees(&current_root)?;
        let owner_root = owner_root(&current_root, &worktrees);
        let bucket = owner_root.join(GOAT_DIR).join(WORKTREES_DIR);
        Ok(Self {
            owner_root,
            common_dir,
            bucket,
        })
    }
}

struct ManagedWorktree {
    label: String,
    path: PathBuf,
    branch: String,
}

fn validate_label(label: &str) -> Result<(), WorktreeError> {
    if label.is_empty() {
        return Err(invalid_label(label, "label cannot be empty"));
    }
    if label == "." || label == ".." {
        return Err(invalid_label(label, "label cannot be . or .."));
    }
    if label.starts_with('-') {
        return Err(invalid_label(label, "label cannot start with -"));
    }
    if label.starts_with('.') {
        return Err(invalid_label(label, "label cannot start with ."));
    }
    if label.ends_with('.') {
        return Err(invalid_label(label, "label cannot end with ."));
    }
    if label.contains("..") {
        return Err(invalid_label(label, "label cannot contain .."));
    }
    if std::path::Path::new(label)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("lock"))
    {
        return Err(invalid_label(label, "label cannot end with .lock"));
    }
    if label
        .bytes()
        .any(|byte| !byte.is_ascii_alphanumeric() && !matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(invalid_label(
            label,
            "label can contain only ASCII letters, numbers, ., _, and -",
        ));
    }
    Ok(())
}

fn invalid_label(label: &str, reason: &'static str) -> WorktreeError {
    WorktreeError::InvalidLabel {
        label: label.to_owned(),
        reason,
    }
}

fn branch_name(label: &str) -> String {
    format!("{BRANCH_PREFIX}{label}")
}

fn owner_root(current_root: &Path, worktrees: &[GitWorktree]) -> PathBuf {
    for worktree in worktrees {
        let bucket = worktree.path.join(GOAT_DIR).join(WORKTREES_DIR);
        if current_root.starts_with(&bucket) {
            return worktree.path.clone();
        }
    }
    current_root.to_path_buf()
}

fn verify_existing_worktree(repo: &Repo, path: &Path) -> Result<(), WorktreeError> {
    if !path.is_dir() {
        return Err(WorktreeError::PathCollision {
            path: path.to_path_buf(),
        });
    }
    let actual = common_dir(path).map_err(|_| WorktreeError::PathCollision {
        path: path.to_path_buf(),
    })?;
    if actual != repo.common_dir {
        return Err(WorktreeError::WrongRepository {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

fn has_unique_commits(repo: &Repo, label: &str, branch: &str) -> Result<bool, WorktreeError> {
    let branch_oid = commit_oid(&repo.owner_root, branch)?;
    if read_metadata(&repo.bucket, label)?.is_some_and(|metadata| {
        metadata
            .created_base_oid
            .is_some_and(|base_oid| base_oid == branch_oid)
    }) {
        return Ok(false);
    }
    let output = git_output(
        &repo.owner_root,
        &[
            os("for-each-ref"),
            os("--format=%(refname:short)"),
            os("--contains"),
            os(branch),
            os("refs/heads"),
            os("refs/remotes"),
        ],
    )?;
    for line in output
        .stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if line == branch || line.starts_with(BRANCH_PREFIX) || line.ends_with("/HEAD") {
            continue;
        }
        return Ok(false);
    }
    Ok(true)
}

fn managed_worktrees(repo: &Repo) -> Result<Vec<ManagedWorktree>, WorktreeError> {
    let worktrees = git_worktrees(&repo.owner_root)?;
    let mut out = Vec::new();
    for worktree in worktrees {
        let Ok(rel) = worktree.path.strip_prefix(&repo.bucket) else {
            continue;
        };
        let components: Vec<_> = rel.components().collect();
        let [Component::Normal(label)] = components.as_slice() else {
            continue;
        };
        let label = label.to_string_lossy().to_string();
        if label.starts_with('.') {
            continue;
        }
        let branch = worktree.branch.unwrap_or_else(|| branch_name(&label));
        out.push(ManagedWorktree {
            label,
            path: worktree.path,
            branch,
        });
    }
    out.sort_by(|a, b| a.label.cmp(&b.label));
    Ok(out)
}

fn ensure_local_exclude(repo: &Repo) -> Result<(), WorktreeError> {
    let output = git_output(
        &repo.owner_root,
        &[os("rev-parse"), os("--git-path"), os("info/exclude")],
    )?;
    let raw = output.stdout.trim();
    let path = PathBuf::from(raw);
    let path = if path.is_absolute() {
        path
    } else {
        repo.owner_root.join(path)
    };
    let existing = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(WorktreeError::Io { path, source });
        }
    };
    if existing.lines().any(|line| line.trim() == EXCLUDE_ENTRY) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| WorktreeError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let mut next = existing;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(EXCLUDE_ENTRY);
    next.push('\n');
    fs::write(&path, next).map_err(|source| WorktreeError::Io { path, source })
}

fn copy_worktree_include(invocation_cwd: &Path, target: &Path) -> Result<(), WorktreeError> {
    let source_root = repo_root(invocation_cwd)?;
    let include_path = source_root.join(".worktreeinclude");
    if !include_path.exists() {
        return Ok(());
    }
    let mut builder = GitignoreBuilder::new(&source_root);
    if let Some(err) = builder.add(&include_path) {
        return Err(WorktreeError::IgnorePattern {
            path: include_path,
            message: err.to_string(),
        });
    }
    let matcher = builder
        .build()
        .map_err(|err| WorktreeError::IgnorePattern {
            path: include_path.clone(),
            message: err.to_string(),
        })?;
    let output = git_output(
        &source_root,
        &[
            os("ls-files"),
            os("-z"),
            os("--others"),
            os("--ignored"),
            os("--exclude-standard"),
        ],
    )?;
    for rel in output.stdout.split('\0').filter(|rel| !rel.is_empty()) {
        let source = source_root.join(rel);
        if source.starts_with(source_root.join(GOAT_DIR).join(WORKTREES_DIR)) {
            continue;
        }
        if !source.is_file() {
            continue;
        }
        if !matcher
            .matched_path_or_any_parents(&source, false)
            .is_ignore()
        {
            continue;
        }
        let destination = target.join(rel);
        if destination.exists() {
            continue;
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|source| WorktreeError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::copy(&source, &destination).map_err(|source_err| WorktreeError::Io {
            path: destination,
            source: source_err,
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path, process::Command, process::Stdio};

    use super::{
        EXCLUDE_ENTRY, WorkspaceKind, WorktreeError, prepare_from_cwd, remove_from_cwd,
        validate_label, workspace,
    };
    use crate::git::{branch_exists, parse_worktrees};

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    fn git_repo() -> Option<tempfile::TempDir> {
        if !git_available() {
            return None;
        }
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir(&repo).unwrap();
        run(&repo, &["init", "-b", "main"]);
        run(&repo, &["config", "user.email", "test@example.invalid"]);
        run(&repo, &["config", "user.name", "Test"]);
        fs::write(repo.join("README.md"), "hello\n").unwrap();
        run(&repo, &["add", "README.md"]);
        run(&repo, &["commit", "-m", "init"]);
        Some(dir)
    }

    fn git_repo_with_origin() -> Option<tempfile::TempDir> {
        let dir = git_repo()?;
        let repo = dir.path().join("repo");
        run(
            &repo,
            &[
                "remote",
                "add",
                "origin",
                "https://example.invalid/repo.git",
            ],
        );
        run(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        run(
            &repo,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );
        Some(dir)
    }

    fn run(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn output(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    #[test]
    fn validates_labels() {
        for label in ["plan", "agent", "feature-auth", "v1.2", "bug_fix"] {
            validate_label(label).unwrap();
        }
        for label in [
            "",
            "feature/auth",
            "../x",
            "-plan",
            ".hidden",
            "trail.",
            "two..dots",
            "name.lock",
            ".",
            "..",
            "has space",
        ] {
            assert!(matches!(
                validate_label(label),
                Err(WorktreeError::InvalidLabel { .. })
            ));
        }
    }

    #[test]
    fn parses_worktree_porcelain() {
        let input = "worktree /repo\nHEAD abc\nbranch refs/heads/main\n\nworktree /repo/.goat/worktrees/plan\nHEAD def\nbranch refs/heads/worktree-plan\n\n";
        let parsed = parse_worktrees(input);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].branch.as_deref(), Some("main"));
        assert_eq!(parsed[1].branch.as_deref(), Some("worktree-plan"));
    }

    #[test]
    fn creates_worktree_from_origin_head() {
        let Some(dir) = git_repo_with_origin() else {
            return;
        };
        let repo = dir.path().join("repo");
        let launch = prepare_from_cwd("plan", &repo).unwrap();
        assert_eq!(
            launch.path.canonicalize().unwrap(),
            repo.join(".goat/worktrees/plan").canonicalize().unwrap()
        );
        assert_eq!(
            output(&launch.path, &["branch", "--show-current"]).trim(),
            "worktree-plan"
        );
        assert!(repo.join(".goat/worktrees/.metadata/plan.json").exists());
        let exclude = fs::read_to_string(repo.join(".git/info/exclude")).unwrap();
        assert_eq!(exclude.matches(EXCLUDE_ENTRY).count(), 1);
        let status = output(
            &repo,
            &["status", "--porcelain=v1", "--untracked-files=normal"],
        );
        assert!(status.trim().is_empty());
    }

    #[test]
    fn workspace_main_from_repo_root() {
        let Some(dir) = git_repo_with_origin() else {
            return;
        };
        let repo = dir.path().join("repo");
        let ws = workspace(&repo).unwrap();
        assert_eq!(ws.kind, WorkspaceKind::Main);
        assert_eq!(ws.owner_root, repo.canonicalize().unwrap());
        assert_eq!(ws.git_branch, "main");
    }

    #[test]
    fn head_branch_tracks_checkout() {
        let Some(dir) = git_repo() else {
            return;
        };
        let repo = dir.path().join("repo");
        let ws = workspace(&repo).unwrap();
        assert_eq!(ws.head_branch().as_deref(), Some("main"));
        run(&repo, &["checkout", "-b", "feature-x"]);
        assert_eq!(ws.head_branch().as_deref(), Some("feature-x"));
    }

    #[test]
    fn workspace_managed_from_worktree_path() {
        let Some(dir) = git_repo_with_origin() else {
            return;
        };
        let repo = dir.path().join("repo");
        let launch = prepare_from_cwd("plan", &repo).unwrap();
        let ws = workspace(&launch.path).unwrap();
        assert!(matches!(
            ws.kind,
            WorkspaceKind::Managed { ref label } if label == "plan"
        ));
        assert_eq!(ws.owner_root, repo.canonicalize().unwrap());
        assert_eq!(ws.git_branch, "worktree-plan");
    }

    #[test]
    fn reopens_existing_dirty_worktree() {
        let Some(dir) = git_repo_with_origin() else {
            return;
        };
        let repo = dir.path().join("repo");
        let launch = prepare_from_cwd("plan", &repo).unwrap();
        fs::write(launch.path.join("dirty.txt"), "dirty\n").unwrap();
        let reopened = prepare_from_cwd("plan", &repo).unwrap();
        assert_eq!(reopened.path, launch.path);
        assert!(launch.path.join("dirty.txt").exists());
    }

    #[test]
    fn creates_from_head_without_origin_head() {
        let Some(dir) = git_repo() else {
            return;
        };
        let repo = dir.path().join("repo");
        let launch = prepare_from_cwd("plan", &repo).unwrap();
        assert_eq!(
            output(&launch.path, &["branch", "--show-current"]).trim(),
            "worktree-plan"
        );
    }

    #[test]
    fn launching_from_managed_worktree_uses_owner_bucket() {
        let Some(dir) = git_repo_with_origin() else {
            return;
        };
        let repo = dir.path().join("repo");
        let first = prepare_from_cwd("plan", &repo).unwrap();
        let second = prepare_from_cwd("agent", &first.path).unwrap();
        assert_eq!(
            second.path.canonicalize().unwrap(),
            repo.join(".goat/worktrees/agent").canonicalize().unwrap()
        );
    }

    #[test]
    fn worktreeinclude_copies_only_ignored_matches() {
        let Some(dir) = git_repo_with_origin() else {
            return;
        };
        let repo = dir.path().join("repo");
        fs::write(repo.join(".gitignore"), ".env\n*.local.json\n").unwrap();
        fs::write(
            repo.join(".worktreeinclude"),
            ".env\nconfig/*.local.json\nREADME.md\n",
        )
        .unwrap();
        fs::create_dir(repo.join("config")).unwrap();
        fs::write(repo.join(".env"), "secret\n").unwrap();
        fs::write(repo.join("config/app.local.json"), "{}\n").unwrap();
        let launch = prepare_from_cwd("plan", &repo).unwrap();
        assert!(launch.path.join(".env").exists());
        assert!(launch.path.join("config/app.local.json").exists());
        let readme = fs::read_to_string(launch.path.join("README.md")).unwrap();
        assert_eq!(readme.replace("\r\n", "\n"), "hello\n");
    }

    #[test]
    fn remove_refuses_dirty_worktree() {
        let Some(dir) = git_repo_with_origin() else {
            return;
        };
        let repo = dir.path().join("repo");
        let launch = prepare_from_cwd("plan", &repo).unwrap();
        fs::write(launch.path.join("dirty.txt"), "dirty\n").unwrap();
        let result = remove_from_cwd("plan", &repo);
        assert!(matches!(result, Err(WorktreeError::DirtyWorktree { .. })));
    }

    #[test]
    fn remove_clean_worktree_removes_branch_and_metadata() {
        let Some(dir) = git_repo_with_origin() else {
            return;
        };
        let repo = dir.path().join("repo");
        let launch = prepare_from_cwd("plan", &repo).unwrap();
        assert!(launch.path.exists());
        assert!(repo.join(".goat/worktrees/.metadata/plan.json").exists());
        remove_from_cwd("plan", &repo).unwrap();
        assert!(!launch.path.exists());
        assert!(!repo.join(".goat/worktrees/.metadata/plan.json").exists());
        assert!(!branch_exists(&repo, "worktree-plan").unwrap());
    }

    #[test]
    fn remove_refuses_unique_commits() {
        let Some(dir) = git_repo_with_origin() else {
            return;
        };
        let repo = dir.path().join("repo");
        let launch = prepare_from_cwd("plan", &repo).unwrap();
        fs::write(launch.path.join("commit.txt"), "commit\n").unwrap();
        run(&launch.path, &["add", "commit.txt"]);
        run(&launch.path, &["commit", "-m", "work"]);
        let result = remove_from_cwd("plan", &repo);
        assert!(matches!(result, Err(WorktreeError::UniqueCommits { .. })));
    }
}
