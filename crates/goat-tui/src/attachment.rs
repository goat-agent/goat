use std::{borrow::Cow, io::Cursor, path::PathBuf};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use goat_protocol::InputAttachment;
use image::{GenericImageView as _, ImageFormat, imageops::FilterType};

#[cfg(unix)]
const ESCAPE: Option<char> = Some('\\');
#[cfg(not(unix))]
const ESCAPE: Option<char> = None;

const MAX_ATTACHMENTS: usize = 8;
const MAX_SOURCE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_SIDE: u32 = 2048;
const MAX_PIXELS: u64 = 12_000_000;
const MAX_ENCODED_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug)]
pub(crate) enum AttachError {
    Empty,
    TooMany,
    NotImages,
    TooLarge(String),
    Decode(String),
    Clipboard(String),
}

impl std::fmt::Display for AttachError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("no image found"),
            Self::TooMany => write!(f, "attach up to {MAX_ATTACHMENTS} images at once"),
            Self::NotImages => f.write_str("paste did not contain only image paths"),
            Self::TooLarge(label) => write!(f, "image is too large: {label}"),
            Self::Decode(label) => write!(f, "could not read image: {label}"),
            Self::Clipboard(message) => write!(f, "clipboard image unavailable: {message}"),
        }
    }
}

pub(crate) fn paste_contains_only_image_paths(text: &str) -> bool {
    parse_pasted_paths(text).is_ok()
}

pub(crate) fn attachments_from_paste(text: &str) -> Result<Vec<InputAttachment>, AttachError> {
    let paths = parse_pasted_paths(text)?;
    if paths.len() > MAX_ATTACHMENTS {
        return Err(AttachError::TooMany);
    }
    paths
        .iter()
        .map(|path| attachment_from_path(path))
        .collect()
}

pub(crate) fn extract_image_paths(text: &str) -> (String, Vec<InputAttachment>) {
    let mut attachments = Vec::new();
    let mut kept: Vec<String> = Vec::new();
    for line in text.split('\n') {
        let tokens = split_tokens(line);
        if tokens.len() == 1
            && attachments.len() < MAX_ATTACHMENTS
            && let Some(att) = image_attachment_from_token(&tokens[0])
        {
            attachments.push(att);
            continue;
        }
        kept.push(line.to_owned());
    }
    (kept.join("\n"), attachments)
}

fn image_file(candidate: &str) -> Option<PathBuf> {
    let path = token_to_path(candidate)?;
    (looks_like_image(&path) && path.is_file()).then_some(path)
}

fn image_attachment_from_token(token: &str) -> Option<InputAttachment> {
    image_file(token).and_then(|path| attachment_from_path(&path).ok())
}

pub(crate) fn attachment_from_clipboard() -> Result<InputAttachment, AttachError> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|err| AttachError::Clipboard(err.to_string()))?;
    let image = clipboard
        .get_image()
        .map_err(|err| AttachError::Clipboard(err.to_string()))?;
    encode_rgba(
        image.width,
        image.height,
        image.bytes,
        "clipboard image".to_owned(),
    )
}

fn parse_pasted_paths(text: &str) -> Result<Vec<PathBuf>, AttachError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(AttachError::Empty);
    }
    if let Some(path) = image_file(&unescape(trimmed)) {
        return Ok(vec![path]);
    }
    split_tokens(text)
        .iter()
        .map(|token| image_file(token))
        .collect::<Option<Vec<PathBuf>>>()
        .filter(|paths| !paths.is_empty())
        .ok_or(AttachError::NotImages)
}

fn unescape(text: &str) -> String {
    let mut out = String::new();
    let mut escaped = false;
    for ch in text.chars() {
        if escaped {
            out.push(ch);
            escaped = false;
        } else if ESCAPE == Some(ch) {
            escaped = true;
        } else {
            out.push(ch);
        }
    }
    out
}

fn split_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in text.trim().chars() {
        if escaped {
            cur.push(ch);
            escaped = false;
            continue;
        }
        if ESCAPE == Some(ch) {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            } else {
                cur.push(ch);
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            continue;
        }
        if ch.is_whitespace() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(ch);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn token_to_path(token: &str) -> Option<PathBuf> {
    if let Some(rest) = token.strip_prefix("file://") {
        let decoded = percent_encoding::percent_decode_str(rest)
            .decode_utf8()
            .ok()?;
        return Some(PathBuf::from(decoded.as_ref()));
    }
    Some(PathBuf::from(token))
}

fn looks_like_image(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tif" | "tiff"
            )
        })
}

fn attachment_from_path(path: &std::path::Path) -> Result<InputAttachment, AttachError> {
    let label = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("image")
        .to_owned();
    let meta = std::fs::metadata(path).map_err(|_| AttachError::Decode(label.clone()))?;
    if meta.len() > MAX_SOURCE_BYTES {
        return Err(AttachError::TooLarge(label));
    }
    let img = image::open(path).map_err(|_| AttachError::Decode(label.clone()))?;
    encode_dynamic(img, label)
}

fn encode_rgba(
    width: usize,
    height: usize,
    bytes: Cow<'_, [u8]>,
    label: String,
) -> Result<InputAttachment, AttachError> {
    let width = u32::try_from(width).map_err(|_| AttachError::TooLarge(label.clone()))?;
    let height = u32::try_from(height).map_err(|_| AttachError::TooLarge(label.clone()))?;
    let Some(buffer) = image::RgbaImage::from_raw(width, height, bytes.into_owned()) else {
        return Err(AttachError::Decode(label));
    };
    encode_dynamic(image::DynamicImage::ImageRgba8(buffer), label)
}

fn encode_dynamic(
    mut img: image::DynamicImage,
    label: String,
) -> Result<InputAttachment, AttachError> {
    let (width, height) = img.dimensions();
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(AttachError::TooLarge(label));
    }
    let longest = width.max(height);
    if longest > MAX_SIDE {
        let ratio = f64::from(MAX_SIDE) / f64::from(longest);
        let w = (f64::from(width) * ratio).round().max(1.0) as u32;
        let h = (f64::from(height) * ratio).round().max(1.0) as u32;
        img = img.resize(w, h, FilterType::Lanczos3);
    }
    let mut cursor = Cursor::new(Vec::new());
    img.write_to(&mut cursor, ImageFormat::Png)
        .map_err(|_| AttachError::Decode(label.clone()))?;
    let bytes = cursor.into_inner();
    if bytes.len() > MAX_ENCODED_BYTES {
        return Err(AttachError::TooLarge(label));
    }
    Ok(InputAttachment {
        media_type: "image/png".to_owned(),
        data: STANDARD.encode(bytes),
        label,
    })
}

#[cfg(test)]
mod tests {
    use super::{extract_image_paths, split_tokens};

    #[test]
    fn split_quoted_paths() {
        assert_eq!(
            split_tokens("'/tmp/a b.png' /tmp/c.png"),
            vec!["/tmp/a b.png", "/tmp/c.png"]
        );
    }

    #[test]
    fn extract_keeps_plain_multiword_text() {
        let (text, atts) = extract_image_paths("hello world\nfoo bar");
        assert_eq!(text, "hello world\nfoo bar");
        assert!(atts.is_empty());
    }

    #[test]
    fn extract_ignores_nonexistent_image_path() {
        let (text, atts) = extract_image_paths("/no/such/screenshot.png");
        assert_eq!(text, "/no/such/screenshot.png");
        assert!(atts.is_empty());
    }

    #[test]
    fn dropped_path_with_unescaped_space_promotes() {
        use super::attachments_from_paste;
        let path = std::env::temp_dir().join("goat drop test image.png");
        image::RgbaImage::new(2, 2).save(&path).unwrap();
        let pasted = path.display().to_string();
        assert!(pasted.contains(' '), "path must contain an unescaped space");
        let atts = attachments_from_paste(&pasted).expect("should parse as one image");
        assert_eq!(atts.len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn nonexistent_spaced_path_is_not_promoted() {
        use super::paste_contains_only_image_paths;
        assert!(!paste_contains_only_image_paths(
            "/no/such/Screenshot 2026 at 1 PM.png"
        ));
    }
}
