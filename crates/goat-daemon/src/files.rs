use goat_api::{DiffFile, FileChunk, FsReadParams, FsWriteParams, GitDiffOutput, GitDiffParams};
use goat_wire::envelope::{CallError, ErrorCode, Execution};

pub const MAX_READ_BYTES: u64 = 1024 * 1024;
pub const MAX_WRITE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_DIFF_BYTES: usize = 1024 * 1024;

fn not_found(message: String) -> CallError {
    CallError::new(ErrorCode::NotFound, message).with_execution(Execution::NotStarted)
}

fn too_large(message: String) -> CallError {
    CallError::new(ErrorCode::TooLarge, message).with_execution(Execution::NotStarted)
}

fn failed(message: String) -> CallError {
    CallError::new(ErrorCode::Internal, message).with_execution(Execution::KnownFailed)
}

pub fn read(params: &FsReadParams) -> Result<FileChunk, CallError> {
    let path = std::path::Path::new(&params.path);
    let meta =
        std::fs::metadata(path).map_err(|err| not_found(format!("{}: {err}", params.path)))?;
    if meta.is_dir() {
        return Err(not_found(format!("{} is a directory", params.path)));
    }
    let total = meta.len();
    let offset = params.offset.min(total);
    let requested = params.len.unwrap_or(MAX_READ_BYTES);
    let capped = requested.min(MAX_READ_BYTES);
    let available = total - offset;
    let len = capped.min(available);

    let bytes = read_range(path, offset, len)?;
    let content = String::from_utf8(bytes).map_err(|_| {
        CallError::new(
            ErrorCode::Conflict,
            format!(
                "{} is not valid UTF-8; read it as an attachment instead",
                params.path
            ),
        )
        .with_execution(Execution::NotStarted)
    })?;

    Ok(FileChunk {
        path: params.path.clone(),
        offset,
        len,
        total,
        truncated: offset + len < total,
        content,
    })
}

fn read_range(path: &std::path::Path, offset: u64, len: u64) -> Result<Vec<u8>, CallError> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).map_err(|err| not_found(format!("open: {err}")))?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|err| failed(format!("seek: {err}")))?;
    let mut buffer = vec![0u8; usize::try_from(len).unwrap_or(0)];
    file.read_exact(&mut buffer)
        .map_err(|err| failed(format!("read: {err}")))?;
    Ok(buffer)
}

pub fn write(params: &FsWriteParams) -> Result<u64, CallError> {
    if params.content.len() > MAX_WRITE_BYTES {
        return Err(too_large(format!(
            "{} bytes exceeds the {MAX_WRITE_BYTES} byte write limit",
            params.content.len()
        )));
    }
    let path = std::path::Path::new(&params.path);
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|err| failed(format!("create_dir_all: {err}")))?;
    }
    if params.offset == 0 {
        std::fs::write(path, params.content.as_bytes())
            .map_err(|err| failed(format!("write: {err}")))?;
    } else {
        use std::io::{Seek, SeekFrom, Write};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|err| failed(format!("open: {err}")))?;
        file.seek(SeekFrom::Start(params.offset))
            .map_err(|err| failed(format!("seek: {err}")))?;
        file.write_all(params.content.as_bytes())
            .map_err(|err| failed(format!("write: {err}")))?;
    }
    Ok(params.content.len() as u64)
}

pub fn diff(params: &GitDiffParams) -> Result<GitDiffOutput, CallError> {
    let mut command = std::process::Command::new("git");
    command
        .arg("-C")
        .arg(&params.cwd)
        .arg("diff")
        .arg("--numstat")
        .arg("-z");
    if let Some(rev) = &params.rev {
        command.arg(rev);
    }
    if !params.paths.is_empty() {
        command.arg("--");
        for path in &params.paths {
            command.arg(path);
        }
    }
    let numstat = run(command)?;

    let mut command = std::process::Command::new("git");
    command.arg("-C").arg(&params.cwd).arg("diff");
    if let Some(rev) = &params.rev {
        command.arg(rev);
    }
    if !params.paths.is_empty() {
        command.arg("--");
        for path in &params.paths {
            command.arg(path);
        }
    }
    let patch = run(command)?;
    let truncated = patch.len() > MAX_DIFF_BYTES;

    Ok(GitDiffOutput {
        files: parse_numstat(&numstat, &patch, truncated),
        truncated,
    })
}

fn run(mut command: std::process::Command) -> Result<String, CallError> {
    let output = command
        .output()
        .map_err(|err| not_found(format!("git: {err}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(not_found(format!("git failed: {stderr}")));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn parse_numstat(numstat: &str, full_diff: &str, truncated: bool) -> Vec<DiffFile> {
    let mut files = Vec::new();
    for record in numstat.split('\0') {
        let record = record.trim();
        if record.is_empty() {
            continue;
        }
        let mut parts = record.split('\t');
        let (Some(added), Some(removed), Some(file_path)) =
            (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        files.push(DiffFile {
            path: file_path.to_owned(),
            status: if added == "-" && removed == "-" {
                "binary".to_owned()
            } else {
                "modified".to_owned()
            },
            added: added.parse().unwrap_or(0),
            removed: removed.parse().unwrap_or(0),
            patch: if truncated {
                String::new()
            } else {
                patch_for(full_diff, file_path)
            },
        });
    }
    files
}

fn patch_for(full_diff: &str, file_path: &str) -> String {
    let marker = format!("diff --git a/{file_path} b/{file_path}");
    let Some(start) = full_diff.find(&marker) else {
        return String::new();
    };
    let rest = &full_diff[start..];
    match rest[marker.len()..].find("\ndiff --git ") {
        Some(end) => rest[..=(marker.len() + end)].to_owned(),
        None => rest.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_READ_BYTES, MAX_WRITE_BYTES, diff, parse_numstat, patch_for, read, write};
    use goat_api::{FsReadParams, FsWriteParams, GitDiffParams};
    use goat_wire::envelope::{ErrorCode, Execution};

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("goat-files-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reading_a_whole_small_file_is_not_truncated() {
        let dir = scratch("read");
        let path = dir.join("a.txt");
        std::fs::write(&path, "hello").unwrap();
        let chunk = read(&FsReadParams {
            path: path.display().to_string(),
            offset: 0,
            len: None,
        })
        .unwrap();
        assert_eq!(chunk.content, "hello");
        assert_eq!(chunk.total, 5);
        assert_eq!(chunk.len, 5);
        assert!(!chunk.truncated);
    }

    #[test]
    fn reading_a_range_reports_truncation_and_the_real_total() {
        let dir = scratch("range");
        let path = dir.join("b.txt");
        std::fs::write(&path, "0123456789").unwrap();
        let chunk = read(&FsReadParams {
            path: path.display().to_string(),
            offset: 2,
            len: Some(3),
        })
        .unwrap();
        assert_eq!(chunk.content, "234");
        assert_eq!(chunk.offset, 2);
        assert_eq!(chunk.len, 3);
        assert_eq!(chunk.total, 10);
        assert!(chunk.truncated);
    }

    #[test]
    fn a_read_past_the_end_returns_nothing_rather_than_failing() {
        let dir = scratch("past");
        let path = dir.join("c.txt");
        std::fs::write(&path, "abc").unwrap();
        let chunk = read(&FsReadParams {
            path: path.display().to_string(),
            offset: 99,
            len: Some(10),
        })
        .unwrap();
        assert_eq!(chunk.content, "");
        assert_eq!(chunk.offset, 3);
        assert_eq!(chunk.len, 0);
        assert!(!chunk.truncated);
    }

    #[test]
    fn an_oversized_request_is_capped_rather_than_refused() {
        let dir = scratch("cap");
        let path = dir.join("d.txt");
        std::fs::write(&path, "xy").unwrap();
        let chunk = read(&FsReadParams {
            path: path.display().to_string(),
            offset: 0,
            len: Some(MAX_READ_BYTES * 10),
        })
        .unwrap();
        assert_eq!(chunk.len, 2);
    }

    #[test]
    fn a_missing_file_and_a_directory_are_both_not_found() {
        let dir = scratch("missing");
        let missing = read(&FsReadParams {
            path: dir.join("nope.txt").display().to_string(),
            offset: 0,
            len: None,
        })
        .unwrap_err();
        assert_eq!(missing.code, ErrorCode::NotFound);
        assert_eq!(missing.execution, Some(Execution::NotStarted));

        let is_dir = read(&FsReadParams {
            path: dir.display().to_string(),
            offset: 0,
            len: None,
        })
        .unwrap_err();
        assert_eq!(is_dir.code, ErrorCode::NotFound);
        assert!(is_dir.message.contains("directory"));
    }

    #[test]
    fn binary_content_is_refused_with_a_pointer_to_attachments() {
        let dir = scratch("binary");
        let path = dir.join("e.bin");
        std::fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();
        let err = read(&FsReadParams {
            path: path.display().to_string(),
            offset: 0,
            len: None,
        })
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::Conflict);
        assert!(err.retry_is_safe());
    }

    #[test]
    fn writing_creates_missing_parents_and_reports_the_length() {
        let dir = scratch("write");
        let path = dir.join("nested/deep/f.txt");
        let len = write(&FsWriteParams {
            path: path.display().to_string(),
            content: "written".to_owned(),
            offset: 0,
        })
        .unwrap();
        assert_eq!(len, 7);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "written");
    }

    #[test]
    fn writing_at_an_offset_patches_in_place() {
        let dir = scratch("offset");
        let path = dir.join("g.txt");
        std::fs::write(&path, "aaaaa").unwrap();
        write(&FsWriteParams {
            path: path.display().to_string(),
            content: "bb".to_owned(),
            offset: 1,
        })
        .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "abbaa");
    }

    #[test]
    fn an_oversized_write_is_refused_before_touching_the_disk() {
        let dir = scratch("toolarge");
        let path = dir.join("h.txt");
        let err = write(&FsWriteParams {
            path: path.display().to_string(),
            content: "x".repeat(MAX_WRITE_BYTES + 1),
            offset: 0,
        })
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::TooLarge);
        assert_eq!(err.execution, Some(Execution::NotStarted));
        assert!(!path.exists(), "nothing must be written when refused");
    }

    #[test]
    fn diff_outside_a_repository_is_reported_not_panicked() {
        let dir = scratch("nogit");
        let err = diff(&GitDiffParams {
            cwd: dir.display().to_string(),
            rev: None,
            paths: Vec::new(),
        })
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound);
        assert!(err.retry_is_safe());
    }

    #[test]
    fn numstat_records_become_diff_files() {
        let numstat = "3\t1\tsrc/a.rs\0-\t-\tassets/logo.png\0";
        let patch = "diff --git a/src/a.rs b/src/a.rs\n@@\n+one\ndiff --git a/assets/logo.png b/assets/logo.png\nBinary\n";
        let files = parse_numstat(numstat, patch, false);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "src/a.rs");
        assert_eq!(files[0].added, 3);
        assert_eq!(files[0].removed, 1);
        assert_eq!(files[0].status, "modified");
        assert!(files[0].patch.contains("+one"));
        assert!(!files[0].patch.contains("logo.png"));
        assert_eq!(files[1].status, "binary");
    }

    #[test]
    fn a_truncated_diff_drops_the_patch_bodies_but_keeps_the_counts() {
        let numstat = "3\t1\tsrc/a.rs\0";
        let files = parse_numstat(numstat, "irrelevant", true);
        assert_eq!(files[0].added, 3);
        assert!(files[0].patch.is_empty());
    }

    #[test]
    fn a_patch_for_an_absent_path_is_empty_rather_than_wrong() {
        assert!(patch_for("diff --git a/x b/x\n", "y").is_empty());
    }
}
