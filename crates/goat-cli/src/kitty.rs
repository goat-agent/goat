use std::fmt::Write as _;
use std::io::{self, Write};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use rustix::event::{PollFd, PollFlags, Timespec, poll};

pub(crate) const ROWS: u16 = 4;

const QUERY: &str = "\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c";
const SUPPORTED: &str = "\x1b_Gi=31;OK\x1b\\";
const WIPE: &str = "\r\x1b[2K";
const WAIT: Duration = Duration::from_millis(300);
const CHUNK: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cell {
    pub(crate) width: u16,
    pub(crate) height: u16,
}

pub(crate) fn detect(is_tmux: bool) -> Option<Cell> {
    if is_tmux {
        return None;
    }
    let mut out = io::stdout();
    out.write_all(QUERY.as_bytes()).ok()?;
    out.flush().ok()?;
    let reply = read_reply();
    out.write_all(WIPE.as_bytes()).ok()?;
    out.flush().ok()?;
    is_supported(&reply?).then(cell)
}

pub(crate) fn transmit(id: u8, png: &[u8]) -> String {
    let data = BASE64.encode(png);
    let chunks: Vec<&[u8]> = data.as_bytes().chunks(CHUNK).collect();
    let mut out = String::with_capacity(data.len() + chunks.len() * 32);
    for (index, chunk) in chunks.iter().enumerate() {
        let more = u8::from(index + 1 < chunks.len());
        let chunk = std::str::from_utf8(chunk).unwrap_or_default();
        if index == 0 {
            write!(out, "\x1b_Ga=t,i={id},f=100,q=2,m={more};{chunk}\x1b\\").ok();
        } else {
            write!(out, "\x1b_Gm={more},q=2;{chunk}\x1b\\").ok();
        }
    }
    out
}

pub(crate) fn place(id: u8, columns: u16) -> String {
    format!("\x1b_Ga=p,i={id},c={columns},r={ROWS},C=1,q=2\x1b\\")
}

pub(crate) fn remove(id: u8) -> String {
    format!("\x1b_Ga=d,d=i,i={id},q=2\x1b\\")
}

fn read_reply() -> Option<Vec<u8>> {
    let stdin = rustix::stdio::stdin();
    let deadline = Instant::now() + WAIT;
    let mut reply = Vec::new();
    let mut buf = [0; 256];
    while !has_attributes(&reply) {
        let left = deadline.checked_duration_since(Instant::now())?;
        let timeout = Timespec::try_from(left).ok()?;
        let mut fds = [PollFd::new(&stdin, PollFlags::IN)];
        if poll(&mut fds, Some(&timeout)).ok()? == 0 {
            return None;
        }
        let read = rustix::io::read(stdin, &mut buf).ok()?;
        if read == 0 {
            return None;
        }
        reply.extend_from_slice(&buf[..read]);
    }
    Some(reply)
}

fn has_attributes(reply: &[u8]) -> bool {
    reply
        .windows(3)
        .position(|window| window == b"\x1b[?")
        .is_some_and(|at| reply[at..].contains(&b'c'))
}

fn is_supported(reply: &[u8]) -> bool {
    reply
        .windows(SUPPORTED.len())
        .any(|window| window == SUPPORTED.as_bytes())
}

fn cell() -> Cell {
    let fallback = Cell {
        width: 8,
        height: 16,
    };
    let Ok(size) = crossterm::terminal::window_size() else {
        return fallback;
    };
    if size.columns == 0 || size.rows == 0 || size.width == 0 || size.height == 0 {
        return fallback;
    }
    Cell {
        width: (size.width / size.columns).max(1),
        height: (size.height / size.rows).max(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_graphics_reply_before_the_attributes_means_support() {
        let reply = b"\x1bP>|ghostty\x1b\\\x1b_Gi=31;OK\x1b\\\x1b[?62;22c";
        assert!(has_attributes(reply));
        assert!(is_supported(reply));
    }

    #[test]
    fn only_device_attributes_means_no_graphics() {
        assert!(!is_supported(b"\x1b[?62;22c"));
    }

    #[test]
    fn an_error_reply_is_not_support() {
        assert!(!is_supported(b"\x1b_Gi=31;EINVAL: bad\x1b\\\x1b[?62c"));
    }

    #[test]
    fn a_placement_keeps_the_cursor_still() {
        assert_eq!(place(7, 12), "\x1b_Ga=p,i=7,c=12,r=4,C=1,q=2\x1b\\");
    }

    #[test]
    fn large_images_are_sent_in_chunks() {
        let sent = transmit(5, &[0; 4000]);
        assert!(sent.starts_with("\x1b_Ga=t,i=5,f=100,q=2,m=1;"));
        assert!(sent.contains("\x1b_Gm=0,q=2;"));
    }
}
