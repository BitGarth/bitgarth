use super::DbError;
use crate::client_capabilities::{
    CapabilityId, ClientCapabilityRecord, ClientKeyVerifier, ClientPermission,
};
use crate::models::{UserId, parse_datetime};
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, Row, params, types::Type};
use std::{collections::HashSet, io, str::FromStr};

fn invalid_column(index: usize, message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        index,
        Type::Text,
        Box::new(io::Error::new(io::ErrorKind::InvalidData, message.into())),
    )
}

fn parse_record(row: &Row<'_>) -> rusqlite::Result<ClientCapabilityRecord> {
    let capability_id_raw: String = row.get(0)?;
    let user_id_raw: String = row.get(1)?;
    let verifier_raw: Vec<u8> = row.get(2)?;
    let permission_raw: String = row.get(5)?;
    let created_at_raw: String = row.get(6)?;
    let expires_at_raw: Option<String> = row.get(7)?;
    let last_used_at_raw: Option<String> = row.get(8)?;
    let revoked_at_raw: Option<String> = row.get(9)?;

    let capability_id =
        CapabilityId::from_str(&capability_id_raw).map_err(|error| invalid_column(0, error))?;
    let user_id =
        UserId::from_str(&user_id_raw).map_err(|error| invalid_column(1, error.to_string()))?;
    let verifier_bytes: [u8; 32] = verifier_raw
        .try_into()
        .map_err(|_| invalid_column(2, "client key verifier must be 32 bytes"))?;
    let permission =
        ClientPermission::from_db(&permission_raw).map_err(|error| invalid_column(5, error))?;
    let created_at =
        parse_datetime(&created_at_raw).map_err(|error| invalid_column(6, error.to_string()))?;
    let parse_optional_time = |index, value: Option<String>| {
        value
            .map(|raw| {
                parse_datetime(&raw).map_err(|error| invalid_column(index, error.to_string()))
            })
            .transpose()
    };

    Ok(ClientCapabilityRecord {
        capability_id,
        user_id,
        key_verifier: ClientKeyVerifier::from_bytes(verifier_bytes),
        wrapped_dek: row.get(3)?,
        wrap_nonce: row.get(4)?,
        permission,
        created_at,
        expires_at: parse_optional_time(7, expires_at_raw)?,
        last_used_at: parse_optional_time(8, last_used_at_raw)?,
        revoked_at: parse_optional_time(9, revoked_at_raw)?,
    })
}

const RECORD_COLUMNS: &str = "capability_id, user_id, key_verifier, wrapped_dek, wrap_nonce, permission, created_at, expires_at, last_used_at, revoked_at";

pub(crate) fn insert_active_client_capability(
    record: &ClientCapabilityRecord,
) -> Result<(), DbError> {
    if record.wrapped_dek.is_some() != record.wrap_nonce.is_some() || record.revoked_at.is_some() {
        return Err(DbError::new(
            "active client capability requires matching wrap material and no revocation",
        ));
    }
    #[cfg(not(feature = "dev-config"))]
    if record.wrapped_dek.is_none() {
        return Err(DbError::new(
            "active client capability requires wrap material and no revocation",
        ));
    }

    super::with_db_mut(|conn| {
        conn.execute(
            "INSERT INTO client_capabilities (
                capability_id, user_id, key_verifier, wrapped_dek, wrap_nonce, permission,
                created_at, expires_at, last_used_at, revoked_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL)",
            params![
                record.capability_id.to_string(),
                record.user_id.to_string(),
                record.key_verifier.as_bytes().as_slice(),
                record.wrapped_dek.as_deref(),
                record.wrap_nonce.as_deref(),
                record.permission.as_str(),
                record.created_at.to_rfc3339(),
                record.expires_at.map(|value| value.to_rfc3339()),
                record.last_used_at.map(|value| value.to_rfc3339()),
            ],
        )
        .map_err(|error| DbError::from_rusqlite_error("insert active client capability", error))?;
        Ok(())
    })
}

pub(crate) fn find_capability_identity_by_verifier(
    verifier: ClientKeyVerifier,
) -> Result<Option<ClientCapabilityRecord>, DbError> {
    super::with_db(|conn| {
        conn.query_row(
            &format!("SELECT {RECORD_COLUMNS} FROM client_capabilities WHERE key_verifier = ?1"),
            params![verifier.as_bytes().as_slice()],
            parse_record,
        )
        .optional()
        .map_err(|error| DbError::from_rusqlite_error("find capability identity", error))
    })
}

pub(crate) fn load_client_capability(
    capability_id: CapabilityId,
) -> Result<Option<ClientCapabilityRecord>, DbError> {
    super::with_db(|conn| {
        conn.query_row(
            &format!("SELECT {RECORD_COLUMNS} FROM client_capabilities WHERE capability_id = ?1"),
            params![capability_id.to_string()],
            parse_record,
        )
        .optional()
        .map_err(|error| DbError::from_rusqlite_error("load client capability", error))
    })
}

pub(crate) fn load_active_client_capability(
    capability_id: CapabilityId,
    now: DateTime<Utc>,
) -> Result<Option<ClientCapabilityRecord>, DbError> {
    super::with_db(|conn| {
        conn.query_row(
            &format!(
                "SELECT {RECORD_COLUMNS} FROM client_capabilities
                 WHERE capability_id = ?1 AND revoked_at IS NULL
                   AND (expires_at IS NULL OR expires_at > ?2)"
            ),
            params![capability_id.to_string(), now.to_rfc3339()],
            parse_record,
        )
        .optional()
        .map_err(|error| DbError::from_rusqlite_error("load active capability", error))
    })
}

pub(crate) fn clear_expired_client_capability_wrap(
    user_id: UserId,
    capability_id: CapabilityId,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    super::with_db_mut(|conn| {
        let updated = conn
            .execute(
                "UPDATE client_capabilities SET wrapped_dek = NULL, wrap_nonce = NULL
                 WHERE capability_id = ?1 AND user_id = ?2 AND revoked_at IS NULL
                   AND wrapped_dek IS NOT NULL AND wrap_nonce IS NOT NULL
                   AND expires_at IS NOT NULL AND expires_at <= ?3",
                params![
                    capability_id.to_string(),
                    user_id.to_string(),
                    now.to_rfc3339()
                ],
            )
            .map_err(|error| {
                DbError::from_rusqlite_error("clear expired capability wrap", error)
            })?;
        Ok(updated == 1)
    })
}

pub(crate) fn load_client_capabilities_for_user(
    user_id: UserId,
) -> Result<Vec<ClientCapabilityRecord>, DbError> {
    super::with_db(|conn| {
        let mut statement = conn
            .prepare(&format!(
                "SELECT {RECORD_COLUMNS} FROM client_capabilities
                 WHERE user_id = ?1 ORDER BY created_at, capability_id"
            ))
            .map_err(|error| DbError::from_rusqlite_error("prepare capability list", error))?;
        let rows = statement
            .query_map(params![user_id.to_string()], parse_record)
            .map_err(|error| DbError::from_rusqlite_error("query capability list", error))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| DbError::from_rusqlite_error("read capability list", error))
    })
}

pub(crate) fn record_client_capability_activity(
    capability_id: CapabilityId,
    user_id: UserId,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    super::with_db_mut(|conn| {
        let transaction = conn.transaction().map_err(|error| {
            DbError::from_rusqlite_error("begin capability activity update", error)
        })?;
        let now = now.to_rfc3339();
        let updated = transaction
            .execute(
                "UPDATE client_capabilities SET last_used_at = ?1
                 WHERE capability_id = ?2 AND user_id = ?3 AND revoked_at IS NULL
                   AND (expires_at IS NULL OR expires_at > ?1)",
                params![now, capability_id.to_string(), user_id.to_string()],
            )
            .map_err(|error| DbError::from_rusqlite_error("update capability activity", error))?;
        if updated != 1 {
            return Err(DbError::new("client capability is not active"));
        }
        transaction
            .execute(
                "UPDATE users SET last_login_at = ?1 WHERE user_id = ?2",
                params![now, user_id.to_string()],
            )
            .map_err(|error| DbError::from_rusqlite_error("update client user activity", error))?;
        transaction
            .commit()
            .map_err(|error| DbError::from_rusqlite_error("commit capability activity", error))?;
        Ok(())
    })
}

pub(crate) fn list_expired_client_capabilities(
    now: DateTime<Utc>,
) -> Result<Vec<ClientCapabilityRecord>, DbError> {
    super::with_db(|conn| {
        let mut statement = conn
            .prepare(&format!(
                "SELECT {RECORD_COLUMNS} FROM client_capabilities
                 WHERE revoked_at IS NULL AND wrapped_dek IS NOT NULL
                   AND wrap_nonce IS NOT NULL AND expires_at IS NOT NULL AND expires_at <= ?1
                 ORDER BY expires_at, capability_id"
            ))
            .map_err(|error| {
                DbError::from_rusqlite_error("prepare expired capability list", error)
            })?;
        let rows = statement
            .query_map(params![now.to_rfc3339()], parse_record)
            .map_err(|error| {
                DbError::from_rusqlite_error("query expired capability list", error)
            })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| DbError::from_rusqlite_error("read expired capability list", error))
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RevokeClientCapabilityResult {
    Revoked,
    AlreadyRevoked,
    NotFound,
}

pub(crate) fn revoke_client_capability(
    user_id: UserId,
    capability_id: CapabilityId,
    revoked_at: DateTime<Utc>,
) -> Result<RevokeClientCapabilityResult, DbError> {
    super::with_db_mut(|conn| {
        let transaction = conn.transaction().map_err(|error| {
            DbError::from_rusqlite_error("begin client capability revocation", error)
        })?;
        let updated = transaction
            .execute(
                "UPDATE client_capabilities
                 SET wrapped_dek = NULL, wrap_nonce = NULL, revoked_at = ?1
                 WHERE capability_id = ?2 AND user_id = ?3 AND revoked_at IS NULL",
                params![
                    revoked_at.to_rfc3339(),
                    capability_id.to_string(),
                    user_id.to_string()
                ],
            )
            .map_err(|error| DbError::from_rusqlite_error("revoke client capability", error))?;
        let result = if updated == 1 {
            RevokeClientCapabilityResult::Revoked
        } else {
            let existing = transaction
                .query_row(
                    "SELECT revoked_at FROM client_capabilities
                     WHERE capability_id = ?1 AND user_id = ?2",
                    params![capability_id.to_string(), user_id.to_string()],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()
                .map_err(|error| {
                    DbError::from_rusqlite_error("verify client capability revocation", error)
                })?;
            match existing {
                Some(Some(_)) => RevokeClientCapabilityResult::AlreadyRevoked,
                None => RevokeClientCapabilityResult::NotFound,
                Some(None) => {
                    return Err(DbError::new("active client capability was not revoked"));
                }
            }
        };
        transaction.commit().map_err(|error| {
            DbError::from_rusqlite_error("commit client capability revocation", error)
        })?;
        Ok(result)
    })
}

pub(crate) fn capability_ids_for_user(user_id: UserId) -> Result<HashSet<CapabilityId>, DbError> {
    super::with_db(|conn| {
        let capability_ids = {
            let mut statement = conn
                .prepare(
                    "SELECT capability_id FROM client_capabilities
                     WHERE user_id = ?1",
                )
                .map_err(|error| DbError::from_rusqlite_error("prepare capability IDs", error))?;
            let rows = statement
                .query_map(params![user_id.to_string()], |row| row.get::<_, String>(0))
                .map_err(|error| DbError::from_rusqlite_error("query capability IDs", error))?;
            rows.map(|row| {
                let raw =
                    row.map_err(|error| DbError::from_rusqlite_error("read capability ID", error))?;
                CapabilityId::from_str(&raw).map_err(|error| {
                    DbError::new(format!("invalid capability ID in database: {error}"))
                })
            })
            .collect::<Result<HashSet<_>, DbError>>()?
        };
        Ok(capability_ids)
    })
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;
    use crate::db::{acquire_test_runtime, ensure_test_app_user, with_db};
    use chrono::Duration;

    fn setup() -> (crate::db::TestRuntimeGuard, UserId) {
        let runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        ensure_test_app_user(user_id);
        (runtime, user_id)
    }

    fn active_record(
        user_id: UserId,
        capability_id: CapabilityId,
        raw_key: &[u8; 32],
        now: DateTime<Utc>,
        expires_at: Option<DateTime<Utc>>,
    ) -> ClientCapabilityRecord {
        ClientCapabilityRecord {
            capability_id,
            user_id,
            key_verifier: ClientKeyVerifier::from_raw_key(raw_key),
            wrapped_dek: Some(vec![1, 2, 3]),
            wrap_nonce: Some(vec![4; 12]),
            permission: ClientPermission::BalancesRead,
            created_at: now,
            expires_at,
            last_used_at: None,
            revoked_at: None,
        }
    }

    #[cfg(feature = "dev-config")]
    fn active_unencrypted_record(
        user_id: UserId,
        capability_id: CapabilityId,
        raw_key: &[u8; 32],
        now: DateTime<Utc>,
        expires_at: Option<DateTime<Utc>>,
    ) -> ClientCapabilityRecord {
        let mut record = active_record(user_id, capability_id, raw_key, now, expires_at);
        record.wrapped_dek = None;
        record.wrap_nonce = None;
        record
    }

    #[test]
    fn schema_enforces_verifier_permission_and_private_data_boundary() {
        let (_runtime, user_id) = setup();
        let now = Utc::now();
        let raw_key = [91_u8; 32];
        let record = active_record(
            user_id,
            CapabilityId::from_bytes([1_u8; 32]),
            &raw_key,
            now,
            None,
        );
        insert_active_client_capability(&record).expect("first capability should insert");

        let columns = with_db(|conn| {
            let mut statement = conn
                .prepare("PRAGMA table_info(client_capabilities)")
                .map_err(|error| {
                    DbError::from_rusqlite_error("prepare capability schema", error)
                })?;
            statement
                .query_map([], |row| row.get::<_, String>(1))
                .map_err(|error| DbError::from_rusqlite_error("query capability schema", error))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| DbError::from_rusqlite_error("read capability schema", error))
        })
        .expect("capability columns should load");
        assert_eq!(
            columns,
            vec![
                "capability_id",
                "user_id",
                "key_verifier",
                "wrapped_dek",
                "wrap_nonce",
                "permission",
                "created_at",
                "expires_at",
                "last_used_at",
                "revoked_at",
            ]
        );
        assert!(
            !columns
                .iter()
                .any(|column| { column.contains("raw_key") || column.contains("display_name") })
        );

        let stored_verifier: Vec<u8> = with_db(|conn| {
            conn.query_row(
                "SELECT key_verifier FROM client_capabilities WHERE capability_id = ?1",
                params![record.capability_id.to_string()],
                |row| row.get(0),
            )
            .map_err(|error| DbError::from_rusqlite_error("load stored verifier", error))
        })
        .expect("stored verifier should load");
        assert_ne!(stored_verifier, raw_key);

        let duplicate = active_record(
            user_id,
            CapabilityId::from_bytes([2_u8; 32]),
            &raw_key,
            now,
            None,
        );
        assert!(insert_active_client_capability(&duplicate).is_err());

        let invalid_permission = with_db(|conn| {
            conn.execute(
                "INSERT INTO client_capabilities (
                    capability_id, user_id, key_verifier, wrapped_dek, wrap_nonce,
                    permission, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 'transactions_read', ?6)",
                params![
                    CapabilityId::from_bytes([3_u8; 32]).to_string(),
                    user_id.to_string(),
                    [3_u8; 32].as_slice(),
                    [1_u8].as_slice(),
                    [2_u8; 12].as_slice(),
                    now.to_rfc3339(),
                ],
            )
            .map_err(|error| DbError::from_rusqlite_error("insert invalid permission", error))
        });
        assert!(invalid_permission.is_err());

        let incomplete_wrap = with_db(|conn| {
            conn.execute(
                "INSERT INTO client_capabilities (
                    capability_id, user_id, key_verifier, wrapped_dek, wrap_nonce,
                    permission, created_at
                 ) VALUES (?1, ?2, ?3, ?4, NULL, 'balances_read', ?5)",
                params![
                    CapabilityId::from_bytes([4_u8; 32]).to_string(),
                    user_id.to_string(),
                    [4_u8; 32].as_slice(),
                    [1_u8].as_slice(),
                    now.to_rfc3339(),
                ],
            )
            .map_err(|error| DbError::from_rusqlite_error("insert incomplete wrap", error))
        });
        assert!(incomplete_wrap.is_err());
    }

    #[cfg(feature = "dev-config")]
    #[test]
    fn keyless_development_capability_is_active() {
        let (_runtime, user_id) = setup();
        let now = Utc::now();
        let capability_id = CapabilityId::from_bytes([5_u8; 32]);
        let record = active_unencrypted_record(user_id, capability_id, &[5_u8; 32], now, None);

        insert_active_client_capability(&record)
            .expect("keyless development capability should insert");
        assert_eq!(
            load_active_client_capability(capability_id, now)
                .expect("keyless capability should load"),
            Some(record)
        );
    }

    #[cfg(feature = "dev-config")]
    #[test]
    fn unencrypted_capability_uses_expiry_and_revocation_as_lifecycle_state() {
        let (_runtime, user_id) = setup();
        let now = Utc::now();
        let capability_id = CapabilityId::from_bytes([6_u8; 32]);
        let record = active_unencrypted_record(user_id, capability_id, &[6_u8; 32], now, None);
        insert_active_client_capability(&record).expect("keyless capability should insert");

        assert_eq!(
            load_active_client_capability(capability_id, now)
                .expect("keyless capability lookup should succeed"),
            Some(record)
        );
        assert!(
            capability_ids_for_user(user_id)
                .expect("capability IDs should load")
                .contains(&capability_id)
        );
        record_client_capability_activity(capability_id, user_id, now)
            .expect("keyless capability activity should record");

        assert_eq!(
            revoke_client_capability(user_id, capability_id, now)
                .expect("keyless capability should revoke"),
            RevokeClientCapabilityResult::Revoked
        );
        assert!(
            load_active_client_capability(capability_id, now)
                .expect("revoked capability lookup should succeed")
                .is_none()
        );
        assert!(record_client_capability_activity(capability_id, user_id, now).is_err());

        let expired_id = CapabilityId::from_bytes([7_u8; 32]);
        insert_active_client_capability(&active_unencrypted_record(
            user_id,
            expired_id,
            &[7_u8; 32],
            now - Duration::minutes(2),
            Some(now - Duration::minutes(1)),
        ))
        .expect("expired keyless capability should insert");
        assert!(
            load_active_client_capability(expired_id, now)
                .expect("expired capability lookup should succeed")
                .is_none()
        );
    }

    #[test]
    fn optional_expiry_and_revocation_destroy_wrap_authority() {
        let (_runtime, user_id) = setup();
        let now = Utc::now();
        let never_expires_id = CapabilityId::from_bytes([10_u8; 32]);
        insert_active_client_capability(&active_record(
            user_id,
            never_expires_id,
            &[10_u8; 32],
            now,
            None,
        ))
        .expect("non-expiring capability should insert");
        assert!(
            load_active_client_capability(never_expires_id, now)
                .expect("active capability should load")
                .is_some()
        );

        let expired_id = CapabilityId::from_bytes([11_u8; 32]);
        insert_active_client_capability(&active_record(
            user_id,
            expired_id,
            &[11_u8; 32],
            now - Duration::minutes(2),
            Some(now - Duration::minutes(1)),
        ))
        .expect("expired fixture should insert");
        assert!(
            load_active_client_capability(expired_id, now)
                .expect("expired capability load should succeed")
                .is_none()
        );
        assert!(
            clear_expired_client_capability_wrap(user_id, expired_id, now)
                .expect("expired wrap clear should succeed")
        );
        let expired = load_client_capabilities_for_user(user_id)
            .expect("capability list should load")
            .into_iter()
            .find(|record| record.capability_id == expired_id)
            .expect("expired row should remain for audit");
        assert_eq!((expired.wrapped_dek, expired.wrap_nonce), (None, None));

        assert_eq!(
            revoke_client_capability(user_id, never_expires_id, now)
                .expect("revocation should succeed"),
            RevokeClientCapabilityResult::Revoked
        );
        assert_eq!(
            revoke_client_capability(user_id, never_expires_id, now + Duration::seconds(1))
                .expect("same revocation should be idempotent"),
            RevokeClientCapabilityResult::AlreadyRevoked
        );
        assert_eq!(
            revoke_client_capability(UserId::new(), never_expires_id, now)
                .expect("other user lookup should succeed"),
            RevokeClientCapabilityResult::NotFound
        );
        let revoked =
            find_capability_identity_by_verifier(ClientKeyVerifier::from_raw_key(&[10_u8; 32]))
                .expect("revoked identity should load")
                .expect("revoked row should remain for audit");
        assert_eq!((revoked.wrapped_dek, revoked.wrap_nonce), (None, None));
        assert_eq!(revoked.revoked_at, Some(now));
    }

    #[test]
    fn durable_capability_ids_include_revoked_and_expired_rows() {
        let (_runtime, user_id) = setup();
        let now = Utc::now();
        let revoked_id = CapabilityId::from_bytes([18_u8; 32]);
        insert_active_client_capability(&active_record(
            user_id,
            revoked_id,
            &[18_u8; 32],
            now,
            None,
        ))
        .expect("revoked fixture should insert");
        revoke_client_capability(user_id, revoked_id, now).expect("revoked fixture should revoke");

        let expired_id = CapabilityId::from_bytes([19_u8; 32]);
        insert_active_client_capability(&active_record(
            user_id,
            expired_id,
            &[19_u8; 32],
            now - Duration::minutes(2),
            Some(now - Duration::minutes(1)),
        ))
        .expect("expired fixture should insert");

        assert_eq!(
            capability_ids_for_user(user_id).expect("durable IDs should load"),
            HashSet::from([revoked_id, expired_id])
        );
    }

    #[test]
    fn activity_updates_capability_and_user_in_one_transaction() {
        let (_runtime, user_id) = setup();
        let now = Utc::now();
        let capability_id = CapabilityId::from_bytes([20_u8; 32]);
        insert_active_client_capability(&active_record(
            user_id,
            capability_id,
            &[20_u8; 32],
            now - Duration::minutes(1),
            None,
        ))
        .expect("capability should insert");

        record_client_capability_activity(capability_id, user_id, now)
            .expect("activity should update atomically");
        let (capability_activity, user_activity): (String, String) = with_db(|conn| {
            let capability_activity = conn
                .query_row(
                    "SELECT last_used_at FROM client_capabilities WHERE capability_id = ?1",
                    params![capability_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|error| DbError::from_rusqlite_error("load capability activity", error))?;
            let user_activity = conn
                .query_row(
                    "SELECT last_login_at FROM users WHERE user_id = ?1",
                    params![user_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|error| DbError::from_rusqlite_error("load user activity", error))?;
            Ok::<_, DbError>((capability_activity, user_activity))
        })
        .expect("activity timestamps should load");
        assert_eq!(capability_activity, now.to_rfc3339());
        assert_eq!(user_activity, now.to_rfc3339());
    }
}
