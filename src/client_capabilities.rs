use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use rand::{RngCore, rngs::OsRng};
use sha2::{Digest, Sha256};
use std::{fmt, str::FromStr};

pub(crate) const VERIFIER_DOMAIN: &[u8] = b"bitgarth-client-key-verifier-v1\0";
pub(crate) const WRAP_DOMAIN: &[u8] = b"bitgarth-client-key-wrap-v1\0";

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct CapabilityId([u8; 32]);

impl CapabilityId {
    pub(crate) fn new() -> Self {
        let mut bytes = [0_u8; 32];
        OsRng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    pub(crate) const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub(crate) const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for CapabilityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("CapabilityId")
            .field(&self.to_string())
            .finish()
    }
}

impl fmt::Display for CapabilityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&URL_SAFE_NO_PAD.encode(self.0))
    }
}

impl FromStr for CapabilityId {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let decoded = URL_SAFE_NO_PAD
            .decode(value)
            .map_err(|_| "capability ID must be canonical unpadded base64url".to_owned())?;
        let bytes: [u8; 32] = decoded
            .try_into()
            .map_err(|_| "capability ID must decode to 32 bytes".to_owned())?;
        let capability_id = Self(bytes);
        if capability_id.to_string() != value {
            return Err("capability ID must be canonical unpadded base64url".to_owned());
        }
        Ok(capability_id)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ClientKeyVerifier([u8; 32]);

impl ClientKeyVerifier {
    pub(crate) fn from_raw_key(raw_key: &[u8; 32]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(VERIFIER_DOMAIN);
        hasher.update(raw_key);
        Self(hasher.finalize().into())
    }

    pub(crate) const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub(crate) const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for ClientKeyVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientKeyVerifier").finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClientPermission {
    BalancesRead,
}

impl ClientPermission {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::BalancesRead => "balances_read",
        }
    }

    pub(crate) fn from_db(value: &str) -> Result<Self, String> {
        match value {
            "balances_read" => Ok(Self::BalancesRead),
            _ => Err(format!("unknown client permission: {value}")),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ClientCapabilityRecord {
    pub(crate) capability_id: CapabilityId,
    pub(crate) user_id: crate::models::UserId,
    pub(crate) key_verifier: ClientKeyVerifier,
    pub(crate) wrapped_dek: Option<Vec<u8>>,
    pub(crate) wrap_nonce: Option<Vec<u8>>,
    pub(crate) permission: ClientPermission,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) expires_at: Option<DateTime<Utc>>,
    pub(crate) last_used_at: Option<DateTime<Utc>>,
    pub(crate) revoked_at: Option<DateTime<Utc>>,
}

impl fmt::Debug for ClientCapabilityRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientCapabilityRecord")
            .field("capability_id", &self.capability_id)
            .field("user_id", &self.user_id)
            .field("permission", &self.permission)
            .field("created_at", &self.created_at)
            .field("expires_at", &self.expires_at)
            .field("last_used_at", &self.last_used_at)
            .field("revoked_at", &self.revoked_at)
            .field(
                "has_wrap_material",
                &(self.wrapped_dek.is_some() && self.wrap_nonce.is_some()),
            )
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "server")]
const _: () = {
    // Pairing endpoints consume these constructors in later plan tasks.
    let _ = VERIFIER_DOMAIN;
    let _ = WRAP_DOMAIN;
    let _ = CapabilityId::new;
    let _ = CapabilityId::from_bytes;
    let _ = CapabilityId::as_bytes;
    let _ = ClientKeyVerifier::from_raw_key;
};

#[cfg(test)]
mod tests {
    use super::{CapabilityId, ClientKeyVerifier, ClientPermission, VERIFIER_DOMAIN, WRAP_DOMAIN};

    #[test]
    fn verifier_is_domain_separated_sha256() {
        let raw_key = [7_u8; 32];
        let verifier = ClientKeyVerifier::from_raw_key(&raw_key);

        assert_eq!(VERIFIER_DOMAIN, b"bitgarth-client-key-verifier-v1\0");
        assert_eq!(verifier.as_bytes().len(), 32);
        assert_ne!(verifier.as_bytes(), &raw_key);
    }

    #[test]
    fn capability_id_round_trips_as_canonical_base64url() {
        let capability_id = CapabilityId::from_bytes([11_u8; 32]);
        let encoded = capability_id.to_string();

        assert_eq!(encoded.len(), 43);
        assert_eq!(encoded.parse::<CapabilityId>().unwrap(), capability_id);
    }

    #[test]
    fn v1_permission_and_wrap_domain_are_immutable() {
        assert_eq!(ClientPermission::BalancesRead.as_str(), "balances_read");
        assert_eq!(WRAP_DOMAIN, b"bitgarth-client-key-wrap-v1\0");
    }
}
