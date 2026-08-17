#[cfg(any(
    test,
    all(
        feature = "server",
        any(
            all(not(test), not(feature = "desktop")),
            all(test, not(bitgarth_db_unit_only))
        )
    )
))]
use super::amount_storage::parse_optional_split_amount as parse_optional_split_amount_parts;
use super::amount_storage::parse_split_amount as parse_split_amount_parts;
use super::error::DbError;
use super::user_db::with_user_db;
use crate::amounts::UnsignedAmount;
use crate::asset_capabilities::{asset_instance, synced_asset_instance, synced_asset_instance_id};
use crate::asset_views::ManualAssetInstanceIdView;
use crate::models::UserId;
#[cfg(any(
    test,
    all(
        feature = "server",
        any(
            all(not(test), not(feature = "desktop")),
            all(test, not(bitgarth_db_unit_only))
        )
    )
))]
use crate::models::parse_datetime;
#[cfg(any(
    test,
    all(
        feature = "server",
        any(
            all(not(test), not(feature = "desktop")),
            all(test, not(bitgarth_db_unit_only))
        )
    )
))]
use crate::transactions::AccountTransactionDirection;
use crate::wallets::{
    ACCOUNT_LABEL_MAX_LENGTH, Label, ManualAssetBalanceAssertionId, ManualAssetDisplayScale,
    Network, SyncedAssetId, ValidatedManualAssetUnitCode, ValidatedMasterFingerprint,
    WALLET_LABEL_MAX_LENGTH, WalletAccountId, WalletId,
};
use chrono::{DateTime, NaiveDate, Utc};
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExportAccountBoundaryMode {
    Native,
    ManualAsset,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExportCommodity {
    pub(crate) unit_code: String,
    pub(crate) decimal_precision: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExportAccountRow {
    pub(crate) account_id: WalletAccountId,
    pub(crate) wallet_id: WalletId,
    pub(crate) boundary_mode: ExportAccountBoundaryMode,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) commodity: ExportCommodity,
    pub(crate) native_asset_id: Option<SyncedAssetId>,
    pub(crate) native_network: Option<Network>,
    pub(crate) account_label: Label,
    pub(crate) wallet_label: Label,
    pub(crate) wallet_accessor_label: Option<Label>,
    pub(crate) wallet_master_fingerprint: Option<ValidatedMasterFingerprint>,
    pub(crate) primary_account_number: Option<u32>,
    /// For `ManualAsset` boundary rows, this carries the catalog-backed manual
    /// asset id. `None` for `Native` rows.
    pub(crate) manual_asset_instance_id: Option<ManualAssetInstanceIdView>,
    pub(crate) manual_symbol: Option<String>,
    pub(crate) manual_asset_name: Option<String>,
    pub(crate) manual_network_name: Option<String>,
    pub(crate) manual_coingecko_id: Option<String>,
    pub(crate) manual_asset_source: Option<String>,
    pub(crate) manual_precision_source: Option<String>,
    pub(crate) manual_coingecko_platform_id: Option<String>,
    pub(crate) manual_provider_platform_asset_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(any(
    test,
    all(
        feature = "server",
        any(
            all(not(test), not(feature = "desktop")),
            all(test, not(bitgarth_db_unit_only))
        )
    )
))]
pub(crate) struct ExportAccountTransactionLedgerRow {
    pub(crate) account_id: WalletAccountId,
    pub(crate) tx_hash: String,
    pub(crate) direction: AccountTransactionDirection,
    pub(crate) fee: Option<UnsignedAmount>,
    pub(crate) balance_delta: i128,
    pub(crate) occurred_at: DateTime<Utc>,
    pub(crate) block_height: Option<i64>,
    pub(crate) nonce: Option<i64>,
    pub(crate) min_transfer_index: Option<i64>,
    pub(crate) closing_balance: Option<UnsignedAmount>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExportManualAssetBalanceAssertionRow {
    pub(crate) account_id: WalletAccountId,
    pub(crate) assertion_id: ManualAssetBalanceAssertionId,
    pub(crate) asserted_on: NaiveDate,
    pub(crate) asserted_balance: UnsignedAmount,
    pub(crate) note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(any(
    test,
    all(
        feature = "server",
        any(
            all(not(test), not(feature = "desktop")),
            all(test, not(bitgarth_db_unit_only))
        )
    )
))]
pub(crate) struct ExportNativeApiBalanceAssertionRow {
    pub(crate) account_id: WalletAccountId,
    pub(crate) assertion_id: String,
    pub(crate) asserted_on: NaiveDate,
    pub(crate) asserted_balance: UnsignedAmount,
}

#[derive(Debug, Clone)]
#[cfg(any(
    test,
    all(
        feature = "server",
        any(
            all(not(test), not(feature = "desktop")),
            all(test, not(bitgarth_db_unit_only))
        )
    )
))]
struct NativeApiBalanceAssertionAggregate {
    asserted_balance: UnsignedAmount,
    asserted_at: Option<DateTime<Utc>>,
    address_count: u32,
    complete: bool,
}

#[cfg(any(
    test,
    all(
        feature = "server",
        any(
            all(not(test), not(feature = "desktop")),
            all(test, not(bitgarth_db_unit_only))
        )
    )
))]
impl NativeApiBalanceAssertionAggregate {
    fn empty() -> Self {
        Self {
            asserted_balance: UnsignedAmount::zero(),
            asserted_at: None,
            address_count: 0,
            complete: true,
        }
    }
}

fn parse_optional_label(
    raw: Option<String>,
    max_length: usize,
    field_name: &'static str,
) -> Result<Option<Label>, DbError> {
    raw.map(|value| {
        Label::parse_with_limit(&value, max_length)
            .map_err(|err| DbError::new(format!("Invalid {field_name} in DB: {err}")))
    })
    .transpose()
}

fn parse_optional_master_fingerprint(
    raw: Option<String>,
) -> Result<Option<ValidatedMasterFingerprint>, DbError> {
    raw.map(|value| {
        ValidatedMasterFingerprint::parse(&value).map_err(|err| {
            DbError::new(format!(
                "Invalid wallet master_fingerprint in DB for export account row: {err}"
            ))
        })
    })
    .transpose()
}

fn parse_optional_primary_account_number(raw: Option<i64>) -> Result<Option<u32>, DbError> {
    raw.map(|value| {
        if value < 0 {
            return Err(DbError::new(format!(
                "Invalid negative derivation_account in DB for export account row: {value}"
            )));
        }

        let account_index = u32::try_from(value).map_err(|_| {
            DbError::new(format!(
                "derivation_account out of range in DB for export account row: {value}"
            ))
        })?;

        account_index.checked_add(1).ok_or_else(|| {
            DbError::new(format!(
                "derivation_account overflow in DB for export account row: {value}"
            ))
        })
    })
    .transpose()
}

fn parse_export_account_boundary_mode(raw: &str) -> Result<ExportAccountBoundaryMode, DbError> {
    match raw {
        "native" => Ok(ExportAccountBoundaryMode::Native),
        "manual" => Ok(ExportAccountBoundaryMode::ManualAsset),
        _ => Err(DbError::new(format!(
            "Invalid export account boundary_mode in DB: {raw}"
        ))),
    }
}

#[cfg(any(
    test,
    all(
        feature = "server",
        any(
            all(not(test), not(feature = "desktop")),
            all(test, not(bitgarth_db_unit_only))
        )
    )
))]
fn parse_export_tx_type(raw: &str) -> Result<AccountTransactionDirection, DbError> {
    match raw {
        "receive" => Ok(AccountTransactionDirection::Incoming),
        "send" => Ok(AccountTransactionDirection::Outgoing),
        "self_transfer" => Ok(AccountTransactionDirection::SelfTransfer),
        _ => Err(DbError::new(format!(
            "Invalid tx_type in export row: {raw}"
        ))),
    }
}

fn parse_split_amount(
    hi: i64,
    lo: i64,
    field_name: &'static str,
) -> Result<UnsignedAmount, DbError> {
    parse_split_amount_parts(hi, lo)
        .map_err(|err| DbError::new(format!("Invalid {field_name} split amount from DB: {err}")))
}

#[cfg(any(
    test,
    all(
        feature = "server",
        any(
            all(not(test), not(feature = "desktop")),
            all(test, not(bitgarth_db_unit_only))
        )
    )
))]
fn parse_optional_split_amount_if_present(
    hi: Option<i64>,
    lo: Option<i64>,
    field_name: &'static str,
) -> Result<Option<UnsignedAmount>, DbError> {
    parse_optional_split_amount_parts(hi, lo)
        .map_err(|err| DbError::new(format!("Invalid {field_name} split amount from DB: {err}")))
}

fn parse_asserted_on(raw: &str, field_name: &'static str) -> Result<NaiveDate, DbError> {
    NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .map_err(|err| DbError::new(format!("Invalid {field_name} in DB: {err}")))
}

fn validate_manual_export_asset_id(raw: &str) -> Result<(), DbError> {
    crate::asset_capabilities::unsynced::UnsyncedAssetId::parse(raw)
        .map(|_| ())
        .map_err(|err| DbError::new(format!("Invalid manual export asset_id in DB: {err}")))
}

fn validate_manual_export_network_id(raw: &str) -> Result<(), DbError> {
    crate::asset_capabilities::unsynced::UnsyncedNetworkId::parse(raw)
        .map(|_| ())
        .map_err(|err| DbError::new(format!("Invalid manual export network_id in DB: {err}")))
}

fn validate_manual_export_coingecko_id(raw: &str) -> Result<(), DbError> {
    crate::asset_capabilities::unsynced::CoingeckoAssetId::parse(raw)
        .map(|_| ())
        .map_err(|err| DbError::new(format!("Invalid manual export coingecko_id in DB: {err}")))
}

fn validate_manual_export_name(field_name: &'static str, value: String) -> Result<String, DbError> {
    if value.trim().is_empty() {
        return Err(DbError::new(format!(
            "Invalid manual export {field_name} in DB: cannot be empty"
        )));
    }
    Ok(value)
}

fn validate_manual_export_symbol(value: Option<String>) -> Result<Option<String>, DbError> {
    value
        .map(|symbol| {
            let mut chars = symbol.chars();
            let Some(_) = chars.next() else {
                return Err(DbError::new(
                    "Invalid manual export symbol in DB: cannot be empty",
                ));
            };
            if chars.next().is_some() {
                return Err(DbError::new(
                    "Invalid manual export symbol in DB: must be one character",
                ));
            }
            Ok(symbol)
        })
        .transpose()
}

#[cfg(any(
    test,
    all(
        feature = "server",
        any(
            all(not(test), not(feature = "desktop")),
            all(test, not(bitgarth_db_unit_only))
        )
    )
))]
fn add_amount(
    left: UnsignedAmount,
    right: UnsignedAmount,
    field_name: &'static str,
) -> Result<UnsignedAmount, DbError> {
    left.checked_add(right)
        .map_err(|_| DbError::new(format!("Unsigned overflow while summing {field_name}")))
}

pub(crate) fn load_all_accounts_for_export(
    user_id: UserId,
) -> Result<Vec<ExportAccountRow>, DbError> {
    with_user_db(user_id, |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT
                    export_rows.id,
                    export_rows.wallet_id,
                    export_rows.account_label,
                    export_rows.wallet_label,
                    export_rows.wallet_master_fingerprint,
                    export_rows.wallet_accessor_label,
                    export_rows.primary_derivation_account,
                    export_rows.boundary_mode,
                    export_rows.created_at,
                    export_rows.asset_id,
                    export_rows.native_network,
                    export_rows.unit_code,
                    export_rows.decimal_precision,
                    export_rows.manual_network_id,
                    export_rows.manual_symbol,
                    export_rows.manual_asset_name,
                    export_rows.manual_network_name,
                    export_rows.manual_coingecko_id,
                    export_rows.manual_asset_source,
                    export_rows.manual_precision_source,
                    export_rows.manual_coingecko_platform_id,
                    export_rows.manual_provider_platform_asset_ref
                 FROM (
                     SELECT
                        a.id AS id,
                        a.wallet_id AS wallet_id,
                        a.label AS account_label,
                        w.label AS wallet_label,
                        w.master_fingerprint AS wallet_master_fingerprint,
                        (
                            SELECT wa.accessor_label
                            FROM wallet_accessors wa
                            WHERE wa.wallet_id = w.id
                            ORDER BY wa.created_at ASC, wa.id ASC
                            LIMIT 1
                        ) AS wallet_accessor_label,
                        (
                            SELECT hk.derivation_account
                            FROM digital_asset_account_hd_keys hk
                            WHERE hk.account_id = a.id
                              AND hk.key_role = 'primary'
                            ORDER BY hk.created_at ASC, hk.id ASC
                            LIMIT 1
                        ) AS primary_derivation_account,
                        'native' AS boundary_mode,
                        a.asset_id AS asset_id,
                        a.network AS native_network,
                        NULL AS unit_code,
                        NULL AS decimal_precision,
                        NULL AS manual_network_id,
                        NULL AS manual_symbol,
                        NULL AS manual_asset_name,
                        NULL AS manual_network_name,
                        NULL AS manual_coingecko_id,
                        NULL AS manual_asset_source,
                        NULL AS manual_precision_source,
                        NULL AS manual_coingecko_platform_id,
                        NULL AS manual_provider_platform_asset_ref,
                        a.created_at AS created_at
                     FROM digital_asset_accounts a
                     JOIN wallets w ON w.id = a.wallet_id
                     UNION ALL
                     SELECT
                        a.id AS id,
                        a.wallet_id AS wallet_id,
                        a.label AS account_label,
                        w.label AS wallet_label,
                        w.master_fingerprint AS wallet_master_fingerprint,
                        (
                            SELECT wa.accessor_label
                            FROM wallet_accessors wa
                            WHERE wa.wallet_id = w.id
                            ORDER BY wa.created_at ASC, wa.id ASC
                            LIMIT 1
                        ) AS wallet_accessor_label,
                        NULL AS primary_derivation_account,
                        'manual' AS boundary_mode,
                        a.asset_id AS asset_id,
                        NULL AS native_network,
                        a.unit_code AS unit_code,
                        a.decimal_precision AS decimal_precision,
                        a.network_id AS manual_network_id,
                        a.symbol AS manual_symbol,
                        a.asset_name AS manual_asset_name,
                        a.network_name AS manual_network_name,
                        a.coingecko_id AS manual_coingecko_id,
                        a.asset_source AS manual_asset_source,
                        a.precision_source AS manual_precision_source,
                        a.coingecko_platform_id AS manual_coingecko_platform_id,
                        a.provider_platform_asset_ref AS manual_provider_platform_asset_ref,
                        a.created_at AS created_at
                     FROM manual_asset_accounts a
                     JOIN wallets w ON w.id = a.wallet_id
                 ) export_rows
                 ORDER BY export_rows.wallet_id, export_rows.created_at ASC, export_rows.id ASC",
            )
            .map_err(|err| {
                DbError::new(format!("Failed to prepare export account query: {err}"))
            })?;

        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<i64>>(12)?,
                    row.get::<_, Option<String>>(13)?,
                    row.get::<_, Option<String>>(14)?,
                    row.get::<_, Option<String>>(15)?,
                    row.get::<_, Option<String>>(16)?,
                    row.get::<_, Option<String>>(17)?,
                    row.get::<_, Option<String>>(18)?,
                    row.get::<_, Option<String>>(19)?,
                    row.get::<_, Option<String>>(20)?,
                    row.get::<_, Option<String>>(21)?,
                ))
            })
            .map_err(|err| {
                DbError::new(format!("Failed to execute export account query: {err}"))
            })?;

        let mut result = Vec::new();
        for row_result in rows {
            let (
                account_id_raw,
                wallet_id_raw,
                account_label_raw,
                wallet_label_raw,
                wallet_fingerprint_raw,
                wallet_accessor_label_raw,
                primary_derivation_account_raw,
                boundary_mode_raw,
                created_at_raw,
                asset_id_raw,
                native_network_raw,
                unit_code_raw,
                decimal_precision_raw,
                manual_network_id_raw,
                manual_symbol_raw,
                manual_asset_name_raw,
                manual_network_name_raw,
                manual_coingecko_id_raw,
                manual_asset_source_raw,
                manual_precision_source_raw,
                manual_coingecko_platform_id_raw,
                manual_provider_platform_asset_ref_raw,
            ) = row_result
                .map_err(|err| DbError::new(format!("Failed to map export account row: {err}")))?;

            let account_id = WalletAccountId::from_str(&account_id_raw)
                .map_err(|err| DbError::new(format!("Invalid wallet account id in DB: {err}")))?;
            let wallet_id = match wallet_id_raw {
                Some(raw) => WalletId::from_str(&raw)
                    .map_err(|err| DbError::new(format!("Invalid wallet id in DB: {err}")))?,
                None => return Err(DbError::new("Export account has no wallet_id")),
            };

            let account_label = match account_label_raw {
                Some(value) => Label::parse_with_limit(&value, ACCOUNT_LABEL_MAX_LENGTH)
                    .map_err(|err| DbError::new(format!("Invalid account label in DB: {err}")))?,
                None => return Err(DbError::new("Export account has no label")),
            };
            let wallet_label = match wallet_label_raw {
                Some(value) => Label::parse_with_limit(&value, WALLET_LABEL_MAX_LENGTH)
                    .map_err(|err| DbError::new(format!("Invalid wallet label in DB: {err}")))?,
                None => return Err(DbError::new("Export wallet has no label")),
            };
            let wallet_accessor_label = parse_optional_label(
                wallet_accessor_label_raw,
                WALLET_LABEL_MAX_LENGTH,
                "wallet accessor label",
            )?;
            let wallet_master_fingerprint =
                parse_optional_master_fingerprint(wallet_fingerprint_raw)?;
            let primary_account_number =
                parse_optional_primary_account_number(primary_derivation_account_raw)?;
            let boundary_mode = parse_export_account_boundary_mode(&boundary_mode_raw)?;
            let created_at = DateTime::parse_from_rfc3339(&created_at_raw)
                .map(|value| value.with_timezone(&Utc))
                .map_err(|err| {
                    DbError::new(format!("Invalid export account created_at in DB: {err}"))
                })?;
            let (
                commodity,
                native_asset_id,
                native_network,
                manual_asset_instance_id,
                manual_symbol,
                manual_asset_name,
                manual_network_name,
                manual_coingecko_id,
                manual_asset_source,
                manual_precision_source,
                manual_coingecko_platform_id,
                manual_provider_platform_asset_ref,
            ) = match boundary_mode {
                ExportAccountBoundaryMode::Native => {
                    let asset_id_raw = asset_id_raw
                        .ok_or_else(|| DbError::new("Native export account missing asset_id"))?;
                    let asset_id = SyncedAssetId::from_str(&asset_id_raw).ok_or_else(|| {
                        DbError::new(format!("Invalid asset_id in DB: {asset_id_raw}"))
                    })?;
                    let network_raw = native_network_raw
                        .ok_or_else(|| DbError::new("Native export account missing network"))?;
                    let native_network = Network::from_str(&network_raw).ok_or_else(|| {
                        DbError::new(format!("Invalid native network in DB: {network_raw}"))
                    })?;
                    let instance = asset_instance(
                        &synced_asset_instance(synced_asset_instance_id(asset_id))
                            .asset_instance_id,
                    )
                    .ok_or_else(|| DbError::new("synced asset instance not found in registry"))?;
                    (
                        ExportCommodity {
                            unit_code: instance.unit_code.as_str().to_string(),
                            decimal_precision: instance.decimal_precision,
                        },
                        Some(asset_id),
                        Some(native_network),
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                    )
                }
                ExportAccountBoundaryMode::ManualAsset => {
                    let asset_id_raw = asset_id_raw
                        .ok_or_else(|| DbError::new("Manual export account missing asset_id"))?;
                    let network_id_raw = manual_network_id_raw
                        .ok_or_else(|| DbError::new("Manual export account missing network_id"))?;
                    let unit_code_raw = unit_code_raw
                        .ok_or_else(|| DbError::new("Manual export account missing unit_code"))?;
                    let decimal_precision_raw = decimal_precision_raw.ok_or_else(|| {
                        DbError::new("Manual export account missing decimal_precision")
                    })?;
                    let asset_name = manual_asset_name_raw
                        .ok_or_else(|| DbError::new("Manual export account missing asset_name"))?;
                    let network_name = manual_network_name_raw.ok_or_else(|| {
                        DbError::new("Manual export account missing network_name")
                    })?;
                    let coingecko_id = manual_coingecko_id_raw.ok_or_else(|| {
                        DbError::new("Manual export account missing coingecko_id")
                    })?;
                    let asset_source = manual_asset_source_raw.ok_or_else(|| {
                        DbError::new("Manual export account missing asset_source")
                    })?;
                    let precision_source = manual_precision_source_raw.ok_or_else(|| {
                        DbError::new("Manual export account missing precision_source")
                    })?;
                    validate_manual_export_asset_id(&asset_id_raw)?;
                    validate_manual_export_network_id(&network_id_raw)?;
                    let manual_symbol = validate_manual_export_symbol(manual_symbol_raw)?;
                    let asset_name = validate_manual_export_name("asset_name", asset_name)?;
                    let network_name = validate_manual_export_name("network_name", network_name)?;
                    validate_manual_export_coingecko_id(&coingecko_id)?;
                    let unit_code =
                        ValidatedManualAssetUnitCode::parse(&unit_code_raw).map_err(|err| {
                            DbError::new(format!("Invalid manual export unit_code in DB: {err}"))
                        })?;
                    let decimal_precision = ManualAssetDisplayScale::try_from(
                        decimal_precision_raw,
                    )
                    .map_err(|err| {
                        DbError::new(format!(
                            "Invalid manual export decimal_precision in DB: {err}"
                        ))
                    })?;
                    let view = ManualAssetInstanceIdView {
                        asset_id: asset_id_raw.clone(),
                        network_id: network_id_raw.clone(),
                    };
                    (
                        ExportCommodity {
                            unit_code: unit_code.to_string(),
                            decimal_precision: decimal_precision.as_u8(),
                        },
                        None,
                        None,
                        Some(view),
                        manual_symbol,
                        Some(asset_name),
                        Some(network_name),
                        Some(coingecko_id),
                        Some(asset_source),
                        Some(precision_source),
                        manual_coingecko_platform_id_raw,
                        manual_provider_platform_asset_ref_raw,
                    )
                }
            };

            result.push(ExportAccountRow {
                account_id,
                wallet_id,
                boundary_mode,
                created_at,
                commodity,
                native_asset_id,
                native_network,
                account_label,
                wallet_label,
                wallet_accessor_label,
                wallet_master_fingerprint,
                primary_account_number,
                manual_asset_instance_id,
                manual_symbol,
                manual_asset_name,
                manual_network_name,
                manual_coingecko_id,
                manual_asset_source,
                manual_precision_source,
                manual_coingecko_platform_id,
                manual_provider_platform_asset_ref,
            });
        }

        Ok(result)
    })
}

#[cfg(any(
    test,
    all(
        feature = "server",
        any(
            all(not(test), not(feature = "desktop")),
            all(test, not(bitgarth_db_unit_only))
        )
    )
))]
pub(crate) fn load_all_confirmed_account_transaction_ledger_rows_for_export(
    user_id: UserId,
) -> Result<Vec<ExportAccountTransactionLedgerRow>, DbError> {
    with_user_db(user_id, |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT
                    atl.account_id,
                    atl.tx_hash,
                    atl.tx_type,
                    atl.fee_amount_hi,
                    atl.fee_amount_lo,
                    atl.balance_delta_hi,
                    atl.balance_delta_lo,
                    atl.balance_delta_negative,
                    atl.occurred_at,
                    atl.block_height,
                    atl.nonce,
                    atl.min_transfer_index,
                    atl.closing_balance_hi,
                    atl.closing_balance_lo
                 FROM account_transaction_ledger atl
                 JOIN digital_asset_accounts a
                   ON a.id = atl.account_id
                 WHERE atl.status = 'confirmed'
                 ORDER BY
                    a.id ASC,
                    atl.occurred_at ASC,
                    COALESCE(atl.block_height, 9223372036854775807) ASC,
                    COALESCE(atl.nonce, 9223372036854775807) ASC,
                    COALESCE(atl.min_transfer_index, 9223372036854775807) ASC,
                    atl.tx_hash ASC",
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to prepare export account ledger row query: {err}"
                ))
            })?;

        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, Option<i64>>(9)?,
                    row.get::<_, Option<i64>>(10)?,
                    row.get::<_, Option<i64>>(11)?,
                    row.get::<_, Option<i64>>(12)?,
                    row.get::<_, Option<i64>>(13)?,
                ))
            })
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to execute export account ledger row query: {err}"
                ))
            })?;

        let mut result = Vec::new();
        for row_result in rows {
            let (
                account_id_raw,
                tx_hash,
                tx_type_raw,
                fee_hi,
                fee_lo,
                balance_delta_hi,
                balance_delta_lo,
                balance_delta_negative,
                occurred_at_raw,
                block_height,
                nonce,
                min_transfer_index,
                closing_balance_hi,
                closing_balance_lo,
            ) = row_result.map_err(|err| {
                DbError::new(format!("Failed to map export account ledger row: {err}"))
            })?;

            if tx_hash.trim().is_empty() {
                return Err(DbError::new(
                    "Invalid export account ledger row: empty tx_hash",
                ));
            }

            let account_id = WalletAccountId::from_str(&account_id_raw)
                .map_err(|err| DbError::new(format!("Invalid wallet account id in DB: {err}")))?;
            let direction = parse_export_tx_type(&tx_type_raw)?;
            let occurred_at = parse_datetime(&occurred_at_raw)
                .map_err(|err| DbError::new(format!("Invalid occurred_at in DB: {err}")))?;
            let fee = parse_optional_split_amount_if_present(fee_hi, fee_lo, "fee_amount")?;
            let balance_delta =
                super::account_transactions::types::signed_balance_delta_from_split(
                    balance_delta_hi,
                    balance_delta_lo,
                    balance_delta_negative != 0,
                )?;
            let closing_balance = parse_optional_split_amount_if_present(
                closing_balance_hi,
                closing_balance_lo,
                "closing_balance",
            )?;

            result.push(ExportAccountTransactionLedgerRow {
                account_id,
                tx_hash,
                direction,
                fee,
                balance_delta,
                occurred_at,
                block_height,
                nonce,
                min_transfer_index,
                closing_balance,
            });
        }

        Ok(result)
    })
}

pub(crate) fn load_all_manual_asset_balance_assertion_rows_for_export(
    user_id: UserId,
) -> Result<Vec<ExportManualAssetBalanceAssertionRow>, DbError> {
    with_user_db(user_id, |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT
                    rows.account_id,
                    rows.id,
                    rows.asserted_on,
                    rows.balance_amount_hi,
                    rows.balance_amount_lo,
                    rows.note
                 FROM manual_asset_balance_assertions rows
                 ORDER BY
                    rows.account_id ASC,
                    rows.asserted_on ASC,
                    rows.id ASC",
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to prepare manual asset export assertion query: {err}"
                ))
            })?;

        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to execute manual asset export assertion query: {err}"
                ))
            })?;

        let mut result = Vec::new();
        for row_result in rows {
            let (account_id_raw, assertion_id_raw, asserted_on_raw, balance_hi, balance_lo, note) =
                row_result.map_err(|err| {
                    DbError::new(format!(
                        "Failed to map manual asset export assertion row: {err}"
                    ))
                })?;

            let account_id = WalletAccountId::from_str(&account_id_raw).map_err(|err| {
                DbError::new(format!(
                    "Invalid manual asset export wallet account id in DB: {err}"
                ))
            })?;
            let assertion_id =
                ManualAssetBalanceAssertionId::from_str(&assertion_id_raw).map_err(|err| {
                    DbError::new(format!(
                        "Invalid manual asset export assertion id in DB: {err}"
                    ))
                })?;
            let asserted_on =
                parse_asserted_on(&asserted_on_raw, "manual asset export asserted_on")?;
            let asserted_balance = parse_split_amount(
                balance_hi,
                balance_lo,
                "manual asset export asserted_balance",
            )?;

            result.push(ExportManualAssetBalanceAssertionRow {
                account_id,
                assertion_id,
                asserted_on,
                asserted_balance,
                note,
            });
        }

        Ok(result)
    })
}

#[cfg(any(
    test,
    all(
        feature = "server",
        any(
            all(not(test), not(feature = "desktop")),
            all(test, not(bitgarth_db_unit_only))
        )
    )
))]
pub(crate) fn load_all_native_api_balance_assertion_rows_for_export(
    user_id: UserId,
) -> Result<Vec<ExportNativeApiBalanceAssertionRow>, DbError> {
    with_user_db(user_id, |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT
                    da.account_id,
                    da.id,
                    tss.last_completed_at,
                    tss.api_confirmed_balance_hi,
                    tss.api_confirmed_balance_lo
                 FROM digital_asset_addresses da
                 JOIN transaction_sync_state tss
                   ON tss.scope = 'address'
                  AND tss.address_id = da.id
                 WHERE tss.last_result = 'success'
                 ORDER BY da.account_id ASC, da.id ASC",
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to prepare native API balance export query: {err}"
                ))
            })?;

        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                ))
            })
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to execute native API balance export query: {err}"
                ))
            })?;

        let mut by_account: std::collections::HashMap<
            WalletAccountId,
            NativeApiBalanceAssertionAggregate,
        > = std::collections::HashMap::new();
        for row_result in rows {
            let (account_id_raw, address_id_raw, completed_at_raw, balance_hi, balance_lo) =
                row_result.map_err(|err| {
                    DbError::new(format!(
                        "Failed to map native API balance export row: {err}"
                    ))
                })?;
            let account_id = WalletAccountId::from_str(&account_id_raw).map_err(|err| {
                DbError::new(format!(
                    "Invalid native API balance export account_id in DB: {err}"
                ))
            })?;
            let entry = by_account
                .entry(account_id)
                .or_insert_with(NativeApiBalanceAssertionAggregate::empty);
            entry.address_count = entry.address_count.saturating_add(1);

            let (Some(completed_at_raw), Some(balance)) = (
                completed_at_raw,
                parse_optional_split_amount_if_present(balance_hi, balance_lo, "api_balance")?,
            ) else {
                entry.complete = false;
                continue;
            };
            let completed_at = parse_datetime(&completed_at_raw).map_err(|err| {
                DbError::new(format!(
                    "Invalid native API balance export completed_at in DB: {err}"
                ))
            })?;

            entry.asserted_balance = add_amount(
                entry.asserted_balance,
                balance,
                "native API balance assertion",
            )?;
            if entry
                .asserted_at
                .is_none_or(|current| completed_at > current)
            {
                entry.asserted_at = Some(completed_at);
            }

            tracing::trace!(
                account_id = %account_id,
                address_id = %address_id_raw,
                "exports: included API-derived balance assertion address"
            );
        }

        let mut result = by_account
            .into_iter()
            .filter_map(|(account_id, aggregate)| {
                if !aggregate.complete {
                    return None;
                }
                let asserted_at = aggregate.asserted_at?;
                Some(ExportNativeApiBalanceAssertionRow {
                    account_id,
                    assertion_id: format!(
                        "api-balance:{account_id}:{}:{}",
                        asserted_at.to_rfc3339(),
                        aggregate.address_count
                    ),
                    asserted_on: asserted_at.date_naive(),
                    asserted_balance: aggregate.asserted_balance,
                })
            })
            .collect::<Vec<_>>();
        result.sort_by(|left, right| {
            left.account_id
                .to_string()
                .cmp(&right.account_id.to_string())
        });
        Ok(result)
    })
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;
    use crate::db::test_fixtures::{setup_test_user, unique_user_id};
    use crate::db::with_user_db_mut;
    use crate::wallets::IdentitySource;
    use chrono::Utc;
    use rusqlite::params;

    #[test]
    fn export_accounts_load_native_asset_and_network_context() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let wallet_id = WalletId::new();
        let account_id = WalletAccountId::new();
        let now = Utc::now().to_rfc3339();

        with_user_db_mut(user_id, |conn| -> Result<(), DbError> {
            conn.execute(
                "INSERT INTO wallets \
                 (id, label, label_key, master_fingerprint, identity_source, verified_at, created_at, updated_at) \
                 VALUES (?1, 'Native Export Wallet', 'native export wallet', NULL, ?2, NULL, ?3, ?3)",
                params![wallet_id.to_string(), IdentitySource::UserProvided.as_str(), &now],
            )
            .map_err(|err| DbError::new(format!("wallet insert failed: {err}")))?;
            conn.execute(
                "INSERT INTO digital_asset_accounts \
                 (id, wallet_id, label, label_key, asset_id, network, account_kind, created_at, updated_at) \
                 VALUES (?1, ?2, 'Bitcoin Account 1', 'bitcoin account 1', 'bitcoin', 'mainnet',
                         'single_address', ?3, ?3)",
                params![account_id.to_string(), wallet_id.to_string(), &now],
            )
            .map_err(|err| DbError::new(format!("native account insert failed: {err}")))?;
            Ok(())
        })
        .expect("fixture inserts");

        let rows = load_all_accounts_for_export(user_id).expect("export rows load");
        let native = rows
            .iter()
            .find(|row| row.account_id == account_id)
            .expect("native export row exists");

        assert_eq!(native.boundary_mode, ExportAccountBoundaryMode::Native);
        assert_eq!(native.native_asset_id, Some(SyncedAssetId::Bitcoin));
        assert_eq!(
            native.native_network,
            Some(crate::wallets::Network::Mainnet)
        );
        assert_eq!(native.manual_asset_instance_id, None);
    }

    #[test]
    fn export_accounts_accept_catalog_only_manual_asset_rows() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let wallet_id = WalletId::new();
        let account_id = WalletAccountId::new();
        let now = Utc::now().to_rfc3339();

        with_user_db_mut(user_id, |conn| -> Result<(), DbError> {
            conn.execute(
                "INSERT INTO wallets \
                 (id, label, label_key, master_fingerprint, identity_source, verified_at, created_at, updated_at) \
                 VALUES (?1, 'Export Wallet', 'export wallet', NULL, ?2, NULL, ?3, ?3)",
                params![wallet_id.to_string(), IdentitySource::UserProvided.as_str(), &now],
            )
            .map_err(|err| DbError::new(format!("wallet insert failed: {err}")))?;
            conn.execute(
                "INSERT INTO manual_asset_accounts \
                 (id, wallet_id, label, label_key, asset_id, network_id, decimal_precision,
                  unit_code, symbol, asset_name, network_name, coingecko_id, asset_source,
                  precision_source, coingecko_platform_id, provider_platform_asset_ref,
                  created_at, updated_at) \
                 VALUES (?1, ?2, 'ALGO Account 1', 'algo account 1', 'algorand', 'algorand-mainnet',
                         6, 'ALGO', NULL, 'Algorand', 'Algorand', 'algorand',
                         'bitgarth_catalog', 'bitgarth_catalog', NULL, NULL, ?3, ?3)",
                params![account_id.to_string(), wallet_id.to_string(), &now],
            )
            .map_err(|err| DbError::new(format!("manual account insert failed: {err}")))?;
            Ok(())
        })
        .expect("fixture inserts");

        let rows = load_all_accounts_for_export(user_id).expect("export rows load");
        let manual = rows
            .iter()
            .find(|row| row.account_id == account_id)
            .expect("manual export row exists");
        let manual_id = manual
            .manual_asset_instance_id
            .as_ref()
            .expect("manual asset id exists");

        assert_eq!(manual.boundary_mode, ExportAccountBoundaryMode::ManualAsset);
        assert_eq!(manual.commodity.unit_code, "ALGO");
        assert_eq!(manual.commodity.decimal_precision, 6);
        assert_eq!(manual_id.asset_id, "algorand");
        assert_eq!(manual_id.network_id, "algorand-mainnet");
        assert_eq!(manual.manual_symbol, None);
        assert_eq!(manual.manual_asset_name.as_deref(), Some("Algorand"));
        assert_eq!(manual.manual_network_name.as_deref(), Some("Algorand"));
        assert_eq!(manual.manual_coingecko_id.as_deref(), Some("algorand"));
    }

    #[test]
    fn export_accounts_preserve_coingecko_discovery_provenance() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let wallet_id = WalletId::new();
        let account_id = WalletAccountId::new();
        let now = Utc::now().to_rfc3339();

        with_user_db_mut(user_id, |conn| -> Result<(), DbError> {
            conn.execute(
                "INSERT INTO wallets \
                 (id, label, label_key, master_fingerprint, identity_source, verified_at, created_at, updated_at) \
                 VALUES (?1, 'Export Wallet', 'export wallet', NULL, ?2, NULL, ?3, ?3)",
                params![wallet_id.to_string(), IdentitySource::UserProvided.as_str(), &now],
            )
            .map_err(|err| DbError::new(format!("wallet insert failed: {err}")))?;
            conn.execute(
                "INSERT INTO manual_asset_accounts \
                 (id, wallet_id, label, label_key, asset_id, network_id, decimal_precision,
                  unit_code, symbol, asset_name, network_name, coingecko_id, asset_source,
                  precision_source, coingecko_platform_id, provider_platform_asset_ref,
                  created_at, updated_at) \
                 VALUES (?1, ?2, 'USDC Account', 'usdc account', 'usd-coin', 'algorand-mainnet',
                         6, 'USDC', NULL, 'USDC on Algorand', 'Algorand', 'usd-coin',
                         'coingecko_discovery', 'coingecko_platform', 'algorand', '31566704', ?3, ?3)",
                params![account_id.to_string(), wallet_id.to_string(), &now],
            )
            .map_err(|err| DbError::new(format!("manual account insert failed: {err}")))?;
            Ok(())
        })
        .expect("fixture inserts");

        let rows = load_all_accounts_for_export(user_id).expect("export rows load");
        let manual = rows
            .iter()
            .find(|row| row.account_id == account_id)
            .expect("manual export row exists");

        assert_eq!(
            manual.manual_asset_source.as_deref(),
            Some("coingecko_discovery")
        );
        assert_eq!(
            manual.manual_precision_source.as_deref(),
            Some("coingecko_platform")
        );
        assert_eq!(
            manual.manual_coingecko_platform_id.as_deref(),
            Some("algorand")
        );
        assert_eq!(
            manual.manual_provider_platform_asset_ref.as_deref(),
            Some("31566704")
        );
    }

    #[test]
    fn export_accounts_reject_invalid_manual_asset_snapshot_fields() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let wallet_id = WalletId::new();
        let account_id = WalletAccountId::new();
        let now = Utc::now().to_rfc3339();

        with_user_db_mut(user_id, |conn| -> Result<(), DbError> {
            conn.execute(
                "INSERT INTO wallets \
                 (id, label, label_key, master_fingerprint, identity_source, verified_at, created_at, updated_at) \
                 VALUES (?1, 'Export Wallet', 'export wallet', NULL, ?2, NULL, ?3, ?3)",
                params![wallet_id.to_string(), IdentitySource::UserProvided.as_str(), &now],
            )
            .map_err(|err| DbError::new(format!("wallet insert failed: {err}")))?;
            conn.execute(
                "INSERT INTO manual_asset_accounts \
                 (id, wallet_id, label, label_key, asset_id, network_id, decimal_precision,
                  unit_code, symbol, asset_name, network_name, coingecko_id, asset_source,
                  precision_source, coingecko_platform_id, provider_platform_asset_ref,
                  created_at, updated_at) \
                 VALUES (?1, ?2, 'Bad Manual Account', 'bad manual account',
                         'USD Coin', 'algorand-mainnet', 6, 'USDC', NULL,
                         'USDC on Algorand', 'Algorand', 'usd-coin',
                         'bitgarth_catalog', 'bitgarth_catalog', NULL, NULL, ?3, ?3)",
                params![account_id.to_string(), wallet_id.to_string(), &now],
            )
            .map_err(|err| DbError::new(format!("manual account insert failed: {err}")))?;
            Ok(())
        })
        .expect("fixture inserts");

        let result = load_all_accounts_for_export(user_id);

        assert!(matches!(
            result,
            Err(err) if err.to_string().contains("Invalid manual export asset_id")
        ));
    }

    #[test]
    fn export_accounts_accept_migrated_zcash_manual_asset_symbol() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let wallet_id = WalletId::new();
        let account_id = WalletAccountId::new();
        let now = Utc::now().to_rfc3339();

        with_user_db_mut(user_id, |conn| -> Result<(), DbError> {
            conn.execute(
                "INSERT INTO wallets \
                 (id, label, label_key, master_fingerprint, identity_source, verified_at, created_at, updated_at) \
                 VALUES (?1, 'Export Wallet', 'export wallet', NULL, ?2, NULL, ?3, ?3)",
                params![wallet_id.to_string(), IdentitySource::UserProvided.as_str(), &now],
            )
            .map_err(|err| DbError::new(format!("wallet insert failed: {err}")))?;
            conn.execute(
                "INSERT INTO manual_asset_accounts \
                 (id, wallet_id, label, label_key, asset_id, network_id, decimal_precision,
                  unit_code, symbol, asset_name, network_name, coingecko_id, asset_source,
                  precision_source, coingecko_platform_id, provider_platform_asset_ref,
                  created_at, updated_at) \
                 VALUES (?1, ?2, 'Zcash', 'zcash', 'zcash', 'zcash-mainnet',
                         8, 'ZEC', 'ZEC', 'Zcash', 'Zcash', 'zcash',
                         'bitgarth_catalog', 'bitgarth_catalog', NULL, NULL, ?3, ?3)",
                params![account_id.to_string(), wallet_id.to_string(), &now],
            )
            .map_err(|err| DbError::new(format!("manual account insert failed: {err}")))?;
            conn.execute_batch(include_str!(
                "../../migrations/user/V41__normalize_manual_asset_symbols.sql"
            ))
            .map_err(|err| DbError::new(format!("V41 migration failed: {err}")))?;
            Ok(())
        })
        .expect("fixture inserts");

        let rows = load_all_accounts_for_export(user_id).expect("export rows load");
        let manual = rows
            .iter()
            .find(|row| row.account_id == account_id)
            .expect("manual export row exists");

        assert_eq!(manual.manual_symbol, None);
        assert_eq!(manual.manual_asset_name.as_deref(), Some("Zcash"));
    }
}
