use std::io::Cursor;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use goat_api::{HostComputerOutput, Rect};
use image::{DynamicImage, ImageFormat, RgbaImage, imageops};
use screencapturekit::{
    screenshot_manager::{CGImageExt, SCScreenshotManager},
    shareable_content::SCShareableContent,
    stream::{configuration::SCStreamConfiguration, content_filter::SCContentFilter},
};

use crate::DesktopError;

pub(super) fn capture(
    region: Option<&Rect>,
    max_width: u32,
) -> Result<HostComputerOutput, DesktopError> {
    if max_width == 0 {
        return Err(DesktopError::NotStarted(
            "max_width must be greater than zero".into(),
        ));
    }
    if region.is_some_and(|rect| !valid_rect(rect)) {
        return Err(DesktopError::NotStarted(
            "region must have finite coordinates and positive finite dimensions".into(),
        ));
    }

    let content = SCShareableContent::get()
        .map_err(|error| DesktopError::NotStarted(format!("cannot enumerate displays: {error}")))?;
    let displays = content.displays();
    let display = displays
        .iter()
        .find(|display| {
            let frame = display.frame();
            frame.origin.x == 0.0 && frame.origin.y == 0.0
        })
        .ok_or_else(|| DesktopError::NotStarted("main display is unavailable".into()))?;
    let frame = display.frame();
    let bounds = Rect {
        x: frame.origin.x,
        y: frame.origin.y,
        width: frame.size.width,
        height: frame.size.height,
    };
    if !valid_rect(&bounds) {
        return Err(DesktopError::NotStarted(
            "main display has invalid bounds".into(),
        ));
    }
    if let Some(region) = region
        && (region.x < bounds.x
            || region.y < bounds.y
            || region.x + region.width > bounds.x + bounds.width
            || region.y + region.height > bounds.y + bounds.height)
    {
        return Err(DesktopError::NotStarted(
            "region must be inside the main display".into(),
        ));
    }

    let filter = SCContentFilter::create()
        .with_display(display)
        .try_build()
        .map_err(|error| {
            DesktopError::NotStarted(format!("cannot select main display: {error}"))
        })?;
    let scale = f64::from(filter.point_pixel_scale());
    let config = SCStreamConfiguration::new()
        .with_width(pixel_dimension(bounds.width * scale)?)
        .with_height(pixel_dimension(bounds.height * scale)?)
        .with_shows_cursor(true);
    let captured = SCScreenshotManager::capture_image(&filter, &config)
        .map_err(|error| DesktopError::NotStarted(format!("screen capture failed: {error}")))?;
    let width = u32::try_from(captured.width())
        .map_err(|_| DesktopError::NotStarted("captured image width is too large".into()))?;
    let height = u32::try_from(captured.height())
        .map_err(|_| DesktopError::NotStarted("captured image height is too large".into()))?;
    if width == 0 || height == 0 {
        return Err(DesktopError::NotStarted(
            "screen capture returned an empty image".into(),
        ));
    }
    let mut packed_bgra = captured.bgra_data().map_err(|error| {
        DesktopError::NotStarted(format!("cannot read captured pixels: {error}"))
    })?;
    for pixel in packed_bgra.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    let image = RgbaImage::from_raw(width, height, packed_bgra).ok_or_else(|| {
        DesktopError::NotStarted("captured pixels have invalid dimensions".into())
    })?;
    encode(image, region, bounds, max_width)
}

fn valid_rect(rect: &Rect) -> bool {
    rect.x.is_finite()
        && rect.y.is_finite()
        && rect.width.is_finite()
        && rect.height.is_finite()
        && rect.width > 0.0
        && rect.height > 0.0
        && (rect.x + rect.width).is_finite()
        && (rect.y + rect.height).is_finite()
        && rect.x + rect.width > rect.x
        && rect.y + rect.height > rect.y
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn pixel_dimension(value: f64) -> Result<u32, DesktopError> {
    let rounded = value.round();
    if !rounded.is_finite() || rounded < 1.0 || rounded > f64::from(u32::MAX) {
        return Err(DesktopError::NotStarted(
            "main display has invalid pixel dimensions".into(),
        ));
    }
    Ok(rounded as u32)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn pixel_range(start: f64, end: f64, origin: f64, extent: f64, pixels: u32) -> (u32, u32) {
    let limit = f64::from(pixels);
    let first = ((start - origin) / extent * limit)
        .floor()
        .clamp(0.0, limit);
    let last = ((end - origin) / extent * limit).ceil().clamp(0.0, limit);
    (first as u32, last as u32)
}

fn encode(
    mut image: RgbaImage,
    region: Option<&Rect>,
    mut display: Rect,
    max_width: u32,
) -> Result<HostComputerOutput, DesktopError> {
    let (full_width, full_height) = image.dimensions();
    let (left, top, width, height) = if let Some(region) = region {
        let (left, right) = pixel_range(
            region.x,
            region.x + region.width,
            display.x,
            display.width,
            full_width,
        );
        let (top, bottom) = pixel_range(
            region.y,
            region.y + region.height,
            display.y,
            display.height,
            full_height,
        );
        if right <= left || bottom <= top {
            return Err(DesktopError::NotStarted(
                "region contains no captured pixels".into(),
            ));
        }
        let width = right - left;
        let height = bottom - top;
        display.x += f64::from(left) / f64::from(full_width) * display.width;
        display.y += f64::from(top) / f64::from(full_height) * display.height;
        display.width *= f64::from(width) / f64::from(full_width);
        display.height *= f64::from(height) / f64::from(full_height);
        (left, top, width, height)
    } else {
        (0, 0, full_width, full_height)
    };

    if width > max_width {
        let scaled_height =
            (u64::from(height) * u64::from(max_width) + u64::from(width) / 2) / u64::from(width);
        let scaled_height = u32::try_from(scaled_height.max(1))
            .map_err(|_| DesktopError::NotStarted("resized image height is too large".into()))?;
        image = imageops::resize(
            &*imageops::crop_imm(&image, left, top, width, height),
            max_width,
            scaled_height,
            imageops::FilterType::Triangle,
        );
    } else if width != full_width || height != full_height {
        image = imageops::crop_imm(&image, left, top, width, height).to_image();
    }

    let (width, height) = image.dimensions();
    let mut png = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image)
        .write_to(&mut png, ImageFormat::Png)
        .map_err(|error| DesktopError::NotStarted(format!("cannot encode screenshot: {error}")))?;
    Ok(HostComputerOutput::Screenshot {
        media_type: "image/png".into(),
        data: STANDARD.encode(png.into_inner()),
        width,
        height,
        display,
    })
}
