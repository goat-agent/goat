use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const PREFIX: &str = "file://";

pub(super) fn from_path(path: &Path) -> String {
    let mut text = String::from(PREFIX);
    for byte in path.as_os_str().as_encoded_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(byte) {
            text.push(char::from(*byte));
        } else {
            write!(text, "%{byte:02X}").ok();
        }
    }
    text
}

pub(super) fn to_path(uri: &str) -> Option<PathBuf> {
    let encoded = uri.strip_prefix(PREFIX)?.as_bytes();
    let mut bytes = Vec::with_capacity(encoded.len());
    let mut at = 0;
    while at < encoded.len() {
        if encoded[at] == b'%'
            && let Some(byte) = encoded
                .get(at + 1..at + 3)
                .and_then(|hex| std::str::from_utf8(hex).ok())
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            bytes.push(byte);
            at += 3;
            continue;
        }
        bytes.push(encoded[at]);
        at += 1;
    }
    String::from_utf8(bytes).ok().map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_round_trips_through_a_uri() {
        let path = Path::new("/work/my project/src/a+b.rs");

        let uri = from_path(path);

        assert_eq!(uri, "file:///work/my%20project/src/a%2Bb.rs");
        assert_eq!(to_path(&uri).unwrap(), path);
    }
}
