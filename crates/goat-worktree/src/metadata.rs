use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::error::WorktreeError;

pub(crate) const METADATA_DIR: &str = ".metadata";

#[derive(Serialize, Deserialize)]
pub(crate) struct Metadata {
    pub(crate) label: String,
    pub(crate) path: String,
    pub(crate) branch: String,
    pub(crate) created_base_ref_kind: Option<String>,
    pub(crate) created_base_oid: Option<String>,
    pub(crate) created_at_ms: u128,
    pub(crate) last_opened_at_ms: u128,
}

pub(crate) fn write_metadata_open(
    bucket: &Path,
    label: &str,
    path: &Path,
    branch: &str,
    base: Option<(String, String)>,
) -> Result<(), WorktreeError> {
    let now = now_ms();
    let existing = read_metadata(bucket, label)?;
    let (created_at_ms, created_base_ref_kind, created_base_oid) = match (existing, base) {
        (Some(metadata), None) => (
            metadata.created_at_ms,
            metadata.created_base_ref_kind,
            metadata.created_base_oid,
        ),
        (Some(metadata), Some((kind, oid))) => (
            metadata.created_at_ms,
            Some(kind).or(metadata.created_base_ref_kind),
            Some(oid).or(metadata.created_base_oid),
        ),
        (None, Some((kind, oid))) => (now, Some(kind), Some(oid)),
        (None, None) => (now, None, None),
    };
    let metadata = Metadata {
        label: label.to_owned(),
        path: path.display().to_string(),
        branch: branch.to_owned(),
        created_base_ref_kind,
        created_base_oid,
        created_at_ms,
        last_opened_at_ms: now,
    };
    let metadata_path = metadata_path(bucket, label);
    if let Some(parent) = metadata_path.parent() {
        fs::create_dir_all(parent).map_err(|source| WorktreeError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let body = serde_json::to_vec_pretty(&metadata).map_err(WorktreeError::Json)?;
    fs::write(&metadata_path, body).map_err(|source| WorktreeError::Io {
        path: metadata_path,
        source,
    })
}

pub(crate) fn read_metadata(bucket: &Path, label: &str) -> Result<Option<Metadata>, WorktreeError> {
    let path = metadata_path(bucket, label);
    let body = match fs::read(&path) {
        Ok(body) => body,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(WorktreeError::Io { path, source }),
    };
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(WorktreeError::Json)
}

pub(crate) fn metadata_path(bucket: &Path, label: &str) -> PathBuf {
    bucket.join(METADATA_DIR).join(format!("{label}.json"))
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis())
}
