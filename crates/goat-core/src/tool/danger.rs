use std::path::{Component, Path, PathBuf};

use tree_sitter::{Language, Node, Parser};

const SHELLS: &[&str] = &["bash", "sh", "zsh", "dash", "ksh", "fish"];
const PRIVATE: &str = ".goat";
const PRIVATE_REASON: &str = "goat keeps its sign-ins in ~/.goat, so commands may not touch it";
const SAFE_DEVICES: &[&str] = &[
    "/dev/null",
    "/dev/zero",
    "/dev/stdout",
    "/dev/stderr",
    "/dev/stdin",
    "/dev/tty",
    "/dev/random",
    "/dev/urandom",
];
const SYSTEM_DIRS: &[&str] = &[
    "/Applications",
    "/Library",
    "/System",
    "/Users",
    "/Volumes",
    "/bin",
    "/boot",
    "/dev",
    "/etc",
    "/home",
    "/lib",
    "/lib64",
    "/opt",
    "/private",
    "/proc",
    "/root",
    "/sbin",
    "/srv",
    "/sys",
    "/usr",
    "/var",
];
const DISK_PREFIXES: &[&str] = &[
    "/dev/disk",
    "/dev/rdisk",
    "/dev/sd",
    "/dev/hd",
    "/dev/vd",
    "/dev/xvd",
    "/dev/nvme",
    "/dev/mmcblk",
];
const DISKUTIL_ERASERS: &[&str] = &[
    "eraseDisk",
    "eraseVolume",
    "partitionDisk",
    "zeroDisk",
    "randomDisk",
    "secureErase",
    "reformat",
];

pub(crate) struct Checker {
    language: Language,
}

impl std::fmt::Debug for Checker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Checker")
    }
}

struct Scope<'a> {
    workspace: &'a Path,
    home: &'a Path,
}

impl Checker {
    pub(crate) fn new() -> Result<Self, tree_sitter::LanguageError> {
        let language = tree_sitter_bash::LANGUAGE.into();
        Parser::new().set_language(&language)?;
        Ok(Self { language })
    }

    pub(crate) fn check(&self, command: &str, workspace: &Path, home: &Path) -> Option<String> {
        let mut parser = Parser::new();
        if parser.set_language(&self.language).is_err() {
            return Some("the command could not be parsed".to_owned());
        }
        check_source(&mut parser, command, &Scope { workspace, home }, 0)
    }
}

fn check_source(parser: &mut Parser, source: &str, scope: &Scope<'_>, depth: u8) -> Option<String> {
    let tree = parser.parse(source, None)?;
    let mut nested = Vec::new();
    let found = visit(tree.root_node(), source, scope, &mut nested);
    if found.is_some() || depth > 2 {
        return found;
    }
    nested
        .into_iter()
        .find_map(|inner| check_source(parser, &inner, scope, depth + 1))
}

fn visit(
    node: Node<'_>,
    source: &str,
    scope: &Scope<'_>,
    nested: &mut Vec<String>,
) -> Option<String> {
    let found = match node.kind() {
        "command" => check_command(node, source, scope, nested),
        "file_redirect" => check_redirect(node, source, scope),
        "function_definition" => check_fork_bomb(node, source),
        _ => None,
    };
    if found.is_some() {
        return found;
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find_map(|child| visit(child, source, scope, nested))
}

fn check_command(
    node: Node<'_>,
    source: &str,
    scope: &Scope<'_>,
    nested: &mut Vec<String>,
) -> Option<String> {
    let words = command_words(node, source);
    (0..words.len()).find_map(|at| check_words(&words[at..], scope, nested))
}

fn check_words(words: &[String], scope: &Scope<'_>, nested: &mut Vec<String>) -> Option<String> {
    let (name, args) = words.split_first()?;
    let name = name.rsplit('/').next().unwrap_or(name);
    if name != "goat" && args.iter().any(|arg| is_private(arg, scope)) {
        return Some(PRIVATE_REASON.to_owned());
    }
    let recursive = || args.iter().any(|arg| is_recursive_flag(arg));

    if SHELLS.contains(&name) {
        if let Some(at) = args.iter().position(|arg| is_command_flag(arg)) {
            nested.extend(args.get(at + 1).cloned());
        }
        return None;
    }
    match name {
        "eval" => {
            nested.push(args.join(" "));
            None
        }
        "rm" if recursive() => protected_target(args, scope)
            .map(|target| format!("`rm` would recursively delete {target}")),
        "chmod" | "chown" | "chgrp" if recursive() => protected_target(args, scope)
            .map(|target| format!("`{name} -R` would change every file under {target}")),
        "dd" => args
            .iter()
            .filter_map(|arg| arg.strip_prefix("of="))
            .find(|target| is_disk(target))
            .map(|target| format!("`dd` would overwrite the disk {target}")),
        "diskutil" => args
            .iter()
            .find(|arg| DISKUTIL_ERASERS.contains(&arg.as_str()))
            .map(|verb| format!("`diskutil {verb}` erases a disk")),
        _ if name.starts_with("mkfs") => Some(format!("`{name}` formats a disk")),
        _ => None,
    }
}

fn is_command_flag(arg: &str) -> bool {
    arg.strip_prefix('-').is_some_and(|flags| {
        !flags.starts_with('-')
            && flags.ends_with('c')
            && flags.chars().all(|c| c.is_ascii_alphabetic())
    })
}

fn check_redirect(node: Node<'_>, source: &str, scope: &Scope<'_>) -> Option<String> {
    let destination = node.child_by_field_name("destination")?;
    let target = unquote(text(destination, source));
    if is_private(&target, scope) {
        return Some(PRIVATE_REASON.to_owned());
    }
    is_disk(&target).then(|| format!("redirecting output to {target} would overwrite a disk"))
}

fn is_private(raw: &str, scope: &Scope<'_>) -> bool {
    let raw = raw.split_once('=').map_or(raw, |(_, value)| value);
    if raw.is_empty() || raw.starts_with('-') {
        return false;
    }
    let private = normalize(&scope.home.join(PRIVATE));
    normalize(&expand(raw, scope)).starts_with(&private)
}

fn check_fork_bomb(node: Node<'_>, source: &str) -> Option<String> {
    let name = text(node.child_by_field_name("name")?, source);
    let body = node.child_by_field_name("body")?;
    has_self_pipeline(body, source, name).then(|| "this defines a fork bomb".to_owned())
}

fn has_self_pipeline(node: Node<'_>, source: &str, name: &str) -> bool {
    let mut cursor = node.walk();
    if node.kind() == "pipeline" {
        let calls = node
            .named_children(&mut cursor)
            .filter(|child| {
                child.kind() == "command"
                    && child
                        .child_by_field_name("name")
                        .is_some_and(|n| text(n, source) == name)
            })
            .count();
        if calls >= 2 {
            return true;
        }
    }
    node.named_children(&mut cursor)
        .any(|child| has_self_pipeline(child, source, name))
}

fn command_words(node: Node<'_>, source: &str) -> Vec<String> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| {
            matches!(
                child.kind(),
                "command_name"
                    | "word"
                    | "string"
                    | "raw_string"
                    | "concatenation"
                    | "simple_expansion"
                    | "expansion"
                    | "number"
            )
        })
        .map(|child| unquote(text(child, source)))
        .collect()
}

fn is_recursive_flag(arg: &str) -> bool {
    arg == "--recursive"
        || (arg.starts_with('-') && !arg.starts_with("--") && arg.contains(['r', 'R']))
}

fn protected_target(args: &[String], scope: &Scope<'_>) -> Option<String> {
    args.iter()
        .filter(|arg| !arg.starts_with('-'))
        .find(|arg| is_protected(arg, scope))
        .cloned()
}

fn is_protected(raw: &str, scope: &Scope<'_>) -> bool {
    let raw = raw
        .strip_suffix("/*")
        .or_else(|| raw.strip_suffix("/.*"))
        .unwrap_or(raw);
    let raw = if raw.is_empty() { "/" } else { raw };
    let path = normalize(&expand(raw, scope));
    let home = normalize(scope.home);
    let workspace = normalize(scope.workspace);
    path == Path::new("/")
        || SYSTEM_DIRS.iter().any(|dir| path == Path::new(dir))
        || path == home
        || (workspace.starts_with(&path) && workspace != path)
}

fn expand(raw: &str, scope: &Scope<'_>) -> PathBuf {
    let home = scope.home.to_string_lossy();
    let expanded = ["${HOME}", "$HOME"]
        .iter()
        .fold(raw.to_owned(), |acc, var| acc.replace(var, &home));
    let expanded = match expanded.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("{home}{rest}"),
        _ => expanded,
    };
    let path = PathBuf::from(expanded);
    if path.is_absolute() {
        path
    } else {
        scope.workspace.join(path)
    }
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    if out.as_os_str().is_empty() {
        out.push("/");
    }
    out
}

fn is_disk(target: &str) -> bool {
    !SAFE_DEVICES.contains(&target)
        && DISK_PREFIXES
            .iter()
            .any(|prefix| target.starts_with(prefix))
}

fn text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    source.get(node.byte_range()).unwrap_or_default()
}

fn unquote(raw: &str) -> String {
    raw.chars().filter(|c| !matches!(c, '"' | '\'')).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocked(command: &str) -> bool {
        Checker::new()
            .unwrap()
            .check(
                command,
                Path::new("/home/me/code/app"),
                Path::new("/home/me"),
            )
            .is_some()
    }

    #[test]
    fn catastrophic_commands_are_blocked() {
        for command in [
            "rm -rf /",
            "rm -r -f /",
            "rm -fr ~",
            "rm -rf ~/",
            "rm -rf \"$HOME\"",
            "rm -rf ${HOME}/*",
            "rm -rf ..",
            "rm -rf /home",
            "sudo rm -rf /",
            "cd /tmp && rm -rf /",
            "bash -c 'rm -rf /'",
            "chmod -R 777 /",
            "mkfs.ext4 /dev/sda1",
            "dd if=/dev/zero of=/dev/disk2",
            "echo x > /dev/sda",
            "diskutil eraseDisk APFS X disk2",
            ":(){ :|:& };:",
            "bash -lc 'rm -rf /'",
            "sudo -u root rm -rf /",
            "timeout 5 rm -rf /",
            "rm -rf \"$HOME\"/*",
            "dd if=/dev/zero of=\"/dev/sda\"",
            "rm -rf /usr",
            "sh -ec \"chmod -R 000 /etc\"",
        ] {
            assert!(blocked(command), "{command} should be blocked");
        }
    }

    #[test]
    fn commands_that_touch_goats_own_files_are_blocked() {
        for command in [
            "sqlite3 ~/.goat/goat.db 'select * from logins'",
            "cat $HOME/.goat/mcp.toml",
            "cp /home/me/.goat/goat.db /tmp/x",
            "cd ~/.goat",
            "tar -czf out.tgz --directory=/home/me/.goat .",
            "echo x > ~/.goat/mcp.toml",
            "cat ../../.goat/goat.db",
            "bash -c 'cat ~/.goat/goat.db'",
        ] {
            assert!(blocked(command), "{command} should be blocked");
        }
        assert!(!blocked("goat mcp list"));
        assert!(!blocked("cat .goat/mcp.toml"));
    }

    #[test]
    fn ordinary_commands_run() {
        for command in [
            "rm -rf target",
            "rm -rf ./build /tmp/cache",
            "rm -rf .",
            "rm file.txt",
            "chmod +x run.sh",
            "dd if=a of=b",
            "echo hi > /dev/null",
            "cargo test && git status",
            "f() { echo hi; }; f",
            "fib() { fib $(($1-1)); fib $(($1-2)); }",
            "rm -rf /tmp/build",
            "git commit -m 'stop running rm -rf /'",
        ] {
            assert!(!blocked(command), "{command} should run");
        }
    }
}
