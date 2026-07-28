use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "goat",
    version,
    about = "goat — a personal AI agent with a coding capability"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(clap::Args, Debug, Default)]
#[command(about = "`goat code`: launch the coding TUI, or run one of its subcommands.")]
pub struct CodeArgs {
    #[arg(long)]
    pub print_log_path: bool,

    #[arg(long, short = 'w', value_name = "NAME")]
    pub worktree: Option<String>,

    #[arg(long, short = 'c')]
    pub r#continue: bool,

    #[arg(long)]
    pub headless: bool,

    #[arg(long, short = 'p')]
    pub print: bool,

    #[arg(long, value_name = "NAME", default_value = "json")]
    pub protocol: String,

    #[command(subcommand)]
    pub command: Option<CodeCommand>,
}

#[derive(Subcommand, Debug)]
pub enum CodeCommand {
    #[command(subcommand, about = "Manage git worktrees")]
    Worktree(WorktreeCommand),
    #[command(subcommand, about = "Manage search providers")]
    Search(SearchCommand),
}

#[derive(Subcommand)]
pub enum Command {
    #[command(about = "Launch the coding TUI (or a code subcommand)")]
    Code(CodeArgs),
    #[command(subcommand, about = "Manage agents")]
    Agent(goat_agent::cli::agent::Cmd),
    #[command(
        subcommand,
        about = "Connect external services (Linear, …) shared by every agent"
    )]
    Integration(goat_agent::cli::integration::ConnectCmd),
    #[command(about = "First-run setup — providers, then an optional agent")]
    Setup,
    #[command(about = "Parse every config file and report what would load")]
    Doctor(goat_agent::cli::doctor::Args),
    #[command(
        subcommand,
        about = "Manage model providers",
        after_help = "Common flow:
  goat provider list
  goat provider info <provider>
  goat provider login <provider> --key <key>
  goat provider logout <provider> <account>"
    )]
    Provider(ProviderCommand),
    #[command(about = "Update goat")]
    Update {
        #[arg(long)]
        force: bool,
    },
    #[command(subcommand, about = "Manage the local daemon")]
    Daemon(DaemonCommand),
    #[command(subcommand, about = "Manage paired remote devices")]
    Remote(RemoteCommand),
}

#[derive(Subcommand)]
pub enum RemoteCommand {
    #[command(about = "Pair a remote device")]
    Pair {
        #[arg(long, short)]
        label: Option<String>,
    },
    #[command(visible_alias = "ls", about = "List paired remote devices")]
    List,
    #[command(visible_alias = "rm", about = "Revoke a remote device")]
    Revoke { device: String },
}

#[derive(Subcommand)]
pub enum DaemonCommand {
    Serve,
    #[command(visible_alias = "ls", about = "List daemon sessions")]
    List,
    Stop,
    Kill {
        session: u64,
    },
}

#[derive(Subcommand)]
pub enum ProviderCommand {
    #[command(
        visible_alias = "ls",
        about = "List model providers",
        after_help = "Use `goat provider info <provider>` for setup details. Run bare `goat provider login` to pick from available providers."
    )]
    List,
    #[command(
        about = "Connect a provider account",
        after_help = "Examples:
  goat provider login
  goat provider login <provider> --key <key>
  goat provider login <provider>"
    )]
    Login {
        #[arg(help = "Provider id")]
        provider: Option<String>,
        #[arg(long, short, help = "Account name to store, default: default")]
        account: Option<String>,
        #[arg(long, help = "API key for API-key providers")]
        key: Option<String>,
        #[arg(long, value_name = "URL", help = "Provider endpoint override")]
        endpoint: Option<String>,
    },
    #[command(about = "Show setup details for one provider")]
    Info { provider: String },
    #[command(visible_alias = "rm", about = "Remove a provider account")]
    Logout {
        provider: String,
        #[arg(help = "Account name to remove")]
        account: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum SearchCommand {
    #[command(visible_alias = "ls", about = "List search providers")]
    List,
    #[command(about = "Show setup details for one search provider")]
    Info {
        provider: String,
    },
    #[command(about = "Connect a search provider account")]
    Login {
        provider: String,
        #[arg(long, short)]
        account: Option<String>,
        #[arg(long)]
        endpoint: Option<String>,
        #[arg(long)]
        engine: Option<String>,
        #[arg(long)]
        key: Option<String>,
        #[arg(long)]
        default: bool,
    },
    Default {
        target: String,
    },
    #[command(visible_alias = "rm", about = "Remove a search provider account")]
    Logout {
        provider: String,
        account: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum WorktreeCommand {
    #[command(visible_alias = "ls")]
    List,
    #[command(visible_alias = "rm")]
    Remove { label: String },
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{
        Cli, CodeCommand, Command, DaemonCommand, ProviderCommand, RemoteCommand, WorktreeCommand,
    };

    fn code_args(args: &[&str]) -> super::CodeArgs {
        let mut full = vec!["goat", "code"];
        full.extend_from_slice(args);
        match Cli::try_parse_from(full).unwrap().command {
            Some(Command::Code(a)) => a,
            other => panic!(
                "expected code subcommand, got {other:?}",
                other = other.is_some()
            ),
        }
    }

    #[test]
    fn bare_goat_has_no_command() {
        let cli = Cli::try_parse_from(["goat"]).unwrap();
        assert!(cli.command.is_none());
    }

    #[test]
    fn parses_short_worktree_flag() {
        assert_eq!(code_args(&["-w", "plan"]).worktree.as_deref(), Some("plan"));
    }

    #[test]
    fn parses_long_worktree_flag() {
        assert_eq!(
            code_args(&["--worktree", "plan"]).worktree.as_deref(),
            Some("plan")
        );
    }

    #[test]
    fn parses_short_continue_flag() {
        assert!(code_args(&["-c"]).r#continue);
    }

    #[test]
    fn parses_long_continue_flag() {
        assert!(code_args(&["--continue"]).r#continue);
    }

    #[test]
    fn continue_defaults_off() {
        assert!(!code_args(&[]).r#continue);
    }

    #[test]
    fn parses_worktree_list() {
        assert!(matches!(
            code_sub(&["worktree", "list"]),
            CodeCommand::Worktree(WorktreeCommand::List)
        ));
    }

    #[test]
    fn parses_worktree_remove() {
        assert!(matches!(
            code_sub(&["worktree", "remove", "plan"]),
            CodeCommand::Worktree(WorktreeCommand::Remove { label }) if label == "plan"
        ));
    }

    #[test]
    fn parses_update() {
        let cli = Cli::try_parse_from(["goat", "update"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Update { force: false })
        ));
    }

    #[test]
    fn parses_update_force() {
        let cli = Cli::try_parse_from(["goat", "update", "--force"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Update { force: true })));
    }

    #[test]
    fn headless_defaults_off() {
        let args = code_args(&[]);
        assert!(!args.headless);
        assert!(!args.print);
        assert_eq!(args.protocol, "json");
    }

    #[test]
    fn parses_provider_login() {
        let cli = Cli::try_parse_from(["goat", "provider", "login", "openrouter", "--key", "sk"])
            .unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Provider(ProviderCommand::Login { provider, key, .. }))
                if provider.as_deref() == Some("openrouter") && key.as_deref() == Some("sk")
        ));
    }

    #[test]
    fn parses_provider_list() {
        let cli = Cli::try_parse_from(["goat", "provider", "list"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Provider(ProviderCommand::List))
        ));
    }

    #[test]
    fn parses_provider_list_alias() {
        let cli = Cli::try_parse_from(["goat", "provider", "ls"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Provider(ProviderCommand::List))
        ));
    }

    #[test]
    fn parses_provider_login_picker() {
        let cli = Cli::try_parse_from(["goat", "provider", "login"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Provider(ProviderCommand::Login {
                provider: None,
                ..
            }))
        ));
    }

    #[test]
    fn parses_provider_login_endpoint() {
        let cli = Cli::try_parse_from([
            "goat",
            "provider",
            "login",
            "qwen",
            "--endpoint",
            "https://dashscope-us.aliyuncs.com/compatible-mode/v1",
            "--key",
            "sk",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Provider(ProviderCommand::Login { provider, endpoint, .. }))
                if provider.as_deref() == Some("qwen") && endpoint.as_deref() == Some("https://dashscope-us.aliyuncs.com/compatible-mode/v1")
        ));
    }

    #[test]
    fn provider_login_help_shows_examples() {
        let Err(error) = Cli::try_parse_from(["goat", "provider", "login", "--help"]) else {
            panic!("expected help error");
        };
        assert!(error.to_string().contains("goat provider login <provider>"));
    }

    #[test]
    fn parses_provider_info() {
        let cli = Cli::try_parse_from(["goat", "provider", "info", "kimi-code"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Provider(ProviderCommand::Info { provider })) if provider == "kimi-code"
        ));
    }

    #[test]
    fn provider_help_mentions_login_flow() {
        let Err(error) = Cli::try_parse_from(["goat", "provider", "--help"]) else {
            panic!("expected help error");
        };
        let help = error.to_string();
        assert!(help.contains("Manage model providers"));
        assert!(help.contains("goat provider login <provider>"));
    }

    #[test]
    fn parses_provider_logout() {
        let cli =
            Cli::try_parse_from(["goat", "provider", "logout", "openrouter", "default"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Provider(ProviderCommand::Logout { provider, account }))
                if provider == "openrouter" && account == "default"
        ));
    }

    #[test]
    fn parses_provider_logout_alias() {
        let cli = Cli::try_parse_from(["goat", "provider", "rm", "openrouter", "school"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Provider(ProviderCommand::Logout { provider, account }))
                if provider == "openrouter" && account == "school"
        ));
    }

    #[test]
    fn provider_logout_requires_account() {
        let result = Cli::try_parse_from(["goat", "provider", "rm", "openrouter"]);
        assert!(result.is_err());
    }

    #[test]
    fn provider_does_not_accept_service() {
        let result = Cli::try_parse_from([
            "goat",
            "provider",
            "login",
            "openrouter",
            "--service",
            "search",
        ]);
        assert!(result.is_err());
    }

    fn code_sub(args: &[&str]) -> CodeCommand {
        let mut full = vec!["goat", "code"];
        full.extend_from_slice(args);
        match Cli::try_parse_from(full).unwrap().command {
            Some(Command::Code(a)) => a.command.expect("code subcommand"),
            _ => panic!("expected code subcommand"),
        }
    }

    #[test]
    fn parses_search_list() {
        assert!(matches!(
            code_sub(&["search", "list"]),
            CodeCommand::Search(super::SearchCommand::List)
        ));
    }

    #[test]
    fn parses_search_login() {
        assert!(matches!(
            code_sub(&["search", "login", "brave", "--key", "key"]),
            CodeCommand::Search(super::SearchCommand::Login { provider, key, .. })
                if provider == "brave" && key.as_deref() == Some("key")
        ));
    }

    #[test]
    fn search_add_alias_is_removed() {
        let result = Cli::try_parse_from(["goat", "code", "search", "add", "brave"]);
        assert!(result.is_err());
    }

    #[test]
    fn parses_search_logout_alias() {
        assert!(matches!(
            code_sub(&["search", "rm", "brave", "default"]),
            CodeCommand::Search(super::SearchCommand::Logout { provider, account })
                if provider == "brave" && account == "default"
        ));
    }

    #[test]
    fn parses_daemon_list_alias() {
        let cli = Cli::try_parse_from(["goat", "daemon", "ls"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Daemon(DaemonCommand::List))
        ));
    }

    #[test]
    fn daemon_status_is_removed() {
        let result = Cli::try_parse_from(["goat", "daemon", "status"]);
        assert!(result.is_err());
    }

    #[test]
    fn parses_remote_list_alias() {
        let cli = Cli::try_parse_from(["goat", "remote", "ls"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Remote(RemoteCommand::List))
        ));
    }

    #[test]
    fn remote_devices_is_removed() {
        let result = Cli::try_parse_from(["goat", "remote", "devices"]);
        assert!(result.is_err());
    }

    #[test]
    fn auth_command_is_removed() {
        let result = Cli::try_parse_from(["goat", "auth", "list"]);
        assert!(result.is_err());
    }

    #[test]
    fn parses_headless_flag() {
        assert!(code_args(&["--headless"]).headless);
    }

    #[test]
    fn parses_print_flag() {
        assert!(code_args(&["-p"]).print);
    }

    #[test]
    fn parses_protocol_flag() {
        assert_eq!(
            code_args(&["--headless", "--protocol", "json"]).protocol,
            "json"
        );
    }
}
