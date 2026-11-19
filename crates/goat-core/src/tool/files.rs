use std::fmt::Write;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use serde::Serialize;

use super::hashline::{self, Document, RawOp};

const DEFAULT_LIMIT: usize = 2000;
const MAX_LINE_CHARS: usize = 2000;
const SNIFF_BYTES: usize = 8 * 1024;

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Page {
    pub(crate) lines: Vec<String>,
    pub(crate) total: usize,
    pub(crate) next: Option<usize>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Written {
    pub(crate) created: bool,
    pub(crate) lines: usize,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Edited {
    pub(crate) total: usize,
    pub(crate) preview: String,
}

pub(crate) fn read(
    path: &Path,
    shown: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<Page, String> {
    let io = |err: std::io::Error| format!("could not read {shown}: {err}");
    let offset = offset.unwrap_or(1).max(1);
    let last = offset + limit.unwrap_or(DEFAULT_LIMIT).max(1) - 1;
    let mut file = File::open(path).map_err(io)?;
    let metadata = file.metadata().map_err(io)?;
    if metadata.is_dir() {
        return Err(format!("{shown} is a directory; list it with bash"));
    }
    let mut sniff = vec![0; SNIFF_BYTES];
    let sniffed = file.read(&mut sniff).map_err(io)?;
    if sniff[..sniffed].contains(&0) {
        return Err(format!(
            "{shown} is a binary file of {} bytes",
            metadata.len()
        ));
    }

    let mut reader = BufReader::new(std::io::Cursor::new(&sniff[..sniffed]).chain(file));
    let mut lines = Vec::new();
    let mut raw = Vec::new();
    let mut total = 0;
    loop {
        raw.clear();
        if reader.read_until(b'\n', &mut raw).map_err(io)? == 0 {
            break;
        }
        total += 1;
        if total < offset || total > last {
            continue;
        }
        let line = String::from_utf8_lossy(&raw);
        let line = line.trim_end_matches('\n').trim_end_matches('\r');
        lines.push(display_line(total, line));
    }
    Ok(Page {
        lines,
        total,
        next: (last < total).then_some(last + 1),
    })
}

fn display_line(number: usize, line: &str) -> String {
    let mut tagged = hashline::tag_line(number, line);
    let prefix = tagged.len() - line.len();
    if let Some((cut, _)) = line.char_indices().nth(MAX_LINE_CHARS) {
        tagged.truncate(prefix + cut);
        write!(tagged, "…[{} more chars]", line[cut..].chars().count()).ok();
    }
    tagged
}

pub(crate) fn write(path: &Path, shown: &str, content: &str) -> Result<Written, String> {
    let existed = path.exists();
    path.parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(path, content))
        .map_err(|err| format!("could not write {shown}: {err}"))?;
    Ok(Written {
        created: !existed,
        lines: content.lines().count(),
    })
}

pub(crate) fn edit(path: &Path, shown: &str, ops: Vec<RawOp>) -> Result<Edited, String> {
    let text =
        std::fs::read_to_string(path).map_err(|err| format!("could not read {shown}: {err}"))?;
    let mut document = Document::parse(&text);
    let preview = document
        .apply(ops)
        .map_err(|problem| format!("{shown}: {problem}"))?;
    std::fs::write(path, document.render())
        .map_err(|err| format!("could not write {shown}: {err}"))?;
    Ok(Edited {
        total: document.len(),
        preview,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_returns_anchored_lines_and_where_to_continue() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "one\ntwo\nthree\n").unwrap();
        assert_eq!(
            read(&path, "a.txt", Some(2), Some(1)).unwrap(),
            Page {
                lines: vec![hashline::tag_line(2, "two")],
                total: 3,
                next: Some(3),
            }
        );
    }

    #[test]
    fn read_of_a_binary_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b.bin");
        std::fs::write(&path, [0u8, 1, 2]).unwrap();
        assert_eq!(
            read(&path, "b.bin", None, None).unwrap_err(),
            "b.bin is a binary file of 3 bytes"
        );
    }

    #[test]
    fn write_creates_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x/y/z.txt");
        assert_eq!(
            write(&path, "x/y/z.txt", "hi\n").unwrap(),
            Written {
                created: true,
                lines: 1
            }
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), "hi\n");
    }

    #[test]
    fn long_lines_are_cut_for_display() {
        let line = "x".repeat(MAX_LINE_CHARS + 5);
        assert!(display_line(1, &line).ends_with("…[5 more chars]"));
    }
}
