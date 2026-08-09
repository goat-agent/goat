use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

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

pub const REMOTE_HELP: &str =
    "Daemon to talk to; `local` is this machine. Defaults to `goat remote use`";

#[derive(clap::Args, Debug, Default)]
#[command(about = "`goat code`: launch the coding TUI, or run one of its subcommands.")]
pub struct CodeArgs {
    #[arg(long)]
    pub print_log_path: bool,

    #[arg(long, short = 'w', value_name = "NAME")]
    pub worktree: Option<String>,

    #[arg(long, value_name = "NAME", help = REMOTE_HELP)]
    pub remote: Option<String>,

    #[arg(
        long,
        value_name = "PATH",
        help = "Directory to open on the daemon host; remembered per remote"
    )]
    pub dir: Option<String>,

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
    #[command(subcommand, about = "Inspect and end live coding sessions")]
    Session(SessionCommand),
}

#[derive(Subcommand, Debug)]
pub enum SessionCommand {
    #[command(visible_alias = "ls", about = "List live coding sessions")]
    List,
    #[command(about = "End one live coding session")]
    Kill { session: u64 },
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
  goat provider add <name> --endpoint <url> [--key <key>]
  goat provider logout <provider> <account>"
    )]
    Provider(ProviderCommand),
    #[command(
        about = "Apply config changes to the running daemon",
        after_help = "Edit the files under ~/.goat, then run this. Config is validated first: \
when it does not check out nothing is replaced and the running agents keep their old settings."
    )]
    Reload {
        #[arg(long, short, help = "Reload only this agent")]
        agent: Option<String>,
    },
    #[command(subcommand, about = "Manage MCP servers for code sessions")]
    Mcp(McpCommand),
    #[command(about = "Update goat")]
    Update {
        #[arg(long)]
        force: bool,
    },
    #[command(subcommand, about = "Manage the local daemon")]
    Daemon(DaemonCommand),
    #[command(
        subcommand,
        about = "Manage devices allowed to reach this daemon",
        after_help = "Run these on the machine the daemon runs on."
    )]
    Device(DeviceCommand),
    #[command(
        subcommand,
        about = "Manage the daemons this machine connects to",
        after_help = "`local` always exists and names the daemon on this machine.

Common flow:
  goat device add laptop      (on the server; prints a code and a fingerprint)
  goat remote add box --host <ip>:4317 --fingerprint <fp> --code <code>
  goat remote use local       (go back to the local daemon)"
    )]
    Remote(RemoteCommand),
}

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
pub enum McpScope {
    User,
    Project,
}

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
pub enum ConflictPolicy {
    Error,
    Skip,
    Replace,
}

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
pub enum SecretPolicy {
    Error,
    Store,
    Literal,
    Omit,
}

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
pub enum UnsupportedPolicy {
    Error,
    Skip,
    Accept,
}

#[derive(Subcommand, Debug)]
pub enum McpCommand {
    #[command(visible_alias = "ls", about = "List effective MCP servers")]
    List {
        #[arg(long, value_enum)]
        scope: Option<McpScope>,
        #[arg(long)]
        json: bool,
    },
    #[command(about = "Show one MCP server")]
    Get {
        name: String,
        #[arg(long, value_enum)]
        scope: Option<McpScope>,
        #[arg(long)]
        json: bool,
    },
    #[command(
        about = "Add an MCP server",
        after_help = "User-scope servers reach both code sessions and the agent.
Project-scope servers reach code sessions only, because the agent has no working directory.

Examples:
  goat mcp add context7 -- npx -y @upstash/context7-mcp
  goat mcp add remote --url https://example.com/mcp
  goat mcp add local --scope project --env TOKEN=value -- command"
    )]
    Add {
        name: String,
        #[arg(long, value_enum, default_value = "user")]
        scope: McpScope,
        #[arg(long, value_name = "URL")]
        url: Option<String>,
        #[arg(long, value_name = "KEY=VALUE")]
        env: Vec<String>,
        #[arg(long, value_name = "ENV_VAR")]
        bearer_token_env_var: Option<String>,
        #[arg(long)]
        force: bool,
        #[arg(last = true, value_name = "COMMAND")]
        command: Vec<String>,
    },
    #[command(visible_alias = "rm", about = "Remove an MCP server")]
    Remove {
        name: String,
        #[arg(long, value_enum, default_value = "user")]
        scope: McpScope,
    },
    #[command(about = "Import MCP servers from another config")]
    Import {
        path: Option<PathBuf>,
        #[arg(long, value_enum)]
        scope: Option<McpScope>,
        #[arg(long)]
        all: bool,
        #[arg(long, value_enum)]
        on_conflict: Option<ConflictPolicy>,
        #[arg(long, value_enum)]
        on_secret: Option<SecretPolicy>,
        #[arg(long, value_enum)]
        on_unsupported: Option<UnsupportedPolicy>,
        #[arg(long, value_name = "OLD=NEW")]
        rename: Vec<String>,
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        dry_run: bool,
    },
    #[command(about = "Sign in to an HTTP MCP server with OAuth")]
    Login {
        name: String,
        #[arg(long, value_enum)]
        scope: Option<McpScope>,
    },
    #[command(about = "Remove OAuth credentials for an HTTP MCP server")]
    Logout {
        name: String,
        #[arg(long, value_enum)]
        scope: Option<McpScope>,
    },
}

#[derive(Subcommand, Debug)]
pub enum DeviceCommand {
    #[command(about = "Mint a pairing code for a new device")]
    Add {
        #[arg(help = "Label to remember the device by")]
        label: Option<String>,
    },
    #[command(visible_alias = "ls", about = "List paired devices")]
    List,
    #[command(visible_alias = "rm", about = "Revoke a paired device")]
    Remove { device: String },
}

#[derive(Subcommand, Debug)]
pub enum RemoteCommand {
    #[command(about = "Pair with a daemon and remember it under a name")]
    Add {
        #[arg(help = "Name to remember this daemon by")]
        name: String,
        #[arg(long, value_name = "HOST:PORT", help = "Address the daemon listens on")]
        host: String,
        #[arg(long, help = "Server fingerprint printed by `goat device add`")]
        fingerprint: String,
        #[arg(long, help = "Pairing code printed by `goat device add`")]
        code: String,
    },
    #[command(visible_alias = "ls", about = "List known daemons")]
    List,
    #[command(visible_alias = "rm", about = "Forget a daemon")]
    Remove { name: String },
    #[command(about = "Make a daemon the default for this machine")]
    Use { name: String },
}

#[derive(Subcommand)]
pub enum DaemonCommand {
    #[command(
        about = "Make sure a daemon of this build is running in the background",
        after_help = "Idempotent. An idle daemon left over from an older build is replaced; a busy one is preserved."
    )]
    Start,
    #[command(about = "Stop the running daemon and wait for it to exit")]
    Stop,
    #[command(about = "Report which daemon is running, and what it is doing")]
    Status,
    #[command(
        about = "Run the daemon in this process",
        after_help = "Stays in the foreground, so a supervisor can own it. `goat daemon start` starts this in the background."
    )]
    Serve {
        #[arg(long, hide = true)]
        detached: bool,
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
    #[command(
        about = "Add a custom OpenAI-compatible provider",
        after_help = "Examples:
  goat provider add my-proxy --endpoint https://llm.corp/v1 --key sk-...
  goat provider add my-vllm --endpoint http://192.168.1.5:8000/v1
  goat provider add"
    )]
    Add {
        #[arg(help = "Provider name")]
        name: Option<String>,
        #[arg(long, value_name = "URL", help = "OpenAI-compatible base URL")]
        endpoint: Option<String>,
        #[arg(long, help = "API key; omit for keyless endpoints")]
        key: Option<String>,
        #[arg(long, short, help = "Account name to store, default: default")]
        account: Option<String>,
    },
    #[command(about = "Remove a custom provider and its credentials")]
    Remove {
        #[arg(help = "Custom provider name")]
        name: String,
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
        Cli, CodeCommand, Command, ConflictPolicy, DaemonCommand, McpCommand, McpScope,
        ProviderCommand, RemoteCommand, SecretPolicy, SessionCommand, UnsupportedPolicy,
        WorktreeCommand,
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
    fn parses_the_four_daemon_verbs() {
        for (args, expected) in [
            (["goat", "daemon", "start"], DaemonCommand::Start),
            (["goat", "daemon", "stop"], DaemonCommand::Stop),
            (["goat", "daemon", "status"], DaemonCommand::Status),
            (
                ["goat", "daemon", "serve"],
                DaemonCommand::Serve { detached: false },
            ),
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            let Some(Command::Daemon(got)) = cli.command else {
                panic!("expected a daemon subcommand for {args:?}");
            };
            assert_eq!(
                std::mem::discriminant(&got),
                std::mem::discriminant(&expected)
            );
        }
    }

    #[test]
    fn session_verbs_moved_off_the_daemon_noun() {
        assert!(Cli::try_parse_from(["goat", "daemon", "ls"]).is_err());
        assert!(Cli::try_parse_from(["goat", "daemon", "kill", "1"]).is_err());
        assert!(matches!(
            code_sub(&["session", "ls"]),
            CodeCommand::Session(SessionCommand::List)
        ));
        assert!(matches!(
            code_sub(&["session", "kill", "3"]),
            CodeCommand::Session(SessionCommand::Kill { session: 3 })
        ));
    }

    #[test]
    fn daemon_does_not_take_a_remote_target() {
        assert!(Cli::try_parse_from(["goat", "daemon", "--remote", "box", "status"]).is_err());
    }

    #[test]
    fn device_commands_take_no_remote_target() {
        assert!(Cli::try_parse_from(["goat", "device", "--remote", "box", "ls"]).is_err());
    }

    #[test]
    fn code_takes_a_remote_target_and_a_dir() {
        let cli =
            Cli::try_parse_from(["goat", "code", "--remote", "box", "--dir", "/srv/w"]).unwrap();
        let Some(Command::Code(args)) = cli.command else {
            panic!("expected code");
        };
        assert_eq!(args.remote.as_deref(), Some("box"));
        assert_eq!(args.dir.as_deref(), Some("/srv/w"));
    }

    #[test]
    fn remote_add_needs_host_fingerprint_and_code() {
        assert!(Cli::try_parse_from(["goat", "remote", "add", "box"]).is_err());
        let cli = Cli::try_parse_from([
            "goat",
            "remote",
            "add",
            "box",
            "--host",
            "1.2.3.4:4317",
            "--fingerprint",
            "abc",
            "--code",
            "xyz",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Remote(RemoteCommand::Add { name, .. })) if name == "box"
        ));
    }

    #[test]
    fn code_session_keeps_the_remote_target() {
        let cli =
            Cli::try_parse_from(["goat", "code", "--remote", "box", "session", "ls"]).unwrap();
        let Some(Command::Code(args)) = cli.command else {
            panic!("expected code");
        };
        assert_eq!(args.remote.as_deref(), Some("box"));
        assert!(matches!(
            args.command,
            Some(CodeCommand::Session(SessionCommand::List))
        ));
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

    #[test]
    fn parses_mcp_add_stdio() {
        let cli = Cli::try_parse_from([
            "goat",
            "mcp",
            "add",
            "context7",
            "--scope",
            "project",
            "--env",
            "TOKEN=value",
            "--",
            "npx",
            "-y",
            "pkg",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Mcp(McpCommand::Add {
                name,
                scope: McpScope::Project,
                command,
                ..
            })) if name == "context7" && command == ["npx", "-y", "pkg"]
        ));
    }

    #[test]
    fn parses_mcp_add_http() {
        let cli = Cli::try_parse_from([
            "goat",
            "mcp",
            "add",
            "remote",
            "--url",
            "https://example.test/mcp",
            "--bearer-token-env-var",
            "TOKEN",
            "--force",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Mcp(McpCommand::Add {
                url: Some(url),
                bearer_token_env_var: Some(variable),
                force: true,
                ..
            })) if url == "https://example.test/mcp" && variable == "TOKEN"
        ));
    }

    #[test]
    fn parses_noninteractive_mcp_import_policies() {
        let cli = Cli::try_parse_from([
            "goat",
            "mcp",
            "import",
            ".mcp.json",
            "--scope",
            "project",
            "--all",
            "--on-conflict",
            "replace",
            "--on-secret",
            "store",
            "--on-unsupported",
            "skip",
            "--rename",
            "old=new",
            "--yes",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Mcp(McpCommand::Import {
                scope: Some(McpScope::Project),
                all: true,
                on_conflict: Some(ConflictPolicy::Replace),
                on_secret: Some(SecretPolicy::Store),
                on_unsupported: Some(UnsupportedPolicy::Skip),
                yes: true,
                ..
            }))
        ));
    }
}
