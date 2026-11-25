use std::io::Read;
use std::path::Path;

const FILE_BYTES: u64 = 1024 * 1024;

pub(crate) fn read(location: &Path) -> Result<String, String> {
    let file = std::fs::File::open(location)
        .map_err(|err| format!("could not read {}: {err}", location.display()))?;
    let mut bytes = Vec::new();
    file.take(FILE_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|err| format!("could not read {}: {err}", location.display()))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub(crate) fn split(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start_matches('\u{feff}');
    let rest = text.strip_prefix("---")?;
    let rest = rest
        .strip_prefix('\n')
        .or_else(|| rest.strip_prefix("\r\n"))?;
    let mut at = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return Some((&rest[..at], &rest[at + line.len()..]));
        }
        at += line.len();
    }
    None
}

pub(crate) fn fields(front: &str) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    let mut lines = front.lines().peekable();
    while let Some(line) = lines.next() {
        if line.starts_with([' ', '\t', '#', '-']) {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        let block: Option<&str> = match value {
            ">" | ">-" | ">+" => Some(" "),
            "|" | "|-" | "|+" => Some("\n"),
            _ => None,
        };
        let value = if let Some(join) = block {
            let mut parts = Vec::new();
            while let Some(next) =
                lines.next_if(|next| next.is_empty() || next.starts_with([' ', '\t']))
            {
                parts.push(next.trim());
            }
            parts.join(join).trim().to_owned()
        } else {
            scalar(value)
        };
        fields.push((key.trim().to_owned(), value));
    }
    fields
}

fn scalar(value: &str) -> String {
    if let Some(inner) = value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        return inner.replace("\\\"", "\"").replace("\\\\", "\\");
    }
    if let Some(inner) = value
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
    {
        return inner.replace("''", "'");
    }
    match value.find(" #") {
        Some(comment) => value[..comment].trim_end().to_owned(),
        None => value.to_owned(),
    }
}
