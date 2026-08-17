use super::error::DbError;
use super::user_db::with_user_db;
use crate::models::UserId;
use crate::wallets::{LabelKey, WalletAccountId, WalletId};
use rusqlite::{OptionalExtension, params};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WalletAccountLabelKeyRow {
    pub account_id: WalletAccountId,
    pub label_key: LabelKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WalletAccountRecordKind {
    Native,
    Manual,
}

pub(super) fn query_wallet_account_label_keys_in_tx(
    tx: &rusqlite::Transaction<'_>,
    wallet_id: WalletId,
) -> Result<Vec<WalletAccountLabelKeyRow>, DbError> {
    let mut stmt = tx
        .prepare(
            "SELECT id, label_key, 'native', NULL
             FROM digital_asset_accounts
             WHERE wallet_id = ?1
             UNION ALL
             SELECT id, label_key, 'manual', asset_id || ':' || network_id
             FROM manual_asset_accounts
             WHERE wallet_id = ?1",
        )
        .map_err(|e| {
            DbError::new(format!(
                "Failed to prepare wallet-account label_key query: {e}"
            ))
        })?;
    let keys = stmt
        .query_map(params![wallet_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(|e| DbError::new(format!("Failed to query wallet-account label_keys: {e}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| DbError::new(format!("Failed to read wallet-account label_key row: {e}")))?;
    keys.into_iter()
        .map(
            |(account_id_raw, label_key_raw, kind_raw, instance_identity_raw)| {
                let account_id = WalletAccountId::from_str(&account_id_raw)
                    .map_err(|e| DbError::new(format!("Invalid wallet account id in DB: {e}")))?;

                match kind_raw.as_str() {
                    "native" => {}
                    "manual" => {
                        let instance_identity = instance_identity_raw.ok_or_else(|| {
                            DbError::new(
                                "Manual asset account row missing instance identity in label-key query",
                            )
                        })?;
                        if instance_identity.is_empty() {
                            return Err(DbError::new(
                                "Manual asset account row has empty instance identity",
                            ));
                        }
                    }
                    other => {
                        return Err(DbError::new(format!(
                            "Invalid wallet-account kind in label-key query: {other}"
                        )));
                    }
                }

                Ok(WalletAccountLabelKeyRow {
                    account_id,
                    label_key: LabelKey::new(label_key_raw),
                })
            },
        )
        .collect()
}

pub(crate) fn resolve_wallet_account_record_kind(
    user_id: UserId,
    account_id: WalletAccountId,
) -> Result<Option<WalletAccountRecordKind>, DbError> {
    with_user_db(user_id, |conn| {
        let row = conn
            .query_row(
                "SELECT account_kind
                 FROM (
                     SELECT 'native' AS account_kind
                     FROM digital_asset_accounts
                     WHERE id = ?1
                     UNION ALL
                     SELECT 'manual' AS account_kind
                     FROM manual_asset_accounts
                     WHERE id = ?1
                 )
                 LIMIT 1",
                params![account_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to resolve wallet account record kind: {err}"
                ))
            })?;

        row.map(|kind_raw: String| match kind_raw.as_str() {
            "native" => Ok(WalletAccountRecordKind::Native),
            "manual" => Ok(WalletAccountRecordKind::Manual),
            other => Err(DbError::new(format!(
                "Invalid wallet account record kind in DB: {other}"
            ))),
        })
        .transpose()
    })
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;
    use crate::db::test_fixtures::{setup_test_user, unique_user_id, wallet_label};
    use crate::db::user_db::with_user_db_mut;
    use crate::wallets::{IdentitySource, WalletAccountId};
    use chrono::Utc;
    use rusqlite::params;

    #[test]
    fn wallet_account_queries_include_supported_accounts() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let wallet_id = WalletId::new();
        let native_account_id = crate::wallets::DigitalAssetAccountId::new();
        let manual_account_id = WalletAccountId::new();
        let now = Utc::now().to_rfc3339();
        let wallet = wallet_label("Shared Labels");

        with_user_db_mut(user_id, |conn| -> Result<(), DbError> {
            conn.execute(
                "INSERT INTO wallets
                 (id, label, label_key, master_fingerprint, identity_source, verified_at, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    wallet_id.to_string(),
                    wallet.as_str(),
                    wallet.key().as_str(),
                    Option::<String>::None,
                    IdentitySource::UserProvided.as_str(),
                    Option::<String>::None,
                    now,
                    now,
                ],
            )
            .map_err(|e| DbError::new(format!("Failed to insert wallet fixture: {e}")))?;

            conn.execute(
                "INSERT INTO digital_asset_accounts
                 (id, wallet_id, label, label_key, asset_id, network, account_kind, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    native_account_id.to_string(),
                    wallet_id.to_string(),
                    "Bitcoin Account 1",
                    "bitcoin account 1",
                    "bitcoin",
                    "mainnet",
                    "single_address",
                    now,
                    now,
                ],
            )
            .map_err(|e| DbError::new(format!("Failed to insert native account fixture: {e}")))?;

            conn.execute(
                "INSERT INTO manual_asset_accounts
                 (id, wallet_id, label, label_key, asset_id, network_id, decimal_precision,
                  unit_code, symbol, asset_name, network_name, coingecko_id, asset_source,
                  precision_source, coingecko_platform_id, provider_platform_asset_ref,
                  created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                         'bitgarth_catalog', 'bitgarth_catalog', NULL, NULL, ?13, ?14)",
                params![
                    manual_account_id.to_string(),
                    wallet_id.to_string(),
                    "Cardano Account 1",
                    "cardano account 1",
                    "cardano",
                    "cardano-mainnet",
                    6_i64,
                    "ADA",
                    Option::<String>::None,
                    "Cardano",
                    "Cardano",
                    "cardano",
                    now,
                    now,
                ],
            )
            .map_err(|e| DbError::new(format!("Failed to insert manual account fixture: {e}")))?;

            let tx = conn
                .transaction()
                .map_err(|e| DbError::new(format!("Failed to open transaction: {e}")))?;
            let keys = query_wallet_account_label_keys_in_tx(&tx, wallet_id)?;
            let label_keys = keys.into_iter().map(|row| row.label_key).collect::<Vec<_>>();
            assert!(label_keys.contains(&LabelKey::new("bitcoin account 1".to_string())));
            assert!(label_keys.contains(&LabelKey::new("cardano account 1".to_string())));
            tx.rollback()
                .map_err(|e| DbError::new(format!("Failed to rollback transaction: {e}")))?;

            Ok(())
        })
        .expect("wallet-account label keys should load");

        assert_eq!(
            resolve_wallet_account_record_kind(user_id, manual_account_id)
                .expect("manual lookup should succeed"),
            Some(WalletAccountRecordKind::Manual)
        );
    }
}
