use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier};
pub(crate) use argon2::password_hash::{PasswordHashString, SaltString};
use rand::rngs::OsRng;

use crate::db::encryption::argon2_with_params;
use crate::models::{RawPlaintextPassword, ValidatedPlaintextPassword};

fn argon2_password_error() -> argon2::password_hash::Error {
    argon2::password_hash::Error::Password
}

/// Hash a validated password. Returns the password hash string and salt.
pub(crate) fn hash_password(
    password: &ValidatedPlaintextPassword,
) -> Result<(PasswordHashString, SaltString), argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = argon2_with_params().map_err(|_| argon2_password_error())?;
    let password_hash = argon2.hash_password(password.as_str().as_bytes(), &salt)?;
    let password_hash_string = PasswordHashString::from(password_hash);
    Ok((password_hash_string, salt))
}

/// Verify a raw password against a stored password hash.
/// Used for login - we verify raw input before validation since an invalid
/// password format should just fail verification, not cause a validation error.
pub(crate) fn verify_password(
    password: &RawPlaintextPassword,
    password_hash: &str,
) -> Result<bool, argon2::password_hash::Error> {
    let parsed_hash = PasswordHash::new(password_hash)?;
    let argon2 = argon2_with_params().map_err(|_| argon2_password_error())?;
    Ok(argon2
        .verify_password(password.as_str().as_bytes(), &parsed_hash)
        .is_ok())
}
