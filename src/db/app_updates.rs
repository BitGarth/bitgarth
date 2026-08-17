use super::error::DbError;
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};

const UPDATE_STATE_ID: &str = "singleton";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AppUpdateState {
    pub(crate) update_check_enabled: bool,
    pub(crate) last_checked_at: Option<DateTime<Utc>>,
    pub(crate) latest_seen: Option<String>,
    pub(crate) release_url: Option<String>,
    pub(crate) published_at: Option<String>,
}

fn parse_datetime(value: Option<String>) -> Result<Option<DateTime<Utc>>, DbError> {
    value
        .map(|raw| {
            DateTime::parse_from_rfc3339(&raw)
                .map(|dt| dt.with_timezone(&Utc))
                .map_err(|err| DbError::new(format!("invalid update timestamp: {err}")))
        })
        .transpose()
}

pub(crate) fn load_update_state() -> Result<AppUpdateState, DbError> {
    super::with_db(|conn| {
        let row = conn
            .query_row(
                "SELECT update_check_enabled, last_checked_at, latest_seen, release_url, published_at
                 FROM app_update_state WHERE id = ?1",
                params![UPDATE_STATE_ID],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(|err| DbError::from_rusqlite_error("load_update_state", err))?;

        let Some((enabled, checked_at, latest_seen, release_url, published_at)) = row else {
            return Ok(AppUpdateState {
                update_check_enabled: true,
                last_checked_at: None,
                latest_seen: None,
                release_url: None,
                published_at: None,
            });
        };

        Ok(AppUpdateState {
            update_check_enabled: enabled != 0,
            last_checked_at: parse_datetime(checked_at)?,
            latest_seen,
            release_url,
            published_at,
        })
    })
}

pub(crate) fn save_successful_update_check(
    latest_seen: &str,
    release_url: &str,
    published_at: Option<&str>,
    checked_at: DateTime<Utc>,
) -> Result<(), DbError> {
    super::with_db_mut(|conn| {
        conn.execute(
            "INSERT INTO app_update_state
             (id, update_check_enabled, last_checked_at, latest_seen, release_url, published_at, updated_at)
             VALUES (?1, 1, ?2, ?3, ?4, ?5, ?2)
             ON CONFLICT(id) DO UPDATE SET
                last_checked_at = excluded.last_checked_at,
                latest_seen = excluded.latest_seen,
                release_url = excluded.release_url,
                published_at = excluded.published_at,
                updated_at = excluded.updated_at",
            params![
                UPDATE_STATE_ID,
                checked_at.to_rfc3339(),
                latest_seen,
                release_url,
                published_at,
            ],
        )
        .map_err(|err| DbError::from_rusqlite_error("save_successful_update_check", err))?;
        Ok(())
    })
}

pub(crate) fn set_update_check_enabled(
    enabled: bool,
    updated_at: DateTime<Utc>,
) -> Result<(), DbError> {
    super::with_db_mut(|conn| {
        conn.execute(
            "INSERT INTO app_update_state (id, update_check_enabled, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET
                update_check_enabled = excluded.update_check_enabled,
                updated_at = excluded.updated_at",
            params![UPDATE_STATE_ID, enabled as i64, updated_at.to_rfc3339()],
        )
        .map_err(|err| DbError::from_rusqlite_error("set_update_check_enabled", err))?;
        Ok(())
    })
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod tests {
    use crate::db::{
        AppUpdateState, enable_test_mode, load_update_state, reset_test_db,
        save_successful_update_check, set_update_check_enabled,
    };
    use chrono::{TimeZone, Utc};

    fn setup() {
        enable_test_mode();
        reset_test_db();
    }

    #[test]
    fn missing_state_defaults_to_enabled_and_unchecked() {
        setup();
        let state: AppUpdateState = load_update_state().unwrap();
        assert!(state.update_check_enabled);
        assert!(state.last_checked_at.is_none());
        assert!(state.latest_seen.is_none());
    }

    #[test]
    fn save_update_result_round_trips_release_metadata() {
        setup();
        let checked_at = Utc.with_ymd_and_hms(2026, 6, 7, 12, 0, 0).unwrap();
        save_successful_update_check(
            "0.1.5",
            "https://hub.docker.com/r/bitgarth/bitgarth/tags?name=0.1.5",
            Some("2026-06-07T12:00:00Z"),
            checked_at,
        )
        .unwrap();

        let state = load_update_state().unwrap();
        assert!(state.update_check_enabled);
        assert_eq!(state.latest_seen.as_deref(), Some("0.1.5"));
        assert_eq!(
            state.release_url.as_deref(),
            Some("https://hub.docker.com/r/bitgarth/bitgarth/tags?name=0.1.5")
        );
        assert_eq!(state.published_at.as_deref(), Some("2026-06-07T12:00:00Z"));
        assert_eq!(state.last_checked_at, Some(checked_at));
    }

    #[test]
    fn disabling_update_checks_persists_without_clearing_cached_release() {
        setup();
        let checked_at = Utc.with_ymd_and_hms(2026, 6, 7, 12, 0, 0).unwrap();
        save_successful_update_check("0.1.5", "https://example.invalid/release", None, checked_at)
            .unwrap();
        set_update_check_enabled(false, checked_at).unwrap();

        let state = load_update_state().unwrap();
        assert!(!state.update_check_enabled);
        assert_eq!(state.latest_seen.as_deref(), Some("0.1.5"));
    }
}
