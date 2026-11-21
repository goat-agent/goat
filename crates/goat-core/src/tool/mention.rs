use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use super::{files, spill};
use crate::image::{self, Picture};

const LISTED: usize = 200;
const TRAILING: &[char] = &['.', ',', ';', ':', '!', '?', ')', ']', '}', '"', '\''];

#[derive(Debug)]
pub(crate) enum Attached {
    File { path: Box<str>, text: Box<str> },
    Image(Picture),
}

pub(crate) fn mentioned(text: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut previous: Option<char> = None;
    for (at, c) in text.char_indices() {
        if c == '@' && previous.is_none_or(char::is_whitespace) {
            let rest = &text[at + 1..];
            let raw = &rest[..rest.find(char::is_whitespace).unwrap_or(rest.len())];
            if !raw.is_empty() && !found.contains(&raw) {
                found.push(raw);
            }
        }
        previous = Some(c);
    }
    found
}

pub(crate) fn attach(
    resolve: impl Fn(&str) -> PathBuf,
    spill_dir: &Path,
    raw: &str,
) -> Option<Attached> {
    let (raw, path) = [raw, raw.trim_end_matches(TRAILING)]
        .into_iter()
        .filter(|candidate| !candidate.is_empty())
        .find_map(|candidate| {
            let path = resolve(candidate);
            path.exists().then_some((candidate, path))
        })?;
    let text = if path.is_dir() {
        listing(&path)
    } else {
        match files::image(&path, raw) {
            Ok(Some(bytes)) => match image::prepare(bytes) {
                Ok(picture) => return Some(Attached::Image(picture)),
                Err(err) => format!("{raw} could not be attached: {err}"),
            },
            Ok(None) => match files::read(&path, raw, None, None) {
                Ok(page) => page_text(&page),
                Err(problem) => problem,
            },
            Err(problem) => problem,
        }
    };
    Some(Attached::File {
        path: raw.into(),
        text: spill::fit(text, spill_dir).into(),
    })
}

pub fn workspace_files(root: &Path, limit: usize) -> Vec<String> {
    let mut found: Vec<String> = WalkBuilder::new(root)
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.depth() > 0)
        .take(limit)
        .filter_map(|entry| {
            let relative = entry.path().strip_prefix(root).ok()?.to_str()?;
            let is_dir = entry.file_type().is_some_and(|kind| kind.is_dir());
            Some(if is_dir {
                format!("{relative}/")
            } else {
                relative.to_owned()
            })
        })
        .collect();
    found.sort();
    found
}

fn page_text(page: &files::Page) -> String {
    let mut text = page.lines.join("\n");
    if let Some(next) = page.next {
        write!(
            text,
            "\n[{} lines in all; read from line {next} for the rest]",
            page.total
        )
        .ok();
    }
    text
}

fn listing(dir: &Path) -> String {
    let mut entries: Vec<String> = WalkBuilder::new(dir)
        .max_depth(Some(1))
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.depth() == 1)
        .map(|entry| {
            let name = entry.file_name().to_string_lossy();
            if entry.file_type().is_some_and(|kind| kind.is_dir()) {
                format!("{name}/")
            } else {
                name.into_owned()
            }
        })
        .collect();
    if entries.is_empty() {
        return "(an empty directory)".to_owned();
    }
    entries.sort();
    let total = entries.len();
    entries.truncate(LISTED);
    let mut text = entries.join("\n");
    if total > LISTED {
        write!(text, "\n[and {} more]", total - LISTED).ok();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attach_in(dir: &Path, raw: &str) -> Option<Attached> {
        attach(|raw| dir.join(raw), &dir.join(".spill"), raw)
    }

    #[test]
    fn mentions_start_a_word() {
        assert_eq!(
            mentioned("look at @src/a.rs and @b.rs, not me@mail.com or @src/a.rs"),
            ["src/a.rs", "b.rs,"]
        );
    }

    #[test]
    fn a_file_comes_with_anchored_lines() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();

        let Some(Attached::File { path, text }) = attach_in(dir.path(), "a.txt.") else {
            panic!("expected a file");
        };

        assert_eq!(&*path, "a.txt");
        assert_eq!(
            &*text,
            format!(
                "{}\n{}",
                super::super::hashline::tag_line(1, "one"),
                super::super::hashline::tag_line(2, "two")
            )
        );
    }

    #[test]
    fn a_directory_comes_as_a_listing_without_ignored_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::create_dir_all(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();

        let Some(Attached::File { text, .. }) = attach_in(dir.path(), ".") else {
            panic!("expected a listing");
        };

        assert_eq!(&*text, "Cargo.toml\nsrc/");
    }

    #[test]
    fn a_missing_path_is_left_as_text() {
        let dir = tempfile::tempdir().unwrap();

        assert!(attach_in(dir.path(), "nowhere.rs").is_none());
    }

    #[test]
    fn workspace_files_skip_ignored_ones_and_mark_directories() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::create_dir_all(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "").unwrap();

        assert_eq!(workspace_files(dir.path(), 100), ["src/", "src/main.rs"]);
    }
}
