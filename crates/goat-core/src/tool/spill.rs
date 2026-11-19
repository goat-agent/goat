use std::collections::VecDeque;
use std::fmt::Write as _;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

const HEAD_BYTES: usize = 20 * 1024;
const TAIL_BYTES: usize = 20 * 1024;

#[derive(Debug)]
pub(crate) struct Kept {
    pub(crate) text: String,
    pub(crate) file: Option<PathBuf>,
}

pub(crate) struct Capture {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    total: usize,
    file: File,
    path: PathBuf,
}

impl Capture {
    pub(crate) fn create(dir: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!("{}.log", uuid::Uuid::now_v7()));
        Ok(Self {
            head: Vec::new(),
            tail: VecDeque::new(),
            total: 0,
            file: File::create(&path)?,
            path,
        })
    }

    pub(crate) fn push(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.file.write_all(bytes)?;
        self.total += bytes.len();
        let room = HEAD_BYTES.saturating_sub(self.head.len());
        let (head, rest) = bytes.split_at(room.min(bytes.len()));
        self.head.extend_from_slice(head);
        self.tail.extend(rest);
        let excess = self.tail.len().saturating_sub(TAIL_BYTES);
        self.tail.drain(..excess);
        Ok(())
    }

    pub(crate) fn finish(self) -> std::io::Result<Kept> {
        let kept = self.head.len() + self.tail.len();
        let mut text = String::from_utf8_lossy(&self.head).into_owned();
        let tail: Vec<u8> = self.tail.into_iter().collect();
        if kept == self.total {
            drop(self.file);
            std::fs::remove_file(&self.path)?;
            text.push_str(&String::from_utf8_lossy(&tail));
            return Ok(Kept { text, file: None });
        }
        write!(
            text,
            "\n[... {} bytes omitted; full output in {} ...]\n",
            self.total - kept,
            self.path.display()
        )
        .ok();
        text.push_str(&String::from_utf8_lossy(&tail));
        Ok(Kept {
            text,
            file: Some(self.path),
        })
    }
}

pub(crate) fn fit(text: String, dir: &Path) -> String {
    if text.len() <= HEAD_BYTES + TAIL_BYTES {
        return text;
    }
    let kept = Capture::create(dir)
        .and_then(|mut capture| capture.push(text.as_bytes()).map(|()| capture))
        .and_then(Capture::finish);
    match kept {
        Ok(kept) => kept.text,
        Err(err) => {
            let mut cut = HEAD_BYTES;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            format!(
                "{}\n[... {} bytes omitted; they could not be saved: {err} ...]",
                &text[..cut],
                text.len() - cut
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_kept_whole_without_a_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(fit("hi".to_owned(), dir.path()), "hi");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn long_text_keeps_head_and_tail_and_spills_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let text = format!("start{}end", "x".repeat(100_000));
        let kept = fit(text, dir.path());
        assert!(kept.starts_with("start"));
        assert!(kept.ends_with("end"));
        assert!(kept.contains("bytes omitted"));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
