#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GitVerb {
    Commit,
    Amend,
    Push,
    ForcePush,
    Pull,
    Fetch,
    Merge,
    Rebase,
    Branch,
    Switch,
    Tag,
    Stash,
    Reset,
    HardReset,
    Revert,
    CherryPick,
    PrCreate,
    PrMerge,
    PrClose,
}

impl GitVerb {
    pub fn moves_head(self) -> bool {
        matches!(
            self,
            Self::Commit
                | Self::Amend
                | Self::Merge
                | Self::Rebase
                | Self::Pull
                | Self::Revert
                | Self::CherryPick
        )
    }

    pub fn touches_pull_request(self) -> bool {
        matches!(self, Self::PrCreate | Self::PrMerge | Self::PrClose)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitOp {
    pub verb: GitVerb,
    pub target: Option<String>,
    pub remote: Option<String>,
    pub number: Option<u64>,
}

impl GitOp {
    fn bare(verb: GitVerb) -> Self {
        Self {
            verb,
            target: None,
            remote: None,
            number: None,
        }
    }

    fn with_target(verb: GitVerb, target: Option<String>) -> Self {
        Self {
            target,
            ..Self::bare(verb)
        }
    }
}

enum Step {
    Event(GitOp),
    Ignored,
    Opaque,
}

pub fn classify(command: &str) -> Option<Vec<GitOp>> {
    let segments = split(command)?;
    let mut ops: Vec<GitOp> = Vec::new();
    for segment in segments {
        match segment_step(&segment) {
            Step::Event(op) => ops.push(op),
            Step::Ignored => {}
            Step::Opaque => return None,
        }
    }
    if ops.is_empty() {
        return None;
    }
    let mut seen = Vec::new();
    for op in &ops {
        if seen.contains(&op.verb) {
            return None;
        }
        seen.push(op.verb);
    }
    Some(ops)
}

fn split(command: &str) -> Option<Vec<Vec<String>>> {
    let mut segments = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut has_word = false;
    let mut quote: Option<char> = None;
    let mut depth = 0usize;
    let mut chars = command.chars().peekable();

    while let Some(c) = chars.next() {
        if quote == Some('\'') {
            word.push(c);
            has_word = true;
            if c == '\'' {
                quote = None;
            }
            continue;
        }
        match c {
            '\\' => {
                if let Some(next) = chars.next() {
                    word.push(next);
                    has_word = true;
                }
            }
            '\'' | '"' if quote.is_none() => {
                quote = Some(c);
                word.push(c);
                has_word = true;
            }
            c if Some(c) == quote => {
                quote = None;
                word.push(c);
                has_word = true;
            }
            '`' => return None,
            '$' if quote != Some('\'') && chars.peek() == Some(&'(') => {
                chars.next();
                depth += 1;
                word.push_str("$(");
                has_word = true;
            }
            ')' if depth > 0 => {
                depth -= 1;
                word.push(c);
            }
            _ if quote.is_some() || depth > 0 => {
                word.push(c);
                has_word = true;
            }
            '(' | ';' | '|' | '<' | '>' => return None,
            '&' => {
                if chars.peek() == Some(&'&') {
                    chars.next();
                    flush(&mut word, &mut has_word, &mut words);
                    segments.push(std::mem::take(&mut words));
                } else {
                    return None;
                }
            }
            c if c.is_whitespace() => flush(&mut word, &mut has_word, &mut words),
            _ => {
                word.push(c);
                has_word = true;
            }
        }
    }
    if quote.is_some() || depth > 0 {
        return None;
    }
    flush(&mut word, &mut has_word, &mut words);
    segments.push(words);
    Some(segments)
}

fn flush(word: &mut String, has_word: &mut bool, words: &mut Vec<String>) {
    if *has_word {
        words.push(unquote(&std::mem::take(word)));
        *has_word = false;
    } else {
        word.clear();
    }
}

fn unquote(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut quote: Option<char> = None;
    for c in raw.chars() {
        match c {
            '\'' | '"' if quote.is_none() => quote = Some(c),
            c if Some(c) == quote => quote = None,
            c => out.push(c),
        }
    }
    out
}

const RELOCATING: &[&str] = &[
    "cd", "pushd", "popd", "chdir", "exec", "eval", "source", ".", "sh", "bash", "zsh", "fish",
    "env", "xargs", "nohup", "time", "sudo",
];

const GIT_GLOBAL_REJECT: &[&str] = &["-C", "--git-dir", "--work-tree", "--exec-path", "-c"];

const GIT_HARMLESS: &[&str] = &[
    "add",
    "blame",
    "bisect",
    "cat-file",
    "check-ref-format",
    "clean",
    "clone",
    "config",
    "count-objects",
    "describe",
    "diff",
    "diff-tree",
    "for-each-ref",
    "fsck",
    "gc",
    "grep",
    "help",
    "init",
    "log",
    "ls-files",
    "ls-remote",
    "merge-base",
    "mv",
    "name-rev",
    "notes",
    "prune",
    "reflog",
    "remote",
    "restore",
    "rev-list",
    "rev-parse",
    "rm",
    "shortlog",
    "show",
    "sparse-checkout",
    "status",
    "submodule",
    "symbolic-ref",
    "verify-commit",
    "version",
    "whatchanged",
    "worktree",
];

const GH_PR_HARMLESS: &[&str] = &["view", "list", "checks", "diff", "status"];

fn segment_step(words: &[String]) -> Step {
    let Some(head) = words.first() else {
        return Step::Ignored;
    };
    if head.contains('=') || RELOCATING.contains(&head.as_str()) {
        return Step::Opaque;
    }
    match head.as_str() {
        "git" => git_step(&words[1..]),
        "gh" => gh_step(&words[1..]),
        _ => Step::Ignored,
    }
}

fn step(op: Option<GitOp>) -> Step {
    op.map_or(Step::Ignored, Step::Event)
}

fn git_step(rest: &[String]) -> Step {
    let Some(subcommand) = rest.first() else {
        return Step::Opaque;
    };
    if GIT_GLOBAL_REJECT.contains(&subcommand.as_str()) || subcommand.starts_with('-') {
        return Step::Opaque;
    }
    let args = &rest[1..];
    if GIT_HARMLESS.contains(&subcommand.as_str()) {
        return match subcommand.as_str() {
            "branch" => step(branch_op(args)),
            _ => Step::Ignored,
        };
    }
    let op = match subcommand.as_str() {
        "commit" => {
            let verb = if has_flag(args, &["--amend"]) {
                GitVerb::Amend
            } else {
                GitVerb::Commit
            };
            GitOp::bare(verb)
        }
        "push" => push_op(args),
        "pull" => GitOp::with_target(GitVerb::Pull, positional(args, 0)),
        "fetch" => GitOp::with_target(GitVerb::Fetch, positional(args, 0)),
        "merge" => GitOp::with_target(GitVerb::Merge, positional(args, 0)),
        "rebase" => GitOp::with_target(GitVerb::Rebase, positional(args, 0)),
        "revert" => GitOp::with_target(GitVerb::Revert, positional(args, 0)),
        "cherry-pick" => GitOp::with_target(GitVerb::CherryPick, positional(args, 0)),
        "switch" => match created_branch(args, &["-c", "-C"]) {
            Some(name) => GitOp::with_target(GitVerb::Branch, Some(name)),
            None if has_flag(args, &["-c", "-C"]) => GitOp::bare(GitVerb::Branch),
            None => GitOp::with_target(GitVerb::Switch, positional(args, 0)),
        },
        "checkout" => return step(checkout_op(args)),
        "tag" => return step(tag_op(args)),
        "stash" => GitOp::bare(GitVerb::Stash),
        "reset" => {
            let verb = if has_flag(args, &["--hard"]) {
                GitVerb::HardReset
            } else {
                GitVerb::Reset
            };
            GitOp::with_target(verb, positional(args, 0))
        }
        _ => return Step::Opaque,
    };
    Step::Event(op)
}

fn push_op(args: &[String]) -> GitOp {
    let verb = if has_flag(args, &["-f", "--force"]) || has_prefix(args, "--force-with-lease") {
        GitVerb::ForcePush
    } else {
        GitVerb::Push
    };
    GitOp {
        verb,
        target: positional(args, 1),
        remote: positional(args, 0),
        number: None,
    }
}

fn created_branch(args: &[String], flags: &[&str]) -> Option<String> {
    flags.iter().find_map(|flag| flag_value(args, flag))
}

fn checkout_op(args: &[String]) -> Option<GitOp> {
    if args.iter().any(|arg| arg == "--") {
        return None;
    }
    if has_flag(args, &["-b", "-B"]) {
        return Some(GitOp::with_target(
            GitVerb::Branch,
            created_branch(args, &["-b", "-B"]),
        ));
    }
    let target = positional(args, 0)?;
    if positional(args, 1).is_some() {
        return None;
    }
    Some(GitOp::with_target(GitVerb::Switch, Some(target)))
}

fn branch_op(args: &[String]) -> Option<GitOp> {
    if args.iter().any(|arg| arg.starts_with('-')) {
        return None;
    }
    let target = positional(args, 0)?;
    if positional(args, 1).is_some() {
        return None;
    }
    Some(GitOp::with_target(GitVerb::Branch, Some(target)))
}

fn tag_op(args: &[String]) -> Option<GitOp> {
    if has_flag(args, &["-d", "-l", "--list", "--delete"]) {
        return None;
    }
    let target = positional(args, 0)?;
    Some(GitOp::with_target(GitVerb::Tag, Some(target)))
}

fn gh_step(rest: &[String]) -> Step {
    let Some(group) = rest.first() else {
        return Step::Ignored;
    };
    if group != "pr" {
        return Step::Ignored;
    }
    let Some(action) = rest.get(1) else {
        return Step::Opaque;
    };
    if GH_PR_HARMLESS.contains(&action.as_str()) {
        return Step::Ignored;
    }
    let args = &rest[2..];
    let verb = match action.as_str() {
        "create" => GitVerb::PrCreate,
        "merge" => GitVerb::PrMerge,
        "close" => GitVerb::PrClose,
        _ => return Step::Opaque,
    };
    Step::Event(GitOp {
        verb,
        target: flag_value(args, "--head"),
        remote: None,
        number: positional(args, 0).and_then(|arg| arg.parse().ok()),
    })
}

fn has_flag(args: &[String], names: &[&str]) -> bool {
    args.iter().any(|arg| names.contains(&arg.as_str()))
}

fn has_prefix(args: &[String], prefix: &str) -> bool {
    args.iter().any(|arg| arg.starts_with(prefix))
}

const VALUE_FLAGS: &[&str] = &[
    "-o",
    "--push-option",
    "--base",
    "--head",
    "--title",
    "--body",
    "--body-file",
    "--label",
    "--assignee",
    "--reviewer",
    "--milestone",
    "--project",
    "--repo",
    "-m",
    "-F",
    "-t",
    "-b",
    "-B",
    "-c",
    "-C",
    "--onto",
    "--strategy",
    "--message",
    "--file",
];

fn positionals(args: &[String]) -> Vec<&String> {
    let mut out = Vec::new();
    let mut skip = false;
    for arg in args {
        if skip {
            skip = false;
            continue;
        }
        if arg == "--" {
            continue;
        }
        if arg.starts_with('-') {
            if VALUE_FLAGS.contains(&arg.as_str()) {
                skip = true;
            }
            continue;
        }
        out.push(arg);
    }
    out
}

fn positional(args: &[String], index: usize) -> Option<String> {
    let value = positionals(args).get(index).copied()?;
    if value.contains('$') || value.is_empty() {
        return None;
    }
    Some(value.clone())
}

fn flag_value(args: &[String], name: &str) -> Option<String> {
    let inline = format!("{name}=");
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == name {
            let value = iter.next()?;
            return (!value.contains('$')).then(|| value.clone());
        }
        if let Some(value) = arg.strip_prefix(&inline) {
            return (!value.contains('$')).then(|| value.to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{GitVerb, classify};

    fn verbs(command: &str) -> Option<Vec<GitVerb>> {
        classify(command).map(|ops| ops.iter().map(|op| op.verb).collect())
    }

    #[test]
    fn add_commit_push_chain_drops_the_prep_step() {
        assert_eq!(
            verbs(r#"git add -A && git commit -m "feat: x" && git push"#),
            Some(vec![GitVerb::Commit, GitVerb::Push])
        );
    }

    #[test]
    fn push_carries_remote_and_branch() {
        let ops = classify("git push -u origin feat/git-ui").unwrap();
        assert_eq!(ops[0].verb, GitVerb::Push);
        assert_eq!(ops[0].remote.as_deref(), Some("origin"));
        assert_eq!(ops[0].target.as_deref(), Some("feat/git-ui"));
    }

    #[test]
    fn destructive_flags_get_their_own_verb() {
        assert_eq!(
            verbs("git commit --amend --no-edit"),
            Some(vec![GitVerb::Amend])
        );
        assert_eq!(verbs("git push --force"), Some(vec![GitVerb::ForcePush]));
        assert_eq!(
            verbs("git push --force-with-lease origin main"),
            Some(vec![GitVerb::ForcePush])
        );
        assert_eq!(
            verbs("git reset --hard origin/main"),
            Some(vec![GitVerb::HardReset])
        );
    }

    #[test]
    fn branch_creation_is_distinct_from_switching() {
        assert_eq!(verbs("git switch -c feat/x"), Some(vec![GitVerb::Branch]));
        assert_eq!(verbs("git checkout -b feat/x"), Some(vec![GitVerb::Branch]));
        assert_eq!(verbs("git switch main"), Some(vec![GitVerb::Switch]));
        assert_eq!(verbs("git checkout main"), Some(vec![GitVerb::Switch]));
    }

    #[test]
    fn a_created_branch_takes_its_name_from_the_flag() {
        for command in [
            "git switch -c feat/x",
            "git switch -C feat/x",
            "git checkout -b feat/x",
            "git checkout -B feat/x",
        ] {
            let ops = classify(command).unwrap();
            assert_eq!(ops[0].target.as_deref(), Some("feat/x"), "{command}");
        }
        let ops = classify("git switch main").unwrap();
        assert_eq!(ops[0].target.as_deref(), Some("main"));
    }

    #[test]
    fn pr_number_comes_from_the_argument() {
        let ops = classify("gh pr merge 58 --squash").unwrap();
        assert_eq!(ops[0].verb, GitVerb::PrMerge);
        assert_eq!(ops[0].number, Some(58));
    }

    #[test]
    fn a_body_substitution_survives_because_it_is_quoted() {
        let ops =
            classify(r#"gh pr create --title "feat: x" --body "$(cat <<'EOF' - a && b EOF )""#)
                .unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].verb, GitVerb::PrCreate);
    }

    #[test]
    fn read_only_git_is_not_an_event() {
        for command in [
            "git status",
            "git log --oneline -5",
            "git diff HEAD",
            "git rev-parse HEAD",
            "git branch",
            "git branch -a",
            "gh pr view 59",
            "gh pr checks 59 --watch",
        ] {
            assert_eq!(classify(command), None, "{command}");
        }
    }

    #[test]
    fn relocating_and_opaque_commands_degrade() {
        for command in [
            "cd other-repo && git commit -m x",
            "git -C ../other push",
            "GIT_DIR=/tmp/x git commit -m y",
            "for b in a b; do git push origin $b; done",
            "git push; git commit -m x",
            "git commit -m x || git push",
            "git log | head -5",
            "(git commit -m x)",
            "sh -c 'git push'",
            "git commit -m `date`",
            "git push > out.txt",
        ] {
            assert_eq!(classify(command), None, "{command}");
        }
    }

    #[test]
    fn a_repeated_verb_cannot_be_attributed() {
        assert_eq!(classify("git commit -m A && git commit -m B"), None);
    }

    #[test]
    fn unknown_subcommands_degrade_rather_than_guess() {
        assert_eq!(classify("git am patch.diff"), None);
        assert_eq!(classify("git commit -m x && git am patch.diff"), None);
    }

    #[test]
    fn a_non_git_step_is_allowed_between_git_steps() {
        assert_eq!(
            verbs(r#"cargo fmt --all && git commit -m "style" && git push"#),
            Some(vec![GitVerb::Commit, GitVerb::Push])
        );
    }

    #[test]
    fn a_message_containing_the_separator_does_not_split() {
        assert_eq!(
            verbs(r#"git commit -m "fix a && b" && git push"#),
            Some(vec![GitVerb::Commit, GitVerb::Push])
        );
    }

    #[test]
    fn checkout_of_paths_is_not_a_switch() {
        assert_eq!(classify("git checkout -- src/lib.rs"), None);
        assert_eq!(classify("git checkout main src/lib.rs"), None);
    }
}
