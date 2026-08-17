#![cfg(feature = "server")]

use super::types::{
    CAPABILITY_SCHEMA_VERSION_LEGACY, CAPABILITY_SCHEMA_VERSION_V3, EntitlementHolderId,
    EntitlementSource, FeatureEntitlements, TokenClaims,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
#[cfg(all(test, not(bitgarth_db_unit_only)))]
use once_cell::sync::Lazy;
use sha2::{Digest, Sha256};
use std::fmt;
#[cfg(all(test, not(bitgarth_db_unit_only)))]
use std::sync::Mutex;

pub(crate) const APP_SIGNING_PUBLIC_KEY_B64: &str = "gz-MZ_pYAbUp2G4Yy6sfyge4pZCMApSRw_IFTnkYMa0";
#[cfg(any(feature = "dev-config", all(test, not(bitgarth_db_unit_only))))]
const PAYMENT_SIGNING_PUBLIC_KEY_ENV: &str = "BITGARTH_PAYMENT_SIGNING_PUBLIC_KEY_B64";

#[cfg(all(test, not(bitgarth_db_unit_only)))]
static SIGNING_PUBLIC_KEY_OVERRIDE: Lazy<Mutex<Option<String>>> = Lazy::new(|| Mutex::new(None));

#[cfg(all(test, not(bitgarth_db_unit_only)))]
pub(crate) struct SigningPublicKeyOverrideGuard;

#[cfg(all(test, not(bitgarth_db_unit_only)))]
impl Drop for SigningPublicKeyOverrideGuard {
    fn drop(&mut self) {
        let mut override_value = SIGNING_PUBLIC_KEY_OVERRIDE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *override_value = None;
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
pub(crate) fn set_signing_public_key_override_for_test(
    public_key_b64: &str,
) -> SigningPublicKeyOverrideGuard {
    let mut override_value = SIGNING_PUBLIC_KEY_OVERRIDE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *override_value = Some(public_key_b64.to_string());
    SigningPublicKeyOverrideGuard
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VerifiedEntitlementToken {
    pub(crate) compact_token: String,
    pub(crate) claims: TokenClaims,
    pub(crate) entitlements: FeatureEntitlements,
}

pub(crate) fn expected_signing_key_hash() -> Result<String, EntitlementTokenError> {
    let public_key = decode_public_key(&app_signing_public_key_b64())?;
    Ok(URL_SAFE_NO_PAD.encode(Sha256::digest(public_key)))
}

pub(crate) fn verify_entitlement_token(
    compact_token: &str,
    expected_holder_id: EntitlementHolderId,
    now: DateTime<Utc>,
) -> Result<VerifiedEntitlementToken, EntitlementTokenError> {
    let (encoded_claims, encoded_signature) = compact_token
        .split_once('.')
        .ok_or(EntitlementTokenError::MalformedCompactToken)?;
    if encoded_signature.contains('.') {
        return Err(EntitlementTokenError::MalformedCompactToken);
    }

    let claims_bytes = URL_SAFE_NO_PAD
        .decode(encoded_claims)
        .map_err(|_| EntitlementTokenError::InvalidBase64)?;
    let signature_bytes: [u8; 64] = URL_SAFE_NO_PAD
        .decode(encoded_signature)
        .map_err(|_| EntitlementTokenError::InvalidBase64)?
        .try_into()
        .map_err(|_| EntitlementTokenError::InvalidSignature)?;
    let signature = Signature::from_bytes(&signature_bytes);
    let public_key = decode_public_key(&app_signing_public_key_b64())?;
    let verifying_key = VerifyingKey::from_bytes(&public_key)
        .map_err(|_| EntitlementTokenError::InvalidPublicKey)?;
    verifying_key
        .verify(&claims_bytes, &signature)
        .map_err(|_| EntitlementTokenError::InvalidSignature)?;

    let claims: TokenClaims =
        serde_json::from_slice(&claims_bytes).map_err(|_| EntitlementTokenError::InvalidClaims)?;
    validate_claims(&claims, expected_holder_id, now)?;

    let entitlements = FeatureEntitlements::from_capabilities(
        claims.tier.clone(),
        claims.capability_schema_version,
        claims.capabilities.clone(),
        Some(claims.subscription_valid_until),
        Some(claims.token_expires_at),
        EntitlementSource::SignedCentralToken,
    );

    Ok(VerifiedEntitlementToken {
        compact_token: compact_token.to_string(),
        claims,
        entitlements,
    })
}

pub(crate) fn verify_premium_token(
    compact_token: &str,
    expected_holder_id: EntitlementHolderId,
    now: DateTime<Utc>,
) -> Result<VerifiedEntitlementToken, EntitlementTokenError> {
    verify_entitlement_token(compact_token, expected_holder_id, now)
}

fn app_signing_public_key_b64() -> String {
    #[cfg(all(test, not(bitgarth_db_unit_only)))]
    {
        if let Some(value) = SIGNING_PUBLIC_KEY_OVERRIDE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        {
            return value;
        }
    }

    #[cfg(any(feature = "dev-config", all(test, not(bitgarth_db_unit_only))))]
    {
        if let Ok(value) = std::env::var(PAYMENT_SIGNING_PUBLIC_KEY_ENV) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }

    APP_SIGNING_PUBLIC_KEY_B64.to_string()
}

fn decode_public_key(value: &str) -> Result<[u8; 32], EntitlementTokenError> {
    URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| EntitlementTokenError::InvalidPublicKey)?
        .try_into()
        .map_err(|_| EntitlementTokenError::InvalidPublicKey)
}

fn validate_claims(
    claims: &TokenClaims,
    expected_holder_id: EntitlementHolderId,
    now: DateTime<Utc>,
) -> Result<(), EntitlementTokenError> {
    if claims.entitlement_holder_id != expected_holder_id {
        return Err(EntitlementTokenError::WrongEntitlementHolder);
    }
    if !matches!(
        claims.capability_schema_version,
        CAPABILITY_SCHEMA_VERSION_LEGACY | CAPABILITY_SCHEMA_VERSION_V3
    ) {
        return Err(EntitlementTokenError::UnsupportedCapabilitySchemaVersion);
    }
    if claims.subscription_valid_until <= now {
        return Err(EntitlementTokenError::SubscriptionExpired);
    }
    if claims.token_expires_at <= now {
        return Err(EntitlementTokenError::TokenExpired);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EntitlementTokenError {
    InvalidPublicKey,
    MalformedCompactToken,
    InvalidBase64,
    InvalidSignature,
    InvalidClaims,
    WrongEntitlementHolder,
    UnsupportedCapabilitySchemaVersion,
    SubscriptionExpired,
    TokenExpired,
}

pub(crate) type PremiumTokenError = EntitlementTokenError;

impl fmt::Display for EntitlementTokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPublicKey => write!(f, "payment signing public key is invalid"),
            Self::MalformedCompactToken => write!(f, "premium token must be claims.signature"),
            Self::InvalidBase64 => write!(f, "premium token uses invalid base64url encoding"),
            Self::InvalidSignature => write!(f, "premium token signature is invalid"),
            Self::InvalidClaims => write!(f, "premium token claims are invalid"),
            Self::WrongEntitlementHolder => {
                write!(f, "premium token belongs to a different holder")
            }
            Self::UnsupportedCapabilitySchemaVersion => {
                write!(f, "premium token capability schema version is unsupported")
            }
            Self::SubscriptionExpired => write!(f, "premium subscription is expired"),
            Self::TokenExpired => write!(f, "premium token offline access is expired"),
        }
    }
}

impl std::error::Error for EntitlementTokenError {}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use crate::payments::types::{
        CAPABILITY_SCHEMA_VERSION_V3, EntitlementCapabilities, EntitlementTier,
        SubscriptionSubjectId, TokenId,
    };
    use ed25519_dalek::{Signer, SigningKey};
    use std::error::Error;
    use std::str::FromStr;

    const TEST_PUBLIC_KEY_B64: &str = "O2onvM62pC1io6jQKm8Nc2UyFXcd4kOmOsBIoYtZ2ik";

    fn holder_id() -> EntitlementHolderId {
        EntitlementHolderId::from_str("01JQABCDEF000000000000000A")
            .unwrap_or_else(|_| unreachable!("hardcoded holder id is valid"))
    }

    fn now() -> DateTime<Utc> {
        "2026-04-16T12:00:00Z"
            .parse()
            .unwrap_or_else(|_| unreachable!("hardcoded timestamp is valid"))
    }

    fn claims_for(holder: EntitlementHolderId) -> TokenClaims {
        TokenClaims {
            token_id: TokenId::from_str("01JQABCDEF000000000000000B")
                .unwrap_or_else(|_| unreachable!("hardcoded token id is valid")),
            subscription_subject_id: SubscriptionSubjectId::from_str("01JQABCDEF000000000000000C")
                .unwrap_or_else(|_| unreachable!("hardcoded subject id is valid")),
            entitlement_holder_id: holder,
            tier: EntitlementTier::Premium,
            capability_set_id: Some("capset_premium_v1".to_string()),
            capability_schema_version: CAPABILITY_SCHEMA_VERSION_V3,
            capabilities: EntitlementCapabilities::v3_from_parts(50, 50000, true),
            subscription_valid_until: "2027-04-16T12:00:00Z"
                .parse()
                .unwrap_or_else(|_| unreachable!("hardcoded timestamp is valid")),
            token_expires_at: "2026-04-23T12:00:00Z"
                .parse()
                .unwrap_or_else(|_| unreachable!("hardcoded timestamp is valid")),
            issued_at: now(),
        }
    }

    fn sign_test_token(claims: &TokenClaims) -> Result<String, Box<dyn Error>> {
        let claims_json = serde_json::to_vec(claims)?;
        let signing_key = SigningKey::from_bytes(&[0_u8; 32]);
        let signature = signing_key.sign(&claims_json);
        Ok(format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(claims_json),
            URL_SAFE_NO_PAD.encode(signature.to_bytes())
        ))
    }

    fn sign_claims_json(claims: serde_json::Value) -> Result<String, Box<dyn Error>> {
        let claims_json = serde_json::to_vec(&claims)?;
        let signing_key = SigningKey::from_bytes(&[0_u8; 32]);
        let signature = signing_key.sign(&claims_json);
        Ok(format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(claims_json),
            URL_SAFE_NO_PAD.encode(signature.to_bytes())
        ))
    }

    fn verify_test_token(
        token: &str,
        expected_holder_id: EntitlementHolderId,
        now: DateTime<Utc>,
    ) -> Result<VerifiedEntitlementToken, PremiumTokenError> {
        let (encoded_claims, encoded_signature) = token
            .split_once('.')
            .ok_or(PremiumTokenError::MalformedCompactToken)?;
        let claims_bytes = URL_SAFE_NO_PAD
            .decode(encoded_claims)
            .map_err(|_| PremiumTokenError::InvalidBase64)?;
        let signature_bytes: [u8; 64] = URL_SAFE_NO_PAD
            .decode(encoded_signature)
            .map_err(|_| PremiumTokenError::InvalidBase64)?
            .try_into()
            .map_err(|_| PremiumTokenError::InvalidSignature)?;
        let signature = Signature::from_bytes(&signature_bytes);
        let public_key = decode_public_key(TEST_PUBLIC_KEY_B64)?;
        VerifyingKey::from_bytes(&public_key)
            .map_err(|_| PremiumTokenError::InvalidPublicKey)?
            .verify(&claims_bytes, &signature)
            .map_err(|_| PremiumTokenError::InvalidSignature)?;
        let claims: TokenClaims =
            serde_json::from_slice(&claims_bytes).map_err(|_| PremiumTokenError::InvalidClaims)?;
        validate_claims(&claims, expected_holder_id, now)?;
        let entitlements = FeatureEntitlements::from_capabilities(
            claims.tier.clone(),
            claims.capability_schema_version,
            claims.capabilities.clone(),
            Some(claims.subscription_valid_until),
            Some(claims.token_expires_at),
            EntitlementSource::SignedCentralToken,
        );
        Ok(VerifiedEntitlementToken {
            compact_token: token.to_string(),
            claims,
            entitlements,
        })
    }

    #[test]
    fn expected_key_hash_matches_central_contract() -> Result<(), Box<dyn Error>> {
        assert_eq!(
            expected_signing_key_hash()?,
            "BBqcs8N6ZjOe9PdxCs9p4uFFlpRSgu0Qpd6-1xOlLug"
        );
        Ok(())
    }

    #[test]
    fn test_override_changes_expected_key_hash() -> Result<(), Box<dyn Error>> {
        let _guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
        assert_eq!(
            expected_signing_key_hash()?,
            "E545QOZLVJFyIIjZoNdBYo_IJuCUddNBp4Cs3jxLgHA"
        );
        Ok(())
    }

    #[test]
    fn app_verifier_rejects_malformed_token_before_signature_check() {
        assert_eq!(
            verify_premium_token("not-a-compact-token", holder_id(), now()).err(),
            Some(PremiumTokenError::MalformedCompactToken)
        );
    }

    #[test]
    fn verifier_accepts_valid_central_shaped_token() -> Result<(), Box<dyn Error>> {
        let claims = claims_for(holder_id());
        let token = sign_test_token(&claims)?;
        let verified = verify_test_token(&token, holder_id(), now())?;

        assert_eq!(verified.claims, claims);
        assert_eq!(verified.compact_token, token);
        assert_eq!(verified.entitlements.tier, EntitlementTier::Premium);
        assert_eq!(verified.entitlements.sync_account_slots_limit, 50);
        Ok(())
    }

    #[test]
    fn verifier_uses_v3_canonical_account_limit_and_history_flag() -> Result<(), Box<dyn Error>> {
        let token = sign_claims_json(serde_json::json!({
            "token_id": "01JQABCDEF000000000000000B",
            "subscription_subject_id": "01JQABCDEF000000000000000C",
            "entitlement_holder_id": holder_id(),
            "tier": "basic",
            "capability_set_id": "basic.v3",
            "capability_schema_version": 3,
            "capabilities": {
                "features": {
                    "historical_sync": false,
                    "transaction_history_sync": true,
                    "balance_sync": true,
                    "exchange_rates_current": true,
                    "exchange_rates_history": true,
                    "price_overrides": true,
                    "balance_assertions": true,
                    "hledger_export": true,
                    "tax_reports": true
                },
                "limits": {
                    "accounts": { "total": 10 },
                    "history": { "max_transactions_per_account": 5000 },
                    "synced_accounts": 2
                }
            },
            "subscription_valid_until": "2027-04-16T12:00:00Z",
            "token_expires_at": "2026-04-23T12:00:00Z",
            "issued_at": "2026-04-16T12:00:00Z"
        }))?;

        let _guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
        let verified = verify_entitlement_token(&token, holder_id(), now())?;

        assert_eq!(verified.claims.capability_schema_version, 3);
        assert_eq!(verified.entitlements.sync_account_slots_limit, 10);
        assert!(verified.entitlements.historical_backfill_enabled);
        Ok(())
    }

    #[test]
    fn verifier_rejects_unknown_capability_schema_version() -> Result<(), Box<dyn Error>> {
        let token = sign_claims_json(serde_json::json!({
            "token_id": "01JQABCDEF000000000000000B",
            "subscription_subject_id": "01JQABCDEF000000000000000C",
            "entitlement_holder_id": holder_id(),
            "tier": "premium",
            "capability_set_id": "premium.v4",
            "capability_schema_version": 4,
            "capabilities": {
                "features": {
                    "transaction_history_sync": true
                },
                "limits": {
                    "accounts": { "total": 50 },
                    "history": { "max_transactions_per_account": 50000 }
                }
            },
            "subscription_valid_until": "2027-04-16T12:00:00Z",
            "token_expires_at": "2026-04-23T12:00:00Z",
            "issued_at": "2026-04-16T12:00:00Z"
        }))?;

        let _guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
        assert_eq!(
            verify_entitlement_token(&token, holder_id(), now()).err(),
            Some(PremiumTokenError::UnsupportedCapabilitySchemaVersion)
        );
        Ok(())
    }

    #[test]
    fn verifier_preserves_legacy_v2_capabilities_without_schema_version()
    -> Result<(), Box<dyn Error>> {
        let token = sign_claims_json(serde_json::json!({
            "token_id": "01JQABCDEF000000000000000B",
            "subscription_subject_id": "01JQABCDEF000000000000000C",
            "entitlement_holder_id": holder_id(),
            "tier": "basic",
            "capability_set_id": "basic.v2",
            "capabilities": {
                "features": { "historical_sync": true, "background_sync": true },
                "limits": {
                    "synced_accounts": 10,
                    "history": { "max_transactions_per_account": 10000 }
                }
            },
            "subscription_valid_until": "2027-04-16T12:00:00Z",
            "token_expires_at": "2026-04-23T12:00:00Z",
            "issued_at": "2026-04-16T12:00:00Z"
        }))?;

        let _guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
        let verified = verify_entitlement_token(&token, holder_id(), now())?;

        assert_eq!(verified.claims.capability_schema_version, 2);
        assert_eq!(verified.entitlements.sync_account_slots_limit, 10);
        assert!(verified.entitlements.historical_backfill_enabled);
        Ok(())
    }

    #[test]
    fn verifier_rejects_malformed_and_invalid_base64_tokens() {
        assert_eq!(
            verify_test_token("missing-dot", holder_id(), now()).err(),
            Some(PremiumTokenError::MalformedCompactToken)
        );
        assert_eq!(
            verify_test_token("bad.bad", holder_id(), now()).err(),
            Some(PremiumTokenError::InvalidBase64)
        );
    }

    #[test]
    fn verifier_rejects_wrong_signature() -> Result<(), Box<dyn Error>> {
        let mut token = sign_test_token(&claims_for(holder_id()))?;
        token.push('A');

        assert_eq!(
            verify_test_token(&token, holder_id(), now()).err(),
            Some(PremiumTokenError::InvalidSignature)
        );
        Ok(())
    }

    #[test]
    fn verifier_rejects_wrong_holder() -> Result<(), Box<dyn Error>> {
        let token = sign_test_token(&claims_for(holder_id()))?;
        let other_holder = EntitlementHolderId::from_str("01JQABCDEF000000000000000D")?;

        assert_eq!(
            verify_test_token(&token, other_holder, now()).err(),
            Some(PremiumTokenError::WrongEntitlementHolder)
        );
        Ok(())
    }

    #[test]
    fn verifier_accepts_unknown_tier_with_known_capabilities() -> Result<(), Box<dyn Error>> {
        let mut claims = claims_for(holder_id());
        claims.tier = EntitlementTier::Unknown("enterprise".to_string());
        claims.capabilities = EntitlementCapabilities::v3_from_parts(100, 100000, true);
        let token = sign_test_token(&claims)?;
        let verified = verify_test_token(&token, holder_id(), now())?;

        assert_eq!(
            verified.entitlements.tier,
            EntitlementTier::Unknown("enterprise".to_string())
        );
        assert_eq!(verified.entitlements.sync_account_slots_limit, 100);
        assert_eq!(
            verified
                .entitlements
                .historical_backfill_transactions_per_account,
            100000
        );
        Ok(())
    }

    #[test]
    fn verifier_rejects_expiry() -> Result<(), Box<dyn Error>> {
        let mut claims = claims_for(holder_id());
        claims.subscription_valid_until = "2026-04-16T12:00:00Z".parse()?;
        let expired_subscription = sign_test_token(&claims)?;
        assert_eq!(
            verify_test_token(&expired_subscription, holder_id(), now()).err(),
            Some(PremiumTokenError::SubscriptionExpired)
        );

        let mut claims = claims_for(holder_id());
        claims.token_expires_at = "2026-04-16T12:00:00Z".parse()?;
        let expired_token = sign_test_token(&claims)?;
        assert_eq!(
            verify_test_token(&expired_token, holder_id(), now()).err(),
            Some(PremiumTokenError::TokenExpired)
        );
        Ok(())
    }
}
