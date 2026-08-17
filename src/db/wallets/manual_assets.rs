use super::errors::db_error_from_sqlite;
use crate::db::account_limits::ensure_supported_account_hard_cap_before_insert_in_tx;
use crate::db::error::DbError;
use crate::db::user_db::with_user_db_mut;
use crate::wallets::{
    IdentitySource, ValidatedAddManualAssetAccountAsset, ValidatedAddManualAssetAccountRequest,
    ValidatedCoinGeckoManualAssetSnapshot, ValidatedManualAssetUnitCode, WalletAccountId, WalletId,
    generate_unique_custom_account_label,
};
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};

struct ManualAssetAccountSnapshot {
    asset_id: String,
    network_id: String,
    decimal_precision: crate::wallets::ManualAssetDisplayScale,
    unit_code: ValidatedManualAssetUnitCode,
    symbol: Option<String>,
    asset_name: String,
    network_name: String,
    coingecko_id: String,
    asset_source: &'static str,
    precision_source: &'static str,
    coingecko_platform_id: Option<String>,
    provider_platform_asset_ref: Option<String>,
}

#[derive(Debug)]
pub(crate) struct AddManualAssetAccountDbResult {
    pub(crate) wallet_id: WalletId,
    pub(crate) account_id: WalletAccountId,
}

pub(crate) fn add_manual_asset_account(
    user_id: crate::models::UserId,
    request: ValidatedAddManualAssetAccountRequest,
    now: DateTime<Utc>,
) -> Result<AddManualAssetAccountDbResult, DbError> {
    with_user_db_mut(user_id, |conn| {
        let tx = conn.transaction().map_err(|e| {
            DbError::new(format!(
                "Failed to start manual asset account creation transaction: {e}"
            ))
        })?;
        let timestamp = now.to_rfc3339();

        let wallet_id = match request.wallet_id {
            Some(wallet_id) => {
                let wallet_exists = tx
                    .query_row(
                        "SELECT 1 FROM wallets WHERE id = ?1 LIMIT 1",
                        params![wallet_id.to_string()],
                        |_| Ok(()),
                    )
                    .optional()
                    .map_err(|e| {
                        DbError::new(format!("Failed to verify manual asset wallet: {e}"))
                    })?;
                if wallet_exists.is_none() {
                    return Err(DbError::new("Wallet not found"));
                }
                wallet_id
            }
            None => {
                let new_wallet_id = WalletId::new();
                let effective_label = request.wallet_label.clone().ok_or_else(|| {
                    DbError::new("wallet_label is required when creating a wallet")
                })?;
                let wallet_label_key = effective_label.key();
                tx.execute(
                    "INSERT INTO wallets \
                     (id, label, label_key, master_fingerprint, identity_source, verified_at, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        new_wallet_id.to_string(),
                        effective_label.as_str(),
                        wallet_label_key.as_str(),
                        Option::<String>::None,
                        IdentitySource::UserProvided.as_str(),
                        Option::<String>::None,
                        &timestamp,
                        &timestamp,
                    ],
                )
                .map_err(|e| db_error_from_sqlite("Failed to insert wallet", e))?;
                new_wallet_id
            }
        };

        let snapshot = manual_asset_account_snapshot(request.asset)?;

        let label = super::labels::resolve_new_account_label(
            &tx,
            wallet_id,
            request.account_label.as_ref(),
            |keys| {
                generate_unique_custom_account_label(&snapshot.unit_code, keys).map_err(|e| {
                    DbError::new(format!(
                        "Failed to generate manual asset account label for {}: {e}",
                        snapshot.unit_code.as_str()
                    ))
                })
            },
        )?;
        let label_key = label.key();
        let account_id = WalletAccountId::new();
        ensure_supported_account_hard_cap_before_insert_in_tx(&tx, 1)?;
        tx.execute(
            "INSERT INTO manual_asset_accounts
             (id, wallet_id, label, label_key, asset_id, network_id, decimal_precision,
              unit_code, symbol, asset_name, network_name, coingecko_id, asset_source,
              precision_source, coingecko_platform_id, provider_platform_asset_ref,
              created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![
                account_id.to_string(),
                wallet_id.to_string(),
                label.as_str(),
                label_key.as_str(),
                snapshot.asset_id,
                snapshot.network_id,
                i64::from(snapshot.decimal_precision.as_u8()),
                snapshot.unit_code.as_str(),
                snapshot.symbol,
                snapshot.asset_name,
                snapshot.network_name,
                snapshot.coingecko_id,
                snapshot.asset_source,
                snapshot.precision_source,
                snapshot.coingecko_platform_id,
                snapshot.provider_platform_asset_ref,
                &timestamp,
                &timestamp,
            ],
        )
        .map_err(|e| db_error_from_sqlite("Failed to insert manual asset account", e))?;

        tx.commit().map_err(|e| {
            DbError::new(format!(
                "Failed to commit manual asset account creation: {e}"
            ))
        })?;

        Ok(AddManualAssetAccountDbResult {
            wallet_id,
            account_id,
        })
    })
}

fn manual_asset_account_snapshot(
    asset: ValidatedAddManualAssetAccountAsset,
) -> Result<ManualAssetAccountSnapshot, DbError> {
    match asset {
        ValidatedAddManualAssetAccountAsset::BitGarthCatalog { candidate_id } => {
            let candidate = crate::asset_capabilities::manual_catalog_candidate(&candidate_id)
                .map_err(|e| DbError::new(format!("Failed to load manual asset catalog: {e}")))?
                .ok_or_else(|| DbError::new("Manual asset instance not found in catalog"))?;
            let unit_code = ValidatedManualAssetUnitCode::parse(&candidate.unit_code)
                .map_err(|e| DbError::new(format!("Invalid manual asset unit code: {e}")))?;
            Ok(ManualAssetAccountSnapshot {
                asset_id: candidate.asset_id,
                network_id: candidate.network_id,
                decimal_precision: crate::wallets::ManualAssetDisplayScale::from_u8(
                    candidate.decimal_precision,
                ),
                unit_code,
                symbol: candidate.symbol,
                asset_name: candidate.asset_name,
                network_name: candidate.network_name,
                coingecko_id: candidate.coingecko_id,
                asset_source: "bitgarth_catalog",
                precision_source: "bitgarth_catalog",
                coingecko_platform_id: None,
                provider_platform_asset_ref: None,
            })
        }
        ValidatedAddManualAssetAccountAsset::CoinGeckoDiscovery { snapshot } => Ok(
            coingecko_snapshot_to_manual_asset_account_snapshot(snapshot),
        ),
    }
}

fn coingecko_snapshot_to_manual_asset_account_snapshot(
    snapshot: ValidatedCoinGeckoManualAssetSnapshot,
) -> ManualAssetAccountSnapshot {
    ManualAssetAccountSnapshot {
        asset_id: snapshot.asset_id.as_str().to_string(),
        network_id: snapshot.network_id.as_str().to_string(),
        decimal_precision: snapshot.decimal_precision,
        unit_code: snapshot.unit_code,
        symbol: snapshot.symbol,
        asset_name: snapshot.asset_name,
        network_name: snapshot.network_name,
        coingecko_id: snapshot.coingecko_id.as_str().to_string(),
        asset_source: "coingecko_discovery",
        precision_source: snapshot.precision_source.as_db_str(),
        coingecko_platform_id: snapshot.coingecko_platform_id,
        provider_platform_asset_ref: snapshot.provider_platform_asset_ref,
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;
    use crate::db::test_fixtures::{setup_test_user, unique_user_id};
    use crate::db::user_db::with_user_db;
    use crate::wallets::{
        Label, ManualAssetDisplayScale, ValidatedCoinGeckoManualAssetPrecisionSource,
        WALLET_LABEL_MAX_LENGTH,
    };
    use chrono::Utc;

    fn cardano_request(
        wallet_id: Option<WalletId>,
        wallet_label: Option<Label>,
    ) -> ValidatedAddManualAssetAccountRequest {
        ValidatedAddManualAssetAccountRequest {
            wallet_id,
            wallet_label,
            account_label: None,
            asset: ValidatedAddManualAssetAccountAsset::BitGarthCatalog {
                candidate_id: crate::asset_capabilities::ManualAssetCatalogCandidateId::Unsynced(
                    crate::asset_capabilities::unsynced::UnsyncedAssetInstanceId {
                        asset_id: crate::asset_capabilities::unsynced::UnsyncedAssetId::parse(
                            "cardano",
                        )
                        .expect("asset id"),
                        network_id: crate::asset_capabilities::unsynced::UnsyncedNetworkId::parse(
                            "cardano-mainnet",
                        )
                        .expect("network id"),
                    },
                ),
            },
        }
    }

    #[test]
    fn add_manual_asset_account_inserts_manual_asset_table_row() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let now = Utc::now();

        let response = add_manual_asset_account(
            user_id,
            cardano_request(
                None,
                Some(
                    Label::parse_with_limit("Manual Wallet", WALLET_LABEL_MAX_LENGTH)
                        .expect("wallet label should parse"),
                ),
            ),
            now,
        )
        .expect("add manual asset");

        with_user_db(user_id, |conn| -> Result<(), DbError> {
            let row = conn
                .query_row(
                    "SELECT asset_id, network_id, unit_code, decimal_precision,
                            asset_name, network_name, coingecko_id
                     FROM manual_asset_accounts WHERE id = ?1",
                    params![response.account_id.to_string()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, String>(6)?,
                        ))
                    },
                )
                .map_err(|e| DbError::new(format!("manual row read failed: {e}")))?;
            assert_eq!(
                row,
                (
                    "cardano".to_string(),
                    "cardano-mainnet".to_string(),
                    "ADA".to_string(),
                    6_i64,
                    "Cardano".to_string(),
                    "Cardano".to_string(),
                    "cardano".to_string(),
                )
            );
            Ok(())
        })
        .expect("manual row should be readable");
    }

    #[test]
    fn add_manual_asset_account_inserts_synced_catalog_manual_row() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let now = Utc::now();

        let response = add_manual_asset_account(
            user_id,
            ValidatedAddManualAssetAccountRequest {
                wallet_id: None,
                wallet_label: Some(
                    Label::parse_with_limit("Manual BTC", WALLET_LABEL_MAX_LENGTH)
                        .expect("wallet label should parse"),
                ),
                account_label: None,
                asset: ValidatedAddManualAssetAccountAsset::BitGarthCatalog {
                    candidate_id: crate::asset_capabilities::ManualAssetCatalogCandidateId::Synced(
                        crate::asset_capabilities::SyncedAssetInstanceId::BtcBitcoinMainnet,
                    ),
                },
            },
            now,
        )
        .expect("add manual BTC asset");

        with_user_db(user_id, |conn| -> Result<(), DbError> {
            let row = conn
                .query_row(
                    "SELECT asset_id, network_id, unit_code, decimal_precision,
                            asset_name, network_name, coingecko_id, asset_source
                     FROM manual_asset_accounts WHERE id = ?1",
                    params![response.account_id.to_string()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, String>(6)?,
                            row.get::<_, String>(7)?,
                        ))
                    },
                )
                .map_err(|e| DbError::new(format!("manual BTC row read failed: {e}")))?;

            assert_eq!(row.0, "bitcoin");
            assert_eq!(row.1, "bitcoin-mainnet");
            assert_eq!(row.2, "BTC");
            assert_eq!(row.3, 8);
            assert_eq!(row.4, "Bitcoin");
            assert_eq!(row.5, "Bitcoin");
            assert_eq!(row.6, "bitcoin");
            assert_eq!(row.7, "bitgarth_catalog");
            Ok(())
        })
        .expect("manual BTC row should load");
    }

    #[test]
    fn add_manual_asset_account_inserts_coingecko_discovery_metadata() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let now = Utc::now();

        let response = add_manual_asset_account(
            user_id,
            ValidatedAddManualAssetAccountRequest {
                wallet_id: None,
                wallet_label: Some(
                    Label::parse_with_limit("Manual Wallet", WALLET_LABEL_MAX_LENGTH)
                        .expect("wallet label should parse"),
                ),
                account_label: None,
                asset: ValidatedAddManualAssetAccountAsset::CoinGeckoDiscovery {
                    snapshot: ValidatedCoinGeckoManualAssetSnapshot {
                        asset_id: crate::asset_capabilities::AssetId::owned(
                            "adappter-token".to_string(),
                        )
                        .expect("asset id should parse"),
                        network_id: crate::asset_capabilities::unsynced::UnsyncedNetworkId::parse(
                            "ethereum-mainnet",
                        )
                        .expect("network id should parse"),
                        decimal_precision: ManualAssetDisplayScale::from_u8(6),
                        unit_code: ValidatedManualAssetUnitCode::parse("ADP")
                            .expect("unit code should parse"),
                        symbol: Some("adp".to_string()),
                        asset_name: "Adappter Token".to_string(),
                        network_name: "Ethereum".to_string(),
                        coingecko_id: crate::asset_capabilities::unsynced::CoingeckoAssetId::parse(
                            "adappter-token",
                        )
                        .expect("coingecko id should parse"),
                        precision_source:
                            ValidatedCoinGeckoManualAssetPrecisionSource::CoingeckoPlatform,
                        coingecko_platform_id: Some("ethereum".to_string()),
                        provider_platform_asset_ref: Some(
                            "0xabc0000000000000000000000000000000000000".to_string(),
                        ),
                    },
                },
            },
            now,
        )
        .expect("add manual asset");

        with_user_db(user_id, |conn| -> Result<(), DbError> {
            let row = conn
                .query_row(
                    "SELECT asset_id, network_id, unit_code, decimal_precision, symbol,
                            asset_name, network_name, coingecko_id, asset_source,
                            precision_source, coingecko_platform_id, provider_platform_asset_ref
                     FROM manual_asset_accounts WHERE id = ?1",
                    params![response.account_id.to_string()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, String>(6)?,
                            row.get::<_, String>(7)?,
                            row.get::<_, String>(8)?,
                            row.get::<_, String>(9)?,
                            row.get::<_, Option<String>>(10)?,
                            row.get::<_, Option<String>>(11)?,
                        ))
                    },
                )
                .map_err(|e| DbError::new(format!("manual row read failed: {e}")))?;
            assert_eq!(
                row,
                (
                    "adappter-token".to_string(),
                    "ethereum-mainnet".to_string(),
                    "ADP".to_string(),
                    6_i64,
                    Some("adp".to_string()),
                    "Adappter Token".to_string(),
                    "Ethereum".to_string(),
                    "adappter-token".to_string(),
                    "coingecko_discovery".to_string(),
                    "coingecko_platform".to_string(),
                    Some("ethereum".to_string()),
                    Some("0xabc0000000000000000000000000000000000000".to_string()),
                )
            );
            Ok(())
        })
        .expect("manual row should be readable");
    }
}
