pub const PAD_X: u16 = 1;
pub const SCROLL_GUTTER: u16 = 1;
pub const OVERLAY_W: u16 = 64;
pub const OVERLAY_CHROME: u16 = 4;
pub const OVERLAY_CHROME_PLAIN: u16 = 3;
pub const LIST_MAX: usize = 10;
pub const METER_WARN: f32 = 70.0;
pub const METER_HIGH: f32 = 90.0;

#[allow(clippy::cast_precision_loss)]
pub fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 10_000 {
        format!("{:.0}k", n as f64 / 1_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        format!("{n}")
    }
}
