use super::super::error::DbError;
use super::types::ChainTipStateRow;
use crate::models::parse_datetime;
use crate::transactions::ChainTipHeight;
use crate::wallets::{Network, SyncedAssetId};
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use ulid::Ulid;

pub(crate) fn upsert_chain_tip_state(
    asset_id: SyncedAssetId,
    network: Network,
    chain_tip_height: ChainTipHeight,
    updated_at: DateTime<Utc>,
) -> Result<(), DbError> {
    super::super::with_db_mut(|conn| {
        conn.execute(
            "INSERT INTO chain_state (id, asset_id, network, chain_height, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(asset_id, network) DO UPDATE SET
               chain_height = excluded.chain_height,
               updated_at = excluded.updated_at",
            params![
                Ulid::new().to_string(),
                asset_id.as_str(),
                network.as_str(),
                chain_tip_height.value(),
                updated_at.to_rfc3339(),
            ],
        )
        .map_err(|err| DbError::new(format!("Failed to upsert chain tip state: {err}")))?;
        Ok(())
    })
}

pub(crate) fn load_chain_tip_state(
    asset_id: SyncedAssetId,
    network: Network,
) -> Result<Option<ChainTipStateRow>, DbError> {
    super::super::with_db(|conn| {
        conn.query_row(
            "SELECT chain_height, updated_at
             FROM chain_state
             WHERE asset_id = ?1 AND network = ?2",
            params![asset_id.as_str(), network.as_str()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|err| DbError::new(format!("Failed to load chain tip state: {err}")))?
        .map(|(height_raw, updated_at_raw)| {
            let chain_tip_height = ChainTipHeight::try_new(height_raw).map_err(|err| {
                DbError::new(format!("Invalid chain_height in chain_state: {err}"))
            })?;
            let updated_at = parse_datetime(&updated_at_raw)
                .map_err(|err| DbError::new(format!("Invalid updated_at in chain_state: {err}")))?;
            Ok(ChainTipStateRow {
                chain_tip_height,
                updated_at,
            })
        })
        .transpose()
    })
}
