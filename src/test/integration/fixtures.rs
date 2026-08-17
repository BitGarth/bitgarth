use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use bitcoin::Network as BitcoinNetwork;
use bitcoin::bip32::{ChildNumber, DerivationPath as BitcoinDerivationPath, Xpriv, Xpub};
use bitcoin::secp256k1::Secp256k1;
use chrono::{Duration, Utc};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{Value, json};
use std::str::FromStr;
use ulid::Ulid;

use crate::models::UserId;
use crate::payments::keys::{
    SigningPublicKeyOverrideGuard, set_signing_public_key_override_for_test,
};
use crate::payments::types::{
    CAPABILITY_SCHEMA_VERSION_V3, EntitlementCapabilities, EntitlementTier, SubscriptionSubjectId,
    TokenClaims, TokenId,
};

use super::IntegrationTestServer;

pub(crate) const TEST_NATIVE_SEGWIT_ZPUB: &str = "zpub6qU5MALAB8Bscej9sTEkgSocaxvLzAYYeytsL9fXfv8W4BTykA99FNDNpftwXMGomwc2KatVrbXo4qXsdBC1DiNHCHGapas9enpPBo8y8Y4";
const TEST_PUBLIC_KEY_B64: &str = "O2onvM62pC1io6jQKm8Nc2UyFXcd4kOmOsBIoYtZ2ik";
const TEST_TOKEN_ID: &str = "01JQABCDEF000000000000000F";
const TEST_SUBSCRIPTION_SUBJECT_ID: &str = "01JQABCDEF000000000000000G";

pub(crate) fn legal_acknowledgement_json() -> Value {
    json!({
        "accepted_terms_version": crate::legal::TERMS_VERSION,
        "accepted_privacy_version": crate::legal::PRIVACY_VERSION
    })
}

pub(crate) fn deterministic_test_xpub(account: u32) -> String {
    let secp = Secp256k1::new();
    let mut seed = [0_u8; 32];
    seed[0..4].copy_from_slice(&account.to_be_bytes());

    let master = match Xpriv::new_master(BitcoinNetwork::Bitcoin, &seed) {
        Ok(value) => value,
        Err(err) => panic!("deterministic master key should succeed: {err}"),
    };

    let path = BitcoinDerivationPath::from(vec![
        ChildNumber::Hardened { index: 84 },
        ChildNumber::Hardened { index: 0 },
        ChildNumber::Hardened { index: account },
    ]);

    let account_xpriv = match master.derive_priv(&secp, &path) {
        Ok(value) => value,
        Err(err) => panic!("deterministic account derivation should succeed: {err}"),
    };

    Xpub::from_priv(&secp, &account_xpriv).to_string()
}

#[derive(Debug, Clone)]
pub(crate) struct WalletAccountFixture {
    pub wallet_id: String,
    pub account_id: String,
    pub address_id: Option<String>,
}

pub(crate) async fn register_user_with_prefix(
    server: &IntegrationTestServer,
    username_prefix: &str,
) -> String {
    let username = format!("{username_prefix}_{}", Ulid::new());
    server
        .post("/_app/auth/register")
        .json(&json!({
            "username": username,
            "password": "SecurePass123",
            "legal_acknowledgement": legal_acknowledgement_json()
        }))
        .await
        .assert_status_ok();
    username
}

pub(crate) async fn register_user(server: &IntegrationTestServer) -> String {
    register_user_with_prefix(server, "integration_user").await
}

pub(crate) struct SignedEntitlementTestGuard {
    _key_guard: SigningPublicKeyOverrideGuard,
}

pub(crate) async fn current_user_id(server: &IntegrationTestServer) -> UserId {
    let response = server.get("/_app/auth/me").await;
    response.assert_status_ok();

    let body: Value = response.json();
    let user_id = body["user"]["user_id"]
        .as_str()
        .expect("auth me should include user_id");
    UserId::from_str(user_id).expect("auth me should return a valid user id")
}

pub(crate) async fn activate_signed_full_report_entitlements(
    server: &IntegrationTestServer,
) -> SignedEntitlementTestGuard {
    let key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let user_id = current_user_id(server).await;
    let now = Utc::now();
    let subject = crate::db::payments::load_or_create_payment_subject(user_id, now)
        .expect("payment subject should load");
    let claims = TokenClaims {
        token_id: TokenId::from_str(TEST_TOKEN_ID).expect("test token id should parse"),
        subscription_subject_id: SubscriptionSubjectId::from_str(TEST_SUBSCRIPTION_SUBJECT_ID)
            .expect("test subscription subject id should parse"),
        entitlement_holder_id: subject.entitlement_holder_id,
        tier: EntitlementTier::Premium,
        capability_set_id: Some("capset_premium_v1".to_string()),
        capability_schema_version: CAPABILITY_SCHEMA_VERSION_V3,
        capabilities: EntitlementCapabilities::v3_from_parts(50, 50000, true),
        subscription_valid_until: now + Duration::days(365),
        token_expires_at: now + Duration::days(7),
        issued_at: now - Duration::minutes(1),
    };
    let token = sign_test_token(&claims);
    let verified =
        crate::payments::keys::verify_premium_token(&token, subject.entitlement_holder_id, now)
            .expect("test entitlement token should verify");
    crate::db::payments::store_verified_premium_token(user_id, None, &verified, None, now)
        .expect("test entitlement token should store");

    SignedEntitlementTestGuard {
        _key_guard: key_guard,
    }
}

fn sign_test_token(claims: &TokenClaims) -> String {
    let claims_json = serde_json::to_vec(claims).expect("claims should serialize");
    let signing_key = SigningKey::from_bytes(&[0_u8; 32]);
    let signature = signing_key.sign(&claims_json);
    format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(claims_json),
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    )
}

pub(crate) async fn add_ethereum_wallet_account(
    server: &IntegrationTestServer,
    address: &str,
    wallet_label: &str,
) -> WalletAccountFixture {
    let response = server
        .post("/_app/user/wallets/ethereum/add")
        .json(&json!({
            "request": {
                "address": address,
                "network": "mainnet",
                "wallet_label": wallet_label
            }
        }))
        .await;
    response.assert_status_ok();

    let body: Value = response.json();
    let wallet_id = body["wallet_id"]
        .as_str()
        .expect("expected wallet_id")
        .to_string();
    let account_id = body["account_id"]
        .as_str()
        .expect("expected account_id")
        .to_string();
    let address_id = body["address_id"]
        .as_str()
        .expect("expected address_id")
        .to_string();

    WalletAccountFixture {
        wallet_id,
        account_id,
        address_id: Some(address_id),
    }
}

pub(crate) async fn select_account_sync_slot(server: &IntegrationTestServer, account_id: &str) {
    server
        .post("/_app/user/wallets/account/sync-slot/select")
        .json(&json!({
            "request": {
                "account_id": account_id
            }
        }))
        .await
        .assert_status_ok();
}

pub(crate) async fn add_native_segwit_xpub_account(
    server: &IntegrationTestServer,
    wallet_label: &str,
) -> WalletAccountFixture {
    add_xpub_wallet_account(
        server,
        TEST_NATIVE_SEGWIT_ZPUB,
        "native_segwit",
        None,
        Some(wallet_label),
    )
    .await
}

pub(crate) async fn add_xpub_wallet_account(
    server: &IntegrationTestServer,
    extended_pubkey: &str,
    address_scheme: &str,
    wallet_id: Option<&str>,
    wallet_label: Option<&str>,
) -> WalletAccountFixture {
    let mut request = serde_json::Map::new();
    request.insert(
        "extended_pubkey".to_string(),
        Value::String(extended_pubkey.to_string()),
    );
    request.insert(
        "address_scheme".to_string(),
        Value::String(address_scheme.to_string()),
    );
    if let Some(wallet_id) = wallet_id {
        request.insert(
            "wallet_id".to_string(),
            Value::String(wallet_id.to_string()),
        );
    }
    if let Some(wallet_label) = wallet_label {
        request.insert(
            "wallet_label".to_string(),
            Value::String(wallet_label.to_string()),
        );
    }

    let response = server
        .post("/_app/user/wallets/xpub/add")
        .json(&json!({ "request": Value::Object(request) }))
        .await;
    response.assert_status_ok();

    let body: Value = response.json();
    let wallet_id = body["wallet_id"]
        .as_str()
        .expect("expected wallet_id")
        .to_string();
    let account_id = body["account_id"]
        .as_str()
        .expect("expected account_id")
        .to_string();

    WalletAccountFixture {
        wallet_id,
        account_id,
        address_id: None,
    }
}
