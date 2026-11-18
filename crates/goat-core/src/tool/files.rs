use std::fmt::Write;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use super::Outcome;
use super::hashline::{self, Document, RawOp};

const DEFAULT_LIMIT: usize = 2000;
const MAX_LINE_CHARS: usize = 2000;
const SNIFF_BYTES: usize = 8 * 1024;

pub(crate) fn read(
    path: &Path,
    shown: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Outcome {
    match read_lines(
        path,
        offset.unwrap_or(1).max(1),
        limit.unwrap_or(DEFAULT_LIMIT).max(1),
    ) {
        Ok(text) => Outcome::ok(text),
        Err(err) => Outcome::error(format!("could not read {shown}: {err}")),
    }
}

fn read_lines(path: &Path, offset: usize, limit: usize) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let metadata = file.metadata()?;
    if metadata.is_dir() {
        return Ok("this is a directory; list it with bash".to_owned());
    }
    let mut sniff = vec![0; SNIFF_BYTES];
    let sniffed = file.read(&mut sniff)?;
    if sniff[..sniffed].contains(&0) {
        return Ok(format!("binary file, {} bytes", metadata.len()));
    }

    let mut reader = BufReader::new(std::io::Cursor::new(&sniff[..sniffed]).chain(file));
    let mut out = String::new();
    let mut raw = Vec::new();
    let mut total = 0;
    let last = offset + limit - 1;
    loop {
        raw.clear();
        if reader.read_until(b'\n', &mut raw)? == 0 {
            break;
        }
        total += 1;
        if total < offset || total > last {
            continue;
        }
        let line = String::from_utf8_lossy(&raw);
        let line = line.trim_end_matches('\n').trim_end_matches('\r');
        out.push_str(&display_line(total, line));
        out.push('\n');
    }

    if total == 0 {
        return Ok("[empty file]".to_owned());
    }
    if offset > total {
        return Ok(format!(
            "[the file has {total} lines; offset {offset} is past the end]"
        ));
    }
    if last < total {
        writeln!(
            out,
            "[lines {offset}-{last} of {total}; continue with offset {}]",
            last + 1
        )
        .ok();
    }
    Ok(out)
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

pub(crate) fn write(path: &Path, shown: &str, content: &str) -> Outcome {
    let existed = path.exists();
    let result = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(path, content));
    match result {
        Ok(()) => {
            let verb = if existed { "overwrote" } else { "created" };
            Outcome::ok(format!(
                "{verb} {shown} ({} lines)",
                content.lines().count()
            ))
        }
        Err(err) => Outcome::error(format!("could not write {shown}: {err}")),
    }
}

pub(crate) fn edit(path: &Path, shown: &str, ops: Vec<RawOp>) -> Outcome {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => return Outcome::error(format!("could not read {shown}: {err}")),
    };
    let mut document = Document::parse(&text);
    let preview = match document.apply(ops) {
        Ok(preview) => preview,
        Err(problem) => return Outcome::error(problem),
    };
    match std::fs::write(path, document.render()) {
        Ok(()) => Outcome::ok(format!(
            "edited {shown}; it now has {} lines. Updated anchors:\n{preview}",
            document.len()
        )),
        Err(err) => Outcome::error(format!("could not write {shown}: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_shows_anchored_lines_and_a_continuation_hint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "one\ntwo\nthree\n").unwrap();
        let outcome = read(&path, "a.txt", Some(2), Some(1));
        assert!(!outcome.is_error);
        assert_eq!(
            &*outcome.output,
            format!(
                "{}\n[lines 2-2 of 3; continue with offset 3]\n",
                hashline::tag_line(2, "two")
            )
        );
    }

    #[test]
    fn read_reports_binary_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b.bin");
        std::fs::write(&path, [0u8, 1, 2]).unwrap();
        assert_eq!(
            &*read(&path, "b.bin", None, None).output,
            "binary file, 3 bytes"
        );
    }

    #[test]
    fn write_creates_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x/y/z.txt");
        let outcome = write(&path, "x/y/z.txt", "hi\n");
        assert_eq!(&*outcome.output, "created x/y/z.txt (1 lines)");
        assert_eq!(std::fs::read_to_string(path).unwrap(), "hi\n");
    }

    #[test]
    fn long_lines_are_cut_for_display() {
        let line = "x".repeat(MAX_LINE_CHARS + 5);
        assert!(display_line(1, &line).ends_with("…[5 more chars]"));
    }
}
