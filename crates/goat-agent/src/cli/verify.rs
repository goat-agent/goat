use std::time::Duration;

use goat_auth::{CredentialService, CredentialStore};
use goat_provider::{AuthMethod, ProviderId, ValidateError, Validated};
use goat_providers::{DEFAULT_ACCOUNT, Registry};

use super::ui;
use super::ui::{Palette, Table};

const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyOutcome {
    Valid,
    InvalidCredentials,
    Unreachable(String),
    Unverifiable(String),
}

pub async fn verify_credential(
    store: &CredentialStore,
    user: &goat_config::UserProviders,
    provider: &str,
    account: &str,
) -> VerifyOutcome {
    let registry = Registry::load(store, user, account);
    let Some(handle) = registry.get(&ProviderId::from(provider)) else {
        return VerifyOutcome::Unverifiable("unknown provider".to_owned());
    };
    if matches!(handle.capabilities().auth, AuthMethod::None) {
        return VerifyOutcome::Unverifiable("local provider".to_owned());
    }
    if !handle.verifies_credentials() {
        return VerifyOutcome::Unverifiable("no live check for this provider".to_owned());
    }
    if !handle.authenticated() {
        return VerifyOutcome::Unverifiable("not connected".to_owned());
    }
    let mut task = handle.validate();
    match tokio::time::timeout(TIMEOUT, &mut task).await {
        Ok(Ok(result)) => classify(result),
        Ok(Err(_join)) => VerifyOutcome::Unreachable("check failed".to_owned()),
        Err(_elapsed) => {
            task.abort();
            VerifyOutcome::Unreachable("timed out".to_owned())
        }
    }
}

fn classify(result: Result<Validated, ValidateError>) -> VerifyOutcome {
    match result {
        Ok(Validated::Live) => VerifyOutcome::Valid,
        Ok(Validated::Assumed) => {
            VerifyOutcome::Unverifiable("connected (not live-checked)".to_owned())
        }
        Err(ValidateError::InvalidCredentials) => VerifyOutcome::InvalidCredentials,
        Err(ValidateError::NoCredentials) => {
            VerifyOutcome::Unverifiable("not connected".to_owned())
        }
        Err(ValidateError::Unreachable(detail)) => VerifyOutcome::Unreachable(detail),
    }
}

pub fn outcome_row(outcome: &VerifyOutcome) -> (&'static str, Palette, String) {
    match outcome {
        VerifyOutcome::Valid => ("ok", Palette::Success, "reachable".to_owned()),
        VerifyOutcome::InvalidCredentials => {
            ("warn", Palette::Warning, "invalid credentials".to_owned())
        }
        VerifyOutcome::Unreachable(detail) => ("warn", Palette::Warning, detail.clone()),
        VerifyOutcome::Unverifiable(detail) => ("skip", Palette::Muted, detail.clone()),
    }
}

pub fn is_warning(outcome: &VerifyOutcome) -> bool {
    matches!(
        outcome,
        VerifyOutcome::InvalidCredentials | VerifyOutcome::Unreachable(_)
    )
}

pub fn report_outcome(outcome: &VerifyOutcome) {
    match outcome {
        VerifyOutcome::Valid => ui::success("verified"),
        VerifyOutcome::InvalidCredentials => ui::warning("invalid credentials"),
        VerifyOutcome::Unreachable(detail) => {
            ui::warning(&format!("could not reach provider ({detail})"));
        }
        VerifyOutcome::Unverifiable(detail) => ui::note(detail),
    }
}

pub async fn render_all(store: &CredentialStore, user: &goat_config::UserProviders) {
    let registry = Registry::new(store, user);
    let mut pairs = Vec::new();
    for provider in registry.all() {
        let id = provider.id().to_string();
        for account in accounts_for(store, &id) {
            pairs.push((id.clone(), account));
        }
    }
    render_table(store, user, &pairs, "no connected providers").await;
}

pub async fn render_accounts(
    store: &CredentialStore,
    user: &goat_config::UserProviders,
    provider: &str,
    accounts: &[String],
) {
    let pairs: Vec<(String, String)> = accounts
        .iter()
        .map(|account| (provider.to_owned(), account.clone()))
        .collect();
    render_table(store, user, &pairs, "no connected providers").await;
}

async fn render_table(
    store: &CredentialStore,
    user: &goat_config::UserProviders,
    pairs: &[(String, String)],
    empty: &str,
) {
    if pairs.is_empty() {
        ui::note(empty);
        return;
    }
    let mut table = Table::new(["provider", "status", "detail"]);
    for (provider, account) in pairs {
        let outcome = verify_credential(store, user, provider, account).await;
        let (status, style, detail) = outcome_row(&outcome);
        table.styled_row(vec![
            (row_label(provider, account), Palette::Plain),
            (status.to_owned(), style),
            (detail, Palette::Plain),
        ]);
    }
    table.render();
}

pub fn accounts_for(store: &CredentialStore, provider: &str) -> Vec<String> {
    store
        .entries()
        .into_iter()
        .filter(|(key, _)| key.service == CredentialService::Model && key.provider == provider)
        .map(|(key, _)| key.account)
        .collect()
}

pub fn row_label(provider: &str, account: &str) -> String {
    if account == DEFAULT_ACCOUNT {
        provider.to_owned()
    } else {
        format!("{provider} ({account})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(name: &str) -> CredentialStore {
        CredentialStore::new(std::env::temp_dir().join(name))
    }

    fn no_user() -> goat_config::UserProviders {
        goat_config::UserProviders::at(std::env::temp_dir().join("goat-agent-verify-no-user.json"))
    }

    #[test]
    fn classify_covers_every_branch() {
        assert_eq!(classify(Ok(Validated::Live)), VerifyOutcome::Valid);
        assert_eq!(
            classify(Ok(Validated::Assumed)),
            VerifyOutcome::Unverifiable("connected (not live-checked)".to_owned())
        );
        assert_eq!(
            classify(Err(ValidateError::InvalidCredentials)),
            VerifyOutcome::InvalidCredentials
        );
        assert_eq!(
            classify(Err(ValidateError::NoCredentials)),
            VerifyOutcome::Unverifiable("not connected".to_owned())
        );
        assert_eq!(
            classify(Err(ValidateError::unreachable("boom"))),
            VerifyOutcome::Unreachable("boom".to_owned())
        );
    }

    #[test]
    fn outcome_row_maps_status_and_style() {
        assert!(matches!(
            outcome_row(&VerifyOutcome::Valid),
            ("ok", Palette::Success, _)
        ));
        assert!(matches!(
            outcome_row(&VerifyOutcome::InvalidCredentials),
            ("warn", Palette::Warning, _)
        ));
        assert!(matches!(
            outcome_row(&VerifyOutcome::Unreachable("x".to_owned())),
            ("warn", Palette::Warning, _)
        ));
        let (label, style, _) = outcome_row(&VerifyOutcome::Unverifiable("local".to_owned()));
        assert_eq!(label, "skip");
        assert!(matches!(style, Palette::Muted));
        assert!(!is_warning(&VerifyOutcome::Unverifiable(
            "local".to_owned()
        )));
    }

    #[tokio::test]
    async fn local_provider_is_unverifiable_offline() {
        let store = temp_store("goat-verify-local-test.json");
        assert_eq!(
            verify_credential(&store, &no_user(), "ollama", DEFAULT_ACCOUNT).await,
            VerifyOutcome::Unverifiable("local provider".to_owned())
        );
    }

    #[tokio::test]
    async fn catalog_only_provider_is_unverifiable_offline() {
        let store = temp_store("goat-verify-catalog-test.json");
        assert_eq!(
            verify_credential(&store, &no_user(), "zai", DEFAULT_ACCOUNT).await,
            VerifyOutcome::Unverifiable("no live check for this provider".to_owned())
        );
    }

    #[tokio::test]
    async fn unknown_provider_is_unverifiable() {
        let store = temp_store("goat-verify-unknown-test.json");
        assert_eq!(
            verify_credential(&store, &no_user(), "nope", DEFAULT_ACCOUNT).await,
            VerifyOutcome::Unverifiable("unknown provider".to_owned())
        );
    }

    #[test]
    fn row_label_hides_default_account() {
        assert_eq!(row_label("openai", DEFAULT_ACCOUNT), "openai");
        assert_eq!(row_label("openai", "work"), "openai (work)");
    }
}
