use super::super::*;
use super::support::*;

#[test]
fn payload_sha256_hex_is_lowercase_and_deterministic() {
    let payload = sample_payload(r#"{"txid":"abc"}"#);
    let hash = PayloadSha256Hex::from_payload(&payload);
    assert_eq!(
        hash.as_str(),
        "ce33091f945d89b755026e2842615f09a4706652bf6b71deb344e2e890139fb4"
    );
    assert!(hash.as_str().chars().all(|ch| ch.is_ascii_hexdigit()));
    assert_eq!(hash.as_str(), hash.as_str().to_lowercase());
}
