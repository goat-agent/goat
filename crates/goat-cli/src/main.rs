mod ask;
mod chat;
mod input;
mod login;
mod paths;
mod render;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "goat",
    version,
    about = "A minimal coding agent for the terminal"
)]
struct Cli {
    #[arg(
        short,
        long,
        global = true,
        help = "Model to use, for example gpt-6.2-astra"
    )]
    model: Option<String>,
    #[arg(
        short,
        long,
        global = true,
        help = "Account to use, for example openai/work"
    )]
    account: Option<String>,
    #[arg(
        short,
        long,
        global = true,
        help = "Reasoning effort, for example high"
    )]
    effort: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Continue the latest session in this directory, or the given one")]
    Resume { session: Option<String> },
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
    let choice = chat::Choice {
        model: cli.model,
        account: cli.account,
        effort: cli.effort,
    };
    match cli.command {
        None => chat::start(&paths, choice).await,
        Some(Command::Resume { session }) => chat::resume(&paths, choice, session.as_deref()).await,
        Some(Command::Login) => login::login(&paths).await,
        Some(Command::Logout { account }) => login::logout(&paths, &account),
        Some(Command::Accounts) => login::list(&paths),
    }
}

fn restore_terminal_on_panic() {
    let report = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        crossterm::terminal::disable_raw_mode().ok();
        report(info);
    }));
}
