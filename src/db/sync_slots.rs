use super::error::DbError;
use super::user_db::with_user_db;
use crate::models::UserId;
use crate::payments::types::EntitlementTier;
use crate::wallets::{DigitalAssetAccountId, DigitalAssetAddressId};
use chrono::{DateTime, Utc};
use rusqlite::OptionalExtension;
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
