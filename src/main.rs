use anyhow::Result;
use clap::{Command, CommandFactory, FromArgMatches, Parser, Subcommand};
use clap_complete::Shell;

mod cli;

// Force-link inventory-based extension crates into the final binary.
use goat_channel_discord as _;
use goat_channel_telegram as _;
use goat_llm_anthropic as _;
use goat_llm_gemini as _;
use goat_llm_moonshot as _;
use goat_llm_openai as _;
use goat_llm_zhipu as _;
use goat_tool_echo as _;
use goat_tool_shell as _;
use goat_tool_skill as _;

const HELP_BRANCH: &str = concat!(
    "\n",
    "  \x1b[1;38;2;139;92;246m{name}\x1b[0m\n",
    "\n",
    "  {about}\n",
    "\n",
    "  \x1b[1mUsage\x1b[0m\n",
    "    {usage}\n",
    "\n",
    "  \x1b[1mCommands\x1b[0m\n",
    "{subcommands}\n",
    "\n",
    "  \x1b[1mOptions\x1b[0m\n",
    "{options}\n",
);

const HELP_LEAF: &str = concat!(
    "\n",
    "  \x1b[1;38;2;139;92;246m{name}\x1b[0m\n",
    "\n",
    "  {about}\n",
    "\n",
    "  \x1b[1mUsage\x1b[0m\n",
    "    {usage}\n",
    "\n",
    "  \x1b[1mOptions\x1b[0m\n",
    "{options}\n",
);

#[derive(Parser, Debug)]
#[command(
    name = "goat",
    version,
    about = "Autonomous personal AI agent",
    help_template = HELP_BRANCH,
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Minimal interactive bootstrap — credentials + first persona.
    Setup,
    /// Start the daemon. Same as running `goat` with no subcommand.
    Run,
    /// Parse every config file and report what would load.
    Doctor(cli::doctor::Args),
    /// Manage LLM provider keys in `~/.goat/credentials.json`.
    #[command(subcommand)]
    Provider(cli::provider::Cmd),
    /// Manage personas under `~/.goat/personas/`.
    #[command(subcommand)]
    Persona(cli::persona::Cmd),
    /// Inspect agent skills under `~/.goat/skills/`.
    #[command(subcommand)]
    Skill(cli::skill::Cmd),
    /// Generate a shell completion script.
    Completions {
        #[arg(value_enum)]
        shell: Shell,
    },
}

fn apply_help(cmd: Command) -> Command {
    fn walk(mut cmd: Command, path: &str) -> Command {
        let display = if path.is_empty() {
            cmd.get_name().to_string()
        } else {
            format!("{path} {}", cmd.get_name())
        };
        let template = if cmd.has_subcommands() {
            HELP_BRANCH
        } else {
            HELP_LEAF
        };
        cmd = cmd.help_template(template).display_name(&display);
        let names: Vec<String> = cmd
            .get_subcommands()
            .map(|c| c.get_name().to_string())
            .collect();
        for name in names {
            let display = display.clone();
            cmd = cmd.mut_subcommand(&name, move |sub| walk(sub, &display));
        }
        cmd
    }
    walk(cmd, "")
}

#[tokio::main]
async fn main() -> Result<()> {
    let cmd = apply_help(Cli::command());
    let matches = cmd.get_matches();
    let cli = Cli::from_arg_matches(&matches)?;
    match cli.command {
        None | Some(Cmd::Run) => goat_runtime::Goat::boot().await?.run().await,
        Some(Cmd::Setup) => cli::setup::run().await,
        Some(Cmd::Doctor(args)) => cli::doctor::run(args).await,
        Some(Cmd::Provider(c)) => cli::provider::run(c).await,
        Some(Cmd::Persona(c)) => cli::persona::run(c).await,
        Some(Cmd::Skill(c)) => cli::skill::run(c).await,
        Some(Cmd::Completions { shell }) => {
            let mut cmd = apply_help(Cli::command());
            let name = cmd.get_name().to_string();
            clap_complete::generate(shell, &mut cmd, name, &mut std::io::stdout());
            Ok(())
        }
    }
}
