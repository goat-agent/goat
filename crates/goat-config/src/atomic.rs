use std::fs;
use std::io::{self, Write};
use std::path::Path;

use crate::SettingsError;

pub fn write_atomic(path: &Path, raw: &[u8]) -> Result<(), SettingsError> {
    write_atomic_using(path, |file| file.write_all(raw))?;
    Ok(())
}

fn write_atomic_using(
    path: &Path,
    write: impl FnOnce(&mut fs::File) -> io::Result<()>,
) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let name = path.file_name().map_or_else(
        || "config.json".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let temporary = parent.join(format!(".{name}.tmp-{}", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    let result = (|| {
        let mut file = options.open(&temporary)?;
        write(&mut file)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    if let Ok(directory) = fs::File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_replacement_never_truncates_destination() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        fs::write(&path, b"original").unwrap();

        let result = write_atomic_using(&path, |temporary| {
            temporary.write_all(b"partial replacement")?;
            assert_eq!(fs::read(&path)?, b"original");
            Err(io::Error::other("interrupted"))
        });

        assert!(result.is_err());
        assert_eq!(fs::read(path).unwrap(), b"original");
    }
}
