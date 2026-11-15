#[cfg(feature = "server")]
mod balance_projection;
#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
pub(crate) use balance_projection::{ProjectedAssetBalance, ProjectedWalletBalance};
#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
pub(crate) use balance_projection::{
    ProjectedBalanceReason, ProjectedBalanceStatus, WalletBalanceProjection,
    load_wallet_balance_projection,
};
#[cfg(feature = "server")]
mod conversions;
#[cfg(feature = "server")]
pub(crate) use conversions::{NativeAccountManualSyncContext, native_account_manual_sync_view};
mod handlers_read;
mod handlers_write;
#[cfg(feature = "server")]
mod helpers;
#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod tests;
mod types;

#[cfg(any(test, feature = "server"))]
pub(crate) use types::CurrentAssetValueView;
#[cfg(any(test, feature = "server"))]
pub(crate) use types::HoldingsReportWalletRow;
#[cfg(feature = "server")]
pub(crate) use types::WalletAggregateBalanceView;
pub(crate) use types::{
    AccountBalanceStateView, AccountCreationStateView, AccountLimitNoticeView,
    AccountReferenceKind, AccountStateView, AccountTransactionCountsView, AccountView,
    BalanceAmountView, BitcoinBalanceObservationView, CustomAccountView, FiatAmountView,
    HoldingsReportResponse, HoldingsValueGapView, ManualAssetAccountView, ManualSyncDisabledReason,
    ManualSyncMode, NativeAccountManualSyncView, NativeAccountView, NativeBalanceStateView,
    ValidateXpubResponse, WalletBalanceView, WalletError, WalletReportAccountRow,
    WalletReportBalanceStateView, WalletReportResponse, WalletValueSummaryView, WalletView,
    WalletsValueSummaryView,
};

#[cfg(any(target_arch = "wasm32", feature = "desktop"))]
pub(crate) use handlers_read::get_wallet_by_fingerprint;
pub(crate) use handlers_read::{
    get_account_addresses, get_holdings_report, get_wallet_report, get_wallets,
    manual_asset_catalog_total, manual_asset_discovery_detail, manual_asset_discovery_price,
    search_manual_asset_instances,
};
#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
pub(crate) use handlers_read::{
    get_account_addresses_for_user, get_holdings_report_for_user, get_wallet_report_for_user,
    get_wallets_for_user, search_manual_asset_instances_for_user,
};
#[cfg(any(target_arch = "wasm32", feature = "desktop"))]
pub(crate) use handlers_write::link_trezor_wallet;
pub(crate) use handlers_write::{
    add_bitcoin_address, add_ethereum_address, add_manual_asset_account, add_xpub, delete_account,
    delete_wallet, move_wallet_account, update_account_label, update_wallet_label, validate_xpub,
};
#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
pub(crate) use handlers_write::{
    add_bitcoin_address_for_user, add_ethereum_address_for_user, add_manual_asset_account_for_user,
    add_xpub_for_user, delete_account_for_user, delete_wallet_for_user,
    move_wallet_account_for_user, update_account_label_for_user, update_wallet_label_for_user,
    validate_xpub_for_user,
};
