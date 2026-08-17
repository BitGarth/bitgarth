use crate::db::error::DbError;
use crate::wallets::{DigitalAssetAddressId, Network, SyncedAssetId};
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};

use super::ids::SourceConnectionId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IntegrationKind {
    Mempool,
    Etherscan,
}

impl IntegrationKind {
    pub(crate) fn as_db_value(&self) -> &'static str {
        match self {
            Self::Mempool => "mempool",
            Self::Etherscan => "etherscan",
        }
    }
}

fn integration_for_asset(asset_id: SyncedAssetId) -> Result<IntegrationKind, DbError> {
    match asset_id {
        SyncedAssetId::Bitcoin => Ok(IntegrationKind::Mempool),
        SyncedAssetId::Ethereum => Ok(IntegrationKind::Etherscan),
    }
}

pub(crate) fn ensure_source_connection_for_address_tx(
    tx: &rusqlite::Transaction<'_>,
    address_id: DigitalAssetAddressId,
    asset_id: SyncedAssetId,
    network: Network,
    normalized_source_key: &str,
    now: DateTime<Utc>,
) -> Result<SourceConnectionId, DbError> {
    let integration = integration_for_asset(asset_id)?;
    let timestamp = now.to_rfc3339();
    let existing_id = tx
        .query_row(
            "SELECT id
             FROM source_connections
             WHERE integration = ?1
               AND network = ?2
               AND normalized_source_key = ?3
             LIMIT 1",
            params![
                integration.as_db_value(),
                network.as_str(),
                normalized_source_key,
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to query existing source connection", err)
        })?;

    if let Some(existing_id) = existing_id {
        let source_connection_id = SourceConnectionId::parse(&existing_id)?;
        tx.execute(
            "UPDATE source_connections
             SET status = ?1,
                 current_digital_asset_address_id = ?2,
                 updated_at = ?3,
                 activated_at = ?4,
                 deactivated_at = NULL
             WHERE id = ?5",
            params![
                SourceConnectionStatus::Active.as_db_value(),
                address_id.to_string(),
                timestamp,
                timestamp,
                source_connection_id.to_string(),
            ],
        )
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to reactivate source connection", err)
        })?;
        return Ok(source_connection_id);
    }

    let source_connection_id = SourceConnectionId::new();
    tx.execute(
        "INSERT INTO source_connections
         (id, integration, network, source_kind, normalized_source_key, status, current_digital_asset_address_id, created_at, updated_at, activated_at, deactivated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            source_connection_id.to_string(),
            integration.as_db_value(),
            network.as_str(),
            SourceConnectionKind::WalletAddressApiWatch.as_db_value(),
            normalized_source_key,
            SourceConnectionStatus::Active.as_db_value(),
            address_id.to_string(),
            timestamp,
            timestamp,
            timestamp,
            Option::<String>::None,
        ],
    )
    .map_err(|err| DbError::from_rusqlite_error("Failed to insert source connection", err))?;
    Ok(source_connection_id)
}

pub(crate) fn deactivate_source_connection_for_address_tx(
    tx: &rusqlite::Transaction<'_>,
    address_id: DigitalAssetAddressId,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    let timestamp = now.to_rfc3339();
    tx.execute(
        "UPDATE source_connections
         SET status = ?1,
             current_digital_asset_address_id = NULL,
             updated_at = ?2,
             deactivated_at = ?3
         WHERE current_digital_asset_address_id = ?4",
        params![
            SourceConnectionStatus::Inactive.as_db_value(),
            timestamp,
            timestamp,
            address_id.to_string(),
        ],
    )
    .map_err(|err| DbError::from_rusqlite_error("Failed to deactivate source connection", err))?;
    Ok(())
}

pub(super) fn load_active_source_connection_id(
    conn: &rusqlite::Connection,
    integration: IntegrationKind,
    network: Network,
    address_id: DigitalAssetAddressId,
) -> Result<SourceConnectionId, DbError> {
    let source_connection_id = conn
        .query_row(
            "SELECT id
             FROM source_connections
             WHERE integration = ?1
               AND network = ?2
               AND current_digital_asset_address_id = ?3
               AND status = ?4
             LIMIT 1",
            params![
                integration.as_db_value(),
                network.as_str(),
                address_id.to_string(),
                SourceConnectionStatus::Active.as_db_value(),
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to load source connection for address", err)
        })?
        .ok_or_else(|| {
            DbError::new(format!(
                "Missing active source connection for {} {} {}",
                integration.as_db_value(),
                network.as_str(),
                address_id
            ))
        })?;
    SourceConnectionId::parse(&source_connection_id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceConnectionKind {
    WalletAddressApiWatch,
}

impl SourceConnectionKind {
    fn as_db_value(&self) -> &'static str {
        match self {
            Self::WalletAddressApiWatch => "wallet_address_api_watch",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceConnectionStatus {
    Active,
    Inactive,
}

impl SourceConnectionStatus {
    fn as_db_value(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Inactive => "inactive",
        }
    }
}
