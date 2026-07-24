use std::cell::RefCell;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use goat_protocol::ToolImageData;
use ratatui::layout::{Rect, Size};
use ratatui_image::{Image, Resize, picker::Picker, protocol::Protocol};

pub(crate) const MAX_IMAGE_ROWS: u16 = 12;

pub(crate) struct TranscriptImage {
    source: ToolImageData,
    cells: Option<(u16, u16)>,
    protocol: RefCell<Option<Result<Protocol, ()>>>,
}

impl std::fmt::Debug for TranscriptImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TranscriptImage")
            .field("media_type", &self.source.media_type)
            .field("cells", &self.cells)
            .field("built", &self.protocol.borrow().is_some())
            .finish()
    }
}

impl TranscriptImage {
    pub(crate) fn new(source: ToolImageData, picker: Option<&Picker>) -> Self {
        let cells = picker.and_then(|p| decode(&source).map(|img| fitted_cells(p, &img)));
        Self {
            source,
            cells,
            protocol: RefCell::new(None),
        }
    }

    pub(crate) fn rows(&self) -> u16 {
        self.cells.map_or(0, |(_, h)| h)
    }

    pub(crate) fn source(&self) -> ToolImageData {
        self.source.clone()
    }

    #[cfg(test)]
    pub(crate) fn fixed(rows: u16) -> Self {
        Self {
            source: ToolImageData {
                media_type: "image/png".to_owned(),
                data: String::new(),
            },
            cells: Some((rows, rows)),
            protocol: RefCell::new(None),
        }
    }

    pub(crate) fn render(&self, frame: &mut ratatui::Frame, area: Rect, picker: &Picker) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        if self.protocol.borrow().is_none() {
            let built = decode(&self.source)
                .and_then(|img| {
                    let size = self.cells.map(|(w, h)| Size::new(w, h))?;
                    picker.new_protocol(img, size, Resize::Fit(None)).ok()
                })
                .ok_or(());
            *self.protocol.borrow_mut() = Some(built);
        }
        let guard = self.protocol.borrow();
        if let Some(Ok(protocol)) = guard.as_ref() {
            frame.render_widget(Image::new(protocol).allow_clipping(true), area);
        }
    }
}

pub(crate) fn render_zoom(
    frame: &mut ratatui::Frame,
    area: Rect,
    picker: &Picker,
    source: &ToolImageData,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let Some(img) = decode(source) else {
        return;
    };
    let size = Size::new(area.width, area.height);
    let Ok(protocol) = picker.new_protocol(img, size, Resize::Fit(None)) else {
        return;
    };
    frame.render_widget(Image::new(&protocol).allow_clipping(true), area);
}

fn decode(source: &ToolImageData) -> Option<image::DynamicImage> {
    let bytes = STANDARD.decode(source.data.as_bytes()).ok()?;
    image::load_from_memory(&bytes).ok()
}

fn fitted_cells(picker: &Picker, img: &image::DynamicImage) -> (u16, u16) {
    use image::GenericImageView as _;
    let font = picker.font_size();
    let (px_w, px_h) = img.dimensions();
    let font_w = u32::from(font.width.max(1));
    let font_h = u32::from(font.height.max(1));
    let cols = px_w.div_ceil(font_w).max(1);
    let rows = px_h.div_ceil(font_h).max(1);
    let rows = rows.min(u32::from(MAX_IMAGE_ROWS));
    let scaled_cols = if px_h == 0 {
        cols
    } else {
        let target_px_h = rows * font_h;
        let scaled_px_w = (u64::from(px_w) * u64::from(target_px_h) / u64::from(px_h)) as u32;
        scaled_px_w.div_ceil(font_w).max(1)
    };
    (
        u16::try_from(scaled_cols).unwrap_or(u16::MAX),
        u16::try_from(rows).unwrap_or(MAX_IMAGE_ROWS),
    )
}

pub(crate) fn query_picker() -> Option<Picker> {
    Picker::from_query_stdio().ok()
}
