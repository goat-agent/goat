use encoding_rs::Encoding;

use crate::fetch::RawFetch;

const SNIFF_LIMIT: usize = 1024;

pub(crate) fn decode(raw: &RawFetch) -> String {
    let label = charset_from_content_type(raw.content_type.as_deref())
        .or_else(|| sniff_meta_charset(&raw.body));
    let encoding = label
        .and_then(|value| Encoding::for_label(value.trim().as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    let (text, _, _) = encoding.decode(&raw.body);
    text.into_owned()
}

fn charset_from_content_type(content_type: Option<&str>) -> Option<String> {
    let value = content_type?.to_ascii_lowercase();
    let idx = value.find("charset")?;
    let after = value[idx + "charset".len()..].trim_start();
    let after = after.strip_prefix('=')?.trim_start();
    Some(take_label(after)).filter(|label| !label.is_empty())
}

fn sniff_meta_charset(body: &[u8]) -> Option<String> {
    let head = &body[..body.len().min(SNIFF_LIMIT)];
    let text = String::from_utf8_lossy(head).to_ascii_lowercase();
    let idx = text.find("charset")?;
    let after = text[idx + "charset".len()..].trim_start();
    let after = after.strip_prefix('=')?.trim_start();
    Some(take_label(after)).filter(|label| !label.is_empty())
}

fn take_label(input: &str) -> String {
    let trimmed = input.trim_start_matches(['"', '\'']);
    let end = trimmed
        .find(['"', '\'', ';', ' ', '/', '>', '\t', '\n', '\r'])
        .unwrap_or(trimmed.len());
    trimmed[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::decode;
    use crate::fetch::RawFetch;

    fn raw(body: Vec<u8>, content_type: Option<&str>) -> RawFetch {
        RawFetch {
            final_url: "https://example.com/".to_owned(),
            status: 200,
            content_type: content_type.map(str::to_owned),
            body,
            overflowed: false,
        }
    }

    #[test]
    fn decodes_shift_jis_from_header() {
        let (bytes, _, _) = encoding_rs::SHIFT_JIS.encode("日本語テスト");
        let out = decode(&raw(
            bytes.into_owned(),
            Some("text/html; charset=shift_jis"),
        ));
        assert_eq!(out, "日本語テスト");
    }

    #[test]
    fn decodes_gbk_from_header() {
        let (bytes, _, _) = encoding_rs::GBK.encode("中文测试");
        let out = decode(&raw(bytes.into_owned(), Some("text/html; charset=gbk")));
        assert_eq!(out, "中文测试");
    }

    #[test]
    fn decodes_euc_kr_from_meta_when_header_absent() {
        let (body, _, _) = encoding_rs::EUC_KR
            .encode("<html><head><meta charset=\"euc-kr\"></head><body>한국어</body></html>");
        let out = decode(&raw(body.into_owned(), None));
        assert!(out.contains("한국어"));
    }

    #[test]
    fn falls_back_to_utf8_when_unlabeled() {
        let out = decode(&raw("hello über".as_bytes().to_vec(), None));
        assert_eq!(out, "hello über");
    }

    #[test]
    fn invalid_label_falls_back_to_utf8() {
        let out = decode(&raw(
            "plain".as_bytes().to_vec(),
            Some("text/plain; charset=bogus-enc"),
        ));
        assert_eq!(out, "plain");
    }
}
