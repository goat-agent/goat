use std::io::Write as _;

fn main() -> std::io::Result<()> {
    let mut text = serde_json::to_string_pretty(&goat_api::schema_document())?;
    text.push('\n');
    if let Some(path) = std::env::args().nth(1) {
        return std::fs::write(path, text);
    }
    std::io::stdout().write_all(text.as_bytes())
}
