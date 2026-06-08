//! Shared command-safety guard for tools that spawn OS processes.
//!
//! [`deny_reason`] returns `Some(&str)` when a shell command string should be
//! rejected, and `None` when it is safe to pass to a process spawner.

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShellToken {
    Word(String),
    Op,
}

pub fn shell_tokens(command: &str) -> Vec<ShellToken> {
    let mut chars = command.chars().peekable();
    let mut tokens = Vec::new();
    let mut word = String::new();

    while let Some(ch) = chars.next() {
        match ch {
            '\'' => {
                for next in chars.by_ref() {
                    if next == '\'' {
                        break;
                    }
                    word.push(next);
                }
            }
            '"' => {
                while let Some(next) = chars.next() {
                    if next == '"' {
                        break;
                    }
                    if next == '\\' {
                        if let Some(escaped) = chars.next() {
                            word.push(escaped);
                        }
                    } else {
                        word.push(next);
                    }
                }
            }
            '\\' => {
                if let Some(next) = chars.next() {
                    word.push(next);
                }
            }
            ' ' | '\t' | '\r' => {
                push_word(&mut tokens, &mut word);
            }
            '\n' | ';' | '|' | '&' => {
                push_word(&mut tokens, &mut word);
                if matches!(ch, '|' | '&') && chars.peek() == Some(&ch) {
                    chars.next();
                }
                tokens.push(ShellToken::Op);
            }
            '#' if word.is_empty() => break,
            _ => word.push(ch),
        }
    }
    push_word(&mut tokens, &mut word);
    tokens
}

pub fn push_word(tokens: &mut Vec<ShellToken>, word: &mut String) {
    if !word.is_empty() {
        tokens.push(ShellToken::Word(std::mem::take(word)));
    }
}

pub fn deny_reason(command: &str) -> Option<&'static str> {
    let compact = command.split_whitespace().collect::<String>();
    let compact_lower = compact.to_ascii_lowercase();
    if compact_lower.contains(":(){:|:&};:") {
        return Some("fork bomb pattern");
    }

    let tokens = shell_tokens(command);
    let words: Vec<&str> = tokens
        .iter()
        .filter_map(|token| match token {
            ShellToken::Word(word) => Some(word.as_str()),
            ShellToken::Op => None,
        })
        .collect();

    if words.iter().any(|word| {
        matches!(
            command_basename(word).to_ascii_lowercase().as_str(),
            "sudo" | "su" | "doas"
        )
    }) {
        return Some("privilege escalation");
    }

    if words.iter().any(|word| {
        let name = command_basename(word).to_ascii_lowercase();
        name == "mkswap" || name.starts_with("mkfs")
    }) {
        return Some("filesystem formatting");
    }

    if contains_destructive_dd(&tokens) {
        return Some("destructive raw disk write");
    }

    if contains_broad_recursive_rm(&tokens) {
        return Some("broad recursive deletion");
    }

    if contains_broad_recursive_permission_change(&tokens) {
        return Some("broad permission/ownership change");
    }

    None
}

pub fn contains_broad_recursive_rm(tokens: &[ShellToken]) -> bool {
    for (idx, token) in tokens.iter().enumerate() {
        let ShellToken::Word(word) = token else {
            continue;
        };
        if command_basename(word) != "rm" {
            continue;
        }
        let args = words_until_op(&tokens[idx + 1..]);
        if rm_args_are_broad_recursive_delete(&args) {
            return true;
        }
    }
    false
}

pub fn rm_args_are_broad_recursive_delete(args: &[&str]) -> bool {
    let mut recursive = false;
    let mut after_options = false;
    let mut targets = Vec::new();

    for arg in args {
        if !after_options && *arg == "--" {
            after_options = true;
            continue;
        }
        if !after_options && arg.starts_with('-') && *arg != "-" {
            if rm_option_is_recursive(arg) {
                recursive = true;
            }
            continue;
        }
        targets.push(*arg);
    }

    recursive
        && targets
            .iter()
            .any(|target| dangerous_recursive_target(target))
}

pub fn rm_option_is_recursive(arg: &str) -> bool {
    matches!(arg, "-r" | "-R" | "--recursive" | "-d")
        || (arg.starts_with('-')
            && !arg.starts_with("--")
            && arg.chars().any(|ch| matches!(ch, 'r' | 'R')))
}

pub fn contains_destructive_dd(tokens: &[ShellToken]) -> bool {
    for (idx, token) in tokens.iter().enumerate() {
        let ShellToken::Word(word) = token else {
            continue;
        };
        if command_basename(word) != "dd" {
            continue;
        }
        let args = words_until_op(&tokens[idx + 1..]);
        if args.iter().any(|arg| {
            arg.strip_prefix("of=").is_some_and(|target| {
                target.starts_with("/dev/") || dangerous_recursive_target(target)
            })
        }) {
            return true;
        }
    }
    false
}

pub fn contains_broad_recursive_permission_change(tokens: &[ShellToken]) -> bool {
    for (idx, token) in tokens.iter().enumerate() {
        let ShellToken::Word(word) = token else {
            continue;
        };
        let name = command_basename(word);
        if !matches!(name, "chmod" | "chown") {
            continue;
        }
        let args = words_until_op(&tokens[idx + 1..]);
        let recursive = args.iter().any(|arg| {
            matches!(*arg, "-R" | "--recursive")
                || (arg.starts_with('-')
                    && !arg.starts_with("--")
                    && arg.chars().any(|ch| ch == 'R'))
        });
        if recursive && args.iter().any(|arg| dangerous_recursive_target(arg)) {
            return true;
        }
    }
    false
}

pub fn words_until_op(tokens: &[ShellToken]) -> Vec<&str> {
    tokens
        .iter()
        .take_while(|token| !matches!(token, ShellToken::Op))
        .filter_map(|token| match token {
            ShellToken::Word(word) => Some(word.as_str()),
            ShellToken::Op => None,
        })
        .collect()
}

pub fn command_basename(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

pub fn dangerous_recursive_target(target: &str) -> bool {
    let target = target.trim();
    if target.is_empty() {
        return false;
    }
    let lower = target.to_ascii_lowercase();
    let normalized = trim_redundant_trailing_slashes(lower.as_str());

    if matches!(
        normalized.as_str(),
        "/" | "/*"
            | "/."
            | "/.."
            | "."
            | "./"
            | "./*"
            | ".."
            | "../"
            | "../*"
            | "*"
            | ".*"
            | "$home"
            | "${home}"
            | "$home/*"
            | "${home}/*"
            | "$pwd"
            | "${pwd}"
            | "$pwd/*"
            | "${pwd}/*"
            | "~"
            | "~/*"
    ) {
        return true;
    }

    if is_home_or_home_contents(&normalized) || is_sensitive_home_path(&normalized) {
        return true;
    }

    false
}

pub fn trim_redundant_trailing_slashes(s: &str) -> String {
    if s == "/" {
        return s.to_string();
    }
    let trimmed = s.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn is_home_or_home_contents(target: &str) -> bool {
    let Ok(home) = std::env::var("HOME") else {
        return false;
    };
    let home = trim_redundant_trailing_slashes(home.to_ascii_lowercase().as_str());
    target == home || target == format!("{home}/*")
}

pub fn is_sensitive_home_path(target: &str) -> bool {
    const SENSITIVE: &[&str] = &[".goat", ".ssh", ".gnupg", ".aws", ".config"];
    SENSITIVE.iter().any(|name| {
        let tilde = format!("~/{name}");
        let dollar = format!("$home/{name}");
        let braced = format!("${{home}}/{name}");
        target == tilde
            || target.starts_with(&(tilde + "/"))
            || target == dollar
            || target.starts_with(&(dollar + "/"))
            || target == braced
            || target.starts_with(&(braced + "/"))
            || std::env::var("HOME").is_ok_and(|home| {
                let home = trim_redundant_trailing_slashes(home.to_ascii_lowercase().as_str());
                let absolute = format!("{home}/{name}");
                target == absolute || target.starts_with(&(absolute + "/"))
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denies_catastrophic_commands() {
        assert!(deny_reason("rm -rf /").is_some());
        assert!(deny_reason("rm -rf -- /").is_some());
        assert!(deny_reason("rm -rf ~").is_some());
        assert!(deny_reason("rm -rf $HOME").is_some());
        assert!(deny_reason("rm -rf .").is_some());
        assert!(deny_reason("rm -rf ./").is_some());
        assert!(deny_reason("rm -rf *").is_some());
        assert!(deny_reason("rm -rf ~/.goat").is_some());
        assert!(deny_reason("rm -rf ~/.ssh").is_some());
        assert!(deny_reason("sudo whoami").is_some());
        assert!(deny_reason("doas whoami").is_some());
        assert!(deny_reason("mkfs.ext4 /dev/sda").is_some());
        assert!(deny_reason("dd if=image of=/dev/disk4").is_some());
        assert!(deny_reason("chmod -R 777 /").is_some());
        assert!(deny_reason("echo ok").is_none());
    }

    #[test]
    fn allows_specific_project_local_deletions() {
        assert!(deny_reason("rm -rf .omc").is_none());
        assert!(deny_reason("rm -rf .omx").is_none());
        assert!(deny_reason("rm -rf target").is_none());
        assert!(deny_reason("rm -rf /tmp/goat-scratch").is_none());
        assert!(deny_reason("chmod -R 700 .omc").is_none());
    }
}
