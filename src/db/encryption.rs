//! Database encryption infrastructure for user databases.
//!
//! This module implements SQLCipher encryption for user databases using a DEK/KEK architecture:
//! - DEK (Data Encryption Key): Random 256-bit key used for SQLCipher encryption
//! - Password KEK: Derived from user password via Argon2id, wraps DEK in envelope
//! - Session KEK: Derived from session token + server secret via HKDF-SHA256, wraps DEK in session row
//!
//! Key wrapping uses AES-256-GCM. The DEK never changes for a database's lifetime,
//! enabling fast password changes (re-wrap only, not re-encryption).

use crate::client_capabilities::{CapabilityId, ClientPermission, WRAP_DOMAIN};
use crate::db::DbError;
use crate::models::{SessionId, UserId};
use aes_gcm::{
    AeadCore, Aes256Gcm, KeyInit,
    aead::{Aead, Payload},
};
use argon2::{Algorithm, Argon2, Params, ParamsBuilder, Version};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use chrono::{DateTime, Utc};
use hkdf::Hkdf;
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::path::PathBuf;
use ulid::Ulid;
use zeroize::{Zeroize, ZeroizeOnDrop};

pub(crate) const ARGON2_MEMORY_COST: u32 = 19456;
pub(crate) const ARGON2_TIME_COST: u32 = 2;
pub(crate) const ARGON2_PARALLELISM: u32 = 4;

const SESSION_KEK_INFO_PREFIX: &[u8] = b"bitgarth-session-kek-v1";

pub(crate) const SESSION_WRAP_SECRET_ENV: &str = "BITGARTH_SESSION_WRAP_SECRET";
const SESSION_WRAP_SECRET_FILENAME: &str = "session-wrap-secret";

static CACHED_SERVER_MASTER_SECRET: std::sync::Mutex<Option<ServerMasterSecret>> =
    std::sync::Mutex::new(None);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EncryptionError(String);

impl EncryptionError {
    pub(crate) fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

impl std::fmt::Display for EncryptionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for EncryptionError {}

impl From<EncryptionError> for DbError {
    fn from(err: EncryptionError) -> Self {
        DbError::new(err.to_string())
    }
}

#[derive(Clone, ZeroizeOnDrop)]
pub(crate) struct Dek([u8; 32]);

impl Dek {
    pub(crate) fn generate() -> Self {
        let mut bytes = [0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    pub(crate) fn as_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub(crate) fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for Dek {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dek").finish_non_exhaustive()
    }
}

#[derive(Clone, ZeroizeOnDrop)]
pub(crate) struct PasswordKek([u8; 32]);

impl PasswordKek {
    pub(crate) fn derive(password: &str, salt: &[u8]) -> Result<Self, EncryptionError> {
        let params = argon2_params()?;
        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

        let mut key = [0u8; 32];
        argon2
            .hash_password_into(password.as_bytes(), salt, &mut key)
            .map_err(|e| EncryptionError::new(format!("Password KEK derivation failed: {e}")))?;

        Ok(Self(key))
    }

    fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for PasswordKek {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PasswordKek").finish_non_exhaustive()
    }
}

#[derive(Clone, ZeroizeOnDrop)]
pub(crate) struct SessionKek([u8; 32]);

impl SessionKek {
    pub(crate) fn derive(
        server_secret: &[u8],
        raw_session_token: &str,
        session_id: SessionId,
        user_id: UserId,
    ) -> Result<Self, EncryptionError> {
        let token_bytes = BASE64.decode(raw_session_token).map_err(|e| {
            EncryptionError::new(format!("Session token base64 decode failed: {e}"))
        })?;

        let mut info = SESSION_KEK_INFO_PREFIX.to_vec();
        info.extend_from_slice(session_id.to_string().as_bytes());
        info.push(0);
        info.extend_from_slice(user_id.to_string().as_bytes());

        let hkdf: Hkdf<Sha256> = Hkdf::new(Some(server_secret), &token_bytes);

        let mut key = [0u8; 32];
        hkdf.expand(&info, &mut key)
            .map_err(|e| EncryptionError::new(format!("Session KEK derivation failed: {e}")))?;

        Ok(Self(key))
    }

    fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for SessionKek {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionKek").finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WrapId(String);

impl WrapId {
    pub(crate) fn new() -> Self {
        Self(Ulid::new().to_string())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum KekId {
    #[serde(rename = "password")]
    Password,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WrappedDek {
    pub(crate) wrap_id: WrapId,
    pub(crate) kek_id: KekId,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) salt: String,
    pub(crate) nonce: String,
    pub(crate) ciphertext: String,
}

impl WrappedDek {
    pub(crate) fn new_password_wrapper(dek: &Dek, password: &str) -> Result<Self, EncryptionError> {
        let mut salt = [0u8; 16];
        OsRng.fill_bytes(&mut salt);

        let kek = PasswordKek::derive(password, &salt)?;

        let (nonce, ciphertext) = wrap_dek(dek, kek.as_bytes())?;

        Ok(Self {
            wrap_id: WrapId::new(),
            kek_id: KekId::Password,
            created_at: Utc::now(),
            salt: BASE64.encode(salt),
            nonce: BASE64.encode(nonce),
            ciphertext: BASE64.encode(ciphertext),
        })
    }

    pub(crate) fn unwrap_with_password(&self, password: &str) -> Result<Dek, EncryptionError> {
        let salt = BASE64
            .decode(&self.salt)
            .map_err(|e| EncryptionError::new(format!("Salt base64 decode failed: {e}")))?;

        let kek = PasswordKek::derive(password, &salt)?;

        let nonce = BASE64
            .decode(&self.nonce)
            .map_err(|e| EncryptionError::new(format!("Nonce base64 decode failed: {e}")))?;

        let ciphertext = BASE64
            .decode(&self.ciphertext)
            .map_err(|e| EncryptionError::new(format!("Ciphertext base64 decode failed: {e}")))?;

        unwrap_dek(&nonce, &ciphertext, kek.as_bytes())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct SqlcipherCompatibility(u8);

impl SqlcipherCompatibility {
    pub(crate) fn as_u32(&self) -> u32 {
        u32::from(self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub(crate) enum DbEnvelope {
    Encrypted {
        sqlcipher_version: SqlcipherCompatibility,
        wrapped_keys: Vec<WrappedDek>,
    },
    #[cfg(feature = "dev-config")]
    UnencryptedDev,
}

impl DbEnvelope {
    pub(crate) fn new_encrypted(password: &str) -> Result<(Self, Dek), EncryptionError> {
        let dek = Dek::generate();
        let wrapped = WrappedDek::new_password_wrapper(&dek, password)?;
        let sqlcipher_version = current_sqlcipher_compatibility()?;

        Ok((
            Self::Encrypted {
                sqlcipher_version,
                wrapped_keys: vec![wrapped],
            },
            dek,
        ))
    }

    #[cfg(feature = "dev-config")]
    pub(crate) fn unencrypted_dev() -> Self {
        Self::UnencryptedDev
    }

    pub(crate) fn unwrap_with_password(&self, password: &str) -> Result<Dek, EncryptionError> {
        match self {
            Self::Encrypted { wrapped_keys, .. } => {
                unwrap_password_dek_from_wrappers(wrapped_keys, password)
            }
            #[cfg(feature = "dev-config")]
            Self::UnencryptedDev => Err(EncryptionError::new(
                "Cannot unwrap DEK from unencrypted envelope",
            )),
        }
    }

    pub(crate) fn sqlcipher_compatibility(&self) -> Option<SqlcipherCompatibility> {
        match self {
            Self::Encrypted {
                sqlcipher_version, ..
            } => Some(sqlcipher_version.clone()),
            #[cfg(feature = "dev-config")]
            Self::UnencryptedDev => None,
        }
    }

    pub(crate) fn add_password_wrapper(
        &mut self,
        dek: &Dek,
        password: &str,
    ) -> Result<WrapId, EncryptionError> {
        match self {
            Self::Encrypted { wrapped_keys, .. } => {
                let wrapped = WrappedDek::new_password_wrapper(dek, password)?;
                let wrap_id = wrapped.wrap_id.clone();
                wrapped_keys.push(wrapped);
                Ok(wrap_id)
            }
            #[cfg(feature = "dev-config")]
            Self::UnencryptedDev => Err(EncryptionError::new(
                "Cannot add wrapper to unencrypted envelope",
            )),
        }
    }

    pub(crate) fn compact_password_wrappers(&mut self, keep_wrap_id: &str) {
        match self {
            Self::Encrypted { wrapped_keys, .. } => {
                wrapped_keys.retain(|w| w.wrap_id.as_str() == keep_wrap_id);
            }
            #[cfg(feature = "dev-config")]
            Self::UnencryptedDev => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnlockAuthority {
    PasswordLogin,
    SessionRestore { session_id: SessionId },
    ClientKey { capability_id: CapabilityId },
}

#[derive(Debug, Clone)]
pub(crate) enum UserDbOpenMode {
    Encrypted {
        dek: Dek,
        authority: UnlockAuthority,
        sqlcipher_compatibility: SqlcipherCompatibility,
    },
    #[cfg(feature = "dev-config")]
    UnencryptedDev,
    #[cfg(all(test, feature = "db-tests"))]
    PlaintextTest,
}

#[derive(Clone)]
pub(crate) enum SessionCreationContext {
    Encrypted {
        dek: Dek,
        server_secret: ServerMasterSecret,
    },
    #[cfg(all(test, feature = "db-tests"))]
    PlaintextTest,
    #[cfg(feature = "dev-config")]
    UnencryptedDev,
}

pub(crate) struct SessionWrapper {
    pub(crate) nonce: Vec<u8>,
    pub(crate) ciphertext: Vec<u8>,
}

impl SessionWrapper {
    pub(crate) fn wrap(
        dek: &Dek,
        server_secret: &[u8],
        raw_session_token: &str,
        session_id: SessionId,
        user_id: UserId,
    ) -> Result<Self, EncryptionError> {
        let kek = SessionKek::derive(server_secret, raw_session_token, session_id, user_id)?;
        let (nonce, ciphertext) = wrap_dek(dek, kek.as_bytes())?;

        Ok(Self { nonce, ciphertext })
    }

    pub(crate) fn unwrap(
        &self,
        server_secret: &[u8],
        raw_session_token: &str,
        session_id: SessionId,
        user_id: UserId,
    ) -> Result<Dek, EncryptionError> {
        let kek = SessionKek::derive(server_secret, raw_session_token, session_id, user_id)?;
        unwrap_dek(&self.nonce, &self.ciphertext, kek.as_bytes())
    }

    pub(crate) fn nonce_base64(&self) -> String {
        BASE64.encode(&self.nonce)
    }

    pub(crate) fn ciphertext_base64(&self) -> String {
        BASE64.encode(&self.ciphertext)
    }

    pub(crate) fn from_base64(
        nonce_b64: &str,
        ciphertext_b64: &str,
    ) -> Result<Self, EncryptionError> {
        let nonce = BASE64
            .decode(nonce_b64)
            .map_err(|e| EncryptionError::new(format!("Nonce base64 decode failed: {e}")))?;

        let ciphertext = BASE64
            .decode(ciphertext_b64)
            .map_err(|e| EncryptionError::new(format!("Ciphertext base64 decode failed: {e}")))?;

        Ok(Self { nonce, ciphertext })
    }
}

#[derive(Clone, ZeroizeOnDrop)]
struct ClientKeyKek([u8; 32]);

impl ClientKeyKek {
    fn derive(raw_client_key: &[u8; 32], context: &[u8]) -> Result<Self, EncryptionError> {
        let hkdf: Hkdf<Sha256> = Hkdf::new(Some(WRAP_DOMAIN), raw_client_key);
        let mut key = [0_u8; 32];
        hkdf.expand(context, &mut key).map_err(|error| {
            EncryptionError::new(format!(
                "Client Key wrapping-key derivation failed: {error}"
            ))
        })?;
        Ok(Self(key))
    }
}

impl std::fmt::Debug for ClientKeyKek {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientKeyKek").finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ClientKeyWrapper {
    pub(crate) nonce: Vec<u8>,
    pub(crate) ciphertext: Vec<u8>,
}

impl std::fmt::Debug for ClientKeyWrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientKeyWrapper").finish_non_exhaustive()
    }
}

impl ClientKeyWrapper {
    pub(crate) fn wrap(
        dek: &Dek,
        raw_client_key: &[u8; 32],
        user_id: UserId,
        capability_id: CapabilityId,
        permission: ClientPermission,
    ) -> Result<Self, EncryptionError> {
        let context = client_key_context(user_id, capability_id, permission.as_str());
        let kek = ClientKeyKek::derive(raw_client_key, &context)?;
        let cipher = Aes256Gcm::new_from_slice(&kek.0)
            .map_err(|error| EncryptionError::new(format!("Failed to create cipher: {error}")))?;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: dek.as_bytes(),
                    aad: &context,
                },
            )
            .map_err(|error| {
                EncryptionError::new(format!("Client Key DEK encryption failed: {error}"))
            })?;
        Ok(Self {
            nonce: nonce.to_vec(),
            ciphertext,
        })
    }

    pub(crate) fn unwrap(
        &self,
        raw_client_key: &[u8; 32],
        user_id: UserId,
        capability_id: CapabilityId,
        permission: ClientPermission,
    ) -> Result<Dek, EncryptionError> {
        self.unwrap_with_permission_value(
            raw_client_key,
            user_id,
            capability_id,
            permission.as_str(),
        )
    }

    fn unwrap_with_permission_value(
        &self,
        raw_client_key: &[u8; 32],
        user_id: UserId,
        capability_id: CapabilityId,
        permission: &str,
    ) -> Result<Dek, EncryptionError> {
        let context = client_key_context(user_id, capability_id, permission);
        let kek = ClientKeyKek::derive(raw_client_key, &context)?;
        let cipher = Aes256Gcm::new_from_slice(&kek.0)
            .map_err(|error| EncryptionError::new(format!("Failed to create cipher: {error}")))?;
        let nonce: [u8; 12] = self
            .nonce
            .as_slice()
            .try_into()
            .map_err(|_| EncryptionError::new("Nonce must be 12 bytes"))?;
        let plaintext = cipher
            .decrypt(
                (&nonce).into(),
                Payload {
                    msg: &self.ciphertext,
                    aad: &context,
                },
            )
            .map_err(|error| {
                EncryptionError::new(format!("Client Key DEK decryption failed: {error}"))
            })?;
        let bytes: [u8; 32] = plaintext
            .try_into()
            .map_err(|_| EncryptionError::new("DEK is not 32 bytes"))?;
        Ok(Dek::from_bytes(bytes))
    }

    #[cfg(all(test, feature = "db-tests"))]
    pub(crate) fn unwrap_with_permission_for_test(
        &self,
        raw_client_key: &[u8; 32],
        user_id: UserId,
        capability_id: CapabilityId,
        permission: &str,
    ) -> Result<Dek, EncryptionError> {
        self.unwrap_with_permission_value(raw_client_key, user_id, capability_id, permission)
    }
}

fn client_key_context(user_id: UserId, capability_id: CapabilityId, permission: &str) -> Vec<u8> {
    let mut context = Vec::with_capacity(
        WRAP_DOMAIN.len()
            + user_id.to_string().len()
            + capability_id.to_string().len()
            + permission.len()
            + 3,
    );
    context.extend_from_slice(WRAP_DOMAIN);
    context.extend_from_slice(user_id.to_string().as_bytes());
    context.push(0);
    context.extend_from_slice(capability_id.to_string().as_bytes());
    context.push(0);
    context.extend_from_slice(permission.as_bytes());
    context.push(0);
    context
}

const _: () = {
    // Public API endpoints consume capability unlocks in later plan tasks.
    let _ = ClientKeyWrapper::wrap;
    let _ = ClientKeyWrapper::unwrap;
    let _ = UnlockAuthority::ClientKey {
        capability_id: CapabilityId::from_bytes([0_u8; 32]),
    };
};

fn wrap_dek(dek: &Dek, kek: &[u8; 32]) -> Result<(Vec<u8>, Vec<u8>), EncryptionError> {
    let cipher = Aes256Gcm::new_from_slice(kek)
        .map_err(|e| EncryptionError::new(format!("Failed to create cipher: {e}")))?;

    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);

    let ciphertext = cipher
        .encrypt(&nonce, dek.as_hex().as_bytes())
        .map_err(|e| EncryptionError::new(format!("DEK encryption failed: {e}")))?;

    Ok((nonce.to_vec(), ciphertext))
}

fn unwrap_dek(nonce: &[u8], ciphertext: &[u8], kek: &[u8; 32]) -> Result<Dek, EncryptionError> {
    let cipher = Aes256Gcm::new_from_slice(kek)
        .map_err(|e| EncryptionError::new(format!("Failed to create cipher: {e}")))?;

    let nonce_arr: [u8; 12] = nonce
        .try_into()
        .map_err(|_| EncryptionError::new("Nonce must be 12 bytes"))?;

    let plaintext = cipher
        .decrypt((&nonce_arr).into(), ciphertext)
        .map_err(|e| EncryptionError::new(format!("DEK decryption failed: {e}")))?;

    let hex_str = String::from_utf8(plaintext)
        .map_err(|e| EncryptionError::new(format!("DEK is not valid UTF-8: {e}")))?;

    let bytes = hex::decode(&hex_str)
        .map_err(|e| EncryptionError::new(format!("DEK is not valid hex: {e}")))?;

    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| EncryptionError::new("DEK is not 32 bytes"))?;

    Ok(Dek::from_bytes(arr))
}

fn unwrap_password_dek_from_wrappers(
    wrapped_keys: &[WrappedDek],
    password: &str,
) -> Result<Dek, EncryptionError> {
    let mut found_dek: Option<Dek> = None;

    for wrapped in wrapped_keys {
        if wrapped.kek_id != KekId::Password {
            continue;
        }

        match wrapped.unwrap_with_password(password) {
            Ok(dek) => {
                if let Some(ref existing) = found_dek {
                    if existing.as_hex() != dek.as_hex() {
                        return Err(EncryptionError::new(
                            "Envelope corruption: multiple password wrappers yielded different DEKs",
                        ));
                    }
                } else {
                    found_dek = Some(dek);
                }
            }
            Err(_) => {
                continue;
            }
        }
    }

    found_dek.ok_or_else(|| EncryptionError::new("No password wrapper could unwrap the DEK"))
}

pub(crate) fn argon2_params() -> Result<Params, EncryptionError> {
    ParamsBuilder::new()
        .m_cost(ARGON2_MEMORY_COST)
        .t_cost(ARGON2_TIME_COST)
        .p_cost(ARGON2_PARALLELISM)
        .build()
        .map_err(|e| EncryptionError::new(format!("Failed to build Argon2 params: {e}")))
}

pub(crate) fn argon2_with_params() -> Result<Argon2<'static>, EncryptionError> {
    let params = argon2_params()?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

fn parse_sqlcipher_runtime_major(version: &str) -> Result<SqlcipherCompatibility, EncryptionError> {
    let major_str: String = version
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect();

    if major_str.is_empty() {
        return Err(EncryptionError::new(format!(
            "Failed to parse SQLCipher major version from runtime string: {version}"
        )));
    }

    let major = major_str.parse::<u8>().map_err(|err| {
        EncryptionError::new(format!(
            "Failed to parse SQLCipher major version from runtime string {version}: {err}"
        ))
    })?;

    Ok(SqlcipherCompatibility(major))
}

pub(crate) fn current_sqlcipher_compatibility() -> Result<SqlcipherCompatibility, EncryptionError> {
    let conn = rusqlite::Connection::open_in_memory()
        .map_err(|err| EncryptionError::new(format!("Failed to open SQLCipher probe DB: {err}")))?;

    conn.execute_batch(
        "PRAGMA key = \"x'0000000000000000000000000000000000000000000000000000000000000000'\"",
    )
    .map_err(|err| {
        EncryptionError::new(format!(
            "Failed to key SQLCipher probe DB for version lookup: {err}"
        ))
    })?;

    let runtime_version: String = conn
        .query_row("PRAGMA cipher_version", [], |row| row.get(0))
        .map_err(|err| {
            EncryptionError::new(format!(
                "Failed to read SQLCipher runtime version from probe DB: {err}"
            ))
        })?;

    parse_sqlcipher_runtime_major(runtime_version.trim())
}

#[derive(Debug, Clone)]
pub(crate) struct ServerMasterSecret(Vec<u8>);

impl ServerMasterSecret {
    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub(crate) fn from_base64(s: &str) -> Result<Self, EncryptionError> {
        let bytes = BASE64.decode(s).map_err(|e| {
            EncryptionError::new(format!("Server secret base64 decode failed: {e}"))
        })?;

        if bytes.len() != 32 {
            return Err(EncryptionError::new(format!(
                "Server secret must be 32 bytes, got {}",
                bytes.len()
            )));
        }

        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| EncryptionError::new("Server secret must be exactly 32 bytes"))?;

        Ok(Self(arr.to_vec()))
    }

    fn generate() -> Self {
        let mut bytes = [0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        Self(bytes.to_vec())
    }

    fn to_base64(&self) -> String {
        BASE64.encode(&self.0)
    }
}

impl Zeroize for ServerMasterSecret {
    fn zeroize(&mut self) {
        self.0.iter_mut().for_each(|b| b.zeroize());
    }
}

impl Drop for ServerMasterSecret {
    fn drop(&mut self) {
        self.zeroize();
    }
}

pub(crate) fn resolve_server_master_secret() -> Result<ServerMasterSecret, EncryptionError> {
    let env_value = std::env::var(SESSION_WRAP_SECRET_ENV).ok();
    resolve_server_master_secret_impl(env_value.as_deref())
}

#[cfg(all(test, feature = "db-tests"))]
pub(crate) fn replace_cached_server_master_secret_for_test(
    replacement: Option<ServerMasterSecret>,
) -> Option<ServerMasterSecret> {
    let mut cached = CACHED_SERVER_MASTER_SECRET
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::mem::replace(&mut *cached, replacement)
}

#[cfg(all(test, feature = "db-tests"))]
pub(crate) fn generate_server_master_secret_for_test() -> ServerMasterSecret {
    ServerMasterSecret::generate()
}

fn resolve_server_master_secret_impl(
    env_value: Option<&str>,
) -> Result<ServerMasterSecret, EncryptionError> {
    if let Some(value) = env_value {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(EncryptionError::new(format!(
                "Environment variable {SESSION_WRAP_SECRET_ENV} is set but empty"
            )));
        }
        return ServerMasterSecret::from_base64(trimmed);
    }

    if let Some(runtime_context) = crate::runtime_context::current_runtime_context() {
        return resolve_server_master_secret_at(runtime_context.project_dir());
    }

    let mut cached = CACHED_SERVER_MASTER_SECRET
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(ref secret) = *cached {
        return Ok(secret.clone());
    }

    let project_dir = crate::project_paths::get_project_dir()
        .map_err(|e| EncryptionError::new(format!("Failed to resolve project dir: {e}")))?;

    let secret = resolve_server_master_secret_at(&project_dir)?;
    *cached = Some(secret.clone());
    Ok(secret)
}

fn resolve_server_master_secret_at(
    project_dir: &std::path::Path,
) -> Result<ServerMasterSecret, EncryptionError> {
    let secret_path = project_dir
        .join("app")
        .join("data")
        .join(SESSION_WRAP_SECRET_FILENAME);

    if let Some(parent) = secret_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            EncryptionError::new(format!(
                "Failed to create server secret directory at {}: {e}",
                parent.display()
            ))
        })?;
    }

    if let Some(recovered) = recover_server_master_secret_file(&secret_path)? {
        return Ok(recovered);
    }

    if secret_path.exists() {
        return read_server_master_secret_file(&secret_path);
    }

    let secret = {
        let secret = ServerMasterSecret::generate();
        let temp_path = secret_path.with_extension("tmp");

        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .mode(0o600)
                .open(&temp_path)
                .and_then(|mut f| std::io::Write::write_all(&mut f, secret.to_base64().as_bytes()))
                .map_err(|e| {
                    EncryptionError::new(format!(
                        "Failed to write server secret file at {}: {e}",
                        temp_path.display()
                    ))
                })?;
        }

        #[cfg(not(unix))]
        {
            std::fs::write(&temp_path, secret.to_base64()).map_err(|e| {
                EncryptionError::new(format!(
                    "Failed to write server secret file at {}: {e}",
                    temp_path.display()
                ))
            })?;
        }

        std::fs::rename(&temp_path, &secret_path).map_err(|e| {
            EncryptionError::new(format!(
                "Failed to atomically rename server secret file: {e}"
            ))
        })?;

        secret
    };
    Ok(secret)
}

fn read_server_master_secret_file(
    path: &std::path::Path,
) -> Result<ServerMasterSecret, EncryptionError> {
    let contents = std::fs::read_to_string(path).map_err(|e| {
        EncryptionError::new(format!(
            "Failed to read server secret file at {}: {e}",
            path.display()
        ))
    })?;
    ServerMasterSecret::from_base64(contents.trim())
}

fn recover_server_master_secret_file(
    secret_path: &std::path::Path,
) -> Result<Option<ServerMasterSecret>, EncryptionError> {
    let temp_path = secret_path.with_extension("tmp");

    if secret_path.exists() {
        if temp_path.exists() {
            let _ = std::fs::remove_file(&temp_path);
        }
        return Ok(None);
    }

    if !temp_path.exists() {
        return Ok(None);
    }

    let contents = std::fs::read_to_string(&temp_path).map_err(|e| {
        EncryptionError::new(format!(
            "Failed to read recovered server secret file at {}: {e}",
            temp_path.display()
        ))
    })?;

    let recovered = match ServerMasterSecret::from_base64(contents.trim()) {
        Ok(secret) => secret,
        Err(_) => {
            let _ = std::fs::remove_file(&temp_path);
            return Ok(None);
        }
    };

    match std::fs::rename(&temp_path, secret_path) {
        Ok(()) => Ok(Some(recovered)),
        Err(_rename_error) if secret_path.exists() => {
            read_server_master_secret_file(secret_path).map(Some)
        }
        Err(rename_error) => Err(EncryptionError::new(format!(
            "Failed to recover server secret file from {} to {}: {rename_error}",
            temp_path.display(),
            secret_path.display()
        ))),
    }
}

pub(crate) fn user_envelope_path(user_id: UserId) -> Result<PathBuf, EncryptionError> {
    crate::project_paths::get_user_envelope_path(user_id)
        .map_err(|e| EncryptionError::new(format!("Failed to resolve user envelope path: {e}")))
}

pub(crate) fn write_envelope(
    user_id: UserId,
    envelope: &DbEnvelope,
) -> Result<(), EncryptionError> {
    let path = user_envelope_path(user_id)?;
    write_envelope_at(&path, envelope)
}

fn write_envelope_at(path: &std::path::Path, envelope: &DbEnvelope) -> Result<(), EncryptionError> {
    recover_envelope_file_if_needed(path)?;

    let contents = serde_json::to_string_pretty(envelope)
        .map_err(|e| EncryptionError::new(format!("Failed to serialize envelope: {e}")))?;

    let temp_path = path.with_extension("tmp");

    std::fs::write(&temp_path, &contents).map_err(|e| {
        EncryptionError::new(format!(
            "Failed to write envelope file at {}: {e}",
            temp_path.display()
        ))
    })?;

    atomic_replace_file_with_backup(&temp_path, path)?;

    Ok(())
}

fn atomic_replace_file_with_backup(
    temp_path: &std::path::Path,
    dest_path: &std::path::Path,
) -> Result<(), EncryptionError> {
    #[cfg(unix)]
    {
        std::fs::rename(temp_path, dest_path).map_err(|e| {
            EncryptionError::new(format!(
                "Failed to atomically replace envelope file at {}: {e}",
                dest_path.display()
            ))
        })?;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        let backup_path = dest_path.with_extension("bak");

        if !dest_path.exists() {
            std::fs::rename(temp_path, dest_path).map_err(|e| {
                EncryptionError::new(format!(
                    "Failed to install envelope file at {}: {e}",
                    dest_path.display()
                ))
            })?;
            Ok(())
        } else {
            if backup_path.exists() {
                std::fs::remove_file(&backup_path).map_err(|e| {
                    EncryptionError::new(format!(
                        "Failed to remove stale envelope backup at {}: {e}",
                        backup_path.display()
                    ))
                })?;
            }

            std::fs::rename(dest_path, &backup_path).map_err(|e| {
                EncryptionError::new(format!(
                    "Failed to backup envelope file from {} to {}: {e}",
                    dest_path.display(),
                    backup_path.display()
                ))
            })?;

            if let Err(rename_error) = std::fs::rename(temp_path, dest_path) {
                if !dest_path.exists() && backup_path.exists() {
                    std::fs::rename(&backup_path, dest_path).map_err(|restore_error| {
                        EncryptionError::new(format!(
                            "Failed to replace envelope file at {}: {rename_error}; failed to restore backup from {}: {restore_error}",
                            dest_path.display(),
                            backup_path.display()
                        ))
                    })?;
                }

                return Err(EncryptionError::new(format!(
                    "Failed to replace envelope file at {}: {rename_error}",
                    dest_path.display()
                )));
            }

            if backup_path.exists() {
                std::fs::remove_file(&backup_path).map_err(|e| {
                    EncryptionError::new(format!(
                        "Failed to remove envelope backup at {}: {e}",
                        backup_path.display()
                    ))
                })?;
            }

            Ok(())
        }
    }
}

fn recover_envelope_file_if_needed(dest_path: &std::path::Path) -> Result<(), EncryptionError> {
    let backup_path = dest_path.with_extension("bak");
    let temp_path = dest_path.with_extension("tmp");

    if dest_path.exists() {
        if backup_path.exists() {
            std::fs::remove_file(&backup_path).map_err(|e| {
                EncryptionError::new(format!(
                    "Failed to remove stale envelope backup at {}: {e}",
                    backup_path.display()
                ))
            })?;
        }
        if temp_path.exists() {
            std::fs::remove_file(&temp_path).map_err(|e| {
                EncryptionError::new(format!(
                    "Failed to remove stale envelope temp file at {}: {e}",
                    temp_path.display()
                ))
            })?;
        }
        return Ok(());
    }

    if backup_path.exists() {
        std::fs::rename(&backup_path, dest_path).map_err(|e| {
            EncryptionError::new(format!(
                "Failed to restore envelope backup from {} to {}: {e}",
                backup_path.display(),
                dest_path.display()
            ))
        })?;

        if temp_path.exists() {
            let _ = std::fs::remove_file(&temp_path);
        }

        return Ok(());
    }

    if !temp_path.exists() {
        return Ok(());
    }

    let contents = std::fs::read_to_string(&temp_path).map_err(|e| {
        EncryptionError::new(format!(
            "Failed to read recovered envelope temp file at {}: {e}",
            temp_path.display()
        ))
    })?;

    serde_json::from_str::<DbEnvelope>(&contents).map_err(|e| {
        EncryptionError::new(format!(
            "Failed to parse recovered envelope temp file at {}: {e}",
            temp_path.display()
        ))
    })?;

    std::fs::rename(&temp_path, dest_path).map_err(|e| {
        EncryptionError::new(format!(
            "Failed to recover envelope file from {} to {}: {e}",
            temp_path.display(),
            dest_path.display()
        ))
    })
}

pub(crate) fn read_envelope(user_id: UserId) -> Result<DbEnvelope, EncryptionError> {
    let path = user_envelope_path(user_id)?;
    read_envelope_at(&path)
}

pub(crate) fn read_envelope_path(path: &std::path::Path) -> Result<DbEnvelope, EncryptionError> {
    read_envelope_at(path)
}

fn read_envelope_at(path: &std::path::Path) -> Result<DbEnvelope, EncryptionError> {
    recover_envelope_file_if_needed(path)?;

    if !path.exists() {
        return Err(EncryptionError::new(format!(
            "Envelope file not found at {}",
            path.display()
        )));
    }

    let contents = std::fs::read_to_string(path).map_err(|e| {
        EncryptionError::new(format!(
            "Failed to read envelope file at {}: {e}",
            path.display()
        ))
    })?;

    serde_json::from_str(&contents)
        .map_err(|e| EncryptionError::new(format!("Failed to parse envelope JSON: {e}")))
}

#[cfg(feature = "dev-config")]
pub(crate) fn should_use_unencrypted_dev() -> bool {
    match std::env::var("BITGARTH_USER_DB_UNENCRYPTED") {
        Ok(value) => matches!(value.trim().to_lowercase().as_str(), "1" | "true"),
        Err(_) => false,
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    fn envelope_path_for_test(project_dir: &std::path::Path, user_id: UserId) -> PathBuf {
        project_dir
            .join("users")
            .join(user_id.to_string())
            .join("data")
            .join(format!("u{user_id}.json"))
    }

    #[test]
    fn dek_generation_produces_256_bit_key() {
        let dek = Dek::generate();
        assert_eq!(dek.as_hex().len(), 64);
    }

    #[test]
    fn dek_hex_roundtrip() {
        let dek = Dek::generate();
        let hex = dek.as_hex();
        let bytes = hex::decode(&hex).expect("hex should decode");
        let arr: [u8; 32] = bytes.try_into().expect("should be 32 bytes");
        let dek2 = Dek::from_bytes(arr);
        assert_eq!(dek.as_hex(), dek2.as_hex());
    }

    #[test]
    fn password_kek_derivation_produces_32_bytes() {
        let password = "test_password_123";
        let salt = [0u8; 16];
        let kek = PasswordKek::derive(password, &salt).expect("should derive");
        drop(kek);
    }

    #[test]
    fn password_kek_deterministic_with_same_salt() {
        let password = "test_password_123";
        let salt = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let kek1 = PasswordKek::derive(password, &salt).expect("should derive");
        let kek2 = PasswordKek::derive(password, &salt).expect("should derive");
        assert_eq!(kek1.as_bytes(), kek2.as_bytes());
    }

    #[test]
    fn password_kek_different_with_different_password() {
        let salt = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let kek1 = PasswordKek::derive("password1", &salt).expect("should derive");
        let kek2 = PasswordKek::derive("password2", &salt).expect("should derive");
        assert_ne!(kek1.as_bytes(), kek2.as_bytes());
    }

    #[test]
    fn session_kek_derivation_produces_32_bytes() {
        let server_secret = [0u8; 32];
        let session_token = BASE64.encode([42u8; 32]);
        let session_id = SessionId::new();
        let user_id = UserId::new();

        let kek = SessionKek::derive(&server_secret, &session_token, session_id, user_id)
            .expect("should derive");
        drop(kek);
    }

    #[test]
    fn session_kek_deterministic_with_same_inputs() {
        let server_secret = [1u8; 32];
        let session_token = BASE64.encode([42u8; 32]);
        let session_id = SessionId::new();
        let user_id = UserId::new();

        let kek1 = SessionKek::derive(&server_secret, &session_token, session_id, user_id)
            .expect("should derive");
        let kek2 = SessionKek::derive(&server_secret, &session_token, session_id, user_id)
            .expect("should derive");

        assert_eq!(kek1.as_bytes(), kek2.as_bytes());
    }

    #[test]
    fn session_kek_different_with_different_token() {
        let server_secret = [1u8; 32];
        let session_id = SessionId::new();
        let user_id = UserId::new();

        let token1 = BASE64.encode([1u8; 32]);
        let token2 = BASE64.encode([2u8; 32]);

        let kek1 = SessionKek::derive(&server_secret, &token1, session_id, user_id)
            .expect("should derive");
        let kek2 = SessionKek::derive(&server_secret, &token2, session_id, user_id)
            .expect("should derive");

        assert_ne!(kek1.as_bytes(), kek2.as_bytes());
    }

    #[test]
    fn session_kek_different_with_different_session_id() {
        let server_secret = [1u8; 32];
        let token = BASE64.encode([42u8; 32]);
        let user_id = UserId::new();

        let kek1 = SessionKek::derive(&server_secret, &token, SessionId::new(), user_id)
            .expect("should derive");
        let kek2 = SessionKek::derive(&server_secret, &token, SessionId::new(), user_id)
            .expect("should derive");

        assert_ne!(kek1.as_bytes(), kek2.as_bytes());
    }

    #[test]
    fn key_wrapping_roundtrip_password() {
        let dek = Dek::generate();
        let password = "secure_password_123";

        let wrapped = WrappedDek::new_password_wrapper(&dek, password).expect("should wrap");
        let unwrapped = wrapped
            .unwrap_with_password(password)
            .expect("should unwrap");

        assert_eq!(dek.as_hex(), unwrapped.as_hex());
    }

    #[test]
    fn key_wrapping_wrong_password_fails() {
        let dek = Dek::generate();

        let wrapped =
            WrappedDek::new_password_wrapper(&dek, "correct_password").expect("should wrap");
        let result = wrapped.unwrap_with_password("wrong_password");

        assert!(result.is_err());
    }

    #[test]
    fn key_wrapping_roundtrip_session() {
        let dek = Dek::generate();
        let server_secret = [1u8; 32];
        let token = BASE64.encode([42u8; 32]);
        let session_id = SessionId::new();
        let user_id = UserId::new();

        let wrapper = SessionWrapper::wrap(&dek, &server_secret, &token, session_id, user_id)
            .expect("should wrap");
        let unwrapped = wrapper
            .unwrap(&server_secret, &token, session_id, user_id)
            .expect("should unwrap");

        assert_eq!(dek.as_hex(), unwrapped.as_hex());
    }

    #[test]
    fn session_wrapper_base64_roundtrip() {
        let dek = Dek::generate();
        let server_secret = [1u8; 32];
        let token = BASE64.encode([42u8; 32]);
        let session_id = SessionId::new();
        let user_id = UserId::new();

        let wrapper = SessionWrapper::wrap(&dek, &server_secret, &token, session_id, user_id)
            .expect("should wrap");

        let nonce_b64 = wrapper.nonce_base64();
        let ciphertext_b64 = wrapper.ciphertext_base64();

        let wrapper2 =
            SessionWrapper::from_base64(&nonce_b64, &ciphertext_b64).expect("should parse");
        let unwrapped = wrapper2
            .unwrap(&server_secret, &token, session_id, user_id)
            .expect("should unwrap");

        assert_eq!(dek.as_hex(), unwrapped.as_hex());
    }

    #[test]
    fn envelope_creation_and_password_unwrap() {
        let password = "secure_password_123";
        let (envelope, dek) = DbEnvelope::new_encrypted(password).expect("should create");
        let current_compatibility =
            current_sqlcipher_compatibility().expect("should detect sqlcipher compatibility");

        match &envelope {
            DbEnvelope::Encrypted {
                sqlcipher_version,
                wrapped_keys,
            } => {
                assert_eq!(sqlcipher_version, &current_compatibility);
                assert_eq!(wrapped_keys.len(), 1);
            }
            #[cfg(feature = "dev-config")]
            DbEnvelope::UnencryptedDev => panic!("should be encrypted"),
        }

        let unwrapped = envelope
            .unwrap_with_password(password)
            .expect("should unwrap");
        assert_eq!(dek.as_hex(), unwrapped.as_hex());
    }

    #[test]
    fn envelope_wrong_password_fails() {
        let password = "correct_password";
        let (envelope, _) = DbEnvelope::new_encrypted(password).expect("should create");

        let result = envelope.unwrap_with_password("wrong_password");
        assert!(result.is_err());
    }

    #[test]
    fn envelope_add_password_wrapper() {
        let password1 = "password1";
        let (mut envelope, dek) = DbEnvelope::new_encrypted(password1).expect("should create");

        let password2 = "password2";
        envelope
            .add_password_wrapper(&dek, password2)
            .expect("should add wrapper");

        match &envelope {
            DbEnvelope::Encrypted { wrapped_keys, .. } => {
                assert_eq!(wrapped_keys.len(), 2);
            }
            #[cfg(feature = "dev-config")]
            DbEnvelope::UnencryptedDev => panic!("should be encrypted"),
        }

        let unwrapped1 = envelope
            .unwrap_with_password(password1)
            .expect("should unwrap with old");
        let unwrapped2 = envelope
            .unwrap_with_password(password2)
            .expect("should unwrap with new");

        assert_eq!(dek.as_hex(), unwrapped1.as_hex());
        assert_eq!(dek.as_hex(), unwrapped2.as_hex());
    }

    #[test]
    fn envelope_compact_password_wrappers() {
        let password1 = "password1";
        let (mut envelope, dek) = DbEnvelope::new_encrypted(password1).expect("should create");

        let password2 = "password2";
        envelope
            .add_password_wrapper(&dek, password2)
            .expect("should add wrapper");

        match &envelope {
            DbEnvelope::Encrypted { wrapped_keys, .. } => {
                assert_eq!(wrapped_keys.len(), 2);
                let keep_id = wrapped_keys[1].wrap_id.as_str().to_string();
                envelope.compact_password_wrappers(&keep_id);
            }
            #[cfg(feature = "dev-config")]
            DbEnvelope::UnencryptedDev => panic!("should be encrypted"),
        }

        match &envelope {
            DbEnvelope::Encrypted { wrapped_keys, .. } => {
                assert_eq!(wrapped_keys.len(), 1);
            }
            #[cfg(feature = "dev-config")]
            DbEnvelope::UnencryptedDev => panic!("should be encrypted"),
        }

        let result = envelope.unwrap_with_password(password2);
        assert!(result.is_ok());
    }

    #[test]
    fn envelope_json_roundtrip() {
        let password = "secure_password_123";
        let (envelope, dek) = DbEnvelope::new_encrypted(password).expect("should create");

        let json = serde_json::to_string(&envelope).expect("should serialize");
        let parsed: DbEnvelope = serde_json::from_str(&json).expect("should deserialize");

        let unwrapped = parsed
            .unwrap_with_password(password)
            .expect("should unwrap");
        assert_eq!(dek.as_hex(), unwrapped.as_hex());
    }

    #[test]
    fn envelope_json_has_expected_structure() {
        let password = "secure_password_123";
        let (envelope, _) = DbEnvelope::new_encrypted(password).expect("should create");
        let current_compatibility =
            current_sqlcipher_compatibility().expect("should detect sqlcipher compatibility");

        let json = serde_json::to_string_pretty(&envelope).expect("should serialize");
        let value: serde_json::Value = serde_json::from_str(&json).expect("should parse");

        assert_eq!(value["mode"], "encrypted");
        assert_eq!(value["sqlcipher_version"], current_compatibility.as_u32());
        assert!(value["wrapped_keys"].is_array());
    }

    #[test]
    fn parse_sqlcipher_runtime_major_accepts_runtime_version_string() {
        let compatibility = parse_sqlcipher_runtime_major("4.6.1 community").expect("should parse");
        assert_eq!(compatibility.as_u32(), 4);
    }

    #[test]
    fn parse_sqlcipher_runtime_major_rejects_invalid_string() {
        let result = parse_sqlcipher_runtime_major("community build");
        assert!(result.is_err());
    }

    #[test]
    fn argon2_params_match_spec() {
        let params = argon2_params().expect("should create params");
        assert_eq!(params.m_cost(), ARGON2_MEMORY_COST);
        assert_eq!(params.t_cost(), ARGON2_TIME_COST);
        assert_eq!(params.p_cost(), ARGON2_PARALLELISM);
    }

    #[test]
    fn server_master_secret_from_base64() {
        let original = [42u8; 32];
        let b64 = BASE64.encode(original);
        let secret = ServerMasterSecret::from_base64(&b64).expect("should parse");
        assert_eq!(secret.as_bytes(), original);
    }

    #[test]
    fn server_master_secret_rejects_wrong_length() {
        let short = BASE64.encode([0u8; 16]);
        let result = ServerMasterSecret::from_base64(&short);
        assert!(result.is_err());
    }

    #[test]
    fn server_master_secret_rejects_invalid_base64() {
        let result = ServerMasterSecret::from_base64("not valid base64!!!");
        assert!(result.is_err());
    }

    #[test]
    fn resolve_server_master_secret_rejects_empty_env() {
        let result = resolve_server_master_secret_impl(Some(""));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("is set but empty"));

        let result = resolve_server_master_secret_impl(Some("   "));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("is set but empty"));
    }

    #[test]
    fn multiple_password_wrappers_corruption_detection() {
        let dek1 = Dek::generate();
        let dek2 = Dek::generate();

        let wrapped1 = WrappedDek::new_password_wrapper(&dek1, "password1").expect("should wrap");
        let wrapped2 = WrappedDek::new_password_wrapper(&dek2, "password2").expect("should wrap");

        let envelope = DbEnvelope::Encrypted {
            sqlcipher_version: SqlcipherCompatibility(4),
            wrapped_keys: vec![wrapped1, wrapped2],
        };

        let result = envelope.unwrap_with_password("password1");
        assert!(result.is_ok());

        let result = envelope.unwrap_with_password("password2");
        assert!(result.is_ok());
    }

    #[test]
    fn resolve_server_master_secret_generates_and_persists() {
        let temp_dir =
            std::env::temp_dir().join(format!("bitgarth_server_secret_test_{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&temp_dir).expect("should create temp dir");
        let secret_path = temp_dir
            .join("app")
            .join("data")
            .join(SESSION_WRAP_SECRET_FILENAME);

        let secret1 = resolve_server_master_secret_at(&temp_dir).expect("should generate secret");
        assert_eq!(secret1.as_bytes().len(), 32);
        assert!(secret_path.exists());

        let secret2 =
            resolve_server_master_secret_at(&temp_dir).expect("should read persisted secret");
        assert_eq!(secret1.as_bytes(), secret2.as_bytes());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn resolve_server_master_secret_recovers_from_temp_file() {
        let temp_dir = std::env::temp_dir().join(format!(
            "bitgarth_server_secret_recovery_test_{}",
            ulid::Ulid::new()
        ));
        let secret_dir = temp_dir.join("app").join("data");
        std::fs::create_dir_all(&secret_dir).expect("should create temp dir");

        let expected = ServerMasterSecret::generate();
        let secret_path = secret_dir.join(SESSION_WRAP_SECRET_FILENAME);
        let temp_path = secret_path.with_extension("tmp");
        std::fs::write(&temp_path, expected.to_base64()).expect("should write temp secret");

        let recovered = resolve_server_master_secret_at(&temp_dir).expect("should recover secret");
        assert_eq!(expected.as_bytes(), recovered.as_bytes());
        assert!(secret_path.exists());
        assert!(!temp_path.exists());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn envelope_write_and_read_roundtrip() {
        let temp_dir =
            std::env::temp_dir().join(format!("bitgarth_envelope_test_{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&temp_dir).expect("should create temp dir");

        let user_id = UserId::new();
        let password = "test_password_123";
        let path = envelope_path_for_test(&temp_dir, user_id);
        std::fs::create_dir_all(path.parent().expect("path should have parent"))
            .expect("should create user data dir");

        let (envelope, dek) = DbEnvelope::new_encrypted(password).expect("should create");

        write_envelope_at(&path, &envelope).expect("should write");

        let read_back = read_envelope_at(&path).expect("should read");

        let unwrapped = read_back
            .unwrap_with_password(password)
            .expect("should unwrap");
        assert_eq!(dek.as_hex(), unwrapped.as_hex());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn read_envelope_missing_file_error() {
        let temp_dir = std::env::temp_dir().join(format!(
            "bitgarth_envelope_missing_test_{}",
            ulid::Ulid::new()
        ));
        std::fs::create_dir_all(&temp_dir).expect("should create temp dir");

        let user_id = UserId::new();
        let path = envelope_path_for_test(&temp_dir, user_id);
        std::fs::create_dir_all(path.parent().expect("path should have parent"))
            .expect("should create user data dir");
        let result = read_envelope_at(&path);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not found"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn read_envelope_recovers_from_backup_file() {
        let temp_dir = std::env::temp_dir().join(format!(
            "bitgarth_envelope_backup_recovery_test_{}",
            ulid::Ulid::new()
        ));
        std::fs::create_dir_all(&temp_dir).expect("should create temp dir");

        let user_id = UserId::new();
        let password = "test_password_123";
        let (envelope, dek) = DbEnvelope::new_encrypted(password).expect("should create");

        let envelope_path = envelope_path_for_test(&temp_dir, user_id);
        std::fs::create_dir_all(envelope_path.parent().expect("path should have parent"))
            .expect("should create user data dir");
        let backup_path = envelope_path.with_extension("bak");
        let contents = serde_json::to_string_pretty(&envelope).expect("should serialize");
        std::fs::write(&backup_path, contents).expect("should write backup");

        let recovered = read_envelope_at(&envelope_path).expect("should recover from backup");
        let unwrapped = recovered
            .unwrap_with_password(password)
            .expect("should unwrap");
        assert_eq!(dek.as_hex(), unwrapped.as_hex());
        assert!(envelope_path.exists());
        assert!(!backup_path.exists());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn read_envelope_recovers_from_temp_file() {
        let temp_dir = std::env::temp_dir().join(format!(
            "bitgarth_envelope_temp_recovery_test_{}",
            ulid::Ulid::new()
        ));
        std::fs::create_dir_all(&temp_dir).expect("should create temp dir");

        let user_id = UserId::new();
        let password = "test_password_123";
        let (envelope, dek) = DbEnvelope::new_encrypted(password).expect("should create");

        let envelope_path = envelope_path_for_test(&temp_dir, user_id);
        std::fs::create_dir_all(envelope_path.parent().expect("path should have parent"))
            .expect("should create user data dir");
        let temp_path = envelope_path.with_extension("tmp");
        let contents = serde_json::to_string_pretty(&envelope).expect("should serialize");
        std::fs::write(&temp_path, contents).expect("should write temp");

        let recovered = read_envelope_at(&envelope_path).expect("should recover from temp");
        let unwrapped = recovered
            .unwrap_with_password(password)
            .expect("should unwrap");
        assert_eq!(dek.as_hex(), unwrapped.as_hex());
        assert!(envelope_path.exists());
        assert!(!temp_path.exists());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn unlock_authority_variants_exist() {
        let _ = UnlockAuthority::PasswordLogin;
        let _ = UnlockAuthority::SessionRestore {
            session_id: SessionId::new(),
        };
    }

    #[test]
    fn user_db_open_mode_variants_exist() {
        let dek = Dek::generate();
        let mode = UserDbOpenMode::Encrypted {
            dek: dek.clone(),
            authority: UnlockAuthority::PasswordLogin,
            sqlcipher_compatibility: SqlcipherCompatibility(4),
        };
        match &mode {
            UserDbOpenMode::Encrypted {
                dek,
                authority,
                sqlcipher_compatibility,
            } => {
                assert_eq!(dek.as_hex().len(), 64);
                matches!(authority, UnlockAuthority::PasswordLogin);
                assert_eq!(sqlcipher_compatibility.as_u32(), 4);
            }
            #[cfg(feature = "dev-config")]
            UserDbOpenMode::UnencryptedDev => panic!("unexpected variant"),
            #[cfg(all(test, feature = "db-tests"))]
            UserDbOpenMode::PlaintextTest => panic!("unexpected variant"),
        }
        #[cfg(all(test, feature = "db-tests"))]
        {
            let _ = UserDbOpenMode::PlaintextTest;
        }
    }
}
