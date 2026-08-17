use crate::db::error::DbError;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExactPayloadBytes(Vec<u8>);

impl ExactPayloadBytes {
    pub(crate) fn try_new(bytes: Vec<u8>) -> Result<Self, DbError> {
        if bytes.is_empty() {
            return Err(DbError::new("exact payload bytes cannot be empty"));
        }
        Ok(Self(bytes))
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PayloadSha256Hex(String);

impl PayloadSha256Hex {
    pub(crate) fn from_payload(payload: &ExactPayloadBytes) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(payload.as_slice());
        Self(hex::encode(hasher.finalize()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}
