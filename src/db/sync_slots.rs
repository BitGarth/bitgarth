use super::error::DbError;
use super::user_db::{with_user_db, with_user_db_mut};
use crate::asset_capabilities::{sync_provider, synced_asset_instance, synced_asset_instance_id};
use crate::models::UserId;
use crate::payments::types::EntitlementTier;
use crate::wallets::{DigitalAssetAccountId, DigitalAssetAddressId, SyncedAssetId};
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{OptionalExtension, params};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AccountSyncSlotRecord {
    pub(crate) account_id: DigitalAssetAccountId,
    pub(crate) selected_at: DateTime<Utc>,
    pub(crate) selected_under_tier: EntitlementTier,
}

pub(crate) fn load_account_sync_slots(
    user_id: UserId,
) -> Result<Vec<AccountSyncSlotRecord>, DbError> {
    with_user_db(user_id, query_account_sync_slots)
}

pub(crate) fn load_account_sync_slot_map(
    user_id: UserId,
) -> Result<HashMap<DigitalAssetAccountId, AccountSyncSlotRecord>, DbError> {
    Ok(load_account_sync_slots(user_id)?
        .into_iter()
        .map(|record| (record.account_id, record))
        .collect())
}

pub(crate) fn select_account_sync_slot(
    user_id: UserId,
    account_id: DigitalAssetAccountId,
    selected_under_tier: &EntitlementTier,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        conn.execute(
            "INSERT INTO account_sync_slots (account_id, selected_at, selected_under_tier)
             SELECT id, ?2, ?3 FROM digital_asset_accounts WHERE id = ?1
             ON CONFLICT(account_id) DO NOTHING",
            params![
                account_id.to_string(),
                format_timestamp(now),
                selected_under_tier.as_str(),
            ],
        )
        .map_err(|err| DbError::from_rusqlite_error("Failed to select sync slot", err))?;
        Ok(())
    })
}

pub(crate) fn account_supports_free_balance_sync(
    user_id: UserId,
    account_id: DigitalAssetAccountId,
) -> Result<bool, DbError> {
    with_user_db(user_id, |conn| {
        let asset_id_raw = conn
            .query_row(
                "SELECT asset_id
                 FROM digital_asset_accounts
                 WHERE id = ?1
                 LIMIT 1",
                [account_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to load sync slot account asset", err)
            })?
            .ok_or_else(|| DbError::new("Account not found"))?;
        let asset_id = SyncedAssetId::from_str(&asset_id_raw)
            .ok_or_else(|| DbError::new(format!("Invalid account asset_id: {asset_id_raw}")))?;
        let provider = sync_provider(
            synced_asset_instance(synced_asset_instance_id(asset_id)).default_sync_provider,
        );
        Ok(provider.capabilities.supports_balance_only_sync)
    })
}

pub(crate) fn upsert_imported_account_sync_slot(
    tx: &rusqlite::Transaction<'_>,
    account_id: DigitalAssetAccountId,
    selected_at: DateTime<Utc>,
    selected_under_tier: &EntitlementTier,
) -> Result<(), DbError> {
    tx.execute(
        "INSERT INTO account_sync_slots (account_id, selected_at, selected_under_tier)
         SELECT id, ?2, ?3 FROM digital_asset_accounts WHERE id = ?1
         ON CONFLICT(account_id) DO NOTHING",
        params![
            account_id.to_string(),
            format_timestamp(selected_at),
            selected_under_tier.as_str(),
        ],
    )
    .map_err(|err| DbError::from_rusqlite_error("Failed to import account sync slot", err))?;
    Ok(())
}

pub(crate) fn resolve_address_sync_slot_account(
    user_id: UserId,
    address_id: DigitalAssetAddressId,
) -> Result<Option<DigitalAssetAccountId>, DbError> {
    with_user_db(user_id, |conn| {
        let raw = conn
            .query_row(
                "SELECT account_id FROM digital_asset_addresses WHERE id = ?1 LIMIT 1",
                [address_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to resolve address sync slot account", err)
            })?;

        raw.map(|value| {
            DigitalAssetAccountId::from_str(&value)
                .map_err(|err| DbError::new(format!("Invalid address account_id in DB: {err}")))
        })
        .transpose()
    })
}

pub(crate) fn active_sync_slot_account_ids(
    records: &[AccountSyncSlotRecord],
    limit: u16,
) -> HashSet<DigitalAssetAccountId> {
    let mut ordered = records.to_vec();
    ordered.sort_by(|left, right| {
        left.selected_at.cmp(&right.selected_at).then_with(|| {
            left.account_id
                .to_string()
                .cmp(&right.account_id.to_string())
        })
    });

    ordered
        .into_iter()
        .take(usize::from(limit))
        .map(|record| record.account_id)
        .collect()
}

fn query_account_sync_slots(
    conn: &rusqlite::Connection,
) -> Result<Vec<AccountSyncSlotRecord>, DbError> {
    let mut stmt = conn
        .prepare(
            "SELECT account_id, selected_at, selected_under_tier
             FROM account_sync_slots
             ORDER BY selected_at ASC, account_id ASC",
        )
        .map_err(|err| DbError::from_rusqlite_error("Failed to prepare sync slot query", err))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|err| DbError::from_rusqlite_error("Failed to load sync slots", err))?;

    let mut result = Vec::new();
    for row in rows {
        let (account_id_raw, selected_at_raw, selected_under_tier_raw) =
            row.map_err(|err| DbError::from_rusqlite_error("Failed to map sync slot row", err))?;
        result.push(AccountSyncSlotRecord {
            account_id: DigitalAssetAccountId::from_str(&account_id_raw)
                .map_err(|err| DbError::new(format!("Invalid sync slot account_id: {err}")))?,
            selected_at: selected_at_raw
                .parse::<DateTime<Utc>>()
                .map_err(|err| DbError::new(format!("Invalid sync slot selected_at: {err}")))?,
            selected_under_tier: EntitlementTier::from_str(&selected_under_tier_raw).map_err(
                |err| DbError::new(format!("Invalid sync slot selected_under_tier: {err}")),
            )?,
        });
    }

    Ok(result)
}

fn format_timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    fn at(value: &str) -> DateTime<Utc> {
        value.parse().expect("test timestamp should parse")
    }

    #[test]
    fn active_sync_slot_account_ids_uses_sticky_selection_order() {
        let late = DigitalAssetAccountId::new();
        let early = DigitalAssetAccountId::new();
        let over_limit = DigitalAssetAccountId::new();
        let records = vec![
            AccountSyncSlotRecord {
                account_id: late,
                selected_at: at("2026-04-02T00:00:00Z"),
                selected_under_tier: EntitlementTier::Free,
            },
            AccountSyncSlotRecord {
                account_id: early,
                selected_at: at("2026-04-01T00:00:00Z"),
                selected_under_tier: EntitlementTier::Free,
            },
            AccountSyncSlotRecord {
                account_id: over_limit,
                selected_at: at("2026-04-03T00:00:00Z"),
                selected_under_tier: EntitlementTier::Free,
            },
        ];

        let active = active_sync_slot_account_ids(&records, 2);

        assert!(active.contains(&early));
        assert!(active.contains(&late));
        assert!(!active.contains(&over_limit));
    }
}
