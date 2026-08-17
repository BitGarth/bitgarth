mod balance;
#[cfg(all(test, feature = "db-tests"))]
mod db_tests;
mod ledger_rebuild;
mod page_query;
pub(super) mod types;
#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod unit_tests;
mod wallet_report;

pub(in crate::db) use ledger_rebuild::bitcoin_account_has_complete_history_proof_for_repair;
pub(crate) use ledger_rebuild::{
    BitcoinAccountCompletionPublication, BitcoinAddressProofPublication,
    BitcoinHdDiscoveryPublication, load_bitcoin_account_history_coverage,
    load_bitcoin_history_repair_pending, publish_bitcoin_account_completion,
    rebuild_account_transaction_ledger, rebuild_account_transaction_ledger_conn,
    rebuild_account_transaction_ledger_with_unknown_bitcoin_basis,
};
pub(crate) use page_query::load_account_transactions_pages;
pub(crate) use types::AccountTransactionLedgerPage;
pub(crate) use wallet_report::{
    HoldingsReportData, HoldingsReportWalletData, WalletReportBalanceState, WalletReportLoadError,
    load_holdings_report, load_holdings_report_range_plan, load_wallet_report,
    load_wallet_report_range_plan,
};
