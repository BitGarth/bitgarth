#![cfg(all(test, feature = "server"))]

use super::balance_projection::{ProjectedAssetBalance, ProjectedBalanceStatus};
use super::conversions::*;
use super::helpers::*;
use super::types::*;
use crate::account_limits::{AccountActivationState, ClassifiedAccount, SupportedAccountKind};
use crate::amounts::UnsignedAmount;
use crate::asset_capabilities::AssetId;
use crate::asset_capabilities::unsynced::{CoingeckoAssetId, UnsyncedNetworkId};
use crate::balance_reliability::{BalanceProvisionalReason, BalanceReliability};
use crate::db::{AccountSyncSlotRecord, ManualAssetAccountRow};
use crate::payments::types::EntitlementTier;
use crate::tasks::automatic_sync::AutomaticSyncAddTarget;
use crate::tasks::{JobId, JobKey, TriggerParams, TriggerSource, UserTransactionMonitorParams};
use crate::transactions::TransactionSyncScope;
use crate::transactions::{AddressBalanceSummary, NativeBalanceState};
use crate::wallets::{
    ACCOUNT_LABEL_MAX_LENGTH, AccountIndex, AccountKind, AccountWithHdKeys, AddressScheme,
    AddressSourceType, DerivationPath, DigitalAssetAccountId, DigitalAssetAddressRecord, HdKeyId,
    HdKeyRecord, IdentitySource, KeyRole, KeySource, Label, ManualAssetDisplayScale, Network,
    SyncedAssetId, ValidatedExtendedPubkey, ValidatedManualAssetUnitCode, WALLET_LABEL_MAX_LENGTH,
    WalletId, WalletSummary, WalletWithDetails,
};
use bitcoin::Network as BitcoinNetwork;
use bitcoin::bip32::{ChildNumber, DerivationPath as BitcoinDerivationPath, Xpriv, Xpub};
use bitcoin::secp256k1::Secp256k1;
use std::collections::{HashMap, HashSet};

fn test_balance(asset_id: SyncedAssetId, confirmed: u128, _pending: u128) -> AddressBalanceSummary {
    AddressBalanceSummary::known(asset_id, UnsignedAmount::from_u128(confirmed))
}

fn known_balance_amount(view: &WalletBalanceView) -> &BalanceAmountView {
    match &view.balance_state {
        AccountBalanceStateView::Known { amount } => amount,
        AccountBalanceStateView::Unknown => panic!("expected known balance"),
    }
}

fn known_aggregate_balance_amount(view: &WalletAggregateBalanceView) -> &BalanceAmountView {
    match &view.balance_state {
        AccountBalanceStateView::Known { amount } => amount,
        AccountBalanceStateView::Unknown => panic!("expected known balance"),
    }
}

fn test_projected_native_balance(
    asset_id: SyncedAssetId,
    amount: Option<UnsignedAmount>,
) -> ProjectedAssetBalance {
    let instance = crate::asset_capabilities::asset_instance(
        &crate::asset_capabilities::synced_asset_instance(
            crate::asset_capabilities::synced_asset_instance_id(asset_id),
        )
        .asset_instance_id,
    )
    .expect("synced asset instance must resolve");
    ProjectedAssetBalance {
        asset_id: instance.id.asset_id.as_str().to_string(),
        network_id: crate::asset_capabilities::network_slug(instance.id.network_id).to_string(),
        unit: instance.unit_code.as_str().to_string(),
        amount,
        decimal_precision: instance.decimal_precision,
        status: if amount.is_some() {
            ProjectedBalanceStatus::Final
        } else {
            ProjectedBalanceStatus::Unknown
        },
        reasons: Vec::new(),
    }
}

fn test_rendered_entry(
    asset_id: SyncedAssetId,
    network: Network,
    linked_at: &str,
    confirmed: u128,
    pending: u128,
) -> RenderedAccountBalance {
    RenderedAccountBalance {
        asset_id,
        network,
        linked_at: linked_at.parse().expect("valid datetime"),
        balance: test_balance(asset_id, confirmed, pending),
        balance_reliability: BalanceReliability::finalized(),
    }
}

fn test_unknown_rendered_entry(
    asset_id: SyncedAssetId,
    network: Network,
) -> RenderedAccountBalance {
    RenderedAccountBalance {
        asset_id,
        network,
        linked_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
        balance: AddressBalanceSummary {
            asset_id,
            confirmed: NativeBalanceState::Unknown,
        },
        balance_reliability: BalanceReliability::finalized(),
    }
}

fn test_wallet_label(value: &str) -> Label {
    Label::parse_with_limit(value, WALLET_LABEL_MAX_LENGTH).expect("valid wallet label")
}

fn test_account_label(value: &str) -> Label {
    Label::parse_with_limit(value, ACCOUNT_LABEL_MAX_LENGTH).expect("valid account label")
}

fn deterministic_test_xpub(account: u32) -> String {
    let secp = Secp256k1::new();
    let mut seed = [0_u8; 32];
    seed[0..4].copy_from_slice(&account.to_be_bytes());

    let master = Xpriv::new_master(BitcoinNetwork::Bitcoin, &seed)
        .expect("deterministic test seed should produce a valid Xpriv");
    let path = BitcoinDerivationPath::from(vec![
        ChildNumber::Hardened { index: 84 },
        ChildNumber::Hardened { index: 0 },
        ChildNumber::Hardened { index: account },
    ]);
    let account_xpriv = master
        .derive_priv(&secp, &path)
        .expect("deterministic account derivation should succeed");

    Xpub::from_priv(&secp, &account_xpriv).to_string()
}

fn test_hd_key(address_scheme: AddressScheme) -> HdKeyRecord {
    let account_index = AccountIndex::new(0).expect("valid account index");
    let xpub = deterministic_test_xpub(0);

    HdKeyRecord {
        id: HdKeyId::new(),
        key_role: KeyRole::Primary,
        key_source: KeySource::UserProvided,
        verified_by_accessor_id: None,
        address_scheme,
        extended_pubkey: ValidatedExtendedPubkey::parse(address_scheme, &xpub)
            .expect("test xpub should validate"),
        derivation_path: DerivationPath::bitcoin_for_address_scheme(account_index, address_scheme),
        created_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
        updated_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
    }
}

fn test_address(
    address: &str,
    address_scheme: AddressScheme,
    derivation_change: u32,
    derivation_index: u32,
) -> DigitalAssetAddressRecord {
    DigitalAssetAddressRecord {
        id: crate::wallets::DigitalAssetAddressId::new(),
        asset_id: SyncedAssetId::Bitcoin,
        network: Network::Mainnet,
        address: address.to_string(),
        address_scheme,
        derivation_change: Some(derivation_change),
        derivation_index: Some(derivation_index),
        source_type: AddressSourceType::Derived,
        created_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
        updated_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
    }
}

fn classification(
    account_id: impl Into<crate::wallets::WalletAccountId>,
    kind: SupportedAccountKind,
    state: AccountActivationState,
) -> ClassifiedAccount {
    ClassifiedAccount {
        account_id: account_id.into(),
        kind,
        state,
    }
}

#[test]
fn created_active_account_state_has_no_limit_notice() {
    let account_id = crate::wallets::WalletAccountId::new();

    let view = super::handlers_write::created_account_state_view(
        account_id,
        AccountActivationState::Active,
        3,
    );

    assert_eq!(view.account_id, account_id);
    assert_eq!(view.account_state, AccountStateView::Active);
    assert_eq!(view.account_limit_notice, None);
}

#[test]
fn created_inactive_account_state_has_limit_notice() {
    let account_id = crate::wallets::WalletAccountId::new();

    let view = super::handlers_write::created_account_state_view(
        account_id,
        AccountActivationState::Inactive,
        3,
    );

    assert_eq!(view.account_id, account_id);
    assert_eq!(view.account_state, AccountStateView::Inactive);
    assert_eq!(
        view.account_limit_notice,
        Some(AccountLimitNoticeView {
            message: "You have reached your limit of 3 accounts. This account will be inactive until you upgrade.".to_string(),
            active_account_limit: 3,
        })
    );
}

fn active_native(account_id: DigitalAssetAccountId) -> ClassifiedAccount {
    classification(
        account_id,
        SupportedAccountKind::Native,
        AccountActivationState::Active,
    )
}

fn inactive_native(account_id: DigitalAssetAccountId) -> ClassifiedAccount {
    classification(
        account_id,
        SupportedAccountKind::Native,
        AccountActivationState::Inactive,
    )
}

fn inactive_manual(account_id: crate::wallets::WalletAccountId) -> ClassifiedAccount {
    classification(
        account_id,
        SupportedAccountKind::ManualAsset,
        AccountActivationState::Inactive,
    )
}

#[test]
fn convert_wallet_to_view_keeps_current_bitcoin_balance_final_when_history_is_limited() {
    let account_id = DigitalAssetAccountId::new();
    let wallet = WalletWithDetails {
        wallet: WalletSummary {
            id: WalletId::new(),
            master_fingerprint: None,
            identity_source: IdentitySource::UserProvided,
            verified_at: None,
            label: test_wallet_label("BTC Wallet"),
            created_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
            updated_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
        },
        accessors: Vec::new(),
        accounts: vec![AccountWithHdKeys {
            id: account_id,
            asset_id: SyncedAssetId::Bitcoin,
            network: Network::Mainnet,
            account_model: crate::wallets::AccountModel::Utxo,
            account_kind: AccountKind::HdPubkey,
            label: test_account_label("BTC Account"),
            hd_keys: vec![
                test_hd_key(AddressScheme::Legacy),
                test_hd_key(AddressScheme::NativeSegwit),
            ],
            addresses: vec![test_address(
                "bc1qaccount0receive0",
                AddressScheme::NativeSegwit,
                0,
                0,
            )],
            created_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
            updated_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
        }],
    };

    let mut account_balances = HashMap::new();
    account_balances.insert(
        account_id,
        test_balance(SyncedAssetId::Bitcoin, 50_000, 5_000),
    );
    let projected_balances = [test_projected_native_balance(
        SyncedAssetId::Bitcoin,
        Some(UnsignedAmount::from_u128(50_000)),
    )];

    let view = convert_wallet_to_view(
        wallet,
        &projected_balances,
        &WalletAccountData {
            manual_asset_accounts: &[],
            address_balances: &HashMap::new(),
            account_balances: &account_balances,
            account_balance_reliabilities: &HashMap::from([(
                account_id,
                BalanceReliability::Provisional {
                    reasons: vec![BalanceProvisionalReason::HistoricalCoverageLimited],
                },
            )]),
            custom_account_balances: &HashMap::new(),
            account_transactions: None,
            account_tx_counts: &HashMap::new(),
        },
        &NativeAccountManualSyncContext {
            sync_slots: &HashMap::new(),
            active_sync_slot_account_ids: &std::collections::HashSet::new(),
            slot_limit: 2,
            tier: EntitlementTier::Free,
            historical_backfill_enabled: false,
            historical_backfill_transactions_per_account: 0,
            free_balance_unavailable_account_ids: &std::collections::HashSet::new(),
        },
        &[active_native(account_id)],
    )
    .expect("wallet conversion should succeed");

    assert_eq!(view.accounts.len(), 2);
    let native_segwit = view.accounts.iter().find_map(|a| match a {
        AccountView::Native(n) if n.scheme == AddressScheme::NativeSegwit => Some(n),
        _ => None,
    });
    let legacy = view.accounts.iter().find_map(|a| match a {
        AccountView::Native(n) if n.scheme == AddressScheme::Legacy => Some(n),
        _ => None,
    });
    let native_segwit = native_segwit.expect("expected NativeSegwit account");
    let legacy = legacy.expect("expected Legacy account");
    assert_eq!(
        known_balance_amount(&native_segwit.balance).raw_value,
        "50000"
    );
    assert_eq!(
        native_segwit.balance.balance_reliability,
        BalanceReliability::Final
    );
    assert_eq!(known_balance_amount(&legacy.balance).raw_value, "0");
    assert_eq!(view.balances.len(), 1);
    assert_eq!(view.balances[0].asset_id, "bitcoin");
    assert_eq!(
        known_aggregate_balance_amount(&view.balances[0]).raw_value,
        "50000"
    );
    assert_eq!(
        view.balances[0].balance_reliability,
        BalanceReliability::Final
    );
}

#[test]
fn manual_account_view_uses_db_snapshot_metadata() {
    let wallet_id = WalletId::new();
    let account_id = crate::wallets::WalletAccountId::new();
    let wallet = WalletWithDetails {
        wallet: WalletSummary {
            id: wallet_id,
            master_fingerprint: None,
            identity_source: IdentitySource::UserProvided,
            verified_at: None,
            label: test_wallet_label("Manual Wallet"),
            created_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
            updated_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
        },
        accessors: Vec::new(),
        accounts: Vec::new(),
    };
    let manual_account = ManualAssetAccountRow {
        account_id,
        wallet_id,
        label: test_account_label("USDC Algo"),
        asset_id: AssetId::owned("usd-coin".to_string()).expect("valid asset id"),
        network_id: UnsyncedNetworkId::parse("algorand-mainnet").expect("valid network id"),
        unit_code: ValidatedManualAssetUnitCode::parse("USDC").expect("valid unit code"),
        decimal_precision: ManualAssetDisplayScale::try_from(6).expect("valid precision"),
        symbol: None,
        asset_name: "USDC on Algorand".to_string(),
        network_name: "Algorand".to_string(),
        coingecko_id: CoingeckoAssetId::parse("usd-coin").expect("valid coingecko id"),
        asset_source: "bitgarth_catalog".to_string(),
        precision_source: "bitgarth_catalog".to_string(),
        coingecko_platform_id: None,
        provider_platform_asset_ref: None,
        created_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
        updated_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
    };

    let view = convert_wallet_to_view(
        wallet,
        &[],
        &WalletAccountData {
            manual_asset_accounts: &[manual_account],
            address_balances: &HashMap::new(),
            account_balances: &HashMap::new(),
            account_balance_reliabilities: &HashMap::new(),
            custom_account_balances: &HashMap::new(),
            account_transactions: None,
            account_tx_counts: &HashMap::new(),
        },
        &NativeAccountManualSyncContext {
            sync_slots: &HashMap::new(),
            active_sync_slot_account_ids: &HashSet::new(),
            slot_limit: 2,
            tier: EntitlementTier::Free,
            historical_backfill_enabled: false,
            historical_backfill_transactions_per_account: 0,
            free_balance_unavailable_account_ids: &HashSet::new(),
        },
        &[],
    )
    .expect("wallet conversion should succeed");

    let manual_view = view
        .accounts
        .iter()
        .find_map(|account| match account {
            AccountView::Manual(manual) => Some(manual),
            _ => None,
        })
        .expect("manual account view");
    assert_eq!(manual_view.unit_code, "USDC");
    assert_eq!(manual_view.network_name, "Algorand");
    assert_eq!(manual_view.decimal_precision, 6);
}

#[test]
fn inactive_native_accounts_are_visible_but_not_manually_syncable() {
    let account_id = DigitalAssetAccountId::new();
    let wallet = WalletWithDetails {
        wallet: WalletSummary {
            id: WalletId::new(),
            master_fingerprint: None,
            identity_source: IdentitySource::UserProvided,
            verified_at: None,
            label: test_wallet_label("Inactive Native Wallet"),
            created_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
            updated_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
        },
        accessors: Vec::new(),
        accounts: vec![AccountWithHdKeys {
            id: account_id,
            asset_id: SyncedAssetId::Bitcoin,
            network: Network::Mainnet,
            account_model: crate::wallets::AccountModel::Utxo,
            account_kind: AccountKind::HdPubkey,
            label: test_account_label("Inactive BTC"),
            hd_keys: vec![test_hd_key(AddressScheme::NativeSegwit)],
            addresses: vec![test_address(
                "bc1qinactiveaccount0",
                AddressScheme::NativeSegwit,
                0,
                0,
            )],
            created_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
            updated_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
        }],
    };

    let view = convert_wallet_to_view(
        wallet,
        &[],
        &WalletAccountData {
            manual_asset_accounts: &[],
            address_balances: &HashMap::new(),
            account_balances: &HashMap::new(),
            account_balance_reliabilities: &HashMap::new(),
            custom_account_balances: &HashMap::new(),
            account_transactions: None,
            account_tx_counts: &HashMap::new(),
        },
        &NativeAccountManualSyncContext {
            sync_slots: &HashMap::new(),
            active_sync_slot_account_ids: &HashSet::new(),
            slot_limit: 2,
            tier: EntitlementTier::Free,
            historical_backfill_enabled: false,
            historical_backfill_transactions_per_account: 0,
            free_balance_unavailable_account_ids: &HashSet::new(),
        },
        &[inactive_native(account_id)],
    )
    .expect("wallet conversion should succeed");

    let native_view = view
        .accounts
        .iter()
        .find_map(|account| match account {
            AccountView::Native(native) => Some(native),
            _ => None,
        })
        .expect("inactive native account should remain visible");
    assert_eq!(native_view.account_state, AccountStateView::Inactive);
    assert_eq!(native_view.manual_sync.mode, ManualSyncMode::Unavailable);
    assert_eq!(
        native_view.manual_sync.slot_effect,
        ManualSyncSlotEffect::NoCapacity
    );
    assert_eq!(
        native_view.manual_sync.disabled_reason,
        Some(ManualSyncDisabledReason::AccountInactive)
    );
}

#[test]
fn unclassified_supported_accounts_fail_closed_as_inactive() {
    let account_id = DigitalAssetAccountId::new();
    let wallet = WalletWithDetails {
        wallet: WalletSummary {
            id: WalletId::new(),
            master_fingerprint: None,
            identity_source: IdentitySource::UserProvided,
            verified_at: None,
            label: test_wallet_label("Unclassified Native Wallet"),
            created_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
            updated_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
        },
        accessors: Vec::new(),
        accounts: vec![AccountWithHdKeys {
            id: account_id,
            asset_id: SyncedAssetId::Bitcoin,
            network: Network::Mainnet,
            account_model: crate::wallets::AccountModel::Utxo,
            account_kind: AccountKind::HdPubkey,
            label: test_account_label("Unclassified BTC"),
            hd_keys: vec![test_hd_key(AddressScheme::NativeSegwit)],
            addresses: vec![test_address(
                "bc1qunclassifiedaccount",
                AddressScheme::NativeSegwit,
                0,
                0,
            )],
            created_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
            updated_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
        }],
    };

    let view = convert_wallet_to_view(
        wallet,
        &[],
        &WalletAccountData {
            manual_asset_accounts: &[],
            address_balances: &HashMap::new(),
            account_balances: &HashMap::new(),
            account_balance_reliabilities: &HashMap::new(),
            custom_account_balances: &HashMap::new(),
            account_transactions: None,
            account_tx_counts: &HashMap::new(),
        },
        &NativeAccountManualSyncContext {
            sync_slots: &HashMap::new(),
            active_sync_slot_account_ids: &HashSet::new(),
            slot_limit: 2,
            tier: EntitlementTier::Free,
            historical_backfill_enabled: false,
            historical_backfill_transactions_per_account: 0,
            free_balance_unavailable_account_ids: &HashSet::new(),
        },
        &[],
    )
    .expect("wallet conversion should succeed");

    let native_view = view
        .accounts
        .iter()
        .find_map(|account| match account {
            AccountView::Native(native) => Some(native),
            _ => None,
        })
        .expect("native account should remain visible");
    assert_eq!(native_view.account_state, AccountStateView::Inactive);
    assert_eq!(
        native_view.manual_sync.disabled_reason,
        Some(ManualSyncDisabledReason::AccountInactive)
    );
}

#[test]
fn inactive_manual_accounts_are_visible() {
    let wallet_id = WalletId::new();
    let account_id = crate::wallets::WalletAccountId::new();
    let wallet = WalletWithDetails {
        wallet: WalletSummary {
            id: wallet_id,
            master_fingerprint: None,
            identity_source: IdentitySource::UserProvided,
            verified_at: None,
            label: test_wallet_label("Inactive Manual Wallet"),
            created_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
            updated_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
        },
        accessors: Vec::new(),
        accounts: Vec::new(),
    };
    let manual_account = ManualAssetAccountRow {
        account_id,
        wallet_id,
        label: test_account_label("Inactive USDC"),
        asset_id: AssetId::owned("usd-coin".to_string()).expect("valid asset id"),
        network_id: UnsyncedNetworkId::parse("ethereum-mainnet").expect("valid network id"),
        unit_code: ValidatedManualAssetUnitCode::parse("USDC").expect("valid unit code"),
        decimal_precision: ManualAssetDisplayScale::try_from(6).expect("valid precision"),
        symbol: None,
        asset_name: "USDC".to_string(),
        network_name: "Ethereum".to_string(),
        coingecko_id: CoingeckoAssetId::parse("usd-coin").expect("valid coingecko id"),
        asset_source: "bitgarth_catalog".to_string(),
        precision_source: "bitgarth_catalog".to_string(),
        coingecko_platform_id: None,
        provider_platform_asset_ref: None,
        created_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
        updated_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
    };

    let view = convert_wallet_to_view(
        wallet,
        &[],
        &WalletAccountData {
            manual_asset_accounts: &[manual_account],
            address_balances: &HashMap::new(),
            account_balances: &HashMap::new(),
            account_balance_reliabilities: &HashMap::new(),
            custom_account_balances: &HashMap::new(),
            account_transactions: None,
            account_tx_counts: &HashMap::new(),
        },
        &NativeAccountManualSyncContext {
            sync_slots: &HashMap::new(),
            active_sync_slot_account_ids: &HashSet::new(),
            slot_limit: 2,
            tier: EntitlementTier::Free,
            historical_backfill_enabled: false,
            historical_backfill_transactions_per_account: 0,
            free_balance_unavailable_account_ids: &HashSet::new(),
        },
        &[inactive_manual(account_id)],
    )
    .expect("wallet conversion should succeed");

    let manual_view = view
        .accounts
        .iter()
        .find_map(|account| match account {
            AccountView::Manual(manual) => Some(manual),
            _ => None,
        })
        .expect("inactive manual account should remain visible");
    assert_eq!(manual_view.account_state, AccountStateView::Inactive);
}

#[test]
fn derive_wallet_balances_groups_duplicates_and_sorts_by_link_time() {
    let rendered = vec![
        test_rendered_entry(
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            "2026-02-16T10:02:00Z",
            100_000_000,
            0,
        ),
        test_rendered_entry(
            SyncedAssetId::Ethereum,
            Network::Mainnet,
            "2026-02-16T10:01:00Z",
            1_000_000_000_000_000_000,
            0,
        ),
        test_rendered_entry(
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            "2026-02-16T10:00:00Z",
            23_400_000,
            0,
        ),
    ];

    let balances = derive_wallet_balances(&rendered).expect("derivation should succeed");
    assert_eq!(balances.len(), 2);

    assert_eq!(balances[0].asset_id, "bitcoin");
    assert_eq!(balances[0].network_id, "bitcoin-mainnet");
    assert_eq!(
        known_aggregate_balance_amount(&balances[0]).raw_value,
        "123400000"
    );
    assert_eq!(
        known_aggregate_balance_amount(&balances[0]).formatted_value,
        "1.234"
    );

    assert_eq!(balances[1].asset_id, "ethereum");
    assert_eq!(balances[1].network_id, "ethereum-mainnet");
    assert_eq!(
        known_aggregate_balance_amount(&balances[1]).raw_value,
        "1000000000000000000"
    );
    assert_eq!(
        known_aggregate_balance_amount(&balances[1]).formatted_value,
        "1"
    );
}

#[test]
fn wallet_unknown_balance_survives_aggregation_and_serialization() {
    let balances = derive_wallet_balances(&[
        test_rendered_entry(
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            "2026-02-16T10:00:00Z",
            50_000,
            0,
        ),
        test_unknown_rendered_entry(SyncedAssetId::Bitcoin, Network::Mainnet),
    ])
    .expect("derivation should succeed");

    assert_eq!(balances.len(), 1);
    assert_eq!(balances[0].balance_state, AccountBalanceStateView::Unknown);
    assert_eq!(balances[0].current_value, None);
    assert_eq!(
        serde_json::to_value(&balances[0].balance_state).expect("state should serialize"),
        serde_json::json!({"kind": "unknown"})
    );

    let mut wallets = vec![WalletView {
        id: WalletId::new(),
        label: "Unknown".to_string(),
        master_fingerprint: None,
        logical_account_count: 0,
        has_accessors: false,
        balances,
        accounts: Vec::new(),
        value_summary: None,
    }];
    crate::services::current_prices::apply_wallet_valuations_from_prices_for_test(
        &mut wallets,
        &[],
        &HashMap::from([(
            "bitcoin".to_string(),
            "50000"
                .parse::<rust_decimal::Decimal>()
                .expect("valid price"),
        )]),
        crate::models::CurrencyCode::from_code("USD").expect("valid currency"),
    );
    assert_eq!(wallets[0].balances[0].current_value, None);
}

#[test]
fn derive_wallet_balances_unknown_before_overflow_is_unknown() {
    let balances = derive_wallet_balances(&[
        test_unknown_rendered_entry(SyncedAssetId::Bitcoin, Network::Mainnet),
        test_rendered_entry(
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            "2026-02-16T10:01:00Z",
            u128::MAX,
            0,
        ),
        test_rendered_entry(
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            "2026-02-16T10:02:00Z",
            1,
            0,
        ),
    ])
    .expect("unknown should dominate overflow");

    assert_eq!(balances[0].balance_state, AccountBalanceStateView::Unknown);
}

#[test]
fn derive_wallet_balances_unknown_after_overflow_is_unknown() {
    let balances = derive_wallet_balances(&[
        test_rendered_entry(
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            "2026-02-16T10:00:00Z",
            u128::MAX,
            0,
        ),
        test_rendered_entry(
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            "2026-02-16T10:01:00Z",
            1,
            0,
        ),
        test_unknown_rendered_entry(SyncedAssetId::Bitcoin, Network::Mainnet),
    ])
    .expect("unknown should dominate overflow");

    assert_eq!(balances[0].balance_state, AccountBalanceStateView::Unknown);
}

#[test]
fn derive_wallet_balances_all_known_overflow_is_error() {
    let result = derive_wallet_balances(&[
        test_rendered_entry(
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            "2026-02-16T10:00:00Z",
            u128::MAX,
            0,
        ),
        test_rendered_entry(
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            "2026-02-16T10:01:00Z",
            1,
            0,
        ),
    ]);

    assert!(result.is_err());
}

#[test]
fn derive_wallet_balances_canonical_zero_before_unknown_is_error() {
    let result = derive_wallet_balances(&[
        RenderedAccountBalance {
            asset_id: SyncedAssetId::Bitcoin,
            network: Network::Mainnet,
            linked_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
            balance: AddressBalanceSummary {
                asset_id: SyncedAssetId::Bitcoin,
                confirmed: NativeBalanceState::CanonicalZero,
            },
            balance_reliability: BalanceReliability::finalized(),
        },
        test_unknown_rendered_entry(SyncedAssetId::Bitcoin, Network::Mainnet),
    ]);

    assert!(result.is_err());
}

#[test]
fn derive_wallet_balances_unknown_before_canonical_zero_is_error() {
    let result = derive_wallet_balances(&[
        test_unknown_rendered_entry(SyncedAssetId::Bitcoin, Network::Mainnet),
        RenderedAccountBalance {
            asset_id: SyncedAssetId::Bitcoin,
            network: Network::Mainnet,
            linked_at: "2026-02-16T10:01:00Z".parse().expect("valid datetime"),
            balance: AddressBalanceSummary {
                asset_id: SyncedAssetId::Bitcoin,
                confirmed: NativeBalanceState::CanonicalZero,
            },
            balance_reliability: BalanceReliability::finalized(),
        },
    ]);

    assert!(result.is_err());
}

#[test]
fn derive_wallet_balances_uses_canonical_key_tie_breaker() {
    let rendered = vec![
        test_rendered_entry(
            SyncedAssetId::Ethereum,
            Network::Mainnet,
            "2026-02-16T10:00:00Z",
            1,
            0,
        ),
        test_rendered_entry(
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            "2026-02-16T10:00:00Z",
            1,
            0,
        ),
    ];

    let balances = derive_wallet_balances(&rendered).expect("derivation should succeed");
    assert_eq!(balances.len(), 2);
    assert_eq!(balances[0].asset_id, "bitcoin");
    assert_eq!(balances[1].asset_id, "ethereum");
}

#[test]
fn derive_wallet_balances_includes_zero_balances() {
    let rendered = vec![
        RenderedAccountBalance {
            asset_id: SyncedAssetId::Bitcoin,
            network: Network::Mainnet,
            linked_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
            balance: test_balance(SyncedAssetId::Bitcoin, 0, 0),
            balance_reliability: BalanceReliability::finalized(),
        },
        test_rendered_entry(
            SyncedAssetId::Ethereum,
            Network::Mainnet,
            "2026-02-16T10:00:00Z",
            1_000_000_000_000_000_000,
            0,
        ),
    ];

    let balances = derive_wallet_balances(&rendered).expect("derivation should succeed");
    assert_eq!(balances.len(), 2);
    assert_eq!(balances[0].asset_id, "bitcoin");
    assert_eq!(balances[1].asset_id, "ethereum");
}

#[test]
fn automatic_add_trigger_request_uses_auto_add_source_and_narrow_address_scope() {
    let user_id = crate::models::UserId::new();
    let address_id = crate::wallets::DigitalAssetAddressId::new();

    let request = automatic_add_trigger_request(
        user_id,
        AutomaticSyncAddTarget::BitcoinAddress { address_id },
    );

    assert_eq!(
        request.key,
        JobKey::User {
            job_id: JobId::UserTransactionMonitor,
            user_id,
        }
    );
    assert_eq!(request.source, TriggerSource::AutoAdd);
    assert_eq!(
        request.params,
        TriggerParams::UserTransactionMonitor(UserTransactionMonitorParams {
            run_id: match request.params {
                TriggerParams::UserTransactionMonitor(params) => params.run_id,
                TriggerParams::SessionCleanup(_)
                | TriggerParams::TraceCleanup(_)
                | TriggerParams::InactiveUserCleanup(_)
                | TriggerParams::PriceHistoryReconciliation(_) => {
                    panic!("expected sync params")
                }
            },
            scope: TransactionSyncScope::Address { address_id },
        })
    );
}

#[test]
fn automatic_add_trigger_request_uses_user_scope_for_multi_account_imports() {
    let user_id = crate::models::UserId::new();

    let request =
        automatic_add_trigger_request(user_id, AutomaticSyncAddTarget::MultiAccountImport);

    assert_eq!(request.source, TriggerSource::AutoAdd);
    match request.params {
        TriggerParams::UserTransactionMonitor(params) => {
            assert_eq!(params.scope, TransactionSyncScope::User);
        }
        TriggerParams::SessionCleanup(_)
        | TriggerParams::TraceCleanup(_)
        | TriggerParams::InactiveUserCleanup(_)
        | TriggerParams::PriceHistoryReconciliation(_) => {
            panic!("expected sync params")
        }
    }
}

#[test]
fn automatic_add_trigger_request_uses_auto_add_source_and_narrow_account_scope() {
    let user_id = crate::models::UserId::new();
    let account_id = DigitalAssetAccountId::new();

    let request =
        automatic_add_trigger_request(user_id, AutomaticSyncAddTarget::Account { account_id });

    assert_eq!(
        request.key,
        JobKey::User {
            job_id: JobId::UserTransactionMonitor,
            user_id,
        }
    );
    assert_eq!(request.source, TriggerSource::AutoAdd);
    match request.params {
        TriggerParams::UserTransactionMonitor(params) => {
            assert_eq!(params.scope, TransactionSyncScope::Account { account_id });
        }
        TriggerParams::SessionCleanup(_)
        | TriggerParams::TraceCleanup(_)
        | TriggerParams::InactiveUserCleanup(_)
        | TriggerParams::PriceHistoryReconciliation(_) => {
            panic!("expected sync params")
        }
    }
}

#[test]
fn derive_wallet_balances_empty_input_returns_empty() {
    let balances = derive_wallet_balances(&[]).expect("derivation should succeed");
    assert!(balances.is_empty());
}

#[test]
fn derive_wallet_balances_filters_non_current_balance_reasons() {
    let rendered = vec![
        RenderedAccountBalance {
            asset_id: SyncedAssetId::Bitcoin,
            network: Network::Mainnet,
            linked_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
            balance: test_balance(SyncedAssetId::Bitcoin, 1, 0),
            balance_reliability: BalanceReliability::Provisional {
                reasons: vec![BalanceProvisionalReason::FirstSuccessfulSyncPending],
            },
        },
        RenderedAccountBalance {
            asset_id: SyncedAssetId::Bitcoin,
            network: Network::Mainnet,
            linked_at: "2026-02-16T10:01:00Z".parse().expect("valid datetime"),
            balance: test_balance(SyncedAssetId::Bitcoin, 0, 2),
            balance_reliability: BalanceReliability::Provisional {
                reasons: vec![BalanceProvisionalReason::PendingLedgerState],
            },
        },
    ];

    let balances = derive_wallet_balances(&rendered).expect("derivation should succeed");
    assert_eq!(
        balances[0].balance_reliability,
        BalanceReliability::Provisional {
            reasons: vec![BalanceProvisionalReason::FirstSuccessfulSyncPending],
        }
    );
}

// --- Account view sorting tests ---

fn native_account_view(asset: SyncedAssetId, label: &str) -> AccountView {
    let instance = crate::asset_capabilities::asset_instance(
        &crate::asset_capabilities::synced_asset_instance(
            crate::asset_capabilities::synced_asset_instance_id(asset),
        )
        .asset_instance_id,
    )
    .expect("synced asset instance must resolve");
    AccountView::Native(Box::new(NativeAccountView {
        account_id: crate::wallets::WalletAccountId::new(),
        native_account_id: DigitalAssetAccountId::new(),
        account_number: 1,
        account_state: AccountStateView::Active,
        asset,
        scheme: AddressScheme::NativeSegwit,
        label: label.to_string(),
        derivation_path: None,
        account_reference_kind: AccountReferenceKind::ExtendedPubkey,
        account_reference: "xpub...".to_string(),
        balance: WalletBalanceView {
            asset_id: asset,
            context: WalletBalanceContextView {
                network: Network::Mainnet,
            },
            unit_code: instance.unit_code.as_str().to_string(),
            symbol: instance.symbol.as_ref().map(|s| s.to_string()),
            balance_reliability: BalanceReliability::finalized(),
            balance_state: AccountBalanceStateView::Known {
                amount: BalanceAmountView {
                    raw_value: "0".to_string(),
                    formatted_value: "".to_string(),
                },
            },
            current_value: None,
        },
        transaction_counts: AccountTransactionCountsView::default(),
        has_derived_addresses: false,
        sync_slot: NativeAccountSyncSlotView {
            selected: false,
            active: false,
            can_select: true,
            limit: 2,
            selected_at: None,
            selected_under_tier: None,
        },
        manual_sync: NativeAccountManualSyncView {
            mode: ManualSyncMode::BalanceRefresh,
            slot_effect: ManualSyncSlotEffect::WillSelectAvailableSlot,
            disabled_reason: None,
            used_slots: 0,
            slot_limit: 2,
            next_tier_display_name: Some("Basic".to_string()),
        },
        addresses: AddressesView::default(),
        transactions: vec![],
    }))
}

#[test]
fn price_valuation_populates_native_account_row_current_value() {
    let currency = crate::models::CurrencyCode::from_code("USD").unwrap();
    let prices = HashMap::from([(
        "bitcoin".to_string(),
        "50000".parse::<rust_decimal::Decimal>().unwrap(),
    )]);

    let mut account = native_account_view(SyncedAssetId::Bitcoin, "Bitcoin Account 1");
    let AccountView::Native(native) = &mut account else {
        panic!("expected native account");
    };
    native.balance.balance_state = AccountBalanceStateView::Known {
        amount: BalanceAmountView {
            raw_value: "100000000".to_string(),
            formatted_value: "1".to_string(),
        },
    };
    let wallet_balance = projected_wallet_balance_view(
        &test_projected_native_balance(
            SyncedAssetId::Bitcoin,
            Some(UnsignedAmount::from_u128(100_000_000)),
        ),
        Some("₿".to_string()),
    );
    let mut wallets = vec![WalletView {
        id: WalletId::new(),
        label: "Test".to_string(),
        master_fingerprint: None,
        logical_account_count: 1,
        has_accessors: false,
        balances: vec![wallet_balance],
        accounts: vec![account],
        value_summary: None,
    }];

    crate::services::current_prices::apply_wallet_valuations_from_prices_for_test(
        &mut wallets,
        &[],
        &prices,
        currency,
    );

    let AccountView::Native(native) = &wallets[0].accounts[0] else {
        panic!("expected native account");
    };
    assert_eq!(
        native
            .balance
            .current_value
            .as_ref()
            .map(|value| value.converted_value.as_str()),
        Some("50000")
    );
}

#[test]
fn current_price_known_and_unknown_same_asset_has_no_partial_wallet_value() {
    let currency = crate::models::CurrencyCode::from_code("USD").unwrap();
    let prices = HashMap::from([(
        "bitcoin".to_string(),
        "50000".parse::<rust_decimal::Decimal>().unwrap(),
    )]);
    let mut known = native_account_view(SyncedAssetId::Bitcoin, "Known Bitcoin");
    let mut unknown = native_account_view(SyncedAssetId::Bitcoin, "Unknown Bitcoin");
    let AccountView::Native(known_native) = &mut known else {
        panic!("expected native account");
    };
    known_native.balance.balance_state = AccountBalanceStateView::Known {
        amount: BalanceAmountView {
            raw_value: "100000000".to_string(),
            formatted_value: "1".to_string(),
        },
    };
    let AccountView::Native(unknown_native) = &mut unknown else {
        panic!("expected native account");
    };
    unknown_native.balance.balance_state = AccountBalanceStateView::Unknown;
    let balances = derive_wallet_balances(&[
        test_rendered_entry(
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            "2026-02-16T10:00:00Z",
            100_000_000,
            0,
        ),
        test_unknown_rendered_entry(SyncedAssetId::Bitcoin, Network::Mainnet),
    ])
    .expect("wallet balance should aggregate");
    let mut wallets = vec![WalletView {
        id: WalletId::new(),
        label: "Mixed Bitcoin".to_string(),
        master_fingerprint: None,
        logical_account_count: 2,
        has_accessors: false,
        balances,
        accounts: vec![known, unknown],
        value_summary: None,
    }];

    let page_summary =
        crate::services::current_prices::apply_wallet_valuations_from_prices_for_test(
            &mut wallets,
            &[],
            &prices,
            currency,
        );

    assert_eq!(wallets[0].balances[0].current_value, None);
    assert_eq!(wallets[0].value_summary, None);
    assert_eq!(page_summary.priced_total, "0");
}

#[test]
fn current_price_known_ethereum_and_unknown_bitcoin_has_no_partial_wallet_subtotal() {
    let currency = crate::models::CurrencyCode::from_code("USD").unwrap();
    let prices = HashMap::from([
        (
            "bitcoin".to_string(),
            "50000".parse::<rust_decimal::Decimal>().unwrap(),
        ),
        (
            "ethereum".to_string(),
            "2000".parse::<rust_decimal::Decimal>().unwrap(),
        ),
    ]);
    let mut ethereum = native_account_view(SyncedAssetId::Ethereum, "Known Ethereum");
    let mut bitcoin = native_account_view(SyncedAssetId::Bitcoin, "Unknown Bitcoin");
    let AccountView::Native(ethereum_native) = &mut ethereum else {
        panic!("expected native account");
    };
    ethereum_native.balance.balance_state = AccountBalanceStateView::Known {
        amount: BalanceAmountView {
            raw_value: "1000000000000000000".to_string(),
            formatted_value: "1".to_string(),
        },
    };
    let AccountView::Native(bitcoin_native) = &mut bitcoin else {
        panic!("expected native account");
    };
    bitcoin_native.balance.balance_state = AccountBalanceStateView::Unknown;
    let balances = derive_wallet_balances(&[
        test_rendered_entry(
            SyncedAssetId::Ethereum,
            Network::Mainnet,
            "2026-02-16T10:00:00Z",
            1_000_000_000_000_000_000,
            0,
        ),
        test_unknown_rendered_entry(SyncedAssetId::Bitcoin, Network::Mainnet),
    ])
    .expect("wallet balances should aggregate");
    let mut wallets = vec![WalletView {
        id: WalletId::new(),
        label: "Mixed Assets".to_string(),
        master_fingerprint: None,
        logical_account_count: 2,
        has_accessors: false,
        balances,
        accounts: vec![ethereum, bitcoin],
        value_summary: None,
    }];

    let page_summary =
        crate::services::current_prices::apply_wallet_valuations_from_prices_for_test(
            &mut wallets,
            &[],
            &prices,
            currency,
        );

    assert_eq!(wallets[0].value_summary, None);
    assert_eq!(page_summary.priced_total, "0");
}

fn custom_account_view(unit_code: &str, label: &str) -> AccountView {
    AccountView::Custom(CustomAccountView {
        account_id: crate::wallets::WalletAccountId::new(),
        label: label.to_string(),
        unit_code: unit_code.to_string(),
        decimal_precision: 8,
        symbol: None,
        balance_state: AccountBalanceStateView::Unknown,
        current_value: None,
    })
}

fn extract_sort_key(view: &AccountView) -> (&str, &str) {
    match view {
        AccountView::Native(n) => {
            let code = crate::asset_capabilities::asset_instance(
                &crate::asset_capabilities::synced_asset_instance(
                    crate::asset_capabilities::synced_asset_instance_id(n.asset),
                )
                .asset_instance_id,
            )
            .expect("synced asset instance must resolve")
            .unit_code
            .as_str();
            (code, &n.label)
        }
        AccountView::Custom(c) => (&c.unit_code, &c.label),
        AccountView::Manual(m) => (&m.unit_code, &m.label),
    }
}

#[test]
fn sort_account_views_native_before_custom() {
    let mut accounts = [
        custom_account_view("ADA", "Alpha"),
        native_account_view(SyncedAssetId::Bitcoin, "Alpha"),
    ];
    accounts.sort_by(sort_account_views);

    assert!(matches!(&accounts[0], AccountView::Native(_)));
    assert!(matches!(&accounts[1], AccountView::Custom(_)));
}

#[test]
fn sort_account_views_by_unit_code_alphabetically() {
    let mut accounts = [
        native_account_view(SyncedAssetId::Ethereum, "Alpha"),
        native_account_view(SyncedAssetId::Bitcoin, "Alpha"),
    ];
    accounts.sort_by(sort_account_views);

    let codes: Vec<_> = accounts.iter().map(|a| extract_sort_key(a).0).collect();
    assert_eq!(codes, ["BTC", "ETH"]);
}

#[test]
fn sort_account_views_by_label_alphabetically_within_same_unit_code() {
    let mut accounts = [
        native_account_view(SyncedAssetId::Bitcoin, "Zebra"),
        native_account_view(SyncedAssetId::Bitcoin, "Alpha"),
        native_account_view(SyncedAssetId::Bitcoin, "Mango"),
    ];
    accounts.sort_by(sort_account_views);

    let labels: Vec<_> = accounts.iter().map(|a| extract_sort_key(a).1).collect();
    assert_eq!(labels, ["Alpha", "Mango", "Zebra"]);
}

#[test]
fn sort_account_views_active_accounts_before_inactive_accounts() {
    let mut inactive = native_account_view(SyncedAssetId::Bitcoin, "Alpha");
    let AccountView::Native(native) = &mut inactive else {
        panic!("expected native account");
    };
    native.account_state = AccountStateView::Inactive;

    let active = native_account_view(SyncedAssetId::Bitcoin, "Zebra");
    let mut accounts = [inactive, active];
    accounts.sort_by(sort_account_views);

    let labels: Vec<_> = accounts.iter().map(|a| extract_sort_key(a).1).collect();
    assert_eq!(labels, ["Zebra", "Alpha"]);
}

#[test]
fn sort_account_views_custom_by_unit_code_then_label() {
    let mut accounts = [
        custom_account_view("ZZZ", "Zebra"),
        custom_account_view("ADA", "Alpha"),
        custom_account_view("ZZZ", "Alpha"),
    ];
    accounts.sort_by(sort_account_views);

    let keys: Vec<_> = accounts.iter().map(extract_sort_key).collect();
    assert_eq!(
        keys,
        [("ADA", "Alpha"), ("ZZZ", "Alpha"), ("ZZZ", "Zebra"),]
    );
}

#[test]
fn sort_account_views_full_example() {
    let mut accounts = [
        custom_account_view("ZZZ", "Zebra"),
        native_account_view(SyncedAssetId::Ethereum, "Beta"),
        custom_account_view("ADA", "Alpha"),
        native_account_view(SyncedAssetId::Bitcoin, "Zebra"),
        native_account_view(SyncedAssetId::Ethereum, "Alpha"),
        custom_account_view("ZZZ", "Alpha"),
        native_account_view(SyncedAssetId::Bitcoin, "Alpha"),
    ];
    accounts.sort_by(sort_account_views);

    let keys: Vec<_> = accounts.iter().map(extract_sort_key).collect();
    assert_eq!(
        keys,
        [
            ("BTC", "Alpha"),
            ("BTC", "Zebra"),
            ("ETH", "Alpha"),
            ("ETH", "Beta"),
            ("ADA", "Alpha"),
            ("ZZZ", "Alpha"),
            ("ZZZ", "Zebra"),
        ]
    );
}

// --- Wallet report sorting tests ---

fn report_row(asset_id: Option<AssetId>, unit_code: &str, label: &str) -> WalletReportAccountRow {
    WalletReportAccountRow {
        account_id: crate::wallets::WalletAccountId::new(),
        account_label: label.to_string(),
        catalog_asset_key: asset_id
            .as_ref()
            .map(|id| crate::asset_views::CatalogAssetKey::from_trusted(id.as_str().to_string())),
        asset_display_name: asset_id.as_ref().and_then(|id| {
            crate::asset_capabilities::asset(id).map(|a| a.canonical_name.to_string())
        }),
        unit_code: unit_code.to_string(),
        symbol: None,
        bitcoin_history_coverage: None,
        opening_balance_state: WalletReportBalanceStateView::Unknown,
        opening_balance: None,
        opening_balance_date: None,
        closing_balance_state: WalletReportBalanceStateView::Unknown,
        closing_balance: None,
        closing_balance_date: None,
    }
}

#[test]
fn older_wallet_report_row_without_bitcoin_coverage_deserializes() {
    let mut older_payload =
        serde_json::to_value(report_row(Some(AssetId::BITCOIN), "BTC", "Bitcoin"))
            .expect("report row should serialize");
    older_payload
        .as_object_mut()
        .expect("report row should serialize as an object")
        .remove("bitcoin_history_coverage");

    let decoded: WalletReportAccountRow =
        serde_json::from_value(older_payload).expect("older report row should deserialize");

    assert_eq!(decoded.bitcoin_history_coverage, None);
}

#[test]
fn sort_report_rows_native_before_custom() {
    let mut rows = [
        report_row(None, "ADA", "Alpha"),
        report_row(Some(AssetId::BITCOIN), "BTC", "Alpha"),
    ];
    rows.sort_by(sort_report_account_rows);

    assert!(rows[0].catalog_asset_key.is_some());
    assert!(rows[1].catalog_asset_key.is_none());
}

#[test]
fn sort_report_rows_by_unit_code_then_label() {
    let mut rows = [
        report_row(Some(AssetId::ETHEREUM), "ETH", "Alpha"),
        report_row(Some(AssetId::BITCOIN), "BTC", "Zebra"),
        report_row(Some(AssetId::BITCOIN), "BTC", "Alpha"),
        report_row(None, "ZZZ", "Alpha"),
        report_row(None, "ADA", "Alpha"),
    ];
    rows.sort_by(sort_report_account_rows);

    let keys: Vec<_> = rows
        .iter()
        .map(|r| (&*r.unit_code, &*r.account_label))
        .collect();
    assert_eq!(
        keys,
        [
            ("BTC", "Alpha"),
            ("BTC", "Zebra"),
            ("ETH", "Alpha"),
            ("ADA", "Alpha"),
            ("ZZZ", "Alpha"),
        ]
    );
}

// --- Manual Sync Affordance Tests ---

fn slot_record(account_id: DigitalAssetAccountId) -> AccountSyncSlotRecord {
    AccountSyncSlotRecord {
        account_id,
        selected_at: "2026-05-01T00:00:00Z".parse().expect("valid datetime"),
        selected_under_tier: EntitlementTier::Basic,
    }
}

fn active_set(ids: &[DigitalAssetAccountId]) -> HashSet<DigitalAssetAccountId> {
    ids.iter().copied().collect()
}

fn slot_map(
    records: Vec<AccountSyncSlotRecord>,
) -> HashMap<DigitalAssetAccountId, AccountSyncSlotRecord> {
    records.into_iter().map(|r| (r.account_id, r)).collect()
}

fn unavailable_set(ids: &[DigitalAssetAccountId]) -> HashSet<DigitalAssetAccountId> {
    ids.iter().copied().collect()
}

#[test]
fn manual_sync_selected_transaction_history_account() {
    let account_id = DigitalAssetAccountId::new();
    let view = native_account_manual_sync_view(
        account_id,
        50,
        NativeAccountManualSyncContext {
            sync_slots: &slot_map(vec![slot_record(account_id)]),
            active_sync_slot_account_ids: &active_set(&[account_id]),
            slot_limit: 5,
            tier: EntitlementTier::Basic,
            historical_backfill_enabled: true,
            historical_backfill_transactions_per_account: 1000,
            free_balance_unavailable_account_ids: &unavailable_set(&[]),
        },
    );

    assert_eq!(view.mode, ManualSyncMode::TransactionHistory);
    assert_eq!(view.slot_effect, ManualSyncSlotEffect::AlreadySelected);
    assert_eq!(view.disabled_reason, None);
    assert_eq!(view.used_slots, 1);
    assert_eq!(view.slot_limit, 5);
    assert_eq!(view.next_tier_display_name, Some("Premium".to_string()));
}

#[test]
fn manual_sync_selected_balance_refresh_on_free() {
    let account_id = DigitalAssetAccountId::new();
    let mut record = slot_record(account_id);
    record.selected_under_tier = EntitlementTier::Free;

    let view = native_account_manual_sync_view(
        account_id,
        50,
        NativeAccountManualSyncContext {
            sync_slots: &slot_map(vec![record]),
            active_sync_slot_account_ids: &active_set(&[account_id]),
            slot_limit: 5,
            tier: EntitlementTier::Free,
            historical_backfill_enabled: false,
            historical_backfill_transactions_per_account: 0,
            free_balance_unavailable_account_ids: &unavailable_set(&[]),
        },
    );

    assert_eq!(view.mode, ManualSyncMode::BalanceRefresh);
    assert_eq!(view.slot_effect, ManualSyncSlotEffect::AlreadySelected);
    assert_eq!(view.disabled_reason, None);
}

#[test]
fn manual_sync_paid_over_cap_returns_balance_refresh() {
    let account_id = DigitalAssetAccountId::new();
    let view = native_account_manual_sync_view(
        account_id,
        5000,
        NativeAccountManualSyncContext {
            sync_slots: &slot_map(vec![slot_record(account_id)]),
            active_sync_slot_account_ids: &active_set(&[account_id]),
            slot_limit: 5,
            tier: EntitlementTier::Basic,
            historical_backfill_enabled: true,
            historical_backfill_transactions_per_account: 1000,
            free_balance_unavailable_account_ids: &unavailable_set(&[]),
        },
    );

    assert_eq!(view.mode, ManualSyncMode::BalanceRefresh);
    assert_eq!(view.slot_effect, ManualSyncSlotEffect::AlreadySelected);
    assert_eq!(view.disabled_reason, None);
}

#[test]
fn manual_sync_no_slot_with_available_capacity() {
    let account_id = DigitalAssetAccountId::new();
    let other_id = DigitalAssetAccountId::new();
    let mut other_record = slot_record(other_id);
    other_record.selected_under_tier = EntitlementTier::Basic;

    let view = native_account_manual_sync_view(
        account_id,
        50,
        NativeAccountManualSyncContext {
            sync_slots: &slot_map(vec![other_record]),
            active_sync_slot_account_ids: &active_set(&[other_id]),
            slot_limit: 5,
            tier: EntitlementTier::Basic,
            historical_backfill_enabled: true,
            historical_backfill_transactions_per_account: 1000,
            free_balance_unavailable_account_ids: &unavailable_set(&[]),
        },
    );

    assert_eq!(view.mode, ManualSyncMode::TransactionHistory);
    assert_eq!(
        view.slot_effect,
        ManualSyncSlotEffect::WillSelectAvailableSlot
    );
    assert_eq!(view.disabled_reason, None);
    assert_eq!(view.used_slots, 1);
}

#[test]
fn manual_sync_no_slot_no_capacity_keeps_sync_available() {
    let account_id = DigitalAssetAccountId::new();
    let mut slots = vec![];
    let mut active = vec![];
    for _ in 0..5 {
        let id = DigitalAssetAccountId::new();
        let mut record = slot_record(id);
        record.selected_under_tier = EntitlementTier::Basic;
        slots.push(record);
        active.push(id);
    }

    let view = native_account_manual_sync_view(
        account_id,
        50,
        NativeAccountManualSyncContext {
            sync_slots: &slot_map(slots),
            active_sync_slot_account_ids: &active_set(&active),
            slot_limit: 5,
            tier: EntitlementTier::Basic,
            historical_backfill_enabled: true,
            historical_backfill_transactions_per_account: 1000,
            free_balance_unavailable_account_ids: &unavailable_set(&[]),
        },
    );

    assert_eq!(view.mode, ManualSyncMode::TransactionHistory);
    assert_eq!(view.slot_effect, ManualSyncSlotEffect::NoCapacity);
    assert_eq!(view.disabled_reason, None);
    assert_eq!(view.next_tier_display_name, Some("Premium".to_string()));
}

#[test]
fn manual_sync_unavailable_on_current_plan() {
    let account_id = DigitalAssetAccountId::new();

    let view = native_account_manual_sync_view(
        account_id,
        50,
        NativeAccountManualSyncContext {
            sync_slots: &slot_map(vec![]),
            active_sync_slot_account_ids: &active_set(&[]),
            slot_limit: 5,
            tier: EntitlementTier::Free,
            historical_backfill_enabled: false,
            historical_backfill_transactions_per_account: 0,
            free_balance_unavailable_account_ids: &unavailable_set(&[account_id]),
        },
    );

    assert_eq!(view.mode, ManualSyncMode::Unavailable);
    assert_eq!(view.slot_effect, ManualSyncSlotEffect::NoCapacity);
    assert_eq!(
        view.disabled_reason,
        Some(ManualSyncDisabledReason::SyncUnavailableOnPlan)
    );
}

// --- Capacity Summary Tests ---

#[test]
fn capacity_summary_with_available_slots() {
    let view = synced_account_capacity_view(2, 5, EntitlementTier::Basic);

    assert_eq!(view.used_slots, 2);
    assert_eq!(view.slot_limit, 5);
    assert_eq!(view.available_slots, 3);
    assert_eq!(view.next_tier_display_name, Some("Premium".to_string()));
}

#[test]
fn capacity_summary_at_capacity() {
    let view = synced_account_capacity_view(5, 5, EntitlementTier::Basic);

    assert_eq!(view.used_slots, 5);
    assert_eq!(view.slot_limit, 5);
    assert_eq!(view.available_slots, 0);
    assert_eq!(view.next_tier_display_name, Some("Premium".to_string()));
}

#[test]
fn capacity_summary_free_tier() {
    let view = synced_account_capacity_view(0, 5, EntitlementTier::Free);

    assert_eq!(view.used_slots, 0);
    assert_eq!(view.slot_limit, 5);
    assert_eq!(view.available_slots, 5);
    assert_eq!(view.next_tier_display_name, Some("Basic".to_string()));
}

#[test]
fn capacity_summary_serializes_free_balance_synced_copy() {
    let view = synced_account_capacity_view(3, 5, EntitlementTier::Free);
    let serialized = serde_json::to_value(view).expect("capacity view should serialize");

    assert_eq!(serialized["summary"], "3 of 5 balance-synced accounts used");
}

#[test]
fn capacity_summary_serializes_paid_synced_copy() {
    let view = synced_account_capacity_view(3, 10, EntitlementTier::Basic);
    let serialized = serde_json::to_value(view).expect("capacity view should serialize");

    assert_eq!(serialized["summary"], "3 of 10 synced accounts used");
}

#[test]
fn account_limit_summary_uses_active_account_wording() {
    let view = account_limit_view(3, 2, 5);
    let serialized = serde_json::to_value(view).expect("account limit view should serialize");

    assert_eq!(serialized["active_count"], 3);
    assert_eq!(serialized["inactive_count"], 2);
    assert_eq!(serialized["active_limit"], 5);
    assert_eq!(serialized["summary"], "3 of 5 active accounts used");
    assert!(
        !serialized["summary"]
            .as_str()
            .expect("summary should be a string")
            .contains("sync")
    );
}

#[test]
fn capacity_summary_premium_no_next_tier() {
    let view = synced_account_capacity_view(1, 10, EntitlementTier::Premium);

    assert_eq!(view.used_slots, 1);
    assert_eq!(view.slot_limit, 10);
    assert_eq!(view.available_slots, 9);
    assert_eq!(view.next_tier_display_name, None);
}

#[test]
fn capacity_summary_unknown_tier_no_next() {
    let view =
        synced_account_capacity_view(1, 10, EntitlementTier::Unknown("Enterprise".to_string()));

    assert_eq!(view.used_slots, 1);
    assert_eq!(view.available_slots, 9);
    assert_eq!(view.next_tier_display_name, None);
}

#[test]
fn manual_asset_search_response_includes_synced_catalog_assets_and_dedupes_coingecko() {
    let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
    let conn = crate::db::initialize_prices_db().expect("prices db should initialize");
    let retrieved_at = chrono::DateTime::parse_from_rfc3339("2026-06-08T12:00:00Z")
        .expect("timestamp")
        .with_timezone(&chrono::Utc);
    crate::db::replace_or_upsert_coingecko_catalog_rows(
        &conn,
        &[crate::db::CoinGeckoCatalogUpsert {
            provider_asset_id: "bitcoin".to_string(),
            symbol: "btc".to_string(),
            normalized_symbol: "btc".to_string(),
            name: "Bitcoin".to_string(),
            platforms_json: None,
            status: "active".to_string(),
            retrieved_at,
        }],
        retrieved_at,
    )
    .expect("seed synced catalog row");

    let response = super::handlers_read::manual_asset_search_response_for_query("ada")
        .expect("manual asset search should succeed");
    assert_eq!(response.results.len(), 1);
    assert_eq!(
        response.results[0].source,
        crate::wallets::ManualAssetSearchSource::BitGarthCatalog
    );
    assert_eq!(response.results[0].unit_code, "ADA");
    assert_eq!(response.results[0].asset_name, "Cardano");
    assert!(response.results[0].asset_instance_id.is_some());

    let btc = super::handlers_read::manual_asset_search_response_for_query("btc")
        .expect("manual asset search should succeed");
    let btc_row = btc
        .results
        .iter()
        .find(|row| row.unit_code == "BTC")
        .expect("BTC should appear as a BitGarth catalog result");
    assert_eq!(
        btc_row.source,
        crate::wallets::ManualAssetSearchSource::BitGarthCatalog
    );
    assert_eq!(btc_row.asset_name, "Bitcoin");
    assert_eq!(btc_row.network_name, "Bitcoin");
    assert_eq!(btc_row.coingecko_id.as_deref(), Some("bitcoin"));
    assert_eq!(btc_row.decimal_precision, Some(8));
    assert_eq!(
        btc_row.asset_instance_id,
        Some(crate::asset_views::ManualAssetInstanceIdView {
            asset_id: "bitcoin".to_string(),
            network_id: "bitcoin-mainnet".to_string(),
        })
    );
    assert!(btc.results.iter().all(|row| {
        row.source == crate::wallets::ManualAssetSearchSource::BitGarthCatalog
            || row.coingecko_id.as_deref() != Some("bitcoin")
    }));
}

#[test]
fn manual_asset_catalog_total_includes_synced_catalog_assets() {
    let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");

    let (total, _) = crate::services::manual_asset_discovery::catalog_total()
        .expect("manual asset total should compute");
    let bitgarth_total = crate::asset_capabilities::manual_catalog_candidates()
        .expect("manual catalog should load")
        .len();

    assert!(total >= bitgarth_total);
}

#[test]
fn manual_asset_search_response_filters_whole_unsynced_catalog_from_coingecko_rows() {
    let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
    let conn = crate::db::initialize_prices_db().expect("prices db should initialize");
    let retrieved_at = chrono::DateTime::parse_from_rfc3339("2026-06-08T12:00:00Z")
        .expect("timestamp")
        .with_timezone(&chrono::Utc);
    crate::db::replace_or_upsert_coingecko_catalog_rows(
        &conn,
        &[crate::db::CoinGeckoCatalogUpsert {
            provider_asset_id: "cardano".to_string(),
            symbol: "foo".to_string(),
            normalized_symbol: "foo".to_string(),
            name: "Foo Coin".to_string(),
            platforms_json: None,
            status: "active".to_string(),
            retrieved_at,
        }],
        retrieved_at,
    )
    .expect("seed catalog duplicate row");

    let response = super::handlers_read::manual_asset_search_response_for_query("foo")
        .expect("manual asset search should succeed");

    assert!(
        response
            .results
            .iter()
            .all(|row| row.coingecko_id.as_deref() != Some("cardano")),
        "catalog-backed Cardano must not be returned as a CoinGecko-only row"
    );
}

#[test]
fn manual_asset_search_response_includes_seeded_coingecko_catalog_rows_after_local_rows() {
    let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
    let conn = crate::db::initialize_prices_db().expect("prices db should initialize");
    let retrieved_at = chrono::DateTime::parse_from_rfc3339("2026-06-08T12:00:00Z")
        .expect("timestamp")
        .with_timezone(&chrono::Utc);
    crate::db::replace_or_upsert_coingecko_catalog_rows(
        &conn,
        &[
            crate::db::CoinGeckoCatalogUpsert {
                provider_asset_id: "cardano".to_string(),
                symbol: "ada".to_string(),
                normalized_symbol: "ada".to_string(),
                name: "Cardano duplicate".to_string(),
                platforms_json: None,
                status: "active".to_string(),
                retrieved_at,
            },
            crate::db::CoinGeckoCatalogUpsert {
                provider_asset_id: "adappter-token".to_string(),
                symbol: "adp".to_string(),
                normalized_symbol: "adp".to_string(),
                name: "Ada Discovery Token".to_string(),
                platforms_json: Some(r#"{"ethereum":"0xabc","polygon-pos":"0xdef"}"#.to_string()),
                status: "active".to_string(),
                retrieved_at,
            },
        ],
        retrieved_at,
    )
    .expect("seed catalog rows");

    let response = super::handlers_read::manual_asset_search_response_for_query("ada")
        .expect("manual asset search should succeed");

    assert_eq!(
        response.results[0].source,
        crate::wallets::ManualAssetSearchSource::BitGarthCatalog
    );
    assert_eq!(response.results[0].unit_code, "ADA");
    assert!(response.results.iter().all(|row| {
        row.source == crate::wallets::ManualAssetSearchSource::BitGarthCatalog
            || row.coingecko_id.as_deref() != Some("cardano")
    }));
    let coingecko = response
        .results
        .iter()
        .find(|row| row.coingecko_id.as_deref() == Some("adappter-token"))
        .expect("CoinGecko-only row should be returned");
    assert_eq!(
        coingecko.source,
        crate::wallets::ManualAssetSearchSource::CoinGeckoCatalog
    );
    assert_eq!(coingecko.asset_instance_id, None);
    assert_eq!(coingecko.platform_count, Some(2));
}

#[test]
fn manual_asset_search_response_reports_true_match_total_beyond_cap() {
    let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
    let conn = crate::db::initialize_prices_db().expect("prices db should initialize");
    let retrieved_at = chrono::DateTime::parse_from_rfc3339("2026-06-08T12:00:00Z")
        .expect("timestamp")
        .with_timezone(&chrono::Utc);
    // Seed 25 CoinGecko-only rows whose names all start with "zztoken".
    let rows: Vec<crate::db::CoinGeckoCatalogUpsert> = (0..25)
        .map(|i| crate::db::CoinGeckoCatalogUpsert {
            provider_asset_id: format!("zztoken-{i}"),
            symbol: format!("zz{i}"),
            normalized_symbol: format!("zz{i}"),
            name: format!("zztoken {i}"),
            platforms_json: None,
            status: "active".to_string(),
            retrieved_at,
        })
        .collect();
    crate::db::replace_or_upsert_coingecko_catalog_rows(&conn, &rows, retrieved_at)
        .expect("seed catalog rows");

    let response = super::handlers_read::manual_asset_search_response_for_query("zztoken")
        .expect("manual asset search should succeed");

    // Display list stays capped at 25, and the count reflects all 25 matches.
    assert_eq!(response.results.len(), 25);
    assert_eq!(response.total_match_count, 25);
}
