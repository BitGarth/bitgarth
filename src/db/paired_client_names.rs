use super::DbError;
use crate::client_capabilities::CapabilityId;
use crate::models::UserId;
use rusqlite::{OptionalExtension, params};
use std::{
    collections::{HashMap, HashSet},
    str::FromStr,
};

pub(crate) fn insert_paired_client_name(
    user_id: UserId,
    capability_id: CapabilityId,
    display_name: &str,
) -> Result<(), DbError> {
    super::with_user_db_mut(user_id, |conn| {
        conn.execute(
            "INSERT OR IGNORE INTO paired_client_names (capability_id, display_name)
             VALUES (?1, ?2)",
            params![capability_id.to_string(), display_name],
        )
        .map_err(|error| DbError::from_rusqlite_error("insert paired client name", error))?;

        let stored_name: String = conn
            .query_row(
                "SELECT display_name FROM paired_client_names WHERE capability_id = ?1",
                params![capability_id.to_string()],
                |row| row.get(0),
            )
            .map_err(|error| DbError::from_rusqlite_error("verify paired client name", error))?;
        if stored_name != display_name {
            return Err(DbError::new(
                "paired client capability already has a different display name",
            ));
        }
        Ok(())
    })
}

pub(crate) fn load_paired_client_name(
    user_id: UserId,
    capability_id: CapabilityId,
) -> Result<Option<String>, DbError> {
    super::with_user_db(user_id, |conn| {
        conn.query_row(
            "SELECT display_name FROM paired_client_names WHERE capability_id = ?1",
            params![capability_id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| DbError::from_rusqlite_error("load paired client name", error))
    })
}

pub(crate) fn list_paired_client_names(
    user_id: UserId,
) -> Result<HashMap<CapabilityId, String>, DbError> {
    super::with_user_db(user_id, |conn| {
        let mut statement = conn
            .prepare("SELECT capability_id, display_name FROM paired_client_names")
            .map_err(|error| DbError::from_rusqlite_error("prepare paired client names", error))?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| DbError::from_rusqlite_error("query paired client names", error))?;

        rows.map(|row| {
            let (raw_id, display_name) = row
                .map_err(|error| DbError::from_rusqlite_error("read paired client name", error))?;
            let capability_id = CapabilityId::from_str(&raw_id)
                .map_err(|error| DbError::new(format!("invalid paired client ID: {error}")))?;
            Ok((capability_id, display_name))
        })
        .collect()
    })
}

pub(crate) fn delete_paired_client_name(
    user_id: UserId,
    capability_id: CapabilityId,
) -> Result<bool, DbError> {
    super::with_user_db_mut(user_id, |conn| {
        conn.execute(
            "DELETE FROM paired_client_names WHERE capability_id = ?1",
            params![capability_id.to_string()],
        )
        .map(|deleted| deleted == 1)
        .map_err(|error| DbError::from_rusqlite_error("delete paired client name", error))
    })
}

pub(crate) fn remove_orphan_paired_client_names(
    user_id: UserId,
    active_capability_ids: &HashSet<CapabilityId>,
) -> Result<usize, DbError> {
    super::with_user_db_mut(user_id, |conn| {
        remove_orphan_paired_client_names_conn(conn, active_capability_ids)
    })
}

pub(super) fn remove_orphan_paired_client_names_conn(
    conn: &mut rusqlite::Connection,
    active_capability_ids: &HashSet<CapabilityId>,
) -> Result<usize, DbError> {
    let transaction = conn.transaction().map_err(|error| {
        DbError::from_rusqlite_error("begin paired client orphan cleanup", error)
    })?;
    let stored_ids = {
        let mut statement = transaction
            .prepare("SELECT capability_id FROM paired_client_names")
            .map_err(|error| {
                DbError::from_rusqlite_error("prepare paired client orphan cleanup", error)
            })?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| {
                DbError::from_rusqlite_error("query paired client orphan cleanup", error)
            })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|error| {
            DbError::from_rusqlite_error("read paired client orphan cleanup", error)
        })?
    };

    let mut removed = 0;
    for raw_id in stored_ids {
        let keep = CapabilityId::from_str(&raw_id)
            .is_ok_and(|capability_id| active_capability_ids.contains(&capability_id));
        if !keep {
            removed += transaction
                .execute(
                    "DELETE FROM paired_client_names WHERE capability_id = ?1",
                    params![raw_id],
                )
                .map_err(|error| {
                    DbError::from_rusqlite_error("remove orphan paired client name", error)
                })?;
        }
    }
    transaction.commit().map_err(|error| {
        DbError::from_rusqlite_error("commit paired client orphan cleanup", error)
    })?;
    Ok(removed)
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;
    use crate::db::{acquire_test_runtime, setup_test_user, with_user_db};

    #[test]
    fn private_names_are_idempotent_and_orphans_are_removed() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        setup_test_user(user_id);
        let kept_id = CapabilityId::from_bytes([31_u8; 32]);
        let orphan_id = CapabilityId::from_bytes([32_u8; 32]);

        let columns = with_user_db(user_id, |conn| {
            let mut statement = conn
                .prepare("PRAGMA table_info(paired_client_names)")
                .map_err(|error| DbError::from_rusqlite_error("prepare name schema", error))?;
            statement
                .query_map([], |row| row.get::<_, String>(1))
                .map_err(|error| DbError::from_rusqlite_error("query name schema", error))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| DbError::from_rusqlite_error("read name schema", error))
        })
        .expect("name schema should load");
        assert_eq!(columns, vec!["capability_id", "display_name"]);

        insert_paired_client_name(user_id, kept_id, "business").expect("name should insert");
        insert_paired_client_name(user_id, kept_id, "business")
            .expect("same name retry should be idempotent");
        assert!(insert_paired_client_name(user_id, kept_id, "other").is_err());
        insert_paired_client_name(user_id, orphan_id, "orphan")
            .expect("orphan fixture should insert");

        let active_ids = HashSet::from([kept_id]);
        assert_eq!(
            remove_orphan_paired_client_names(user_id, &active_ids)
                .expect("orphan cleanup should succeed"),
            1
        );
        assert_eq!(
            load_paired_client_name(user_id, kept_id).expect("kept name should load"),
            Some("business".to_owned())
        );
        assert_eq!(
            load_paired_client_name(user_id, orphan_id).expect("orphan lookup should succeed"),
            None
        );
        assert_eq!(
            list_paired_client_names(user_id)
                .expect("name list should load")
                .len(),
            1
        );
        assert!(delete_paired_client_name(user_id, kept_id).expect("name delete should succeed"));
    }
}
