use color_eyre::eyre::eyre;
use goat_auth::{Credential, CredentialKey, CredentialStore, SecretString};
use goat_client::Link;
use goat_config::{ClientConfig, LOCAL_REMOTE, RemoteEntry};
use goat_remote::client::DeviceCredentials;

const KEY: &str = "key";
const CERT: &str = "cert";
const CA: &str = "ca";

pub struct Row {
    pub name: String,
    pub address: String,
    pub active: bool,
}

pub fn resolve(requested: Option<&str>) -> color_eyre::Result<Link> {
    let config = ClientConfig::load();
    let name = requested.or(config.default_remote.as_deref());
    match name {
        None | Some(LOCAL_REMOTE) => local(),
        Some(name) => {
            let entry = config
                .remotes
                .get(name)
                .ok_or_else(|| eyre!("no such remote: {name}"))?;
            let credentials = load_credentials(name, &entry.fingerprint)?;
            Ok(Link::remote(
                name.to_owned(),
                entry.host.clone(),
                credentials,
            ))
        }
    }
}

pub fn local() -> color_eyre::Result<Link> {
    let socket_path =
        goat_config::socket_path().ok_or_else(|| eyre!(goat_config::HOME_NOT_FOUND))?;
    let daemon_exe = std::env::current_exe()?;
    Ok(Link::local(socket_path, daemon_exe))
}

pub async fn add(
    name: &str,
    host: &str,
    fingerprint: &str,
    code: &str,
) -> color_eyre::Result<bool> {
    if name == LOCAL_REMOTE {
        return Err(eyre!(
            "`{LOCAL_REMOTE}` names the local daemon and cannot be added"
        ));
    }
    let enrollment = goat_remote::client::enroll(host, code, fingerprint).await?;
    let store = credential_store()?;
    store.store(
        &CredentialKey::remote(name, KEY),
        Credential::ApiKey(SecretString::from(enrollment.key_pem)),
    )?;
    store.store(
        &CredentialKey::remote(name, CERT),
        Credential::ApiKey(SecretString::from(enrollment.cert_pem)),
    )?;
    store.store(
        &CredentialKey::remote(name, CA),
        Credential::ApiKey(SecretString::from(enrollment.ca_cert_pem)),
    )?;

    let mut config = ClientConfig::load();
    config.remotes.insert(
        name.to_owned(),
        RemoteEntry {
            host: host.to_owned(),
            fingerprint: fingerprint.to_owned(),
            last_dir: None,
        },
    );
    let promoted = config.default_remote.is_none();
    if promoted {
        config.default_remote = Some(name.to_owned());
    }
    config.save()?;
    Ok(promoted)
}

pub fn list() -> Vec<Row> {
    let config = ClientConfig::load();
    let active = config.default_remote.as_deref().unwrap_or(LOCAL_REMOTE);
    let socket = goat_config::socket_path()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let mut rows = vec![Row {
        name: LOCAL_REMOTE.to_owned(),
        address: socket,
        active: active == LOCAL_REMOTE,
    }];
    for (name, entry) in &config.remotes {
        rows.push(Row {
            name: name.clone(),
            address: entry.host.clone(),
            active: active == name,
        });
    }
    rows
}

pub fn remove(name: &str) -> color_eyre::Result<bool> {
    let mut config = ClientConfig::load();
    if config.remotes.remove(name).is_none() {
        return Ok(false);
    }
    if config.default_remote.as_deref() == Some(name) {
        config.default_remote = None;
    }
    config.save()?;
    let store = credential_store()?;
    for slot in [KEY, CERT, CA] {
        let _ = store.remove(&CredentialKey::remote(name, slot));
    }
    Ok(true)
}

pub fn select(name: &str) -> color_eyre::Result<()> {
    let mut config = ClientConfig::load();
    if name == LOCAL_REMOTE {
        config.default_remote = None;
    } else if config.remotes.contains_key(name) {
        config.default_remote = Some(name.to_owned());
    } else {
        return Err(eyre!("no such remote: {name}"));
    }
    config.save()?;
    Ok(())
}

pub fn remember_dir(link: &Link, dir: &str) {
    let Link::Remote { name, .. } = link else {
        return;
    };
    let mut config = ClientConfig::load();
    let Some(entry) = config.remotes.get_mut(name) else {
        return;
    };
    if entry.last_dir.as_deref() == Some(dir) {
        return;
    }
    entry.last_dir = Some(dir.to_owned());
    let _ = config.save();
}

pub fn last_dir(name: &str) -> Option<String> {
    ClientConfig::load()
        .remotes
        .get(name)
        .and_then(|entry| entry.last_dir.clone())
}

fn load_credentials(name: &str, fingerprint: &str) -> color_eyre::Result<DeviceCredentials> {
    let store = credential_store()?;
    let read = |slot: &str| -> color_eyre::Result<String> {
        match store.get(&CredentialKey::remote(name, slot)) {
            Some(Credential::ApiKey(secret)) => Ok(secret.expose().to_owned()),
            _ => Err(eyre!(
                "remote `{name}` is missing its {slot}; pair it again with `goat remote add`"
            )),
        }
    };
    Ok(DeviceCredentials {
        key_pem: read(KEY)?,
        cert_pem: read(CERT)?,
        ca_cert_pem: read(CA)?,
        server_fingerprint: fingerprint.to_owned(),
    })
}

fn credential_store() -> color_eyre::Result<CredentialStore> {
    let path = goat_config::auth_path().ok_or_else(|| eyre!(goat_config::HOME_NOT_FOUND))?;
    Ok(CredentialStore::new(path))
}
