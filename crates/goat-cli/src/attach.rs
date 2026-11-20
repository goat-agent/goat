use std::fs::File;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use goat_core::image::{self, Media, Picture};

use crate::kitty::{self, Cell};

const FILE_BYTES: u64 = 50 * 1024 * 1024;
const TILE_COLUMNS: u16 = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Tile {
    pub(crate) id: u8,
    pub(crate) columns: u16,
}

pub(crate) fn pasted_images(text: &str) -> Option<Vec<PathBuf>> {
    let paths: Vec<PathBuf> = words(text.trim()).into_iter().map(path_of).collect();
    let is_images = !paths.is_empty() && paths.iter().all(|path| is_image(path));
    is_images.then_some(paths)
}

pub(crate) fn load(path: &Path) -> Result<Picture, String> {
    let shown = path.display();
    let size = std::fs::metadata(path)
        .map_err(|err| format!("could not read {shown}: {err}"))?
        .len();
    if size > FILE_BYTES {
        return Err(format!("{shown} is over {} MB", FILE_BYTES / 1024 / 1024));
    }
    let bytes = std::fs::read(path).map_err(|err| format!("could not read {shown}: {err}"))?;
    image::prepare(bytes).map_err(|err| format!("could not attach {shown}: {err}"))
}

pub(crate) fn clipboard() -> Result<Option<Picture>, String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|err| format!("could not open the clipboard: {err}"))?;
    let data = match clipboard.get_image() {
        Ok(data) => data,
        Err(arboard::Error::ContentNotAvailable) => return Ok(None),
        Err(err) => return Err(format!("could not read the clipboard: {err}")),
    };
    let width = u32::try_from(data.width).map_err(|_| "the clipboard image is too wide")?;
    let height = u32::try_from(data.height).map_err(|_| "the clipboard image is too tall")?;
    let pixels = ::image::RgbaImage::from_raw(width, height, data.bytes.into_owned())
        .ok_or("the clipboard image is malformed")?;
    let mut png = Cursor::new(Vec::new());
    pixels
        .write_to(&mut png, ::image::ImageFormat::Png)
        .map_err(|err| format!("could not encode the clipboard image: {err}"))?;
    image::prepare(png.into_inner())
        .map(Some)
        .map_err(|err| format!("could not attach the clipboard image: {err}"))
}

pub(crate) fn thumbnail(picture: &Picture, cell: Cell) -> Option<(Vec<u8>, u16)> {
    let decoded = ::image::load_from_memory(&picture.bytes).ok()?;
    let height = u32::from(kitty::ROWS) * u32::from(cell.height);
    let width =
        (u64::from(height) * u64::from(picture.width) / u64::from(picture.height.max(1))).max(1);
    let columns = u16::try_from(width.div_ceil(u64::from(cell.width)))
        .unwrap_or(TILE_COLUMNS)
        .clamp(1, TILE_COLUMNS);
    let small = decoded.thumbnail(u32::from(columns) * u32::from(cell.width), height);
    let mut png = Cursor::new(Vec::new());
    small.write_to(&mut png, ::image::ImageFormat::Png).ok()?;
    Some((png.into_inner(), columns))
}

fn words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (None, '\\') => word.extend(chars.next()),
            (None, '\'' | '"') => quote = Some(c),
            (Some(open), c) if c == open => quote = None,
            (None, c) if c.is_whitespace() => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            (_, c) => word.push(c),
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

fn path_of(word: String) -> PathBuf {
    match word.strip_prefix("file://") {
        Some(rest) => PathBuf::from(decode_percent(rest)),
        None => PathBuf::from(word),
    }
}

fn decode_percent(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let escaped = (bytes[at] == b'%')
            .then(|| text.get(at + 1..at + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        if let Some(byte) = escaped {
            out.push(byte);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn is_image(path: &Path) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let mut head = [0; 16];
    let read = file.read(&mut head).unwrap_or(0);
    Media::sniff(&head[..read]).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_at(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        ::image::RgbImage::new(4, 2).save(&path).unwrap();
        path
    }

    #[test]
    fn a_dropped_path_with_escaped_spaces_is_an_image() {
        let dir = tempfile::tempdir().unwrap();
        let path = png_at(dir.path(), "Screen Shot 1.png");
        let pasted = format!("{} ", path.display().to_string().replace(' ', "\\ "));

        assert_eq!(pasted_images(&pasted), Some(vec![path]));
    }

    #[test]
    fn quoted_and_file_url_paths_are_images() {
        let dir = tempfile::tempdir().unwrap();
        let first = png_at(dir.path(), "a b.png");
        let second = png_at(dir.path(), "c.png");
        let pasted = format!(
            "'{}' file://{}",
            first.display(),
            second.display().to_string().replace(' ', "%20")
        );

        assert_eq!(pasted_images(&pasted), Some(vec![first, second]));
    }

    #[test]
    fn text_with_an_image_path_is_just_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = png_at(dir.path(), "a.png");

        assert_eq!(pasted_images(&format!("look at {}", path.display())), None);
    }

    #[test]
    fn a_text_file_is_not_an_image() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.png");
        std::fs::write(&path, "hello").unwrap();

        assert_eq!(pasted_images(&path.display().to_string()), None);
    }

    #[test]
    fn a_thumbnail_keeps_the_aspect_within_four_rows() {
        let dir = tempfile::tempdir().unwrap();
        let picture = load(&png_at(dir.path(), "wide.png")).unwrap();
        let cell = Cell {
            width: 8,
            height: 16,
        };

        let (png, columns) = thumbnail(&picture, cell).unwrap();

        assert_eq!(columns, 16);
        assert_eq!(Media::sniff(&png), Some(Media::Png));
    }
}
