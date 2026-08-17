use super::error::DbError;
use crate::models::UserId;
use chrono::Utc;
use rusqlite::{OptionalExtension, params};

/// Read the price-fetching consent flag. Missing row = disabled.
pub(crate) fn get_price_fetching_enabled(user_id: UserId) -> Result<bool, DbError> {
    super::with_db(|conn| {
        let enabled: Option<i64> = conn
            .query_row(
                "SELECT price_fetching_enabled FROM app_user_preferences WHERE user_id = ?1",
                params![user_id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| DbError::from_rusqlite_error("get_price_fetching_enabled", e))?;
        Ok(enabled.unwrap_or(0) != 0)
    })
}

/// Upsert the price-fetching consent flag.
#[cfg(test)]
pub(crate) fn set_price_fetching_enabled(user_id: UserId, enabled: bool) -> Result<(), DbError> {
    let _ = set_price_fetching_enabled_with_transition(user_id, enabled)?;
    Ok(())
}

/// Upsert the price-fetching consent flag and report false/missing -> true transitions.
pub(crate) fn set_price_fetching_enabled_with_transition(
    user_id: UserId,
    enabled: bool,
) -> Result<bool, DbError> {
    let now = Utc::now().to_rfc3339();
    super::with_db_mut(|conn| {
        conn.execute("BEGIN IMMEDIATE", [])
            .map_err(|e| DbError::from_rusqlite_error("begin set_price_fetching_enabled", e))?;
        let result = set_price_fetching_enabled_in_transaction(conn, user_id, enabled, &now);
        match result {
            Ok(changed_to_enabled) => match conn.execute("COMMIT", []) {
                Ok(_) => Ok(changed_to_enabled),
                Err(err) => {
                    let commit_error =
                        DbError::from_rusqlite_error("commit set_price_fetching_enabled", err);
                    let _ = conn.execute("ROLLBACK", []);
                    Err(commit_error)
                }
            },
            Err(err) => {
                let _ = conn.execute("ROLLBACK", []);
                Err(err)
            }
        }
    })
}

fn set_price_fetching_enabled_in_transaction(
    conn: &rusqlite::Connection,
    user_id: UserId,
    enabled: bool,
    now: &str,
) -> Result<bool, DbError> {
    let previous_enabled: Option<i64> = conn
        .query_row(
            "SELECT price_fetching_enabled FROM app_user_preferences WHERE user_id = ?1",
            params![user_id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| DbError::from_rusqlite_error("read previous price_fetching_enabled", e))?;
    let changed_to_enabled = enabled && previous_enabled.unwrap_or(0) == 0;

    conn.execute(
        "INSERT INTO app_user_preferences (user_id, price_fetching_enabled, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?3)
         ON CONFLICT(user_id) DO UPDATE SET
             price_fetching_enabled = excluded.price_fetching_enabled,
             updated_at = excluded.updated_at",
        params![user_id.to_string(), enabled as i64, now],
    )
    .map_err(|e| DbError::from_rusqlite_error("set_price_fetching_enabled", e))?;

    Ok(changed_to_enabled)
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use crate::db::{enable_test_mode, reset_test_db, with_db_mut};

    fn setup_user() -> UserId {
        enable_test_mode();
        reset_test_db();
        let user_id = UserId::new();
        let now = Utc::now().to_rfc3339();
        with_db_mut(|conn| {
            conn.execute(
                "INSERT INTO users (user_id, username, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?3)",
                params![user_id.to_string(), format!("price-{user_id}"), now],
            )
            .map_err(|e| DbError::from_rusqlite_error("seed app user", e))?;
            Ok::<(), DbError>(())
        })
        .expect("seed app user");
        user_id
    }

    #[test]
    fn price_fetching_defaults_to_disabled_when_no_row() {
        let user_id = setup_user();
        assert!(!get_price_fetching_enabled(user_id).unwrap());
    }

    #[test]
    fn price_fetching_upsert_round_trips_and_toggles() {
        let user_id = setup_user();
        set_price_fetching_enabled(user_id, true).unwrap();
        assert!(get_price_fetching_enabled(user_id).unwrap());
        set_price_fetching_enabled(user_id, false).unwrap();
        assert!(!get_price_fetching_enabled(user_id).unwrap());
    }

    #[test]
    fn price_fetching_transition_helper_reports_missing_to_enabled() {
        let user_id = setup_user();

        assert!(set_price_fetching_enabled_with_transition(user_id, true).unwrap());
        assert!(get_price_fetching_enabled(user_id).unwrap());
    }

    #[test]
    fn price_fetching_transition_helper_does_not_report_missing_to_disabled() {
        let user_id = setup_user();

        assert!(!set_price_fetching_enabled_with_transition(user_id, false).unwrap());
        assert!(!get_price_fetching_enabled(user_id).unwrap());
    }

    #[test]
    fn price_fetching_transition_helper_reports_false_to_true_only_once() {
        let user_id = setup_user();
        set_price_fetching_enabled(user_id, false).unwrap();

        assert!(set_price_fetching_enabled_with_transition(user_id, true).unwrap());
        assert!(!set_price_fetching_enabled_with_transition(user_id, true).unwrap());
        assert!(get_price_fetching_enabled(user_id).unwrap());
    }

    #[test]
    fn price_fetching_transition_helper_does_not_report_disables() {
        let user_id = setup_user();
        set_price_fetching_enabled(user_id, true).unwrap();

        assert!(!set_price_fetching_enabled_with_transition(user_id, false).unwrap());
        assert!(!get_price_fetching_enabled(user_id).unwrap());
    }
}
