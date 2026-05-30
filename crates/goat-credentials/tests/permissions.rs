//! Verifies that the on-disk credentials file is restricted to owner-only
//! permissions. Unix-only: file modes are meaningless on other platforms.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;

use goat_credentials::JsonFileStore;

fn mode_of(path: &std::path::Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn open_tightens_a_pre_existing_loose_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("credentials.json");
    std::fs::write(&path, "{}").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(mode_of(&path), 0o644);

    let _store = JsonFileStore::open(path.clone()).unwrap();

    assert_eq!(
        mode_of(&path),
        0o600,
        "open() must harden an existing world/group-readable credentials file"
    );
}

#[test]
fn open_is_a_noop_when_no_file_exists_yet() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("credentials.json");

    // Should succeed without creating the file.
    let _store = JsonFileStore::open(path.clone()).unwrap();
    assert!(!path.exists());
}
