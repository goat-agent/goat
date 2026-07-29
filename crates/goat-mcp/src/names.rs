pub fn sanitize_component(input: &str) -> String {
    let mut output = String::new();
    let mut last_was_sep = false;
    for ch in input.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            output.push(ch);
            last_was_sep = false;
        } else if !last_was_sep && !output.is_empty() {
            output.push('_');
            last_was_sep = true;
        }
    }
    while output.ends_with('_') {
        output.pop();
    }
    if output.is_empty() {
        "unnamed".to_owned()
    } else {
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowercases_and_collapses_separators() {
        assert_eq!(sanitize_component("File System"), "file_system");
        assert_eq!(sanitize_component("Read.Path"), "read_path");
        assert_eq!(sanitize_component("a---b"), "a_b");
    }

    #[test]
    fn trailing_separators_are_trimmed() {
        assert_eq!(sanitize_component("name!!!"), "name");
    }

    #[test]
    fn unrepresentable_input_gets_a_placeholder() {
        assert_eq!(sanitize_component("한글"), "unnamed");
        assert_eq!(sanitize_component("!!!"), "unnamed");
    }
}
