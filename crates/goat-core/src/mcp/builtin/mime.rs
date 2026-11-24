use std::fmt::Write as _;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use crate::oauth;

const LINE: usize = 76;
const WORD: usize = 45;

#[derive(Debug, Default)]
pub(crate) struct Draft {
    pub(crate) to: Vec<String>,
    pub(crate) cc: Vec<String>,
    pub(crate) bcc: Vec<String>,
    pub(crate) subject: String,
    pub(crate) text: Option<String>,
    pub(crate) html: Option<String>,
    pub(crate) attachments: Vec<Attachment>,
    pub(crate) in_reply_to: Option<String>,
    pub(crate) references: Option<String>,
}

#[derive(Debug)]
pub(crate) struct Attachment {
    pub(crate) name: Option<String>,
    pub(crate) mime: String,
    pub(crate) bytes: Vec<u8>,
    pub(crate) is_inline: bool,
}

impl Draft {
    pub(crate) fn build(&self) -> Result<String, String> {
        let mut out = String::new();
        for (name, list) in [("To", &self.to), ("Cc", &self.cc), ("Bcc", &self.bcc)] {
            if !list.is_empty() {
                let addresses: Vec<String> = list
                    .iter()
                    .map(|address| encode_address(address))
                    .collect::<Result<_, _>>()?;
                header(&mut out, name, &addresses.join(", "))?;
            }
        }
        header(&mut out, "Subject", &encode_words(&self.subject)?)?;
        if let Some(id) = &self.in_reply_to {
            header(&mut out, "In-Reply-To", id)?;
        }
        if let Some(references) = &self.references {
            header(&mut out, "References", references)?;
        }
        out.push_str("MIME-Version: 1.0\r\n");
        out.push_str(&self.body());
        Ok(out)
    }

    fn body(&self) -> String {
        let text = self
            .text
            .as_deref()
            .map(|text| leaf("text/plain; charset=UTF-8", text.as_bytes(), &[]));
        let html = self
            .html
            .as_deref()
            .map(|html| leaf("text/html; charset=UTF-8", html.as_bytes(), &[]));
        let readable = match (text, html) {
            (Some(text), Some(html)) => multipart("alternative", &[text, html]),
            (Some(only), None) | (None, Some(only)) => only,
            (None, None) => leaf("text/plain; charset=UTF-8", b"", &[]),
        };
        let is_inline = |attachment: &Attachment| attachment.is_inline && self.html.is_some();
        let numbered = || self.attachments.iter().enumerate();
        let inline: Vec<_> = numbered()
            .filter(|(_, attachment)| is_inline(attachment))
            .collect();
        let attached: Vec<_> = numbered()
            .filter(|(_, attachment)| !is_inline(attachment))
            .collect();
        let related = if inline.is_empty() {
            readable
        } else {
            let mut parts = vec![readable];
            parts.extend(
                inline
                    .into_iter()
                    .map(|(index, attachment)| file(index, attachment, "inline")),
            );
            multipart("related", &parts)
        };
        if attached.is_empty() {
            return related;
        }
        let mut parts = vec![related];
        parts.extend(
            attached
                .into_iter()
                .map(|(index, attachment)| file(index, attachment, "attachment")),
        );
        multipart("mixed", &parts)
    }
}

pub(crate) fn display_name(raw: &str) -> (Option<String>, String) {
    let raw = raw.trim();
    let Some((name, rest)) = raw.rsplit_once('<') else {
        return (None, raw.to_owned());
    };
    let address = rest.trim_end_matches('>').trim().to_owned();
    let name = name.trim().trim_matches('"').trim();
    ((!name.is_empty()).then(|| name.to_owned()), address)
}

pub(crate) fn addresses(raw: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut current = String::new();
    let mut is_quoted = false;
    let mut depth = 0_u32;
    for c in raw.chars() {
        match c {
            '"' => is_quoted = !is_quoted,
            '<' if !is_quoted => depth += 1,
            '>' if !is_quoted => depth = depth.saturating_sub(1),
            ',' if !is_quoted && depth == 0 => {
                found.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    found.push(current);
    found
        .into_iter()
        .map(|item| item.trim().to_owned())
        .filter(|item| !item.is_empty())
        .collect()
}

fn header(out: &mut String, name: &str, value: &str) -> Result<(), String> {
    if value.contains(['\r', '\n']) {
        return Err(format!("the {name} field must be on one line"));
    }
    write!(out, "{name}: {value}\r\n").ok();
    Ok(())
}

fn encode_address(raw: &str) -> Result<String, String> {
    if raw.contains(['\r', '\n']) {
        return Err("an address must be on one line".to_owned());
    }
    let (name, address) = display_name(raw);
    if address.is_empty() || !address.is_ascii() || address.contains([' ', ',', '<', '>']) {
        return Err(format!("{raw} is not an email address"));
    }
    Ok(match name {
        Some(name) if name.is_ascii() => format!("\"{}\" <{address}>", name.replace('"', "")),
        Some(name) => format!("{} <{address}>", encode_words(&name)?),
        None => address,
    })
}

fn encode_words(text: &str) -> Result<String, String> {
    if text.contains(['\r', '\n']) {
        return Err("a header must be on one line".to_owned());
    }
    if text.is_ascii() {
        return Ok(text.to_owned());
    }
    let mut words = Vec::new();
    let mut chunk = String::new();
    for c in text.chars() {
        if chunk.len() + c.len_utf8() > WORD {
            words.push(std::mem::take(&mut chunk));
        }
        chunk.push(c);
    }
    words.push(chunk);
    Ok(words
        .iter()
        .map(|word| format!("=?UTF-8?B?{}?=", STANDARD.encode(word)))
        .collect::<Vec<_>>()
        .join("\r\n "))
}

fn leaf(content_type: &str, bytes: &[u8], extra: &[String]) -> String {
    let mut out = format!("Content-Type: {content_type}\r\nContent-Transfer-Encoding: base64\r\n");
    for line in extra {
        out.push_str(line);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    let encoded = STANDARD.encode(bytes);
    for line in encoded.as_bytes().chunks(LINE) {
        out.push_str(&String::from_utf8_lossy(line));
        out.push_str("\r\n");
    }
    out
}

fn file(index: usize, attachment: &Attachment, disposition: &str) -> String {
    let named = attachment.name.as_deref().map_or_else(String::new, |name| {
        format!("; filename*=UTF-8''{}", percent(name))
    });
    let mut extra = vec![format!("Content-Disposition: {disposition}{named}")];
    if disposition == "inline" {
        extra.push(format!("Content-ID: <att{index}>"));
    }
    leaf(&attachment.mime, &attachment.bytes, &extra)
}

fn multipart(kind: &str, parts: &[String]) -> String {
    let boundary = format!("goat-{}", oauth::random_token::<18>());
    let mut out = format!("Content-Type: multipart/{kind}; boundary=\"{boundary}\"\r\n\r\n");
    for part in parts {
        write!(out, "--{boundary}\r\n{part}").ok();
    }
    write!(out, "--{boundary}--\r\n").ok();
    out
}

fn percent(text: &str) -> String {
    text.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_korean_subject_and_name_are_encoded_words() {
        let draft = Draft {
            to: vec!["민오 <me@example.com>".to_owned()],
            subject: "안녕하세요".to_owned(),
            text: Some("본문".to_owned()),
            ..Draft::default()
        };

        let raw = draft.build().unwrap();

        assert!(
            raw.starts_with("To: =?UTF-8?B?66+87Jik?= <me@example.com>\r\n"),
            "{raw}"
        );
        assert!(raw.contains("Subject: =?UTF-8?B?7JWI64WV7ZWY7IS47JqU?=\r\n"));
        assert!(raw.contains("Content-Type: text/plain; charset=UTF-8\r\nContent-Transfer-Encoding: base64\r\n\r\n67O466y4\r\n"));
    }

    #[test]
    fn a_line_break_in_a_header_is_rejected() {
        let draft = Draft {
            to: vec!["me@example.com".to_owned()],
            subject: "hi\r\nBcc: evil@example.com".to_owned(),
            ..Draft::default()
        };

        assert!(draft.build().is_err());
    }

    #[test]
    fn inline_and_regular_attachments_nest_in_related_and_mixed_parts() {
        let draft = Draft {
            to: vec!["me@example.com".to_owned()],
            subject: "s".to_owned(),
            html: Some("<img src=\"cid:att1\">".to_owned()),
            attachments: vec![
                Attachment {
                    name: Some("보고서.pdf".to_owned()),
                    mime: "application/pdf".to_owned(),
                    bytes: b"%PDF".to_vec(),
                    is_inline: false,
                },
                Attachment {
                    name: None,
                    mime: "image/png".to_owned(),
                    bytes: b"png".to_vec(),
                    is_inline: true,
                },
            ],
            ..Draft::default()
        };

        let raw = draft.build().unwrap();

        let mixed = raw.find("multipart/mixed").unwrap();
        let related = raw.find("multipart/related").unwrap();
        assert!(mixed < related);
        assert!(raw.contains("Content-Disposition: inline\r\nContent-ID: <att1>"));
        assert!(raw.contains("filename*=UTF-8''%EB%B3%B4%EA%B3%A0%EC%84%9C.pdf"));
    }

    #[test]
    fn address_lists_split_outside_quotes_and_brackets() {
        let found = addresses("\"Kim, Min\" <a@x.com>, b@y.com,<c@z.com>");

        assert_eq!(found, ["\"Kim, Min\" <a@x.com>", "b@y.com", "<c@z.com>"]);
        assert_eq!(
            display_name("\"Kim, Min\" <a@x.com>"),
            (Some("Kim, Min".to_owned()), "a@x.com".to_owned())
        );
    }
}
