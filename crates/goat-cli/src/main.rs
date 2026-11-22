mod ask;
mod attach;
mod chat;
mod command;
mod highlight;
mod input;
mod kitty;
mod login;
mod markdown;
mod paths;
mod render;
mod target;
mod usage;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "goat",
    version,
    about = "A minimal coding agent for the terminal",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[arg(
        short,
        long,
        help = "Model to use, for example gpt-6.2-sol:high or openai:codex/gpt-6.2-sol"
    )]
    model: Option<String>,
    #[arg(
        short = 'c',
        long = "continue",
        conflicts_with = "resume",
        help = "Continue the latest session in this directory"
    )]
    is_continuing: bool,
    #[arg(short, long, value_name = "ID", help = "Continue the given session")]
    resume: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Sign in with ChatGPT or add an API key")]
    Login,
    #[command(about = "Remove an account")]
    Logout { account: String },
    #[command(about = "List accounts")]
    Accounts,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    restore_terminal_on_panic();
    let cli = Cli::parse();
    let paths = paths::Paths::detect()?;
    let choice = chat::Choice { model: cli.model };
    let Some(command) = cli.command else {
        return match (cli.resume.as_deref(), cli.is_continuing) {
            (None, false) => chat::start(&paths, choice).await,
            (id, _) => chat::resume(&paths, choice, id).await,
        };
    };
    match command {
        Command::Login => login::login(&paths).await,
        Command::Logout { account } => login::logout(&paths, &account),
        Command::Accounts => login::list(&paths),
    }
}

fn restore_terminal_on_panic() {
    let report = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        crossterm::terminal::disable_raw_mode().ok();
        report(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("goat").chain(args.iter().copied()))
    }

    #[test]
    fn continue_picks_the_latest_session() {
        let cli = parse(&["--continue"]).unwrap();

        assert!(cli.is_continuing);
        assert_eq!(cli.resume, None);
    }

    #[test]
    fn resume_takes_a_session_id() {
        let cli = parse(&["-r", "0193"]).unwrap();

        assert_eq!(cli.resume.as_deref(), Some("0193"));
    }

    #[test]
    fn continue_and_resume_together_are_rejected() {
        assert!(parse(&["-c", "-r", "0193"]).is_err());
    }

    #[test]
    fn session_flags_with_a_command_are_rejected() {
        assert!(parse(&["--continue", "login"]).is_err());
    }

    #[test]
    fn resume_is_no_longer_a_command() {
        assert!(parse(&["resume"]).is_err());
    }
}
