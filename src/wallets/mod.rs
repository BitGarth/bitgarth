#![cfg_attr(
    not(feature = "server"),
    expect(
        dead_code,
        reason = "Wallet domain types are server-first; client builds compile only a subset"
    )
)]
#![cfg_attr(
    all(not(target_arch = "wasm32"), not(feature = "desktop")),
    allow(
        unused_imports,
        reason = "Trezor/wallet re-exports are unused when link_trezor is not compiled"
    )
)]

pub(crate) mod bitcoin;
pub(crate) mod display;
pub(crate) mod labels;
pub(crate) mod manual_assets;
#[cfg(any(feature = "server", test))]
mod manual_migration;
pub(crate) mod primitives;
pub(crate) mod records;
pub(crate) mod requests;
pub(crate) mod validation;
pub(crate) mod xpub;

#[cfg(feature = "server")]
pub(crate) use crate::account_model::AccountModel;

// primitives — items used by both client and server
pub(crate) use primitives::{
    ACCOUNT_LABEL_MAX_LENGTH, AccountIndex, AddressScheme, DEFAULT_ACCOUNT_ADDRESSES_PAGE_SIZE,
    DigitalAssetAccountId, DigitalAssetAddressId, Network, RawAccountIndex, ReportDateParam,
    ReportTimezoneParam, SyncedAssetId, TransactionSortDirection, WALLET_LABEL_MAX_LENGTH,
    WalletAccountId, WalletId, WalletReportDateRange,
};

// primitives — server-only items
#[cfg(feature = "server")]
pub(crate) use primitives::{
    ACCOUNT_TRANSACTIONS_PAGE_SIZE, AccessorKind, AccountKind, AddressSourceType, BIP44_GAP_LIMIT,
    DerivationCoinType, DerivationPath, DerivationPurpose, HdKeyId, IdentitySource, KeyRole,
    KeySource, WalletAccessorId,
};

// xpub — items used by both client and server
pub(crate) use xpub::{
    RawExtendedPubkey, RawMasterFingerprint, TrezorDeviceId, TrezorDeviceLabel,
    ValidatedMasterFingerprint, validate_extended_pubkey_format,
};

// xpub — server-only items
#[cfg(feature = "server")]
pub(crate) use xpub::{
    NormalizedExtendedPubkey, ValidatedExtendedPubkey, XPUB_MAINNET_VERSION, YPUB_MAINNET_VERSION,
    ZPUB_MAINNET_VERSION, detect_address_scheme_from_prefix,
};

// labels — items used by both client and server
#[cfg(any(feature = "server", test))]
pub(crate) use labels::ValidatedManualAssetUnitCode;
pub(crate) use labels::{Label, RawLabel};

// labels — server/test only
#[cfg(any(feature = "server", test))]
pub(crate) use labels::ManualAssetDisplayScale;

// labels — server-only items
#[cfg(feature = "server")]
pub(crate) use labels::LabelKey;

#[cfg(all(test, not(bitgarth_db_unit_only)))]
pub(crate) use labels::canonicalize_label;

// records — server-only
#[cfg(feature = "server")]
pub(crate) use records::{
    AccountWithHdKeys, DigitalAssetAddressRecord, HdKeyRecord, WalletAccessorSummary,
    WalletSummary, WalletWithDetails,
};

// manual_assets — items used by both client and server
pub(crate) use manual_assets::{
    AddManualAssetBalanceAssertionRequest, AddManualAssetBalanceAssertionResponse,
    DeleteManualAssetBalanceAssertionRequest, ManualAssetAccountTransactionsResponse,
    ManualAssetBalanceAssertionId, ManualAssetBalanceAssertionRowResponse,
    ManualAssetBalanceAssertionTableResponse, RawManualAssetAssertionNote, RawManualAssetBalance,
    UpdateManualAssetBalanceAssertionRequest,
};

#[cfg(any(feature = "server", test))]
pub(crate) use manual_assets::ManualAssetPrecisionStatus;

// manual_assets — server-only items
#[cfg(feature = "server")]
pub(crate) use manual_assets::{
    ValidatedManualAssetAssertionNote, ValidatedManualAssetBalanceLiteral,
};

#[cfg(any(feature = "server", test))]
pub(crate) use manual_assets::rescale_manual_asset_amount;

#[cfg(any(feature = "server", test))]
pub(crate) use manual_migration::{
    CoinGeckoManualMigrationAccount, CoinGeckoManualMigrationCandidate,
    CoinGeckoManualMigrationPlan, plan_coingecko_manual_migration,
};

// bitcoin — items used by both client and server
pub(crate) use bitcoin::RawBtcAddress;

#[cfg(feature = "server")]
pub(crate) use bitcoin::BtcAddress;

// requests — items used by both client and server
pub(crate) use requests::{
    AccountAddressRowResponse, AccountTransactionRowResponse, AccountTransactionTableResponse,
    AddBtcAddressRequest, AddBtcAddressResponse, AddEthAddressRequest, AddEthAddressResponse,
    AddManualAssetAccountAssetRequest, AddManualAssetAccountRequest, AddManualAssetAccountResponse,
    AddXpubRequest, CoinGeckoManualAssetPrecisionSourceRequest,
    CoinGeckoManualAssetSnapshotRequest, DeleteAccountRequest, DeleteAccountsChoice,
    DeleteWalletRequest, GetAccountAddressesRequest, GetAccountAddressesResponse,
    GetAccountTransactionsResponse, GetWalletByFingerprintRequest, LinkTrezorOutcome,
    LinkTrezorRequest, LinkTrezorResponse, ManualAssetCatalogTotalResponse,
    ManualAssetDiscoveryDetailRequest, ManualAssetDiscoveryDetailResponse,
    ManualAssetDiscoveryPlatformRow, ManualAssetDiscoveryPriceRequest,
    ManualAssetDiscoveryPriceResponse, ManualAssetInstanceSearchRow, ManualAssetSearchSource,
    MoveAccountRequest, MoveAccountResponse, MoveDestination, RawTransactionFilters,
    SearchManualAssetInstancesRequest, SearchManualAssetInstancesResponse,
    SelectAccountSyncSlotRequest, TransactionsEmptyHint, TrezorAccountLinkRequest,
    UpdateAccountLabelRequest, UpdateWalletLabelRequest, ValidateXpubRequest,
    WalletAccountHistoryResponse,
};

// requests — server-only items
#[cfg(feature = "server")]
pub(crate) use requests::GetAccountTransactionsRequest;

// validation — server-only items
#[cfg(feature = "server")]
pub(crate) use validation::{
    AddManualAssetAccountValidationError, TransactionFilters, ValidatedAddManualAssetAccountAsset,
    ValidatedAddManualAssetAccountRequest, ValidatedAddManualAssetBalanceAssertionRequest,
    ValidatedCoinGeckoManualAssetPrecisionSource, ValidatedCoinGeckoManualAssetSnapshot,
    ValidatedLinkTrezorRequest, ValidatedMoveDestination,
    ValidatedUpdateManualAssetBalanceAssertionRequest, hash_device_id,
    validate_link_trezor_request,
};

#[cfg(feature = "server")]
const _: () = {
    let _ = std::mem::size_of::<CoinGeckoManualMigrationAccount>();
    let _ = std::mem::size_of::<CoinGeckoManualMigrationCandidate>();
    let _ = std::mem::size_of::<CoinGeckoManualMigrationPlan>();
    let _ = plan_coingecko_manual_migration
        as fn(
            &CoinGeckoManualMigrationAccount,
            &CoinGeckoManualMigrationCandidate,
        ) -> Option<CoinGeckoManualMigrationPlan>;
    let _ = std::mem::size_of::<ValidatedCoinGeckoManualAssetPrecisionSource>();
};

// display — items used by client account selection flows
#[cfg(any(target_arch = "wasm32", feature = "desktop", test))]
pub(crate) use display::suggest_next_accounts;

// display — server-only items
#[cfg(feature = "server")]
pub(crate) use display::{
    display_account_label, display_wallet_label, generate_move_account_label,
    generate_unique_account_label, generate_unique_custom_account_label,
};
