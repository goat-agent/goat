use std::time::Duration;

use goat_remote::{Authority, Devices, Pairing};

#[tokio::test]
async fn pairing_code_is_single_use() {
    let pairing = Pairing::default();
    let code = pairing.mint("phone".to_owned()).await;
    assert!(pairing.claim(&code).await.is_some());
    assert!(pairing.claim(&code).await.is_none());
}

#[tokio::test]
async fn pairing_code_expires() {
    let pairing = Pairing::new(Duration::from_millis(10));
    let code = pairing.mint("phone".to_owned()).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(pairing.claim(&code).await.is_none());
}

#[tokio::test]
async fn pairing_codes_are_distinct_and_long() {
    let pairing = Pairing::default();
    let a = pairing.mint("a".to_owned()).await;
    let b = pairing.mint("b".to_owned()).await;
    assert_ne!(a, b);
    assert!(a.len() >= 20);
}

#[tokio::test]
async fn enroll_then_revoke_updates_allowlist() {
    let dir = tempfile::tempdir().unwrap();
    let devices = Devices::load(dir.path().join("devices.json")).unwrap();
    let allow = devices.allowlist();
    let device = goat_remote::Device {
        id: "abc123".to_owned(),
        label: "phone".to_owned(),
        fingerprint: "deadbeef".to_owned(),
        paired_at: 1,
    };
    devices.enroll(device).await.unwrap();
    assert!(devices.contains_fingerprint("deadbeef").await);

    let reloaded = Devices::load(dir.path().join("devices.json")).unwrap();
    assert!(reloaded.contains_fingerprint("deadbeef").await);

    assert!(devices.revoke("abc123").await.unwrap());
    assert!(!devices.contains_fingerprint("deadbeef").await);
    assert!(!devices.revoke("abc123").await.unwrap());
    let _ = allow;
}

#[test]
fn ca_signs_device_csr_with_consistent_fingerprint() {
    let dir = tempfile::tempdir().unwrap();
    let authority = Authority::load_or_create(dir.path(), &["127.0.0.1".to_owned()]).unwrap();

    let key = rcgen_keypair();
    let csr_pem = build_csr(&key);
    let signed = authority.sign_device_csr(&csr_pem).unwrap();
    let recomputed = goat_remote::fingerprint_pem(&signed.cert_pem).unwrap();
    assert_eq!(signed.fingerprint, recomputed);

    let reopened = Authority::load_or_create(dir.path(), &["127.0.0.1".to_owned()]).unwrap();
    assert_eq!(
        reopened.server_fingerprint(),
        authority.server_fingerprint()
    );
}

fn rcgen_keypair() -> rcgen::KeyPair {
    rcgen::KeyPair::generate().unwrap()
}

fn build_csr(key: &rcgen::KeyPair) -> String {
    let mut params = rcgen::CertificateParams::new(vec!["device".to_owned()]).unwrap();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "device");
    params.serialize_request(key).unwrap().pem().unwrap()
}
