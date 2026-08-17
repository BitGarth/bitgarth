use super::app_db::with_app_db;
use super::app_db::with_app_db_mut;
use super::error::DbError;
use crate::models::UserId;
use crate::payments::keys::VerifiedEntitlementToken;
#[cfg(test)]
use crate::payments::types::capability_schema_version_from_storage_json;
use crate::payments::types::{
    EntitlementHolderId, EntitlementTier, SubscriptionSubjectId, TokenId,
    entitlement_capabilities_storage_json,
};
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{OptionalExtension, params};
#[cfg(test)]
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AppEntitlementSnapshotSource {
    PaymentPoll,
    PaymentReconcile,
    LoginRefresh,
    PaymentsRefresh,
    Refresh,
}

impl AppEntitlementSnapshotSource {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::PaymentPoll => "payment_poll",
            Self::PaymentReconcile => "payment_reconcile",
            Self::LoginRefresh => "login_refresh",
            Self::PaymentsRefresh => "payments_refresh",
            Self::Refresh => "refresh",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NewAppEntitlementSnapshot {
    pub(crate) user_id: UserId,
    pub(crate) source: AppEntitlementSnapshotSource,
    pub(crate) entitlement_holder_id: EntitlementHolderId,
    pub(crate) subscription_subject_id: Option<SubscriptionSubjectId>,
    pub(crate) token_id: Option<TokenId>,
    pub(crate) entitlement_tier: EntitlementTier,
    pub(crate) subscription_valid_until: Option<DateTime<Utc>>,
    pub(crate) token_expires_at: Option<DateTime<Utc>>,
    pub(crate) token_issued_at: Option<DateTime<Utc>>,
    pub(crate) capability_set_id: Option<String>,
    pub(crate) capabilities_json: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg(test)]
pub(crate) struct AppEntitlementSnapshotRecord {
    pub(crate) snapshot_id: String,
    pub(crate) user_id: UserId,
    pub(crate) recorded_at: DateTime<Utc>,
    pub(crate) source: String,
    pub(crate) entitlement_holder_id: EntitlementHolderId,
    pub(crate) subscription_subject_id: Option<SubscriptionSubjectId>,
    pub(crate) token_id: Option<TokenId>,
    pub(crate) entitlement_tier: EntitlementTier,
    pub(crate) subscription_valid_until: Option<DateTime<Utc>>,
    pub(crate) token_expires_at: Option<DateTime<Utc>>,
    pub(crate) token_issued_at: Option<DateTime<Utc>>,
    pub(crate) capability_set_id: Option<String>,
    pub(crate) capability_schema_version: u16,
    pub(crate) capabilities_json: Option<String>,
}

pub(crate) fn record_verified_app_entitlement_snapshot(
    user_id: UserId,
    source: AppEntitlementSnapshotSource,
    verified: &VerifiedEntitlementToken,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    let capabilities_json = entitlement_capabilities_storage_json(
        verified.claims.capability_schema_version,
        &verified.claims.capabilities,
    )
    .map_err(|err| DbError::new(format!("Failed to serialize capabilities: {err}")))?;
    record_app_entitlement_snapshot(
        &NewAppEntitlementSnapshot {
            user_id,
            source,
            entitlement_holder_id: verified.claims.entitlement_holder_id,
            subscription_subject_id: Some(verified.claims.subscription_subject_id),
            token_id: Some(verified.claims.token_id),
            entitlement_tier: verified.claims.tier.clone(),
            subscription_valid_until: Some(verified.claims.subscription_valid_until),
            token_expires_at: Some(verified.claims.token_expires_at),
            token_issued_at: Some(verified.claims.issued_at),
            capability_set_id: verified.claims.capability_set_id.clone(),
            capabilities_json: Some(capabilities_json),
        },
        now,
    )
}

pub(crate) fn record_app_entitlement_snapshot(
    snapshot: &NewAppEntitlementSnapshot,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    with_app_db_mut(|conn| {
        if let Some(token_id) = snapshot.token_id {
            let existing_snapshot_id = conn
                .query_row(
                    "SELECT snapshot_id FROM app_entitlement_snapshots WHERE token_id = ?1",
                    [token_id.to_storage_value()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(|err| {
                    DbError::from_rusqlite_error(
                        "Failed to query existing app entitlement snapshot",
                        err,
                    )
                })?;
            if let Some(snapshot_id) = existing_snapshot_id {
                update_snapshot_by_id(conn, &snapshot_id, snapshot, now)?;
                return Ok(());
            }
        }

        insert_snapshot(conn, snapshot, now)
    })
}

#[cfg(test)]
pub(crate) fn load_app_entitlement_snapshots_for_user(
    user_id: UserId,
) -> Result<Vec<AppEntitlementSnapshotRecord>, DbError> {
    with_app_db(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT snapshot_id, user_id, recorded_at, source, entitlement_holder_id, \
                        subscription_subject_id, token_id, entitlement_tier, \
                        subscription_valid_until, token_expires_at, token_issued_at, \
                        capability_set_id, capabilities_json \
                 FROM app_entitlement_snapshots \
                 WHERE user_id = ?1 \
                 ORDER BY recorded_at ASC, snapshot_id ASC",
            )
            .map_err(|err| {
                DbError::from_rusqlite_error(
                    "Failed to prepare app entitlement snapshots query",
                    err,
                )
            })?;

        let rows = stmt
            .query_map([user_id.to_string()], parse_snapshot_row)
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to query app entitlement snapshots", err)
            })?;

        let mut snapshots = Vec::new();
        for row in rows {
            snapshots.push(row.map_err(|err| {
                DbError::from_rusqlite_error("Failed to collect app entitlement snapshots", err)
            })?);
        }
        Ok(snapshots)
    })
}

/// True if the user currently holds a paid (non-free) entitlement whose
/// `subscription_valid_until` is still in the future at `now`. Reads only the
/// unencrypted app DB; no user DB is unlocked. Used by the hosted retention
/// disclosure and (later) the inactive-user deletion sweeper.
pub(crate) fn user_has_active_paid_entitlement(
    user_id: UserId,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    with_app_db(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT subscription_valid_until \
                 FROM app_entitlement_snapshots \
                 WHERE user_id = ?1 \
                   AND entitlement_tier != 'free' \
                   AND subscription_valid_until IS NOT NULL",
            )
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to prepare active-paid query", err)
            })?;

        let rows = stmt
            .query_map([user_id.to_string()], |row| row.get::<_, String>(0))
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to query active-paid snapshots", err)
            })?;

        for row in rows {
            let raw = row.map_err(|err| {
                DbError::from_rusqlite_error("Failed to read subscription_valid_until", err)
            })?;
            if let Ok(valid_until) = DateTime::parse_from_rfc3339(&raw)
                && valid_until.with_timezone(&Utc) > now
            {
                return Ok(true);
            }
        }
        Ok(false)
    })
}

fn insert_snapshot(
    conn: &rusqlite::Connection,
    snapshot: &NewAppEntitlementSnapshot,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    let snapshot_id = ulid::Ulid::new().to_string();
    let storage = SnapshotStorageValues::from_snapshot(&snapshot_id, snapshot, now);
    conn.execute(
        "INSERT INTO app_entitlement_snapshots \
         (snapshot_id, user_id, recorded_at, source, entitlement_holder_id, \
          subscription_subject_id, token_id, entitlement_tier, subscription_valid_until, \
          token_expires_at, token_issued_at, capability_set_id, capabilities_json) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            storage.snapshot_id,
            storage.user_id,
            storage.recorded_at,
            storage.source,
            storage.entitlement_holder_id,
            storage.subscription_subject_id,
            storage.token_id,
            storage.entitlement_tier,
            storage.subscription_valid_until,
            storage.token_expires_at,
            storage.token_issued_at,
            storage.capability_set_id,
            storage.capabilities_json,
        ],
    )
    .map_err(|err| {
        DbError::from_rusqlite_error("Failed to insert app entitlement snapshot", err)
    })?;
    Ok(())
}

fn update_snapshot_by_id(
    conn: &rusqlite::Connection,
    snapshot_id: &str,
    snapshot: &NewAppEntitlementSnapshot,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    let storage = SnapshotStorageValues::from_snapshot(snapshot_id, snapshot, now);
    conn.execute(
        "UPDATE app_entitlement_snapshots \
         SET user_id = ?2, recorded_at = ?3, source = ?4, entitlement_holder_id = ?5, \
             subscription_subject_id = ?6, token_id = ?7, entitlement_tier = ?8, \
             subscription_valid_until = ?9, token_expires_at = ?10, token_issued_at = ?11, \
             capability_set_id = ?12, capabilities_json = ?13 \
         WHERE snapshot_id = ?1",
        params![
            storage.snapshot_id,
            storage.user_id,
            storage.recorded_at,
            storage.source,
            storage.entitlement_holder_id,
            storage.subscription_subject_id,
            storage.token_id,
            storage.entitlement_tier,
            storage.subscription_valid_until,
            storage.token_expires_at,
            storage.token_issued_at,
            storage.capability_set_id,
            storage.capabilities_json,
        ],
    )
    .map_err(|err| {
        DbError::from_rusqlite_error("Failed to update app entitlement snapshot", err)
    })?;
    Ok(())
}

struct SnapshotStorageValues<'a> {
    snapshot_id: &'a str,
    user_id: String,
    recorded_at: String,
    source: &'static str,
    entitlement_holder_id: String,
    subscription_subject_id: Option<String>,
    token_id: Option<String>,
    entitlement_tier: &'a str,
    subscription_valid_until: Option<String>,
    token_expires_at: Option<String>,
    token_issued_at: Option<String>,
    capability_set_id: Option<&'a str>,
    capabilities_json: Option<&'a str>,
}

impl<'a> SnapshotStorageValues<'a> {
    fn from_snapshot(
        snapshot_id: &'a str,
        snapshot: &'a NewAppEntitlementSnapshot,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            snapshot_id,
            user_id: snapshot.user_id.to_string(),
            recorded_at: format_timestamp(now),
            source: snapshot.source.as_str(),
            entitlement_holder_id: snapshot.entitlement_holder_id.to_storage_value(),
            subscription_subject_id: snapshot
                .subscription_subject_id
                .map(SubscriptionSubjectId::to_storage_value),
            token_id: snapshot.token_id.map(TokenId::to_storage_value),
            entitlement_tier: snapshot.entitlement_tier.as_str(),
            subscription_valid_until: snapshot.subscription_valid_until.map(format_timestamp),
            token_expires_at: snapshot.token_expires_at.map(format_timestamp),
            token_issued_at: snapshot.token_issued_at.map(format_timestamp),
            capability_set_id: snapshot.capability_set_id.as_deref(),
            capabilities_json: snapshot.capabilities_json.as_deref(),
        }
    }
}

#[cfg(test)]
fn parse_snapshot_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AppEntitlementSnapshotRecord> {
    let user_id_raw: String = row.get(1)?;
    let recorded_at_raw: String = row.get(2)?;
    let entitlement_holder_id_raw: String = row.get(4)?;
    let subscription_subject_id_raw: Option<String> = row.get(5)?;
    let token_id_raw: Option<String> = row.get(6)?;
    let entitlement_tier_raw: String = row.get(7)?;
    let subscription_valid_until_raw: Option<String> = row.get(8)?;
    let token_expires_at_raw: Option<String> = row.get(9)?;
    let token_issued_at_raw: Option<String> = row.get(10)?;
    let capabilities_json: Option<String> = row.get(12)?;
    let capability_schema_version =
        capability_schema_version_from_storage_json(capabilities_json.as_deref());

    Ok(AppEntitlementSnapshotRecord {
        snapshot_id: row.get(0)?,
        user_id: parse_user_id(&user_id_raw)?,
        recorded_at: parse_timestamp(&recorded_at_raw, "recorded_at")?,
        source: row.get(3)?,
        entitlement_holder_id: parse_entitlement_holder_id(&entitlement_holder_id_raw)?,
        subscription_subject_id: subscription_subject_id_raw
            .as_deref()
            .map(parse_subscription_subject_id)
            .transpose()?,
        token_id: token_id_raw.as_deref().map(parse_token_id).transpose()?,
        entitlement_tier: parse_entitlement_tier(&entitlement_tier_raw)?,
        subscription_valid_until: subscription_valid_until_raw
            .as_deref()
            .map(|value| parse_timestamp(value, "subscription_valid_until"))
            .transpose()?,
        token_expires_at: token_expires_at_raw
            .as_deref()
            .map(|value| parse_timestamp(value, "token_expires_at"))
            .transpose()?,
        token_issued_at: token_issued_at_raw
            .as_deref()
            .map(|value| parse_timestamp(value, "token_issued_at"))
            .transpose()?,
        capability_set_id: row.get(11)?,
        capability_schema_version,
        capabilities_json,
    })
}

#[cfg(test)]
fn parse_user_id(value: &str) -> rusqlite::Result<UserId> {
    UserId::from_str(value).map_err(|err| conversion_error("user_id", err))
}

#[cfg(test)]
fn parse_entitlement_holder_id(value: &str) -> rusqlite::Result<EntitlementHolderId> {
    EntitlementHolderId::from_str(value)
        .map_err(|err| conversion_error("entitlement_holder_id", err))
}

#[cfg(test)]
fn parse_subscription_subject_id(value: &str) -> rusqlite::Result<SubscriptionSubjectId> {
    SubscriptionSubjectId::from_str(value)
        .map_err(|err| conversion_error("subscription_subject_id", err))
}

#[cfg(test)]
fn parse_token_id(value: &str) -> rusqlite::Result<TokenId> {
    TokenId::from_str(value).map_err(|err| conversion_error("token_id", err))
}

#[cfg(test)]
fn parse_entitlement_tier(value: &str) -> rusqlite::Result<EntitlementTier> {
    EntitlementTier::from_str(value).map_err(|err| conversion_error("entitlement_tier", err))
}

#[cfg(test)]
fn parse_timestamp(value: &str, field: &'static str) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|err| conversion_error(field, err))
}

#[cfg(test)]
fn conversion_error(
    field: &'static str,
    err: impl std::error::Error + Send + Sync + 'static,
) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(DbError::new(format!(
            "Invalid app entitlement snapshot {field}: {err}"
        ))),
    )
}

fn format_timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;
    use crate::db;
    use crate::payments::types::{
        CAPABILITY_SCHEMA_VERSION_V3, EntitlementCapabilities, EntitlementSource,
        FeatureEntitlements,
    };
    use std::error::Error;

    fn test_now() -> DateTime<Utc> {
        "2026-05-08T12:00:00Z"
            .parse()
            .unwrap_or_else(|_| unreachable!("hardcoded timestamp is valid"))
    }

    fn test_holder_id() -> EntitlementHolderId {
        EntitlementHolderId::from_str("01JQABCDEF000000000000000A")
            .unwrap_or_else(|_| unreachable!("hardcoded holder id is valid"))
    }

    fn test_subject_id() -> SubscriptionSubjectId {
        SubscriptionSubjectId::from_str("01JQABCDEF000000000000000B")
            .unwrap_or_else(|_| unreachable!("hardcoded subject id is valid"))
    }

    fn test_token_id() -> TokenId {
        TokenId::from_str("01JQABCDEF000000000000000C")
            .unwrap_or_else(|_| unreachable!("hardcoded token id is valid"))
    }

    fn snapshot_for(
        user_id: UserId,
        entitlement_tier: EntitlementTier,
        token_id: Option<TokenId>,
    ) -> NewAppEntitlementSnapshot {
        NewAppEntitlementSnapshot {
            user_id,
            source: AppEntitlementSnapshotSource::PaymentPoll,
            entitlement_holder_id: test_holder_id(),
            subscription_subject_id: Some(test_subject_id()),
            token_id,
            entitlement_tier,
            subscription_valid_until: Some(
                "2027-05-08T12:00:00Z"
                    .parse()
                    .unwrap_or_else(|_| unreachable!("hardcoded timestamp is valid")),
            ),
            token_expires_at: Some(
                "2026-05-15T12:00:00Z"
                    .parse()
                    .unwrap_or_else(|_| unreachable!("hardcoded timestamp is valid")),
            ),
            token_issued_at: Some(test_now()),
            capability_set_id: Some("capset_paid_v1".to_string()),
            capabilities_json: Some(
                entitlement_capabilities_storage_json(
                    CAPABILITY_SCHEMA_VERSION_V3,
                    &EntitlementCapabilities::v3_from_parts(10, 10000, true),
                )
                .unwrap_or_else(|_| unreachable!("capabilities serialize")),
            ),
        }
    }

    fn verified_token_for(entitlement_tier: EntitlementTier) -> VerifiedEntitlementToken {
        let claims = crate::payments::types::TokenClaims {
            token_id: test_token_id(),
            subscription_subject_id: test_subject_id(),
            entitlement_holder_id: test_holder_id(),
            tier: entitlement_tier,
            capability_set_id: Some("capset_paid_v1".to_string()),
            capability_schema_version: CAPABILITY_SCHEMA_VERSION_V3,
            capabilities: EntitlementCapabilities::v3_from_parts(10, 10000, true),
            subscription_valid_until: "2027-05-08T12:00:00Z"
                .parse()
                .unwrap_or_else(|_| unreachable!("hardcoded timestamp is valid")),
            token_expires_at: "2026-05-15T12:00:00Z"
                .parse()
                .unwrap_or_else(|_| unreachable!("hardcoded timestamp is valid")),
            issued_at: test_now(),
        };
        let entitlements = FeatureEntitlements::from_capabilities(
            claims.tier.clone(),
            claims.capability_schema_version,
            claims.capabilities.clone(),
            Some(claims.subscription_valid_until),
            Some(claims.token_expires_at),
            EntitlementSource::SignedCentralToken,
        );
        VerifiedEntitlementToken {
            compact_token: "claims.signature".to_string(),
            claims,
            entitlements,
        }
    }

    #[test]
    fn verified_basic_entitlement_snapshot_serializes_paid_metadata() -> Result<(), Box<dyn Error>>
    {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        let verified = verified_token_for(EntitlementTier::Basic);

        record_verified_app_entitlement_snapshot(
            user_id,
            AppEntitlementSnapshotSource::PaymentPoll,
            &verified,
            test_now(),
        )?;

        let snapshots = load_app_entitlement_snapshots_for_user(user_id)?;
        assert_eq!(snapshots.len(), 1);
        let snapshot = &snapshots[0];
        assert_eq!(snapshot.user_id, user_id);
        assert_eq!(snapshot.recorded_at, test_now());
        assert_eq!(snapshot.source, "payment_poll");
        assert_eq!(snapshot.entitlement_holder_id, test_holder_id());
        assert_eq!(snapshot.subscription_subject_id, Some(test_subject_id()));
        assert_eq!(snapshot.token_id, Some(test_token_id()));
        assert_eq!(snapshot.entitlement_tier, EntitlementTier::Basic);
        assert_eq!(
            snapshot.subscription_valid_until,
            Some("2027-05-08T12:00:00Z".parse()?)
        );
        assert_eq!(
            snapshot.token_expires_at,
            Some("2026-05-15T12:00:00Z".parse()?)
        );
        assert_eq!(snapshot.token_issued_at, Some(test_now()));
        assert_eq!(
            snapshot.capability_set_id.as_deref(),
            Some("capset_paid_v1")
        );
        assert_eq!(
            snapshot.capability_schema_version,
            CAPABILITY_SCHEMA_VERSION_V3
        );
        assert!(snapshot.capabilities_json.is_some());
        assert!(
            snapshot
                .capabilities_json
                .as_deref()
                .is_some_and(|json| json.contains("\"capability_schema_version\":3"))
        );
        Ok(())
    }

    #[test]
    fn verified_premium_entitlement_snapshot_serializes_paid_metadata() -> Result<(), Box<dyn Error>>
    {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        let verified = verified_token_for(EntitlementTier::Premium);

        record_verified_app_entitlement_snapshot(
            user_id,
            AppEntitlementSnapshotSource::PaymentPoll,
            &verified,
            test_now(),
        )?;

        let snapshots = load_app_entitlement_snapshots_for_user(user_id)?;
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].entitlement_tier, EntitlementTier::Premium);
        Ok(())
    }

    #[test]
    fn repeated_non_null_token_id_updates_existing_snapshot() -> Result<(), Box<dyn Error>> {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        let mut snapshot = snapshot_for(user_id, EntitlementTier::Basic, Some(test_token_id()));
        record_app_entitlement_snapshot(&snapshot, test_now())?;

        snapshot.source = AppEntitlementSnapshotSource::PaymentsRefresh;
        snapshot.entitlement_tier = EntitlementTier::Premium;
        record_app_entitlement_snapshot(
            &snapshot,
            "2026-05-08T13:00:00Z"
                .parse()
                .unwrap_or_else(|_| unreachable!()),
        )?;

        let snapshots = load_app_entitlement_snapshots_for_user(user_id)?;
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].source, "payments_refresh");
        assert_eq!(snapshots[0].entitlement_tier, EntitlementTier::Premium);
        assert_eq!(
            snapshots[0].recorded_at,
            "2026-05-08T13:00:00Z".parse::<DateTime<Utc>>()?
        );
        Ok(())
    }

    #[test]
    fn null_token_id_allows_multiple_snapshots() -> Result<(), Box<dyn Error>> {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        let snapshot = snapshot_for(user_id, EntitlementTier::Basic, None);

        record_app_entitlement_snapshot(&snapshot, test_now())?;
        record_app_entitlement_snapshot(&snapshot, test_now())?;

        let snapshots = load_app_entitlement_snapshots_for_user(user_id)?;
        assert_eq!(snapshots.len(), 2);
        assert!(snapshots.iter().all(|snapshot| snapshot.token_id.is_none()));
        Ok(())
    }

    #[test]
    fn invalid_source_is_rejected_by_database_check() -> Result<(), Box<dyn Error>> {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        let result: Result<(), DbError> = with_app_db_mut(|conn| {
            conn.execute(
                "INSERT INTO app_entitlement_snapshots \
                 (snapshot_id, user_id, recorded_at, source, entitlement_holder_id, entitlement_tier) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    ulid::Ulid::new().to_string(),
                    user_id.to_string(),
                    format_timestamp(test_now()),
                    "browser_submitted",
                    test_holder_id().to_storage_value(),
                    "basic",
                ],
            )
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to insert invalid snapshot", err)
            })?;
            Ok(())
        });

        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn snapshot_sources_match_storage_values() {
        assert_eq!(
            AppEntitlementSnapshotSource::PaymentPoll.as_str(),
            "payment_poll"
        );
        assert_eq!(
            AppEntitlementSnapshotSource::PaymentReconcile.as_str(),
            "payment_reconcile"
        );
        assert_eq!(
            AppEntitlementSnapshotSource::LoginRefresh.as_str(),
            "login_refresh"
        );
        assert_eq!(
            AppEntitlementSnapshotSource::PaymentsRefresh.as_str(),
            "payments_refresh"
        );
        assert_eq!(AppEntitlementSnapshotSource::Refresh.as_str(), "refresh");
    }

    #[test]
    fn snapshot_schema_excludes_sensitive_payment_and_user_data() -> Result<(), Box<dyn Error>> {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        let snapshot = snapshot_for(user_id, EntitlementTier::Basic, Some(test_token_id()));
        record_app_entitlement_snapshot(&snapshot, test_now())?;

        let columns = with_app_db(|conn| {
            let mut stmt = conn
                .prepare("PRAGMA table_info(app_entitlement_snapshots)")
                .map_err(|err| DbError::from_rusqlite_error("Failed to inspect schema", err))?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(1))
                .map_err(|err| DbError::from_rusqlite_error("Failed to query schema", err))?;
            let mut columns = Vec::new();
            for row in rows {
                columns.push(row.map_err(|err| {
                    DbError::from_rusqlite_error("Failed to collect schema", err)
                })?);
            }
            Ok::<_, DbError>(columns)
        })?;

        for forbidden in [
            "active_token",
            "compact_token",
            "management_secret",
            "order_secret",
            "username",
            "wallet_id",
            "account_id",
            "address",
            "balance",
            "transaction",
        ] {
            assert!(!columns.iter().any(|column| column == forbidden));
        }
        let snapshots = load_app_entitlement_snapshots_for_user(user_id)?;
        assert_ne!(
            snapshots[0].capabilities_json.as_deref(),
            Some("claims.signature")
        );
        Ok(())
    }

    fn new_snapshot(
        user_id: UserId,
        entitlement_tier: EntitlementTier,
        subscription_valid_until: Option<DateTime<Utc>>,
    ) -> NewAppEntitlementSnapshot {
        NewAppEntitlementSnapshot {
            user_id,
            source: AppEntitlementSnapshotSource::PaymentPoll,
            entitlement_holder_id: test_holder_id(),
            subscription_subject_id: Some(test_subject_id()),
            token_id: None,
            entitlement_tier,
            subscription_valid_until,
            token_expires_at: None,
            token_issued_at: None,
            capability_set_id: None,
            capabilities_json: None,
        }
    }

    #[test]
    fn active_paid_entitlement_detection() {
        let _guard = db::acquire_test_runtime().unwrap();
        let now = chrono::Utc::now();
        let user_id = db::unique_user_id();

        // 1. No snapshots → not paid.
        assert!(!user_has_active_paid_entitlement(user_id, now).unwrap());

        // 2. Free tier with a future valid_until → not paid.
        record_app_entitlement_snapshot(
            &new_snapshot(
                user_id,
                EntitlementTier::Free,
                Some(now + chrono::Duration::days(30)),
            ),
            now,
        )
        .unwrap();
        assert!(!user_has_active_paid_entitlement(user_id, now).unwrap());

        // 3. Basic tier valid in the future → paid.
        record_app_entitlement_snapshot(
            &new_snapshot(
                user_id,
                EntitlementTier::Basic,
                Some(now + chrono::Duration::days(30)),
            ),
            now,
        )
        .unwrap();
        assert!(user_has_active_paid_entitlement(user_id, now).unwrap());
    }

    #[test]
    fn lapsed_paid_entitlement_is_not_active() {
        let _guard = db::acquire_test_runtime().unwrap();
        let now = chrono::Utc::now();
        let user_id = db::unique_user_id();
        record_app_entitlement_snapshot(
            &new_snapshot(
                user_id,
                EntitlementTier::Premium,
                Some(now - chrono::Duration::days(1)),
            ),
            now,
        )
        .unwrap();
        assert!(!user_has_active_paid_entitlement(user_id, now).unwrap());
    }
}
