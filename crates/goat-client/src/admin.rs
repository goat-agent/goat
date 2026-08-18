use goat_api::{
    AdminConfigEdit, AdminConfigEditParams, AdminCredentialRemove, AdminCredentialRemoveParams,
    AdminCredentialSet, AdminCredentialSetOutput, AdminCredentialSetParams, Api,
};
use goat_auth::{Credential, CredentialKey, CredentialStore, CredentialValue, SecretString};
use goat_protocol::{Event, NotifyKind};
use goat_providers::Registry;
use tokio::sync::mpsc;

use crate::{ClientError, Link};

pub enum AdminRequest {
    ConfigEdit(Vec<goat_api::ConfigEdit>),
    CredentialSet {
        key: CredentialKey,
        value: CredentialValue,
    },
    CredentialRemove {
        key: CredentialKey,
    },
    ProviderLogin {
        provider: String,
        account: String,
        method: LoginMethod,
    },
}

pub enum LoginMethod {
    ApiKey(String),
    OAuth,
}

pub(crate) async fn dispatch(
    api: &Api,
    link: &Link,
    request: AdminRequest,
    events: &mpsc::Sender<Event>,
) {
    match request {
        AdminRequest::ConfigEdit(edits) => {
            let _ = api
                .call::<AdminConfigEdit>(AdminConfigEditParams { edits })
                .await;
        }
        AdminRequest::CredentialSet { key, value } => {
            let _ = api
                .call::<AdminCredentialSet>(AdminCredentialSetParams { key, value })
                .await;
        }
        AdminRequest::CredentialRemove { key } => {
            let _ = api
                .call::<AdminCredentialRemove>(AdminCredentialRemoveParams { key })
                .await;
        }
        AdminRequest::ProviderLogin {
            provider,
            account,
            method,
        } => {
            if !link.is_local() {
                login_failed(
                    &provider,
                    "a remote daemon reads its own credentials — log in on its host".to_owned(),
                    events,
                )
                .await;
                return;
            }
            let api = api.clone();
            let events = events.clone();
            tokio::spawn(async move { run_login(&api, provider, account, method, &events).await });
        }
    }
}

fn local_world() -> Result<(CredentialStore, goat_config::UserProviders), String> {
    let auth = goat_config::auth_path().ok_or(goat_config::HOME_NOT_FOUND)?;
    let config = goat_config::config_path().ok_or(goat_config::HOME_NOT_FOUND)?;
    Ok((
        CredentialStore::new(auth),
        goat_config::UserProviders::at(config),
    ))
}

async fn acquire(
    provider: &str,
    account: &str,
    method: LoginMethod,
    events: &mpsc::Sender<Event>,
) -> Result<CredentialValue, String> {
    let (store, user) = local_world()?;
    let key = CredentialKey::model(provider, account);
    if store.entries().iter().any(|(stored, _)| stored == &key) {
        return Err(format!("account '{account}' already exists"));
    }
    match method {
        LoginMethod::ApiKey(secret) => Ok(CredentialValue::from(Credential::ApiKey(
            SecretString::from(secret),
        ))),
        LoginMethod::OAuth => {
            let registry = Registry::load(&store, &user, account);
            let tokens = run_oauth(&registry, provider, events).await?;
            Ok(CredentialValue::from(Credential::OAuth(tokens)))
        }
    }
}

async fn run_oauth(
    registry: &Registry,
    provider: &str,
    events: &mpsc::Sender<Event>,
) -> Result<goat_auth::TokenSet, String> {
    let (status_tx, mut status_rx) = mpsc::channel::<String>(8);
    let forward_provider = provider.to_owned();
    let forward_events = events.clone();
    let forwarder = tokio::spawn(async move {
        while let Some(message) = status_rx.recv().await {
            let _ = forward_events
                .send(Event::LoginStatus {
                    provider: forward_provider.clone(),
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

async fn run_login(
    api: &Api,
    provider: String,
    account: String,
    method: LoginMethod,
    events: &mpsc::Sender<Event>,
) {
    let value = match acquire(&provider, &account, method, events).await {
        Ok(value) => value,
        Err(message) => return login_failed(&provider, message, events).await,
    };
    let key = CredentialKey::model(provider.clone(), account);
    match api
        .call::<AdminCredentialSet>(AdminCredentialSetParams { key, value })
        .await
    {
        Ok(outcome) => login_ok(&provider, outcome_note(outcome), events).await,
        Err(err) => login_failed(&provider, err.message, events).await,
    }
}

fn outcome_note(outcome: AdminCredentialSetOutput) -> String {
    match outcome {
        AdminCredentialSetOutput::Verified => String::new(),
        AdminCredentialSetOutput::NotVerifiable => {
            "stored but not verified; validation will happen on first request".to_owned()
        }
        AdminCredentialSetOutput::VerificationFailed { message } => {
            format!("stored but not verified: {message}")
        }
    }
}

async fn login_ok(provider: &str, message: String, events: &mpsc::Sender<Event>) {
    let notice = if message.is_empty() {
        format!("{provider} connected")
    } else {
        format!("{provider} {message}")
    };
    let _ = events
        .send(Event::Notify {
            kind: NotifyKind::Success,
            message: notice,
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

async fn login_failed(provider: &str, message: String, events: &mpsc::Sender<Event>) {
    let _ = events
        .send(Event::LoginStatus {
            provider: provider.to_owned(),
            message,
            done: true,
            ok: false,
        })
        .await;
}

pub async fn edit_config(
    link: &Link,
    edits: Vec<goat_api::ConfigEdit>,
) -> Result<bool, ClientError> {
    crate::admin_call(link, |api| async move {
        api.call::<AdminConfigEdit>(AdminConfigEditParams { edits })
            .await
    })
    .await
    .map(|out| out.changed)
}

pub async fn set_credential(
    link: &Link,
    key: CredentialKey,
    value: CredentialValue,
) -> Result<AdminCredentialSetOutput, ClientError> {
    crate::admin_call(link, |api| async move {
        api.call::<AdminCredentialSet>(AdminCredentialSetParams { key, value })
            .await
    })
    .await
}

pub async fn remove_credential(link: &Link, key: CredentialKey) -> Result<bool, ClientError> {
    crate::admin_call(link, |api| async move {
        api.call::<AdminCredentialRemove>(AdminCredentialRemoveParams { key })
            .await
    })
    .await
    .map(|out| out.removed)
}
