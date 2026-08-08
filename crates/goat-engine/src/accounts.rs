use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use goat_auth::{
    Credential, CredentialKey, CredentialKind, CredentialService, CredentialStore, SecretString,
    TokenSet,
};
use goat_code_store::{CodeStore as Store, Thread};
use goat_config::UserProviders;
use goat_protocol::{
    AccountChoice, AccountEntry, AccountInfo, AuthMethod, Effort, Event, LoginCredential,
    LoginProvider, ModelEntry, ModelTarget, NotifyKind,
};
use goat_provider::{ModelListSource, Provider};
use goat_providers::{DEFAULT_ACCOUNT, Registry};
use tokio::sync::mpsc;

use crate::SessionContext;

const DISCOVER_TIMEOUT_SECS: u64 = 15;

pub(crate) async fn restore_target(
    store: &Store,
    credentials: &CredentialStore,
    user: &UserProviders,
    cwd: &std::path::Path,
) -> Option<ModelTarget> {
    let thread = latest_thread_or_seed(store, cwd).await?;
    let provider = Registry::load(credentials, user, &thread.account)
        .get(&goat_provider::ProviderId::from(thread.provider.as_str()))?;
    if !provider.authenticated() {
        return None;
    }
    Some(ModelTarget {
        provider: thread.provider,
        model: thread.model,
        account: thread.account,
        effort: thread.effort.as_deref().and_then(Effort::parse),
    })
}

async fn latest_thread_or_seed(store: &Store, cwd: &std::path::Path) -> Option<Thread> {
    let key = cwd.display().to_string();
    if let Some(thread) = store.latest_thread_in(key).await.ok().flatten() {
        return Some(thread);
    }
    let owner = worktree_owner_root(cwd)?;
    store
        .latest_thread_in(owner.display().to_string())
        .await
        .ok()
        .flatten()
}

fn worktree_owner_root(cwd: &std::path::Path) -> Option<std::path::PathBuf> {
    let workspace = goat_worktree::workspace(cwd).ok()?;
    if !matches!(workspace.kind, goat_worktree::WorkspaceKind::Managed { .. }) {
        return None;
    }
    Some(workspace.owner_root)
}

pub(crate) async fn emit_accounts_changed(
    events: &mpsc::Sender<Event>,
    registry: &Registry,
    credentials: &CredentialStore,
) {
    let _ = events
        .send(Event::AccountsChanged {
            providers: build_account_entries(registry, credentials),
        })
        .await;
}

pub(crate) async fn handle_remove_account(ctx: &SessionContext, provider: String, name: String) {
    let key = CredentialKey::model(provider.clone(), name.clone());
    if let Err(err) = ctx.credentials.remove(&key) {
        tracing::warn!(%err, "failed to remove account");
    }
    ctx.set_registry(Registry::new(&ctx.credentials, &ctx.user));
    let registry = ctx.registry();
    let entries = discover_ready(&registry, &ctx.credentials, &ctx.user).await;
    let _ = ctx.events.send(Event::ModelListChanged { entries }).await;
    emit_accounts_changed(&ctx.events, &registry, &ctx.credentials).await;
}

pub(crate) fn build_account_entries(
    registry: &Registry,
    credentials: &CredentialStore,
) -> Vec<AccountEntry> {
    let stored = credentials.entries();
    registry
        .all()
        .iter()
        .map(|p| {
            let provider_id = p.id().to_string();
            let auth_method = p.capabilities().auth;
            let is_local = matches!(auth_method, AuthMethod::None);
            let accounts = stored
                .iter()
                .filter(|(key, _)| {
                    key.service == CredentialService::Model && key.provider == provider_id
                })
                .map(|(key, kind)| AccountInfo {
                    name: key.account.clone(),
                    method: match kind {
                        CredentialKind::ApiKey => AuthMethod::ApiKey,
                        CredentialKind::OAuth => AuthMethod::OAuth,
                    },
                })
                .collect();
            AccountEntry {
                display_name: provider_id.clone(),
                provider: provider_id,
                accounts,
                local: is_local,
                login: auth_method,
            }
        })
        .collect()
}

pub(crate) async fn announce_startup(
    events: &mpsc::Sender<Event>,
    registry: &Registry,
    credentials: &CredentialStore,
    user: &UserProviders,
    target: Option<&ModelTarget>,
) {
    let _ = events
        .send(Event::LoginProviders {
            providers: registry
                .all()
                .iter()
                .map(|p| LoginProvider {
                    id: p.id().to_string(),
                    method: p.capabilities().auth,
                })
                .collect(),
        })
        .await;
    let _ = events
        .send(Event::AccountsChanged {
            providers: build_account_entries(registry, credentials),
        })
        .await;
    let _ = events
        .send(Event::ModelListChanged {
            entries: catalog_only(registry, credentials, user),
        })
        .await;
    if let Some(selected) = target {
        let _ = events
            .send(Event::ModelSelected {
                target: selected.clone(),
            })
            .await;
    }
    let providers = provider_accounts(registry, credentials);
    if providers
        .iter()
        .any(|(provider, _)| provider.model_list_source() == ModelListSource::Discover)
    {
        let bg_events = events.clone();
        let bg_credentials = credentials.clone();
        let bg_user = user.clone();
        tokio::spawn(async move {
            let entries = model_list_entries(&providers, &bg_credentials, &bg_user).await;
            let _ = bg_events.send(Event::ModelListChanged { entries }).await;
        });
    }
}

async fn login_succeeded(provider: &str, events: &mpsc::Sender<Event>) {
    let _ = events
        .send(Event::Notify {
            kind: NotifyKind::Success,
            message: format!("{provider} connected"),
        })
        .await;
    let _ = events
        .send(Event::LoginStatus {
            provider: provider.to_owned(),
            message: String::new(),
            done: true,
            ok: true,
        })
        .await;
}

async fn login_stored_unverified(provider: &str, events: &mpsc::Sender<Event>) {
    let message = "stored but not verified; validation will happen on first request".to_owned();
    let _ = events
        .send(Event::Notify {
            kind: NotifyKind::Success,
            message: format!("{provider} {message}"),
        })
        .await;
    let _ = events
        .send(Event::LoginStatus {
            provider: provider.to_owned(),
            message,
            done: true,
            ok: true,
        })
        .await;
}

async fn login_failed(provider: &str, events: &mpsc::Sender<Event>, message: String) {
    let _ = events
        .send(Event::LoginStatus {
            provider: provider.to_owned(),
            message,
            done: true,
            ok: false,
        })
        .await;
}

async fn run_self_oauth(
    provider: &str,
    events: &mpsc::Sender<Event>,
    registry: &Registry,
) -> Result<TokenSet, String> {
    let (status_tx, mut status_rx) = mpsc::channel::<String>(8);
    let status_provider = provider.to_owned();
    let status_events = events.clone();
    let forwarder = tokio::spawn(async move {
        while let Some(message) = status_rx.recv().await {
            let _ = status_events
                .send(Event::LoginStatus {
                    provider: status_provider.clone(),
                    message,
                    done: false,
                    ok: false,
                })
                .await;
        }
    });
    let result = registry.login(provider, status_tx).await;
    let _ = forwarder.await;
    result
}

async fn finalize_login(
    ctx: &SessionContext,
    provider: String,
    name: String,
    key: CredentialKey,
    resolved: Credential,
) {
    if let Err(message) = ctx
        .credentials
        .store(&key, resolved)
        .map_err(|err| err.to_string())
    {
        login_failed(&provider, &ctx.events, message).await;
        emit_accounts_changed(&ctx.events, &ctx.registry(), &ctx.credentials).await;
        return;
    }
    ctx.set_registry(Registry::new(&ctx.credentials, &ctx.user));
    if let Err(message) = validate_stored(&ctx.credentials, &ctx.user, &provider, &name).await {
        let _ = ctx.credentials.remove(&key);
        ctx.set_registry(Registry::new(&ctx.credentials, &ctx.user));
        login_failed(&provider, &ctx.events, message).await;
        emit_accounts_changed(&ctx.events, &ctx.registry(), &ctx.credentials).await;
        return;
    }
    let stored_but_unverified = Registry::load(&ctx.credentials, &ctx.user, &name)
        .get(&goat_provider::ProviderId::from(provider.as_str()))
        .is_some_and(|target| !target.verifies_credentials());
    let entries = discover_ready(&ctx.registry(), &ctx.credentials, &ctx.user).await;
    let _ = ctx.events.send(Event::ModelListChanged { entries }).await;
    if stored_but_unverified {
        login_stored_unverified(&provider, &ctx.events).await;
    } else {
        login_succeeded(&provider, &ctx.events).await;
    }
    emit_accounts_changed(&ctx.events, &ctx.registry(), &ctx.credentials).await;
}

async fn validate_stored(
    credentials: &CredentialStore,
    user: &UserProviders,
    provider: &str,
    name: &str,
) -> Result<(), String> {
    match Registry::load(credentials, user, name).get(&goat_provider::ProviderId::from(provider)) {
        Some(target) => match target.validate().await {
            Ok(result) => result.map(|_| ()).map_err(|err| err.to_string()),
            Err(join) => Err(join.to_string()),
        },
        None => Err("unknown provider".to_owned()),
    }
}

pub(crate) async fn handle_login(
    ctx: &SessionContext,
    provider: String,
    name: String,
    credential: LoginCredential,
    dedup: bool,
) {
    let key = CredentialKey::model(provider.clone(), name.clone());
    if dedup
        && ctx
            .credentials
            .entries()
            .iter()
            .any(|(stored, _)| stored == &key)
    {
        login_failed(
            &provider,
            &ctx.events,
            format!("account '{name}' already exists"),
        )
        .await;
        return;
    }
    let resolved = match credential {
        LoginCredential::ApiKey { key: secret } => Credential::ApiKey(SecretString::from(secret)),
        LoginCredential::OAuth {} => {
            match run_self_oauth(&provider, &ctx.events, &ctx.registry()).await {
                Ok(tokens) => Credential::OAuth(tokens),
                Err(message) => {
                    login_failed(&provider, &ctx.events, message).await;
                    emit_accounts_changed(&ctx.events, &ctx.registry(), &ctx.credentials).await;
                    return;
                }
            }
        }
    };
    finalize_login(ctx, provider, name, key, resolved).await;
}

fn account_names_for(credentials: &CredentialStore, provider_id: &str) -> Vec<String> {
    credentials
        .entries()
        .into_iter()
        .filter(|(key, _)| key.service == CredentialService::Model && key.provider == provider_id)
        .map(|(key, _)| key.account)
        .collect()
}

fn accounts_for_provider(
    credentials: &CredentialStore,
    provider: &dyn goat_provider::Provider,
) -> Option<Vec<String>> {
    let is_local = matches!(provider.capabilities().auth, AuthMethod::None);
    let stored = account_names_for(credentials, &provider.id().to_string());
    if is_local {
        if stored.is_empty() {
            return Some(vec![DEFAULT_ACCOUNT.to_owned()]);
        }
        return Some(stored);
    }
    if !stored.is_empty() {
        return Some(stored);
    }
    if provider.authenticated() {
        return Some(vec![DEFAULT_ACCOUNT.to_owned()]);
    }
    None
}

fn model_entry(
    provider_id: &str,
    model: &str,
    accounts: &[String],
    efforts: Vec<Effort>,
    context_window: Option<u32>,
    supports_images: bool,
) -> ModelEntry {
    ModelEntry {
        provider: provider_id.to_owned(),
        model: model.to_owned(),
        context_window,
        supports_images,
        efforts,
        accounts: accounts
            .iter()
            .map(|account| AccountChoice {
                id: account.clone(),
                display: account.clone(),
                target: ModelTarget {
                    provider: provider_id.to_owned(),
                    model: model.to_owned(),
                    account: account.clone(),
                    effort: None,
                },
            })
            .collect(),
    }
}

fn catalog_only(
    registry: &Registry,
    credentials: &CredentialStore,
    user: &UserProviders,
) -> Vec<ModelEntry> {
    let mut entries = Vec::new();
    for provider in registry.all() {
        if provider.model_list_source() != ModelListSource::Catalog {
            continue;
        }
        let Some(accounts) = accounts_for_provider(credentials, provider.as_ref()) else {
            continue;
        };
        entries.extend(catalog_entries(
            credentials,
            user,
            &provider.id(),
            &accounts,
        ));
    }
    entries
}

fn models_for_provider(
    credentials: &CredentialStore,
    user: &UserProviders,
    provider_id: &goat_provider::ProviderId,
    accounts: &[String],
) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut models = Vec::new();
    for account in accounts {
        let registry = Registry::load(credentials, user, account);
        let Some(provider) = registry.get(provider_id) else {
            continue;
        };
        for id in provider.list_models() {
            if seen.insert(id.clone()) {
                models.push(id);
            }
        }
    }
    models
}

fn catalog_entries(
    credentials: &CredentialStore,
    user: &UserProviders,
    provider_id: &goat_provider::ProviderId,
    accounts: &[String],
) -> Vec<ModelEntry> {
    let provider_id_str = provider_id.to_string();
    models_for_provider(credentials, user, provider_id, accounts)
        .into_iter()
        .map(|id| {
            let (efforts, context_window, supports_images) = accounts
                .iter()
                .find_map(|account| {
                    Registry::load(credentials, user, account)
                        .get(provider_id)
                        .filter(|provider| provider.list_models().iter().any(|m| m == &id))
                        .map(|provider| {
                            (
                                provider.efforts(&id),
                                provider.context_window(&id),
                                provider.supports_images(&id),
                            )
                        })
                })
                .unwrap_or((Vec::new(), None, false));
            model_entry(
                &provider_id_str,
                &id,
                accounts,
                efforts,
                context_window,
                supports_images,
            )
        })
        .collect()
}

fn provider_accounts(
    registry: &Registry,
    credentials: &CredentialStore,
) -> Vec<(Arc<dyn Provider>, Vec<String>)> {
    registry
        .all()
        .iter()
        .filter_map(|provider| {
            accounts_for_provider(credentials, provider.as_ref())
                .map(|accounts| (Arc::clone(provider), accounts))
        })
        .collect()
}

async fn discover_entries(provider: Arc<dyn Provider>, accounts: Vec<String>) -> Vec<ModelEntry> {
    let provider_id = provider.id().to_string();
    let (tx, mut rx) = mpsc::channel(32);
    let handle = provider.discover(tx);
    let mut discovered = Vec::new();
    let collect = async {
        while let Some(info) = rx.recv().await {
            discovered.push(info);
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(DISCOVER_TIMEOUT_SECS), collect).await;
    handle.abort();
    discovered
        .into_iter()
        .map(|info| {
            model_entry(
                &provider_id,
                &info.id,
                &accounts,
                provider.efforts(&info.id),
                provider.context_window(&info.id),
                info.supports_images || provider.supports_images(&info.id),
            )
        })
        .collect()
}

async fn model_list_for_provider(
    provider: Arc<dyn Provider>,
    accounts: Vec<String>,
    credentials: &CredentialStore,
    user: &UserProviders,
) -> Vec<ModelEntry> {
    match provider.model_list_source() {
        ModelListSource::Catalog => catalog_entries(credentials, user, &provider.id(), &accounts),
        ModelListSource::Discover => discover_entries(provider, accounts).await,
    }
}

async fn model_list_entries(
    providers: &[(Arc<dyn Provider>, Vec<String>)],
    credentials: &CredentialStore,
    user: &UserProviders,
) -> Vec<ModelEntry> {
    futures::future::join_all(providers.iter().map(|(provider, accounts)| {
        model_list_for_provider(Arc::clone(provider), accounts.clone(), credentials, user)
    }))
    .await
    .into_iter()
    .flatten()
    .collect()
}

pub(crate) async fn discover_ready(
    registry: &Registry,
    credentials: &CredentialStore,
    user: &UserProviders,
) -> Vec<ModelEntry> {
    let providers = provider_accounts(registry, credentials);
    model_list_entries(&providers, credentials, user).await
}

pub(crate) async fn refresh_model_list(ctx: &SessionContext) {
    let entries = discover_ready(&ctx.registry(), &ctx.credentials, &ctx.user).await;
    let _ = ctx.events.send(Event::ModelListChanged { entries }).await;
}

pub(crate) fn clear_account_registries(cache: &std::sync::Mutex<HashMap<String, Arc<Registry>>>) {
    cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
}

pub(crate) fn provider_for(
    ctx: &SessionContext,
    account: &str,
    id: &goat_provider::ProviderId,
) -> Option<Arc<dyn Provider>> {
    if account == DEFAULT_ACCOUNT {
        return ctx.registry().get(id);
    }
    let mut cache = ctx
        .account_registries
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache
        .entry(account.to_owned())
        .or_insert_with(|| {
            Arc::new(Registry::load_metered(
                &ctx.credentials,
                &ctx.user,
                account,
                ctx.meter.clone(),
            ))
        })
        .get(id)
}

#[cfg(test)]
mod tests {
    use goat_auth::{Credential, CredentialStore, SecretString};
    use goat_provider::{ModelListSource, Provider, ProviderId};

    use super::{catalog_only, latest_thread_or_seed, models_for_provider};
    use goat_code_store::{CodeStore as Store, NewThread};
    use goat_config::UserProviders;
    use goat_providers::Registry;

    fn store(name: &str) -> CredentialStore {
        let path = std::env::temp_dir().join(name);
        let _ = std::fs::remove_file(&path);
        CredentialStore::new(path)
    }

    fn no_user() -> UserProviders {
        UserProviders::at(std::env::temp_dir().join("goat-agent-accounts-no-user.json"))
    }

    fn model_list_source_check(provider: &dyn Provider) -> ModelListSource {
        provider.model_list_source()
    }

    #[test]
    fn local_providers_use_discover_only_lists() {
        let store = store("goat-agent-accounts-local.json");
        let registry = Registry::new(&store, &no_user());
        let ollama = registry
            .get(&ProviderId::from("ollama"))
            .expect("ollama provider");
        assert_eq!(
            model_list_source_check(ollama.as_ref()),
            ModelListSource::Discover
        );
        assert!(ollama.list_models().is_empty());
    }

    #[test]
    fn hosted_providers_use_catalog_lists() {
        let store = store("goat-agent-accounts-hosted.json");
        let registry = Registry::new(&store, &no_user());
        let openai = registry
            .get(&ProviderId::from("openai"))
            .expect("openai provider");
        assert_eq!(
            model_list_source_check(openai.as_ref()),
            ModelListSource::Catalog
        );
        assert!(!openai.list_models().is_empty());
    }

    #[test]
    fn openrouter_uses_live_model_discovery() {
        let store = store("goat-agent-accounts-openrouter.json");
        let registry = Registry::new(&store, &no_user());
        let openrouter = registry
            .get(&ProviderId::from("openrouter"))
            .expect("openrouter provider");
        assert_eq!(
            model_list_source_check(openrouter.as_ref()),
            ModelListSource::Discover
        );
    }

    #[test]
    fn catalog_only_skips_local_providers() {
        let store = store("goat-agent-accounts-catalog-only.json");
        let registry = Registry::new(&store, &no_user());
        let entries = catalog_only(&registry, &store, &no_user());
        assert!(entries.iter().all(|entry| entry.provider != "ollama"));
    }

    #[test]
    fn xai_models_follow_account_credential_kind() {
        let store = store("goat-agent-accounts-xai.json");
        store
            .store(
                &goat_auth::CredentialKey::model("xai", "oauth"),
                Credential::OAuth(
                    goat_auth::TokenSet::from_parts(
                        "access".to_owned(),
                        Some("refresh".to_owned()),
                        Some(3600),
                        None,
                    )
                    .unwrap(),
                ),
            )
            .unwrap();
        store
            .store(
                &goat_auth::CredentialKey::model("xai", "api"),
                Credential::ApiKey(SecretString::from("xai-key".to_owned())),
            )
            .unwrap();
        let oauth_models = models_for_provider(
            &store,
            &no_user(),
            &ProviderId::from("xai"),
            &["oauth".to_owned()],
        );
        assert!(oauth_models.iter().any(|id| id == "grok-4.3"));
        assert!(oauth_models.iter().any(|id| id == "grok-composer-2.5-fast"));
        let api_models = models_for_provider(
            &store,
            &no_user(),
            &ProviderId::from("xai"),
            &["api".to_owned()],
        );
        assert!(api_models.iter().any(|id| id == "grok-4.3"));
        assert!(!api_models.iter().any(|id| id == "grok-composer-2.5-fast"));
    }

    fn git_available() -> bool {
        std::process::Command::new("git")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    fn git(repo: &std::path::Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    fn init_repo(repo: &std::path::Path) {
        std::fs::create_dir(repo).unwrap();
        git(repo, &["init", "-b", "main"]);
        git(repo, &["config", "user.email", "t@example.invalid"]);
        git(repo, &["config", "user.name", "Test"]);
        std::fs::write(repo.join("README.md"), "hello\n").unwrap();
        git(repo, &["add", "README.md"]);
        git(repo, &["commit", "-m", "init"]);
    }

    fn add_worktree(repo: &std::path::Path) -> std::path::PathBuf {
        let worktree = repo.join(".goat").join("worktrees").join("test");
        std::fs::create_dir_all(worktree.parent().unwrap()).unwrap();
        git(
            repo,
            &[
                "worktree",
                "add",
                "-b",
                "worktree-test",
                worktree.to_str().unwrap(),
                "HEAD",
            ],
        );
        worktree
    }

    fn owner_key(worktree: &std::path::Path) -> String {
        goat_worktree::workspace(worktree)
            .unwrap()
            .owner_root
            .display()
            .to_string()
    }

    fn seeded_thread(cwd: &str, model: &str) -> NewThread {
        NewThread {
            cwd: cwd.to_owned(),
            title: None,
            provider: "anthropic".to_owned(),
            model: model.to_owned(),
            account: "default".to_owned(),
            effort: Some("xhigh".to_owned()),
            created_at: 1,
            updated_at: 1,
        }
    }

    #[tokio::test]
    async fn worktree_seeds_model_from_owner_repo() {
        if !git_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        init_repo(&repo);
        let worktree = add_worktree(&repo);

        let store = Store::open(&dir.path().join("db.sqlite")).await.unwrap();
        store
            .create_thread(seeded_thread(&owner_key(&worktree), "claude-opus-4-8"))
            .await
            .unwrap();

        let seeded = latest_thread_or_seed(&store, &worktree).await.unwrap();
        assert_eq!(seeded.model, "claude-opus-4-8");
        assert_eq!(seeded.effort.as_deref(), Some("xhigh"));
    }

    #[tokio::test]
    async fn worktree_own_thread_wins_over_owner_seed() {
        if !git_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        init_repo(&repo);
        let worktree = add_worktree(&repo);

        let store = Store::open(&dir.path().join("db.sqlite")).await.unwrap();
        store
            .create_thread(seeded_thread(&owner_key(&worktree), "claude-opus-4-8"))
            .await
            .unwrap();
        store
            .create_thread(seeded_thread(
                &worktree.display().to_string(),
                "claude-haiku-4-8",
            ))
            .await
            .unwrap();

        let resolved = latest_thread_or_seed(&store, &worktree).await.unwrap();
        assert_eq!(resolved.model, "claude-haiku-4-8");
    }

    #[tokio::test]
    async fn plain_repo_does_not_seed() {
        if !git_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        init_repo(&repo);

        let store = Store::open(&dir.path().join("db.sqlite")).await.unwrap();
        store
            .create_thread(seeded_thread(
                &repo.display().to_string(),
                "claude-opus-4-8",
            ))
            .await
            .unwrap();

        let fresh = repo.join("sub");
        std::fs::create_dir(&fresh).unwrap();
        assert!(latest_thread_or_seed(&store, &fresh).await.is_none());
    }
}
