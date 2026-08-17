use super::error::DbError;
use crate::payments::free_tier::{FreeTierCapabilities, FreeTierObservation};
use crate::payments::types::CAPABILITY_SCHEMA_VERSION_V3;
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};

const FREE_TIER_CACHE_ID: &str = "singleton";

fn parse_utc(value: String, context: &str) -> Result<DateTime<Utc>, DbError> {
    DateTime::parse_from_rfc3339(&value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|err| DbError::new(format!("invalid free tier entitlement {context}: {err}")))
}

pub(crate) fn load_free_tier_entitlement_cache() -> Result<Option<FreeTierObservation>, DbError> {
    super::with_db(|conn| {
        let row = conn
            .query_row(
                "SELECT capability_schema_version, capabilities_json, fetched_at
                 FROM free_tier_entitlement_cache
                 WHERE id = ?1",
                params![FREE_TIER_CACHE_ID],
                |row| {
                    Ok((
                        row.get::<_, u16>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|err| DbError::from_rusqlite_error("load free tier entitlement cache", err))?;

        let Some((capability_schema_version, capabilities_json, fetched_at)) = row else {
            return Ok(None);
        };

        if capability_schema_version != CAPABILITY_SCHEMA_VERSION_V3 {
            return Err(DbError::new(format!(
                "unsupported free tier entitlement capability schema version: {capability_schema_version}"
            )));
        }

        let capabilities: FreeTierCapabilities =
            serde_json::from_str(&capabilities_json).map_err(|err| {
                DbError::new(format!(
                    "invalid free tier entitlement capabilities JSON: {err}"
                ))
            })?;

        Ok(Some(FreeTierObservation {
            observed_at: parse_utc(fetched_at, "fetched_at")?,
            capability_schema_version,
            capabilities,
        }))
    })
}

pub(crate) fn upsert_free_tier_entitlement_cache(
    observation: &FreeTierObservation,
) -> Result<(), DbError> {
    if observation.capability_schema_version != CAPABILITY_SCHEMA_VERSION_V3 {
        return Err(DbError::new(format!(
            "unsupported free tier entitlement capability schema version: {}",
            observation.capability_schema_version
        )));
    }

    let capabilities_json = serde_json::to_string(&observation.capabilities).map_err(|err| {
        DbError::new(format!(
            "serialize free tier entitlement capabilities JSON: {err}"
        ))
    })?;
    let observed_at = observation.observed_at.to_rfc3339();

    super::with_db_mut(|conn| {
        conn.execute(
            "INSERT INTO free_tier_entitlement_cache
             (id, capability_schema_version, capabilities_json, fetched_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4)
             ON CONFLICT(id) DO UPDATE SET
                capability_schema_version = excluded.capability_schema_version,
                capabilities_json = excluded.capabilities_json,
                fetched_at = excluded.fetched_at,
                updated_at = excluded.updated_at
             WHERE excluded.fetched_at >= free_tier_entitlement_cache.fetched_at",
            params![
                FREE_TIER_CACHE_ID,
                observation.capability_schema_version,
                capabilities_json,
                observed_at,
            ],
        )
        .map_err(|err| DbError::from_rusqlite_error("upsert free tier entitlement cache", err))?;
        Ok(())
    })
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use crate::db::{enable_test_mode, reset_test_db};
    use crate::payments::free_tier::free_tier_capabilities_for_test;
    use chrono::{TimeZone, Utc};

    fn setup() {
        enable_test_mode();
        reset_test_db();
    }

    fn observation(accounts: u16, hour: u32) -> FreeTierObservation {
        FreeTierObservation {
            observed_at: Utc.with_ymd_and_hms(2026, 6, 30, hour, 0, 0).unwrap(),
            capability_schema_version: CAPABILITY_SCHEMA_VERSION_V3,
            capabilities: free_tier_capabilities_for_test(accounts),
        }
    }

    #[test]
    fn missing_cache_returns_none() {
        setup();

        assert_eq!(load_free_tier_entitlement_cache().unwrap(), None);
    }

    #[test]
    fn upsert_round_trips_singleton_cache() {
        setup();

        upsert_free_tier_entitlement_cache(&observation(20, 1)).unwrap();
        upsert_free_tier_entitlement_cache(&observation(30, 2)).unwrap();

        let loaded = load_free_tier_entitlement_cache().unwrap().unwrap();
        let row_count = super::super::with_db(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM free_tier_entitlement_cache",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|err| DbError::from_rusqlite_error("count free tier cache rows", err))
        })
        .unwrap();

        assert_eq!(row_count, 1);
        assert_eq!(loaded.observed_at, observation(30, 2).observed_at);
        assert_eq!(loaded.capabilities.limits.accounts.total, 30);
    }

    #[test]
    fn upsert_ignores_older_observation() {
        setup();

        upsert_free_tier_entitlement_cache(&observation(30, 2)).unwrap();
        upsert_free_tier_entitlement_cache(&observation(20, 1)).unwrap();

        let loaded = load_free_tier_entitlement_cache().unwrap().unwrap();
        assert_eq!(loaded.observed_at, observation(30, 2).observed_at);
        assert_eq!(loaded.capabilities.limits.accounts.total, 30);
    }

    #[test]
    fn corrupt_capabilities_json_is_an_error_not_a_silent_default() {
        setup();
        let fetched_at = observation(20, 1).observed_at.to_rfc3339();

        super::super::with_db_mut(|conn| {
            conn.execute(
                "INSERT INTO free_tier_entitlement_cache
                 (id, capability_schema_version, capabilities_json, fetched_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4)",
                params![
                    FREE_TIER_CACHE_ID,
                    CAPABILITY_SCHEMA_VERSION_V3,
                    r#"{"limits":{"history":{"max_transactions_per_account":0}},"features":{"transaction_history_sync":false,"balance_sync":true,"exchange_rates_current":true,"exchange_rates_history":false,"price_overrides":false,"balance_assertions":false,"hledger_export":false,"tax_reports":false}}"#,
                    fetched_at,
                ],
            )
            .map_err(|err| DbError::from_rusqlite_error("seed corrupt free tier cache", err))?;
            Ok::<_, DbError>(())
        })
        .unwrap();

        assert!(load_free_tier_entitlement_cache().is_err());
    }
}
