use std::sync::Arc;
use std::sync::RwLock;

use rustls::DistinguishedName;
use rustls::SignatureScheme;
use rustls::pki_types::{CertificateDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};

use crate::ca::fingerprint_der;

#[derive(Debug, Clone, Default)]
pub struct Allowlist {
    inner: Arc<RwLock<std::collections::HashSet<String>>>,
}

impl Allowlist {
    pub fn replace(&self, fingerprints: impl IntoIterator<Item = String>) {
        let mut guard = self.inner.write().expect("allowlist poisoned");
        *guard = fingerprints.into_iter().collect();
    }

    fn contains(&self, fingerprint: &str) -> bool {
        self.inner
            .read()
            .expect("allowlist poisoned")
            .contains(fingerprint)
    }
}

#[derive(Debug)]
pub struct DeviceVerifier {
    chain: Arc<dyn ClientCertVerifier>,
    allowlist: Allowlist,
}

impl DeviceVerifier {
    pub fn new(chain: Arc<dyn ClientCertVerifier>, allowlist: Allowlist) -> Self {
        Self { chain, allowlist }
    }
}

impl ClientCertVerifier for DeviceVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        self.chain.root_hint_subjects()
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        let verified = self
            .chain
            .verify_client_cert(end_entity, intermediates, now)?;
        let fingerprint = fingerprint_der(end_entity.as_ref());
        if self.allowlist.contains(&fingerprint) {
            Ok(verified)
        } else {
            Err(rustls::Error::General(
                "device certificate is not in the active registry".to_owned(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.chain.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.chain.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.chain.supported_verify_schemes()
    }

    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        false
    }
}
