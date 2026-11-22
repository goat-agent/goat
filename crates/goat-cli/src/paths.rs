use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Context;

const OUTPUT_LIFETIME: Duration = Duration::from_hours(7 * 24);
const SNAPSHOT_LIFETIME: Duration = Duration::from_hours(30 * 24);

pub(crate) struct Paths {
    pub(crate) home: PathBuf,
    pub(crate) database: PathBuf,
    pub(crate) spill: PathBuf,
    pub(crate) snapshots: PathBuf,
    pub(crate) personal: PathBuf,
    pub(crate) workspace: PathBuf,
}

impl Paths {
    pub(crate) fn detect() -> anyhow::Result<Self> {
        let home = std::env::home_dir().context("find the home directory")?;
        let data = home.join(".goat");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&data)
            .with_context(|| format!("create {}", data.display()))?;
        let workspace = std::env::current_dir().context("read the current directory")?;
        let spill = data.join("outputs");
        prune(&spill, OUTPUT_LIFETIME);
        let snapshots = data.join("snapshots");
        prune(&snapshots, SNAPSHOT_LIFETIME);
        Ok(Self {
            database: data.join("goat.db"),
            spill,
            snapshots,
            personal: data.join("AGENTS.md"),
            home,
            workspace,
        })
    }
}

fn prune(dir: &Path, lifetime: Duration) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let expired = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| now.duration_since(modified).unwrap_or_default() > lifetime);
        if !expired {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            std::fs::remove_dir_all(path).ok();
        } else {
            std::fs::remove_file(path).ok();
        }
    }
}
