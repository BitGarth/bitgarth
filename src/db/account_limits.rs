use crate::account_limits::{
    AccountActivationState, ClassifiedAccount, SupportedAccountKind, SupportedAccountLimitRecord,
    classify_supported_accounts, would_exceed_supported_account_hard_cap,
};
use crate::db::DbError;
use crate::db::user_db::with_user_db;
use crate::models::UserId;
use crate::wallets::{DigitalAssetAccountId, WalletAccountId};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, Transaction};
use std::collections::HashSet;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct NativeAccountSyncEligibility {
    pub(crate) account_active: bool,
    pub(crate) provider_or_plan_supports_requested_sync: bool,
}

impl NativeAccountSyncEligibility {
    pub(crate) fn eligible(self) -> bool {
        crate::account_limits::native_account_sync_eligible(
            if self.account_active {
                AccountActivationState::Active
            } else {
                AccountActivationState::Inactive
            },
            true,
            self.provider_or_plan_supports_requested_sync,
        )
    }
}

pub(crate) fn load_supported_account_limit_records(
    user_id: UserId,
) -> Result<Vec<SupportedAccountLimitRecord>, DbError> {
    with_user_db(user_id, query_supported_account_limit_records)
}

pub(crate) fn classify_supported_accounts_for_user(
    user_id: UserId,
    active_limit: usize,
) -> Result<Vec<ClassifiedAccount>, DbError> {
    let records = load_supported_account_limit_records(user_id)?;
    Ok(classify_supported_accounts(records, active_limit))
}

pub(crate) fn classify_supported_accounts_in_tx(
    tx: &Transaction<'_>,
    active_limit: usize,
) -> Result<Vec<ClassifiedAccount>, DbError> {
    let records = query_supported_account_limit_records(tx)?;
    Ok(classify_supported_accounts(records, active_limit))
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
    active_limit: usize,
    account_id: DigitalAssetAccountId,
    requires_free_balance_support: bool,
) -> Result<bool, DbError> {
    Ok(native_account_sync_eligibility_for_user(
        user_id,
        active_limit,
        account_id,
        requires_free_balance_support,
    )?
    .eligible())
}

pub(crate) fn native_account_sync_eligibility_for_user(
    user_id: UserId,
    active_limit: usize,
    account_id: DigitalAssetAccountId,
    requires_free_balance_support: bool,
) -> Result<NativeAccountSyncEligibility, DbError> {
    let classified = classify_supported_accounts_for_user(user_id, active_limit)?;
    let state = account_state_for(&classified, &WalletAccountId::from(account_id));
    let provider_or_plan_supports_requested_sync = !requires_free_balance_support
        || super::sync_slots::account_supports_free_balance_sync(user_id, account_id)?;
    Ok(NativeAccountSyncEligibility {
        account_active: state == AccountActivationState::Active,
        provider_or_plan_supports_requested_sync,
    })
}

pub(crate) fn sync_eligible_native_account_ids_for_user(
    user_id: UserId,
    active_limit: usize,
    requires_free_balance_support: bool,
) -> Result<HashSet<DigitalAssetAccountId>, DbError> {
    classify_supported_accounts_for_user(user_id, active_limit)?
        .into_iter()
        .filter(|account| {
            account.kind == SupportedAccountKind::Native
                && account.state == AccountActivationState::Active
        })
        .map(|account| {
            DigitalAssetAccountId::from_str(&account.account_id.to_string())
                .map_err(|err| DbError::new(format!("Invalid native account id: {err}")))
        })
        .filter_map(|account_id| match account_id {
            Ok(account_id) => {
                let provider_or_plan_supports_requested_sync = !requires_free_balance_support
                    || match super::sync_slots::account_supports_free_balance_sync(
                        user_id, account_id,
                    ) {
                        Ok(supports) => supports,
                        Err(err) => return Some(Err(err)),
                    };
                if crate::account_limits::native_account_sync_eligible(
                    AccountActivationState::Active,
                    true,
                    provider_or_plan_supports_requested_sync,
                ) {
                    Some(Ok(account_id))
                } else {
                    None
                }
            }
            Err(err) => Some(Err(err)),
        })
        .collect()
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
        for index in 0..100 {
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
            ensure_supported_account_hard_cap_before_insert_in_tx(&tx, 1)
        });

        assert!(result.is_err());
    }

    #[test]
    fn hard_cap_helper_allows_exact_cap_before_insert() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let (user_id, wallet_id) = setup_user_with_wallet();
        for index in 0..99 {
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
            ensure_supported_account_hard_cap_before_insert_in_tx(&tx, 1)
        });

        assert!(result.is_ok());
    }
}
