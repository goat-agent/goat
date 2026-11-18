use anyhow::{Context, bail};
use goat_core::account::chatgpt::{self, Authorization};
use goat_core::account::{Account, Credential, Kind};
use goat_core::store::Store;

use crate::ask;
use crate::paths::Paths;

const METHODS: &[&str] = &[
    "ChatGPT (use your ChatGPT plan)",
    "OpenAI API key",
    "Anthropic API key",
];

pub(crate) async fn login(paths: &Paths) -> anyhow::Result<()> {
    let store = Store::open(&paths.database)?;
    let method = ask::choose("Sign in with:", METHODS)?;
    let (kind, default_name) = match method {
        0 => (Kind::OpenAi, "chatgpt"),
        1 => (Kind::OpenAi, "api"),
        _ => (Kind::Anthropic, "api"),
    };
    let entered = ask::line(&format!("Account name [{default_name}]: "))?;
    let name = if entered.is_empty() {
        default_name
    } else {
        entered.as_str()
    };
    let existing = store
        .accounts()?
        .into_iter()
        .find(|account| account.kind == kind && &*account.name == name);

    let credential = if method == 0 {
        let previous = existing
            .as_ref()
            .and_then(|account| match &account.credential {
                Credential::Chatgpt(token) => Some(token),
                Credential::ApiKey { .. } => None,
            });
        Credential::Chatgpt(sign_in(previous).await?)
    } else {
        let key = ask::secret("API key: ")?;
        if key.is_empty() {
            bail!("no API key entered");
        }
        Credential::ApiKey { key: key.into() }
    };

    let account = Account::new(kind, name, credential);
    store.save_account(&account)?;
    ask::say(&format!("Saved {}.", account.label()))?;
    Ok(())
}

async fn sign_in(previous: Option<&chatgpt::Token>) -> anyhow::Result<chatgpt::Token> {
    let authorization = Authorization::start(previous).await?;
    ask::say("Opening your browser to sign in to ChatGPT. If it does not open, visit:")?;
    ask::say(authorization.url())?;
    webbrowser::open(authorization.url()).ok();
    let http = crate::chat::http()?;
    authorization
        .finish(&http)
        .await
        .context("sign in to ChatGPT")
}

pub(crate) fn logout(paths: &Paths, label: &str) -> anyhow::Result<()> {
    let store = Store::open(&paths.database)?;
    let account = find(&store, label)?;
    store.remove_account(account.id)?;
    ask::say(&format!("Removed {}.", account.label()))?;
    Ok(())
}

pub(crate) fn list(paths: &Paths) -> anyhow::Result<()> {
    let store = Store::open(&paths.database)?;
    let accounts = store.accounts()?;
    if accounts.is_empty() {
        ask::say("No accounts. Add one with `goat login`.")?;
    }
    for account in accounts {
        let detail = match &account.credential {
            Credential::ApiKey { .. } => "API key".to_owned(),
            Credential::Chatgpt(token) => match &token.email {
                Some(email) => format!("ChatGPT, {email}"),
                None => "ChatGPT".to_owned(),
            },
        };
        ask::say(&format!("{}  {detail}", account.label()))?;
    }
    Ok(())
}

pub(crate) fn find(store: &Store, label: &str) -> anyhow::Result<Account> {
    let accounts = store.accounts()?;
    let matches: Vec<&Account> = accounts
        .iter()
        .filter(|account| account.label() == label || &*account.name == label)
        .collect();
    match matches.as_slice() {
        [account] => Ok((*account).clone()),
        [] => bail!("no account named {label}; see `goat accounts`"),
        _ => {
            bail!("{label} matches several accounts; use provider/name, for example openai/{label}")
        }
    }
}
