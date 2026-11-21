use std::time::Duration;

use similar::{ChangeTag, TextDiff};

use crate::message::{Diff, DiffLine, Sign};

const CONTEXT: usize = 1;
const SHOWN: usize = 16;
const LINE_CHARS: usize = 200;
const TIMEOUT: Duration = Duration::from_millis(200);

pub(crate) fn between(before: &str, after: &str) -> Diff {
    let diff = TextDiff::configure()
        .timeout(TIMEOUT)
        .diff_lines(before, after);
    let mut out = Diff::default();
    for (index, group) in diff.grouped_ops(CONTEXT).iter().enumerate() {
        if index > 0 && out.lines.len() < SHOWN {
            out.lines.push(DiffLine {
                sign: Sign::Gap,
                number: 0,
                text: "".into(),
            });
        }
        for op in group {
            for change in diff.iter_changes(op) {
                let (sign, index) = match change.tag() {
                    ChangeTag::Equal => (Sign::Same, change.new_index()),
                    ChangeTag::Insert => {
                        out.added += 1;
                        (Sign::Added, change.new_index())
                    }
                    ChangeTag::Delete => {
                        out.removed += 1;
                        (Sign::Removed, change.old_index())
                    }
                };
                if out.lines.len() >= SHOWN {
                    out.hidden += 1;
                    continue;
                }
                out.lines.push(DiffLine {
                    sign,
                    number: index.map_or(0, |at| u32::try_from(at + 1).unwrap_or(u32::MAX)),
                    text: clip(change.value()).into(),
                });
            }
        }
    }
    out
}

fn clip(line: &str) -> &str {
    let line = line.trim_end_matches('\n').trim_end_matches('\r');
    match line.char_indices().nth(LINE_CHARS) {
        Some((cut, _)) => &line[..cut],
        None => line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(diff: &Diff) -> Vec<String> {
        diff.lines
            .iter()
            .map(|line| {
                let sign = match line.sign {
                    Sign::Same => ' ',
                    Sign::Added => '+',
                    Sign::Removed => '-',
                    Sign::Gap => '~',
                };
                format!("{sign}{} {}", line.number, line.text)
            })
            .collect()
    }

    #[test]
    fn a_changed_line_shows_with_one_line_around_it() {
        let diff = between("a\nb\nc\nd\n", "a\nb\nC\nd\n");

        assert_eq!((diff.added, diff.removed), (1, 1));
        assert_eq!(shown(&diff), [" 2 b", "-3 c", "+3 C", " 4 d"]);
    }

    #[test]
    fn distant_changes_are_split_by_a_gap() {
        let before = "1\n2\n3\n4\n5\n6\n7\n8\n";
        let after = "one\n2\n3\n4\n5\n6\n7\neight\n";

        let diff = between(before, after);

        assert_eq!(
            shown(&diff),
            ["-1 1", "+1 one", " 2 2", "~0 ", " 7 7", "-8 8", "+8 eight"]
        );
    }

    #[test]
    fn a_new_file_is_all_added_and_long_ones_are_cut() {
        let content = (1..=40).fold(String::new(), |text, n| text + &n.to_string() + "\n");

        let diff = between("", &content);

        assert_eq!((diff.added, diff.removed), (40, 0));
        assert_eq!(diff.lines.len(), SHOWN);
        assert_eq!(diff.hidden, 40 - 16);
    }
}
