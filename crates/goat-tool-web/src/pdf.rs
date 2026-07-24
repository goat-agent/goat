use pdf_oxide::PdfDocument;

pub(crate) fn extract_text(body: Vec<u8>) -> Option<String> {
    let doc = PdfDocument::from_bytes(body).ok()?;
    let pages = doc.page_count().ok()?;
    let mut out = String::new();
    for page in 0..pages {
        let Ok(text) = doc.extract_text_auto(page) else {
            continue;
        };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(text);
    }
    let out = out.trim();
    if out.is_empty() {
        None
    } else {
        Some(out.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::extract_text;

    #[test]
    fn malformed_pdf_degrades_to_none() {
        assert!(extract_text(b"%PDF-1.7 not really a pdf".to_vec()).is_none());
        assert!(extract_text(b"totally not a pdf".to_vec()).is_none());
        assert!(extract_text(Vec::new()).is_none());
    }
}
