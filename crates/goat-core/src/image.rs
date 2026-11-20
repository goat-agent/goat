use std::fmt::{self, Write as _};
use std::io::Cursor;

use ::image::codecs::jpeg::JpegEncoder;
use ::image::imageops::FilterType;
use ::image::{DynamicImage, ImageFormat, ImageReader};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const LONG_EDGE: u32 = 2048;
const MAX_BYTES: usize = 5 * 1024 * 1024;
const JPEG_QUALITY: u8 = 85;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Media {
    #[serde(rename = "image/png")]
    Png,
    #[serde(rename = "image/jpeg")]
    Jpeg,
    #[serde(rename = "image/gif")]
    Gif,
    #[serde(rename = "image/webp")]
    Webp,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Id(Box<str>);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Image {
    pub id: Id,
    pub media: Media,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub image: Image,
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not a PNG, JPEG, GIF or WebP image")]
    NotImage,
    #[error("decode the image")]
    Decode(#[from] ::image::ImageError),
    #[error("the image is {0} bytes after resizing, over the {MAX_BYTES} byte limit")]
    TooLarge(usize),
}

impl Media {
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Some(Self::Png);
        }
        if bytes.starts_with(b"\xff\xd8\xff") {
            return Some(Self::Jpeg);
        }
        if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            return Some(Self::Gif);
        }
        if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
            return Some(Self::Webp);
        }
        None
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
        }
    }

    fn format(self) -> ImageFormat {
        match self {
            Self::Png => ImageFormat::Png,
            Self::Jpeg => ImageFormat::Jpeg,
            Self::Gif => ImageFormat::Gif,
            Self::Webp => ImageFormat::WebP,
        }
    }
}

impl Id {
    pub(crate) fn of(bytes: &[u8]) -> Self {
        let digest = Sha256::digest(bytes);
        let mut hex = String::with_capacity(digest.len() * 2);
        for byte in digest {
            write!(hex, "{byte:02x}").ok();
        }
        Self(hex.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub fn prepare(bytes: Vec<u8>) -> Result<Picture, Error> {
    let media = Media::sniff(&bytes).ok_or(Error::NotImage)?;
    let (width, height) = reader(&bytes, media).into_dimensions()?;
    if width.max(height) <= LONG_EDGE && bytes.len() <= MAX_BYTES {
        return Ok(picture(bytes, media, width, height));
    }
    let shrunk = reader(&bytes, media)
        .decode()?
        .resize(LONG_EDGE, LONG_EDGE, FilterType::Triangle);
    drop(bytes);
    let (media, encoded) = encode(&shrunk, media)?;
    if encoded.len() > MAX_BYTES {
        return Err(Error::TooLarge(encoded.len()));
    }
    Ok(picture(encoded, media, shrunk.width(), shrunk.height()))
}

fn reader(bytes: &[u8], media: Media) -> ImageReader<Cursor<&[u8]>> {
    ImageReader::with_format(Cursor::new(bytes), media.format())
}

fn encode(image: &DynamicImage, media: Media) -> Result<(Media, Vec<u8>), Error> {
    let mut out = Cursor::new(Vec::new());
    if media == Media::Jpeg {
        let rgb = DynamicImage::ImageRgb8(image.to_rgb8());
        rgb.write_with_encoder(JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY))?;
        return Ok((Media::Jpeg, out.into_inner()));
    }
    image.write_to(&mut out, ImageFormat::Png)?;
    Ok((Media::Png, out.into_inner()))
}

fn picture(bytes: Vec<u8>, media: Media, width: u32, height: u32) -> Picture {
    Picture {
        image: Image {
            id: Id::of(&bytes),
            media,
        },
        width,
        height,
        bytes,
    }
}

#[cfg(test)]
mod tests {
    use ::image::{Rgb, RgbImage};

    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = DynamicImage::ImageRgb8(RgbImage::from_pixel(width, height, Rgb([9, 9, 9])));
        let mut out = Cursor::new(Vec::new());
        image.write_to(&mut out, ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn a_small_image_is_kept_byte_for_byte() {
        let bytes = png(40, 20);

        let picture = prepare(bytes.clone()).unwrap();

        assert_eq!(picture.bytes, bytes);
        assert_eq!((picture.width, picture.height), (40, 20));
        assert_eq!(picture.image.media, Media::Png);
    }

    #[test]
    fn a_large_image_is_shrunk_to_the_long_edge() {
        let picture = prepare(png(4096, 1024)).unwrap();

        assert_eq!((picture.width, picture.height), (2048, 512));
        assert_eq!(Media::sniff(&picture.bytes), Some(Media::Png));
    }

    #[test]
    fn the_same_bytes_have_the_same_hash() {
        let first = prepare(png(8, 8)).unwrap();
        let second = prepare(png(8, 8)).unwrap();

        assert_eq!(first.image.id, second.image.id);
        assert_eq!(first.image.id.as_str().len(), 64);
    }

    #[test]
    fn text_is_not_an_image() {
        assert!(matches!(prepare(b"hello".to_vec()), Err(Error::NotImage)));
    }
}
