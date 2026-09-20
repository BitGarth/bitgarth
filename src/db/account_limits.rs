use crate::account_limits::{
    AccountActivationState, ClassifiedAccount, NativeAccountMode, NativeAccountModeRecord,
    SupportedAccountKind, SupportedAccountLimitRecord, classify_native_account_modes,
    classify_supported_accounts, would_exceed_supported_account_hard_cap,
};
use crate::asset_capabilities::{sync_provider, synced_asset_instance, synced_asset_instance_id};
use crate::db::DbError;
use crate::db::user_db::with_user_db;
use crate::models::UserId;
use crate::payments::types::{AccountAllowancePolicy, FeatureEntitlements};
use crate::wallets::{DigitalAssetAccountId, SyncedAssetId, WalletAccountId};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, Transaction};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;

#[cfg(test)]
pub(crate) fn load_supported_account_limit_records(
    user_id: UserId,
) -> Result<Vec<SupportedAccountLimitRecord>, DbError> {
    with_user_db(user_id, query_supported_account_limit_records)
}

#[cfg(test)]
pub(crate) fn classify_supported_accounts_for_user(
    user_id: UserId,
    active_limit: usize,
) -> Result<Vec<ClassifiedAccount>, DbError> {
    let records = load_supported_account_limit_records(user_id)?;
    Ok(classify_supported_accounts(records, active_limit))
}

pub(crate) fn native_account_modes_for_user(
    user_id: UserId,
    entitlements: &FeatureEntitlements,
) -> Result<HashMap<DigitalAssetAccountId, NativeAccountMode>, DbError> {
    with_user_db(user_id, |conn| {
        query_native_account_modes(conn, entitlements)
    })
}

pub(crate) fn classify_supported_accounts_for_entitlements(
    user_id: UserId,
    entitlements: &FeatureEntitlements,
) -> Result<Vec<ClassifiedAccount>, DbError> {
    with_user_db(user_id, |conn| {
        classify_accounts_in_conn(conn, entitlements)
    })
}

pub(crate) fn load_account_mode_snapshot_for_user(
    user_id: UserId,
    entitlements: &FeatureEntitlements,
) -> Result<
    (
        Vec<ClassifiedAccount>,
        HashMap<DigitalAssetAccountId, NativeAccountMode>,
    ),
    DbError,
> {
    with_user_db(user_id, |conn| {
        let modes = query_native_account_modes(conn, entitlements)?;
        let accounts = classify_accounts_in_conn_with_modes(conn, entitlements, &modes)?;
        Ok((accounts, modes))
    })
}

pub(crate) fn classify_supported_accounts_for_entitlements_in_tx(
    tx: &Transaction<'_>,
    entitlements: &FeatureEntitlements,
) -> Result<Vec<ClassifiedAccount>, DbError> {
    classify_accounts_in_conn(tx, entitlements)
}

fn classify_accounts_in_conn(
    conn: &Connection,
    entitlements: &FeatureEntitlements,
) -> Result<Vec<ClassifiedAccount>, DbError> {
    let modes = query_native_account_modes(conn, entitlements)?;
    classify_accounts_in_conn_with_modes(conn, entitlements, &modes)
}

fn classify_accounts_in_conn_with_modes(
    conn: &Connection,
    entitlements: &FeatureEntitlements,
    modes: &HashMap<DigitalAssetAccountId, NativeAccountMode>,
) -> Result<Vec<ClassifiedAccount>, DbError> {
    let AccountAllowancePolicy::Independent(allowances) = entitlements.account_allowance_policy
    else {
        return Ok(classify_supported_accounts(
            query_supported_account_limit_records(conn)?,
            usize::from(entitlements.sync_account_slots_limit),
        ));
    };
    let mut accounts = modes
        .iter()
        .map(|(id, mode)| ClassifiedAccount {
            account_id: (*id).into(),
            kind: SupportedAccountKind::Native,
            state: if *mode == NativeAccountMode::Inactive {
                AccountActivationState::Inactive
            } else {
                AccountActivationState::Active
            },
        })
        .collect::<Vec<_>>();
    let mut stmt = conn
        .prepare("SELECT id, admitted_at FROM manual_asset_accounts ORDER BY admitted_at, id")
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to prepare manual admission query", err)
        })?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|err| DbError::from_rusqlite_error("Failed to query manual admissions", err))?;
    for (index, row) in rows.enumerate() {
        let (id, _) = row
            .map_err(|err| DbError::from_rusqlite_error("Failed to read manual admission", err))?;
        accounts.push(ClassifiedAccount {
            account_id: WalletAccountId::from_str(&id)
                .map_err(|err| DbError::new(format!("Invalid manual account id: {err}")))?,
            kind: SupportedAccountKind::ManualAsset,
            state: if index < usize::from(allowances.manual()) {
                AccountActivationState::Active
            } else {
                AccountActivationState::Inactive
            },
        });
    }
    Ok(accounts)
}

fn query_native_account_modes(
    conn: &Connection,
    entitlements: &FeatureEntitlements,
) -> Result<HashMap<DigitalAssetAccountId, NativeAccountMode>, DbError> {
    let mut stmt = conn
        .prepare(
            "SELECT a.id, a.asset_id, s.selected_at
         FROM digital_asset_accounts a
         LEFT JOIN account_sync_slots s ON s.account_id = a.id
         ORDER BY s.selected_at, a.id",
        )
        .map_err(|err| DbError::from_rusqlite_error("Failed to prepare native mode query", err))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(|err| DbError::from_rusqlite_error("Failed to query native modes", err))?;
    let mut records = Vec::new();
    for row in rows {
        let (id, asset, admitted_at) =
            row.map_err(|err| DbError::from_rusqlite_error("Failed to read native mode", err))?;
        let admitted_at =
            admitted_at.ok_or_else(|| DbError::new("Missing native account admission"))?;
        let asset = SyncedAssetId::from_str(&asset)
            .ok_or_else(|| DbError::new("Invalid native account asset"))?;
        let capabilities = sync_provider(
            synced_asset_instance(synced_asset_instance_id(asset)).default_sync_provider,
        )
        .capabilities;
        records.push(NativeAccountModeRecord {
            account_id: DigitalAssetAccountId::from_str(&id)
                .map_err(|err| DbError::new(format!("Invalid native account id: {err}")))?,
            admitted_at: admitted_at
                .parse::<DateTime<Utc>>()
                .map_err(|err| DbError::new(format!("Invalid native admitted_at: {err}")))?,
            supports_balance_sync: capabilities.supports_balance_only_sync
                || capabilities.supports_transaction_sync,
            supports_transaction_sync: capabilities.supports_transaction_sync,
        });
    }
    let AccountAllowancePolicy::Independent(allowances) = entitlements.account_allowance_policy
    else {
        let classified = classify_supported_accounts(
            query_supported_account_limit_records(conn)?,
            usize::from(entitlements.sync_account_slots_limit),
        );
        let states = classified
            .into_iter()
            .map(|account| (account.account_id, account.state))
            .collect::<HashMap<_, _>>();
        return Ok(records
            .into_iter()
            .map(|record| {
                let active = states.get(&WalletAccountId::from(record.account_id))
                    == Some(&AccountActivationState::Active)
                    && record.supports_balance_sync;
                let mode = if !active || !entitlements.balance_sync_enabled {
                    NativeAccountMode::Inactive
                } else if record.supports_transaction_sync
                    && entitlements.transaction_history_sync_enabled
                {
                    NativeAccountMode::Transactions
                } else {
                    NativeAccountMode::BalanceOnly
                };
                (record.account_id, mode)
            })
            .collect());
    };
    let mut modes = classify_native_account_modes(records, allowances);
    if !entitlements.balance_sync_enabled {
        modes
            .values_mut()
            .for_each(|mode| *mode = NativeAccountMode::Inactive);
    } else if !entitlements.transaction_history_sync_enabled {
        modes
            .values_mut()
            .filter(|mode| **mode == NativeAccountMode::Transactions)
            .for_each(|mode| *mode = NativeAccountMode::BalanceOnly);
    }
    Ok(modes)
}

pub(crate) fn ensure_supported_account_hard_cap_before_insert_in_tx(
    tx: &Transaction<'_>,
    creating_supported_count: usize,
) -> Result<(), DbError> {
    let current_count: usize = tx
        .query_row(
            "SELECT
                 (SELECT COUNT(*) FROM digital_asset_accounts) +
                 (SELECT COUNT(*) FROM manual_asset_accounts)",
            [],
            |row| row.get(0),
        )
        .map_err(|err| DbError::from_rusqlite_error("Failed to count supported accounts", err))?;

    if would_exceed_supported_account_hard_cap(current_count, creating_supported_count) {
        return Err(DbError::new("Supported account hard cap exceeded"));
    }

    Ok(())
}

pub(crate) fn account_state_for(
    classified: &[ClassifiedAccount],
    account_id: &WalletAccountId,
) -> AccountActivationState {
    classified
        .iter()
        .find(|account| &account.account_id == account_id)
        .map(|account| account.state)
        .unwrap_or(AccountActivationState::Inactive)
}

pub(crate) fn native_account_sync_eligible_for_user(
    user_id: UserId,
    entitlements: &FeatureEntitlements,
    account_id: DigitalAssetAccountId,
) -> Result<bool, DbError> {
    Ok(native_account_modes_for_user(user_id, entitlements)?
        .get(&account_id)
        .is_some_and(|mode| *mode != NativeAccountMode::Inactive))
}

pub(crate) fn sync_eligible_native_account_ids_for_user(
    user_id: UserId,
    entitlements: &FeatureEntitlements,
) -> Result<HashSet<DigitalAssetAccountId>, DbError> {
    Ok(native_account_modes_for_user(user_id, entitlements)?
        .into_iter()
        .filter_map(|(id, mode)| (mode != NativeAccountMode::Inactive).then_some(id))
        .collect())
}

fn query_supported_account_limit_records(
    conn: &Connection,
) -> Result<Vec<SupportedAccountLimitRecord>, DbError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, 'native' AS kind, created_at
             FROM digital_asset_accounts
             UNION ALL
             SELECT id, 'manual_asset' AS kind, created_at
             FROM manual_asset_accounts",
        )
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to prepare supported account limit query", err)
        })?;

    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to query supported account limit rows", err)
        })?;

    let mut records = Vec::new();
    for row in rows {
        let (account_id_raw, kind_raw, created_at_raw) = row.map_err(|err| {
            DbError::from_rusqlite_error("Failed to map supported account limit row", err)
        })?;
        records.push(SupportedAccountLimitRecord {
            account_id: WalletAccountId::from_str(&account_id_raw)
                .map_err(|err| DbError::new(format!("Invalid supported account id: {err}")))?,
            kind: parse_supported_account_kind(&kind_raw)?,
            created_at: DateTime::parse_from_rfc3339(&created_at_raw)
                .map(|value| value.with_timezone(&Utc))
                .map_err(|err| {
                    DbError::new(format!("Invalid supported account created_at: {err}"))
                })?,
        });
    }

    Ok(records)
}

fn parse_supported_account_kind(value: &str) -> Result<SupportedAccountKind, DbError> {
    match value {
        "native" => Ok(SupportedAccountKind::Native),
        "manual_asset" => Ok(SupportedAccountKind::ManualAsset),
        _ => Err(DbError::new(format!(
            "Invalid supported account kind: {value}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account_limits::SupportedAccountKind;
    use crate::db::{acquire_test_runtime, with_user_db_mut};
    use crate::models::UserId;
    use crate::wallets::{IdentitySource, WalletId};
    use chrono::{TimeZone, Utc};
    use rusqlite::params;

    fn timestamp(second: u32) -> String {
        Utc.with_ymd_and_hms(2026, 6, 18, 12, 0, second)
            .unwrap()
            .to_rfc3339()
    }

    fn setup_user_with_wallet() -> (UserId, WalletId) {
        let user_id = UserId::new();
        super::super::user_db::enable_test_mode();
        let sqlcipher_compatibility = super::super::encryption::current_sqlcipher_compatibility()
            .expect("SQLCipher compatibility should probe");
        super::super::user_db::initialize_user_db(
            user_id,
            super::super::encryption::UserDbOpenMode::Encrypted {
                dek: super::super::encryption::Dek::generate(),
                authority: super::super::encryption::UnlockAuthority::PasswordLogin,
                sqlcipher_compatibility,
            },
        )
        .expect("user db should initialize");
        let wallet_id = WalletId::new();
        let now = timestamp(0);

        with_user_db_mut(user_id, |conn| -> Result<(), DbError> {
            conn.execute(
                "INSERT INTO wallets
                 (id, label, label_key, master_fingerprint, identity_source, verified_at, created_at, updated_at)
                 VALUES (?1, 'Limit Wallet', 'limit wallet', NULL, ?2, NULL, ?3, ?3)",
                params![wallet_id.to_string(), IdentitySource::UserProvided.as_str(), &now],
            )
            .map_err(|err| DbError::new(format!("wallet insert failed: {err}")))?;
            Ok(())
        })
        .expect("wallet fixture should insert");

        (user_id, wallet_id)
    }

    fn insert_native_account(
        user_id: UserId,
        wallet_id: WalletId,
        account_id: WalletAccountId,
        label: &str,
        created_at: &str,
    ) {
        with_user_db_mut(user_id, |conn| -> Result<(), DbError> {
            conn.execute(
                "INSERT INTO digital_asset_accounts
                 (id, wallet_id, label, label_key, asset_id, network, account_kind, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 'bitcoin', 'mainnet', 'single_address', ?5, ?5)",
                params![
                    account_id.to_string(),
                    wallet_id.to_string(),
                    label,
                    label.to_ascii_lowercase(),
                    created_at,
                ],
            )
            .map_err(|err| DbError::new(format!("native account insert failed: {err}")))?;
            Ok(())
        })
        .expect("native fixture should insert");
    }

    fn insert_manual_account(
        user_id: UserId,
        wallet_id: WalletId,
        account_id: WalletAccountId,
        label: &str,
        created_at: &str,
    ) {
        with_user_db_mut(user_id, |conn| -> Result<(), DbError> {
            conn.execute(
                "INSERT INTO manual_asset_accounts
                 (id, wallet_id, label, label_key, asset_id, network_id, decimal_precision,
                  unit_code, symbol, asset_name, network_name, coingecko_id, asset_source,
                  precision_source, coingecko_platform_id, provider_platform_asset_ref,
                  created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 'algorand', 'algorand-mainnet', 6,
                         'ALGO', NULL, 'Algorand', 'Algorand', 'algorand',
                         'bitgarth_catalog', 'bitgarth_catalog', NULL, NULL, ?5, ?5)",
                params![
                    account_id.to_string(),
                    wallet_id.to_string(),
                    label,
                    label.to_ascii_lowercase(),
                    created_at,
                ],
            )
            .map_err(|err| DbError::new(format!("manual account insert failed: {err}")))?;
            Ok(())
        })
        .expect("manual fixture should insert");
    }

    #[test]
    fn native_plus_manual_accounts_are_counted() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let (user_id, wallet_id) = setup_user_with_wallet();
        let native_id = WalletAccountId::new();
        let manual_id = WalletAccountId::new();
        insert_native_account(user_id, wallet_id, native_id, "BTC", &timestamp(1));
        insert_manual_account(user_id, wallet_id, manual_id, "ALGO", &timestamp(2));

        let records = load_supported_account_limit_records(user_id).expect("records should load");

        assert_eq!(records.len(), 2);
        assert!(records.iter().any(|record| {
            record.account_id == native_id && record.kind == SupportedAccountKind::Native
        }));
        assert!(records.iter().any(|record| {
            record.account_id == manual_id && record.kind == SupportedAccountKind::ManualAsset
        }));
    }

    #[test]
    fn hard_cap_helper_rejects_when_current_plus_creating_exceeds_cap() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let (user_id, wallet_id) = setup_user_with_wallet();
        for index in 0..1 {
            insert_native_account(
                user_id,
                wallet_id,
                WalletAccountId::new(),
                &format!("BTC {index}"),
                &timestamp(1),
            );
        }

        let result = with_user_db_mut(user_id, |conn| -> Result<(), DbError> {
            let tx = conn
                .transaction()
                .map_err(|err| DbError::new(format!("transaction open failed: {err}")))?;
            ensure_supported_account_hard_cap_before_insert_in_tx(
                &tx,
                crate::account_limits::SUPPORTED_ACCOUNT_HARD_CAP,
            )
        });

        assert!(result.is_err());
    }

    #[test]
    fn hard_cap_helper_allows_exact_cap_before_insert() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let (user_id, wallet_id) = setup_user_with_wallet();
        for index in 0..1 {
            insert_native_account(
                user_id,
                wallet_id,
                WalletAccountId::new(),
                &format!("BTC {index}"),
                &timestamp(1),
            );
        }

        let result = with_user_db_mut(user_id, |conn| -> Result<(), DbError> {
            let tx = conn
                .transaction()
                .map_err(|err| DbError::new(format!("transaction open failed: {err}")))?;
            ensure_supported_account_hard_cap_before_insert_in_tx(
                &tx,
                crate::account_limits::SUPPORTED_ACCOUNT_HARD_CAP - 1,
            )
        });

        assert!(result.is_ok());
    }

    #[test]
    fn independent_account_classification_separates_native_and_manual_order() {
        use crate::payments::account_allowances::AccountAllowances;
        use crate::payments::types::{AccountAllowancePolicy, FeatureEntitlements};

        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let (user_id, wallet_id) = setup_user_with_wallet();
        let native = WalletAccountId::new();
        let manual = WalletAccountId::new();
        insert_manual_account(user_id, wallet_id, manual, "Early manual", &timestamp(1));
        insert_native_account(user_id, wallet_id, native, "Later native", &timestamp(2));
        with_user_db_mut(user_id, |conn| -> Result<(), DbError> {
            conn.execute(
                "INSERT INTO account_sync_slots VALUES (?1, ?2, 'free')",
                params![native.to_string(), timestamp(3)],
            )
            .unwrap();
            conn.execute(
                "UPDATE manual_asset_accounts SET admitted_at = ?2 WHERE id = ?1",
                params![manual.to_string(), timestamp(1)],
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
        let mut entitlements = FeatureEntitlements::free();
        entitlements.account_allowance_policy =
            AccountAllowancePolicy::Independent(AccountAllowances::try_new(1, 1, 1).unwrap());
        entitlements.transaction_history_sync_enabled = true;
        let modes = native_account_modes_for_user(user_id, &entitlements).unwrap();
        let native_id = DigitalAssetAccountId::from_str(&native.to_string()).unwrap();
        assert_eq!(modes[&native_id], NativeAccountMode::Transactions);
        let classified =
            classify_supported_accounts_for_entitlements(user_id, &entitlements).unwrap();
        assert_eq!(
            account_state_for(&classified, &native),
            AccountActivationState::Active
        );
        assert_eq!(
            account_state_for(&classified, &manual),
            AccountActivationState::Active
        );
    }
}
