const INPUT_OPEN: &str = "<shell-input>\n";
const INPUT_CLOSE: &str = "\n</shell-input>\n";
const OUTPUT_OPEN: &str = "<shell-output>\n";
const OUTPUT_CLOSE: &str = "\n</shell-output>";

pub(crate) fn encode(command: &str, output: &str) -> String {
    format!("{INPUT_OPEN}{command}{INPUT_CLOSE}{OUTPUT_OPEN}{output}{OUTPUT_CLOSE}")
}

pub(crate) fn decode(text: &str) -> Option<(String, String)> {
    let rest = text.strip_prefix(INPUT_OPEN)?;
    let (command, rest) = rest.split_once(INPUT_CLOSE)?;
    let output = rest.strip_prefix(OUTPUT_OPEN)?.strip_suffix(OUTPUT_CLOSE)?;
    Some((command.to_owned(), output.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};

    #[test]
    fn roundtrip_simple() {
        let encoded = encode("echo hello", "hello");
        assert_eq!(
            decode(&encoded),
            Some(("echo hello".to_owned(), "hello".to_owned()))
        );
    }

    #[test]
    fn roundtrip_multiline() {
        let command = "for i in 1 2; do\n  echo $i\ndone";
        let output = "1\n2";
        assert_eq!(
            decode(&encode(command, output)),
            Some((command.to_owned(), output.to_owned()))
        );
    }

    #[test]
    fn roundtrip_empty_output() {
        assert_eq!(
            decode(&encode("true", "")),
            Some(("true".to_owned(), String::new()))
        );
    }

    #[test]
    fn decode_rejects_plain_text() {
        assert_eq!(decode("just a normal message"), None);
        assert_eq!(decode("<shell-input>\nls"), None);
        assert_eq!(decode(""), None);
    }
}
