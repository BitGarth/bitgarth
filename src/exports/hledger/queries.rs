use crate::db;
use crate::db::DbError;
use crate::models::UserId;
use crate::transactions::TransactionCount;
use crate::wallets::{DigitalAssetAccountId, SyncedAssetId, WalletAccountId};
use std::collections::HashSet;
use std::str::FromStr;

pub(crate) use db::{
    ExportAccountBoundaryMode, ExportAccountRow, ExportAccountTransactionLedgerRow,
    ExportCommodity, ExportManualAssetBalanceAssertionRow, ExportNativeApiBalanceAssertionRow,
};

pub(crate) fn load_all_accounts_for_export(
    user_id: UserId,
) -> Result<Vec<ExportAccountRow>, DbError> {
    db::load_all_accounts_for_export(user_id)
}

pub(crate) fn load_all_confirmed_account_transaction_ledger_rows_for_export(
    user_id: UserId,
) -> Result<Vec<ExportAccountTransactionLedgerRow>, DbError> {
    db::load_all_confirmed_account_transaction_ledger_rows_for_export(user_id)
}

pub(crate) fn load_all_manual_asset_balance_assertion_rows_for_export(
    user_id: UserId,
) -> Result<Vec<ExportManualAssetBalanceAssertionRow>, DbError> {
    db::load_all_manual_asset_balance_assertion_rows_for_export(user_id)
}

pub(crate) fn load_all_native_api_balance_assertion_rows_for_export(
    user_id: UserId,
) -> Result<Vec<ExportNativeApiBalanceAssertionRow>, DbError> {
    db::load_all_native_api_balance_assertion_rows_for_export(user_id)
}

pub(crate) fn load_incomplete_bitcoin_account_ids_for_export(
    user_id: UserId,
    accounts: &[ExportAccountRow],
    history_cap: TransactionCount,
) -> Result<HashSet<WalletAccountId>, DbError> {
    let mut incomplete = HashSet::new();
    for account in accounts
        .iter()
        .filter(|account| account.native_asset_id == Some(SyncedAssetId::Bitcoin))
    {
        let account_id = DigitalAssetAccountId::from_str(&account.account_id.to_string())
            .map_err(|err| DbError::new(format!("Invalid Bitcoin export account id: {err}")))?;
        if !matches!(
            db::balance_reliability::load_effective_bitcoin_history_coverage(
                user_id,
                account_id,
                history_cap,
            )?,
            Some(db::BitcoinAccountHistoryCoverage::Complete { .. })
        ) {
            incomplete.insert(account.account_id);
        }
    }
    Ok(incomplete)
}
