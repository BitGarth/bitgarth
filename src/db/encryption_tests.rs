use super::encryption::{ClientKeyWrapper, DbEnvelope, Dek, SqlcipherCompatibility};
use crate::client_capabilities::{CapabilityId, ClientPermission};
use crate::models::UserId;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use ulid::Ulid;

#[test]
fn client_key_wrapper_authenticates_every_context_field() {
    let dek = Dek::from_bytes([3_u8; 32]);
    let raw_key = [5_u8; 32];
    let user_id = UserId::new();
    let other_user_id = UserId::new();
    let capability_id = CapabilityId::from_bytes([7_u8; 32]);
    let other_capability_id = CapabilityId::from_bytes([8_u8; 32]);
    let permission = ClientPermission::BalancesRead;
    let wrapper = ClientKeyWrapper::wrap(&dek, &raw_key, user_id, capability_id, permission)
        .expect("Client Key should wrap DEK");

    let unwrapped = wrapper
        .unwrap(&raw_key, user_id, capability_id, permission)
        .expect("matching Client Key context should unwrap");
    assert_eq!(unwrapped.as_hex(), dek.as_hex());
    assert!(
        wrapper
            .unwrap(&[6_u8; 32], user_id, capability_id, permission)
            .is_err(),
        "wrong raw key must not unwrap"
    );
    assert!(
        wrapper
            .unwrap(&raw_key, other_user_id, capability_id, permission)
            .is_err(),
        "wrong user must not unwrap"
    );
    assert!(
        wrapper
            .unwrap(&raw_key, user_id, other_capability_id, permission)
            .is_err(),
        "wrong capability must not unwrap"
    );
    assert!(
        wrapper
            .unwrap_with_permission_for_test(&raw_key, user_id, capability_id, "transactions_read",)
            .is_err(),
        "wrong permission must not unwrap"
    );
}

struct TempDbDir {
    path: PathBuf,
}

impl TempDbDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("bitgarth_sqlcipher_test_{}", Ulid::new()));
        std::fs::create_dir_all(&path).expect("temp sqlcipher test dir should create");
        Self { path }
    }

    fn file(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for TempDbDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn open_encrypted_db(
    path: &Path,
    dek: &Dek,
    compatibility: &SqlcipherCompatibility,
) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(&format!("PRAGMA key = \"x'{}'\"", dek.as_hex()))?;
    conn.pragma_update(None, "cipher_compatibility", compatibility.as_u32())?;
    Ok(conn)
}

#[test]
fn encrypted_db_file_roundtrip_with_password_unwrapped_dek() {
    let temp_dir = TempDbDir::new();
    let db_path = temp_dir.file("user.db");
    let password = "SecurePass123";
    let (envelope, _) = DbEnvelope::new_encrypted(password).expect("envelope should create");
    let dek = envelope
        .unwrap_with_password(password)
        .expect("password should unwrap DEK");
    let compatibility = envelope
        .sqlcipher_compatibility()
        .expect("encrypted envelope should expose compatibility");

    {
        let conn =
            open_encrypted_db(&db_path, &dek, &compatibility).expect("encrypted db should open");
        conn.execute_batch(
            "CREATE TABLE test_data (value TEXT NOT NULL);
             INSERT INTO test_data (value) VALUES ('hello');",
        )
        .expect("encrypted db should accept writes");
    }

    let reopened =
        open_encrypted_db(&db_path, &dek, &compatibility).expect("encrypted db should reopen");
    let value: String = reopened
        .query_row("SELECT value FROM test_data", [], |row| row.get(0))
        .expect("encrypted db should read persisted data");
    assert_eq!(value, "hello");
}

#[test]
fn encrypted_db_file_rejects_wrong_dek() {
    let temp_dir = TempDbDir::new();
    let db_path = temp_dir.file("wrong-key.db");
    let password = "SecurePass123";
    let (envelope, _) = DbEnvelope::new_encrypted(password).expect("envelope should create");
    let compatibility = envelope
        .sqlcipher_compatibility()
        .expect("encrypted envelope should expose compatibility");
    let dek = envelope
        .unwrap_with_password(password)
        .expect("password should unwrap DEK");

    {
        let conn =
            open_encrypted_db(&db_path, &dek, &compatibility).expect("encrypted db should open");
        conn.execute_batch("CREATE TABLE protected_rows (value TEXT NOT NULL);")
            .expect("encrypted db should create schema");
    }

    let wrong_dek = Dek::generate();
    let reopened = open_encrypted_db(&db_path, &wrong_dek, &compatibility)
        .expect("wrong-key open still returns a connection handle");
    let result = reopened.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
        row.get::<_, i64>(0)
    });
    assert!(
        result.is_err(),
        "wrong key should not read encrypted schema"
    );
}

#[test]
fn encrypted_db_file_reopens_with_runtime_reported_compatibility() {
    let temp_dir = TempDbDir::new();
    let db_path = temp_dir.file("compatibility.db");
    let password = "AnotherPass123";
    let (envelope, _) = DbEnvelope::new_encrypted(password).expect("envelope should create");
    let dek = envelope
        .unwrap_with_password(password)
        .expect("password should unwrap DEK");
    let compatibility = envelope
        .sqlcipher_compatibility()
        .expect("encrypted envelope should expose compatibility");

    let conn = open_encrypted_db(&db_path, &dek, &compatibility).expect("encrypted db should open");
    let runtime_version: String = conn
        .query_row("PRAGMA cipher_version", [], |row| row.get(0))
        .expect("cipher version pragma should read");
    assert!(
        runtime_version.starts_with(&compatibility.as_u32().to_string()),
        "runtime version {runtime_version} should align with stored compatibility {}",
        compatibility.as_u32()
    );
}
