use std::path::{Path, PathBuf};

use rcgen::{
    BasicConstraints, CertificateParams, CertificateSigningRequestParams, DnType, IsCa, Issuer,
    KeyPair, KeyUsagePurpose, SanType,
};
use sha2::{Digest, Sha256};

use crate::RemoteError;

pub struct Authority {
    dir: PathBuf,
    issuer: Issuer<'static, KeyPair>,
    ca_cert_pem: String,
    server_cert_pem: String,
    server_key_pem: String,
    server_fingerprint: String,
}

const CA_CERT: &str = "ca.crt";
const CA_KEY: &str = "ca.key";
const SERVER_CERT: &str = "server.crt";
const SERVER_KEY: &str = "server.key";

impl Authority {
    pub fn load_or_create(dir: &Path, advertised: &[String]) -> Result<Self, RemoteError> {
        std::fs::create_dir_all(dir)?;
        let ca_cert_path = dir.join(CA_CERT);
        let ca_key_path = dir.join(CA_KEY);

        let (ca_cert_pem, ca_key_pem) = if ca_cert_path.exists() && ca_key_path.exists() {
            (
                std::fs::read_to_string(&ca_cert_path)?,
                std::fs::read_to_string(&ca_key_path)?,
            )
        } else {
            let key = KeyPair::generate()?;
            let mut params = CertificateParams::default();
            params
                .distinguished_name
                .push(DnType::CommonName, "goat-code remote CA");
            params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
            params.key_usages = vec![
                KeyUsagePurpose::KeyCertSign,
                KeyUsagePurpose::CrlSign,
                KeyUsagePurpose::DigitalSignature,
            ];
            let cert = params.self_signed(&key)?;
            let cert_pem = cert.pem();
            let key_pem = key.serialize_pem();
            write_secret(&ca_key_path, key_pem.as_bytes())?;
            std::fs::write(&ca_cert_path, cert_pem.as_bytes())?;
            (cert_pem, key_pem)
        };

        let ca_key = KeyPair::from_pem(&ca_key_pem)?;
        let issuer = Issuer::from_ca_cert_pem(&ca_cert_pem, ca_key)?;

        let server_cert_path = dir.join(SERVER_CERT);
        let server_key_path = dir.join(SERVER_KEY);
        let (server_cert_pem, server_key_pem) =
            generate_server_leaf(&server_cert_path, &server_key_path, &issuer, advertised)?;
        let server_fingerprint = fingerprint_pem(&server_cert_pem)?;

        Ok(Self {
            dir: dir.to_path_buf(),
            issuer,
            ca_cert_pem,
            server_cert_pem,
            server_key_pem,
            server_fingerprint,
        })
    }

    pub fn ca_cert_pem(&self) -> &str {
        &self.ca_cert_pem
    }

    pub fn server_cert_pem(&self) -> &str {
        &self.server_cert_pem
    }

    pub fn server_key_pem(&self) -> &str {
        &self.server_key_pem
    }

    pub fn server_fingerprint(&self) -> &str {
        &self.server_fingerprint
    }

    pub fn sign_device_csr(&self, csr_pem: &str) -> Result<SignedDevice, RemoteError> {
        let params = CertificateSigningRequestParams::from_pem(csr_pem)?;
        let cert = params.signed_by(&self.issuer)?;
        let cert_pem = cert.pem();
        let fingerprint = fingerprint_pem(&cert_pem)?;
        Ok(SignedDevice {
            cert_pem,
            fingerprint,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

pub struct SignedDevice {
    pub cert_pem: String,
    pub fingerprint: String,
}

fn generate_server_leaf(
    cert_path: &Path,
    key_path: &Path,
    issuer: &Issuer<'static, KeyPair>,
    advertised: &[String],
) -> Result<(String, String), RemoteError> {
    if cert_path.exists() && key_path.exists() {
        return Ok((
            std::fs::read_to_string(cert_path)?,
            std::fs::read_to_string(key_path)?,
        ));
    }
    let key = KeyPair::generate()?;
    let mut params = CertificateParams::default();
    params
        .distinguished_name
        .push(DnType::CommonName, "goat-code remote server");
    params.subject_alt_names = advertised.iter().map(|s| san_for(s)).collect();
    let cert = params.signed_by(&key, issuer)?;
    let cert_pem = cert.pem();
    let key_pem = key.serialize_pem();
    write_secret(key_path, key_pem.as_bytes())?;
    std::fs::write(cert_path, cert_pem.as_bytes())?;
    Ok((cert_pem, key_pem))
}

fn san_for(value: &str) -> SanType {
    if let Ok(ip) = value.parse::<std::net::IpAddr>() {
        SanType::IpAddress(ip)
    } else {
        SanType::DnsName(value.to_owned().try_into().unwrap_or_else(|_| {
            "localhost"
                .to_owned()
                .try_into()
                .expect("localhost is valid")
        }))
    }
}

pub fn fingerprint_pem(pem: &str) -> Result<String, RemoteError> {
    let mut reader = pem.as_bytes();
    let item = rustls_pemfile::certs(&mut reader)
        .next()
        .ok_or(RemoteError::Pem)?
        .map_err(|_| RemoteError::Pem)?;
    Ok(fingerprint_der(item.as_ref()))
}

pub fn fingerprint_der(der: &[u8]) -> String {
    use std::fmt::Write;
    let digest = Sha256::digest(der);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(unix)]
fn write_secret(path: &Path, bytes: &[u8]) -> Result<(), RemoteError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    Ok(())
}

#[cfg(not(unix))]
fn write_secret(path: &Path, bytes: &[u8]) -> Result<(), RemoteError> {
    std::fs::write(path, bytes)?;
    Ok(())
}
