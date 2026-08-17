#![cfg(feature = "server")]

use std::collections::{HashMap, HashSet};

use crate::account_limits::{
    AccountActivationState, ClassifiedAccount, SUPPORTED_ACCOUNT_HARD_CAP,
};
use crate::amounts::{UnsignedAmount, format_unsigned_amount};
use crate::asset_capabilities::{asset_instance, synced_asset_instance, synced_asset_instance_id};
use crate::balance_reliability::{BalanceProvisionalReason, BalanceReliability};
use crate::db::{
    AccountSyncSlotRecord, ManualAssetAccountRow, ManualAssetBalanceState, WalletReportBalanceState,
};
use crate::payments::types::EntitlementTier;
use crate::transactions::{AddressBalanceSummary, NativeBalanceState};
use crate::wallets::WalletAccountId;

#[cfg(test)]
use super::balance_projection::{BalanceContributor, aggregate_contributors};
use super::balance_projection::{
    ProjectedAssetBalance, ProjectedBalanceReason, ProjectedBalanceStatus,
};
use super::helpers::internal_error;
use super::types::{
    AccountBalanceStateView, AccountLimitView, AccountReferenceKind, AccountStateView,
    AccountTransactionCountsView, AccountTransactionView, AccountView, AddressView, AddressesView,
    BalanceAmountView, ManualAssetAccountView, ManualSyncDisabledReason, ManualSyncMode,
    ManualSyncSlotEffect, NativeAccountManualSyncView, NativeAccountSyncSlotView,
    NativeAccountView, SyncedAccountCapacityView, WalletAggregateBalanceView,
    WalletBalanceContextView, WalletBalanceView, WalletError, WalletReportAccountRow,
    WalletReportBalanceStateView, WalletView,
};

// ============ Conversion Functions ============

pub(super) fn balance_amount_view(
    amount: UnsignedAmount,
    decimal_precision: u8,
) -> BalanceAmountView {
    BalanceAmountView {
        raw_value: amount.raw_string(),
        formatted_value: format_unsigned_amount(amount, decimal_precision),
    }
}

pub(super) fn wallet_report_balance_state_view(
    state: WalletReportBalanceState,
    decimal_precision: u8,
) -> WalletReportBalanceStateView {
    match state {
        WalletReportBalanceState::CanonicalZero => WalletReportBalanceStateView::CanonicalZero,
        WalletReportBalanceState::KnownAmount(amount) => {
            WalletReportBalanceStateView::NeedsPrice(balance_amount_view(amount, decimal_precision))
        }
        WalletReportBalanceState::Unknown => WalletReportBalanceStateView::Unknown,
    }
}

pub(super) fn custom_wallet_report_balance_state_view(
    state: ManualAssetBalanceState,
    decimal_precision: u8,
) -> WalletReportBalanceStateView {
    match state {
        ManualAssetBalanceState::Known(amount) if amount == UnsignedAmount::zero() => {
            WalletReportBalanceStateView::CanonicalZero
        }
        ManualAssetBalanceState::Known(amount) => {
            WalletReportBalanceStateView::NeedsPrice(balance_amount_view(amount, decimal_precision))
        }
        ManualAssetBalanceState::Unknown => WalletReportBalanceStateView::Unknown,
    }
}

pub(super) fn custom_wallet_report_balance_value(
    state: ManualAssetBalanceState,
    decimal_precision: u8,
) -> Option<BalanceAmountView> {
    match state {
        ManualAssetBalanceState::Known(amount) => {
            Some(balance_amount_view(amount, decimal_precision))
        }
        ManualAssetBalanceState::Unknown => None,
    }
}

pub(super) fn wallet_balance_view(
    asset_id: crate::wallets::SyncedAssetId,
    network: crate::wallets::Network,
    balance: &AddressBalanceSummary,
    balance_reliability: BalanceReliability,
) -> Result<WalletBalanceView, WalletError> {
    let instance = asset_instance(
        &synced_asset_instance(synced_asset_instance_id(asset_id)).asset_instance_id,
    )
    .ok_or_else(|| {
        internal_error(
            "asset_instance_lookup",
            "synced asset instance not found in registry",
        )
    })?;
    let decimal_precision = instance.decimal_precision;
    let balance_state = match balance.confirmed {
        NativeBalanceState::KnownAmount(amount) => AccountBalanceStateView::Known {
            amount: balance_amount_view(amount, decimal_precision),
        },
        NativeBalanceState::Unknown => AccountBalanceStateView::Unknown,
        NativeBalanceState::CanonicalZero => {
            return Err(internal_error(
                "current_balance_state",
                "canonical zero is only valid at historical boundaries",
            ));
        }
    };
    Ok(WalletBalanceView {
        asset_id,
        context: WalletBalanceContextView { network },
        unit_code: instance.unit_code.as_str().to_string(),
        symbol: instance.symbol.as_ref().map(|s| s.to_string()),
        balance_reliability,
        balance_state,
        current_value: None,
    })
}

pub(super) fn projected_wallet_balance_view(
    balance: &ProjectedAssetBalance,
    symbol: Option<String>,
) -> WalletAggregateBalanceView {
    let reasons = balance.reasons.iter().map(|reason| match reason {
        ProjectedBalanceReason::FirstSuccessfulSyncPending => {
            BalanceProvisionalReason::FirstSuccessfulSyncPending
        }
        ProjectedBalanceReason::InactiveAccountNotSyncing => {
            BalanceProvisionalReason::InactiveAccountNotSyncing
        }
    });
    let balance_reliability = match balance.status {
        ProjectedBalanceStatus::Final | ProjectedBalanceStatus::Unknown => {
            BalanceReliability::finalized()
        }
        ProjectedBalanceStatus::Provisional => BalanceReliability::from_reasons(reasons),
    };
    let balance_state = match balance.amount {
        Some(amount) => AccountBalanceStateView::Known {
            amount: balance_amount_view(amount, balance.decimal_precision),
        },
        None => AccountBalanceStateView::Unknown,
    };

    WalletAggregateBalanceView {
        asset_id: balance.asset_id.clone(),
        network_id: balance.network_id.clone(),
        unit_code: balance.unit.clone(),
        symbol,
        balance_reliability,
        balance_state,
        current_value: None,
    }
}

fn synced_projected_balance_symbol(balance: &ProjectedAssetBalance) -> Option<String> {
    [
        crate::wallets::SyncedAssetId::Bitcoin,
        crate::wallets::SyncedAssetId::Ethereum,
    ]
    .into_iter()
    .find_map(|asset_id| {
        let instance = asset_instance(
            &synced_asset_instance(synced_asset_instance_id(asset_id)).asset_instance_id,
        )?;
        (instance.id.asset_id.as_str() == balance.asset_id
            && crate::asset_capabilities::network_slug(instance.id.network_id)
                == balance.network_id)
            .then(|| instance.symbol.as_ref().map(ToString::to_string))
            .flatten()
    })
}

fn projected_balance_symbol(
    balance: &ProjectedAssetBalance,
    wallet: &crate::wallets::WalletWithDetails,
    manual_asset_accounts: &[ManualAssetAccountRow],
) -> Option<String> {
    synced_projected_balance_symbol(balance).or_else(|| {
        manual_asset_accounts
            .iter()
            .find(|account| {
                account.wallet_id == wallet.wallet.id
                    && account.asset_id.as_str() == balance.asset_id
                    && account.network_id.as_str() == balance.network_id
            })
            .and_then(|account| account.symbol.clone())
    })
}

pub(super) fn zero_balance_summary(
    asset_id: crate::wallets::SyncedAssetId,
) -> Result<AddressBalanceSummary, WalletError> {
    Ok(AddressBalanceSummary::known(
        asset_id,
        UnsignedAmount::zero(),
    ))
}

pub(crate) fn native_account_sync_slot_view(
    account_id: crate::wallets::DigitalAssetAccountId,
    sync_slots: &HashMap<crate::wallets::DigitalAssetAccountId, AccountSyncSlotRecord>,
    active_sync_slot_account_ids: &HashSet<crate::wallets::DigitalAssetAccountId>,
    limit: u16,
    free_balance_unavailable_account_ids: &HashSet<crate::wallets::DigitalAssetAccountId>,
) -> NativeAccountSyncSlotView {
    let selected_count = sync_slots
        .keys()
        .filter(|account_id| !free_balance_unavailable_account_ids.contains(account_id))
        .count();
    let selected = sync_slots.get(&account_id);
    let active = active_sync_slot_account_ids.contains(&account_id);
    let balance_sync_available_on_free =
        !free_balance_unavailable_account_ids.contains(&account_id);

    NativeAccountSyncSlotView {
        selected: selected.is_some(),
        active,
        can_select: balance_sync_available_on_free
            && selected.is_none()
            && selected_count < usize::from(limit),
        limit,
        selected_at: selected.map(|record| record.selected_at.to_rfc3339()),
        selected_under_tier: selected.map(|record| record.selected_under_tier.as_str().to_string()),
    }
}

pub(crate) struct WalletAccountData<'a> {
    pub manual_asset_accounts: &'a [ManualAssetAccountRow],
    pub address_balances: &'a HashMap<String, AddressBalanceSummary>,
    pub account_balances: &'a HashMap<crate::wallets::DigitalAssetAccountId, AddressBalanceSummary>,
    pub account_balance_reliabilities:
        &'a HashMap<crate::wallets::DigitalAssetAccountId, BalanceReliability>,
    pub custom_account_balances: &'a HashMap<WalletAccountId, ManualAssetBalanceState>,
    pub account_transactions: Option<
        &'a HashMap<
            crate::wallets::DigitalAssetAccountId,
            Vec<crate::transactions::AccountTransactionEntry>,
        >,
    >,
    pub account_tx_counts: &'a HashMap<
        crate::wallets::DigitalAssetAccountId,
        crate::transactions::AccountTransactionCounts,
    >,
}

pub(crate) fn next_tier_display_name(tier: &EntitlementTier) -> Option<String> {
    match tier {
        EntitlementTier::Free => Some("Basic".to_string()),
        EntitlementTier::Basic => Some("Premium".to_string()),
        EntitlementTier::Premium => None,
        EntitlementTier::Unknown(_) => None,
    }
}

#[derive(Clone)]
pub(crate) struct NativeAccountManualSyncContext<'a> {
    pub(crate) sync_slots:
        &'a HashMap<crate::wallets::DigitalAssetAccountId, AccountSyncSlotRecord>,
    pub(crate) active_sync_slot_account_ids: &'a HashSet<crate::wallets::DigitalAssetAccountId>,
    pub(crate) slot_limit: u16,
    pub(crate) tier: EntitlementTier,
    pub(crate) historical_backfill_enabled: bool,
    pub(crate) historical_backfill_transactions_per_account: u32,
    pub(crate) free_balance_unavailable_account_ids:
        &'a HashSet<crate::wallets::DigitalAssetAccountId>,
}

pub(crate) fn native_account_manual_sync_view(
    account_id: crate::wallets::DigitalAssetAccountId,
    transaction_count: u32,
    context: NativeAccountManualSyncContext<'_>,
) -> NativeAccountManualSyncView {
    let sync_available = !context
        .free_balance_unavailable_account_ids
        .contains(&account_id);
    let has_slot = context.sync_slots.contains_key(&account_id);
    let used_slots = u16::try_from(context.active_sync_slot_account_ids.len()).unwrap_or(u16::MAX);
    let has_capacity = used_slots < context.slot_limit;

    let mode = if !sync_available {
        ManualSyncMode::Unavailable
    } else if context.historical_backfill_enabled
        && transaction_count <= context.historical_backfill_transactions_per_account
    {
        ManualSyncMode::TransactionHistory
    } else {
        ManualSyncMode::BalanceRefresh
    };

    let (slot_effect, disabled_reason) = if !sync_available {
        (
            ManualSyncSlotEffect::NoCapacity,
            Some(ManualSyncDisabledReason::SyncUnavailableOnPlan),
        )
    } else if has_slot {
        (ManualSyncSlotEffect::AlreadySelected, None)
    } else if has_capacity {
        (ManualSyncSlotEffect::WillSelectAvailableSlot, None)
    } else {
        (ManualSyncSlotEffect::NoCapacity, None)
    };

    NativeAccountManualSyncView {
        mode,
        slot_effect,
        disabled_reason,
        used_slots,
        slot_limit: context.slot_limit,
        next_tier_display_name: next_tier_display_name(&context.tier),
    }
}

pub(crate) fn synced_account_capacity_view(
    used_slots: u16,
    slot_limit: u16,
    tier: EntitlementTier,
) -> SyncedAccountCapacityView {
    let account_label = match tier {
        EntitlementTier::Free => "balance-synced accounts",
        EntitlementTier::Basic | EntitlementTier::Premium | EntitlementTier::Unknown(_) => {
            "synced accounts"
        }
    };

    SyncedAccountCapacityView {
        used_slots,
        slot_limit,
        available_slots: slot_limit.saturating_sub(used_slots),
        summary: format!("{used_slots} of {slot_limit} {account_label} used"),
        next_tier_display_name: next_tier_display_name(&tier),
    }
}

pub(crate) fn account_limit_view(
    active_count: usize,
    inactive_count: usize,
    active_limit: u16,
) -> AccountLimitView {
    let active_count = u16::try_from(active_count).unwrap_or(u16::MAX);
    let inactive_count = u16::try_from(inactive_count).unwrap_or(u16::MAX);
    let hard_cap = u16::try_from(SUPPORTED_ACCOUNT_HARD_CAP).unwrap_or(u16::MAX);

    AccountLimitView {
        active_count,
        inactive_count,
        active_limit,
        hard_cap,
        summary: format!("{active_count} of {active_limit} active accounts used"),
        upgrade_call_to_action: (inactive_count > 0)
            .then(|| "Upgrade to activate inactive accounts.".to_string()),
    }
}

fn account_state_view(state: AccountActivationState) -> AccountStateView {
    match state {
        AccountActivationState::Active => AccountStateView::Active,
        AccountActivationState::Inactive => AccountStateView::Inactive,
    }
}

fn classified_account_state(
    classified_accounts: &[ClassifiedAccount],
    account_id: WalletAccountId,
) -> AccountStateView {
    classified_accounts
        .iter()
        .find(|classified| classified.account_id == account_id)
        .map(|classified| account_state_view(classified.state))
        .unwrap_or(AccountStateView::Inactive)
}

pub(super) fn account_transaction_view(
    asset_id: crate::wallets::SyncedAssetId,
    transaction: &crate::transactions::AccountTransactionEntry,
) -> Result<AccountTransactionView, WalletError> {
    let decimal_precision = asset_instance(
        &synced_asset_instance(synced_asset_instance_id(asset_id)).asset_instance_id,
    )
    .ok_or_else(|| {
        internal_error(
            "asset_instance_lookup",
            "synced asset instance not found in registry",
        )
    })?
    .decimal_precision;
    Ok(AccountTransactionView {
        tx_hash: transaction.tx_hash.clone(),
        status: transaction.status,
        direction: transaction.direction,
        transfer_kind: transaction.transfer_kind.clone(),
        value: balance_amount_view(transaction.value, decimal_precision),
        fee: transaction
            .fee
            .map(|value| balance_amount_view(value, decimal_precision)),
        from_address: transaction
            .from_address
            .as_ref()
            .map(|value| value.as_str().to_string()),
        to_address: transaction
            .to_address
            .as_ref()
            .map(|value| value.as_str().to_string()),
        block_time: transaction
            .block_time
            .as_ref()
            .map(chrono::DateTime::to_rfc3339),
    })
}

#[cfg(test)]
#[derive(Debug, Clone)]
pub(super) struct RenderedAccountBalance {
    pub(super) asset_id: crate::wallets::SyncedAssetId,
    pub(super) network: crate::wallets::Network,
    pub(super) linked_at: chrono::DateTime<chrono::Utc>,
    pub(super) balance: AddressBalanceSummary,
    pub(super) balance_reliability: BalanceReliability,
}

#[cfg(test)]
pub(super) fn derive_wallet_balances(
    rendered: &[RenderedAccountBalance],
) -> Result<Vec<WalletAggregateBalanceView>, WalletError> {
    for entry in rendered {
        if entry.balance.confirmed == NativeBalanceState::CanonicalZero {
            return Err(internal_error(
                "aggregate_confirmed_wallet_balance",
                "canonical zero is only valid at historical boundaries",
            ));
        }
    }
    let contributors = rendered
        .iter()
        .map(|entry| {
            let _legacy_rendering_metadata = (entry.network, entry.linked_at);
            let instance = asset_instance(
                &synced_asset_instance(synced_asset_instance_id(entry.asset_id)).asset_instance_id,
            )
            .ok_or_else(|| {
                internal_error(
                    "asset_instance_lookup",
                    "synced asset instance not found in registry",
                )
            })?;
            Ok(BalanceContributor {
                asset_id: instance.id.asset_id.as_str().to_string(),
                network_id: crate::asset_capabilities::network_slug(instance.id.network_id)
                    .to_string(),
                unit: instance.unit_code.as_str().to_string(),
                amount: match entry.balance.confirmed {
                    NativeBalanceState::KnownAmount(amount) => Some(amount),
                    NativeBalanceState::Unknown => None,
                    NativeBalanceState::CanonicalZero => {
                        unreachable!("canonical zero entries returned before test aggregation")
                    }
                },
                decimal_precision: instance.decimal_precision,
                inactive: false,
                reliability: entry.balance_reliability.clone(),
            })
        })
        .collect::<Result<Vec<_>, WalletError>>()?;
    aggregate_contributors(contributors).map(|balances| {
        balances
            .iter()
            .map(|balance| {
                projected_wallet_balance_view(balance, synced_projected_balance_symbol(balance))
            })
            .collect()
    })
}

pub(super) fn convert_wallet_to_view(
    wallet: crate::wallets::WalletWithDetails,
    projected_balances: &[ProjectedAssetBalance],
    data: &WalletAccountData<'_>,
    sync: &NativeAccountManualSyncContext<'_>,
    classified_accounts: &[ClassifiedAccount],
) -> Result<WalletView, WalletError> {
    let has_accessors = !wallet.accessors.is_empty();
    let manual_account_count = data
        .manual_asset_accounts
        .iter()
        .filter(|account| account.wallet_id == wallet.wallet.id)
        .count();
    let logical_account_count = u32::try_from(
        wallet.accounts.len().saturating_add(manual_account_count),
    )
    .map_err(|_| {
        internal_error(
            "wallet_account_count_range",
            "wallet account count overflow",
        )
    })?;

    let wallet_label = crate::wallets::display_wallet_label(&wallet.wallet.label);

    let mut accounts_view = Vec::new();
    let mut fallback_account_number = 1_u32;

    for account in &wallet.accounts {
        let account_view_id = WalletAccountId::from(account.id);
        let account_state = classified_account_state(classified_accounts, account_view_id);
        let account_number = if let Some(account_index) = account.primary_account_index() {
            account_index.as_u32() + 1
        } else {
            let number = fallback_account_number;
            fallback_account_number = fallback_account_number.saturating_add(1);
            number
        };
        let account_label = crate::wallets::display_account_label(&account.label);

        let schemes_to_render =
            if account.account_kind == crate::wallets::AccountKind::SingleAddress {
                // Single-address accounts use whatever scheme the address was stored with
                // (e.g. Standard for Ethereum, Legacy/NativeSegwit/Taproot for Bitcoin)
                account
                    .addresses
                    .iter()
                    .map(|addr| addr.address_scheme)
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect()
            } else if account.account_model == crate::wallets::AccountModel::Account {
                vec![crate::wallets::AddressScheme::Standard]
            } else {
                vec![
                    crate::wallets::AddressScheme::Legacy,
                    crate::wallets::AddressScheme::NestedSegwit,
                    crate::wallets::AddressScheme::NativeSegwit,
                    crate::wallets::AddressScheme::Taproot,
                ]
            };
        let is_multi_scheme_btc_like = account.account_kind
            == crate::wallets::AccountKind::HdPubkey
            && account.account_model == crate::wallets::AccountModel::Utxo;
        let balance_scheme = if is_multi_scheme_btc_like {
            schemes_to_render
                .iter()
                .copied()
                .find(|scheme| {
                    account
                        .addresses
                        .iter()
                        .any(|address| address.address_scheme == *scheme)
                })
                .or_else(|| schemes_to_render.first().copied())
        } else {
            None
        };
        let account_balance = match data.account_balances.get(&account.id) {
            Some(summary) => summary.clone(),
            None => AddressBalanceSummary::unknown(account.asset_id),
        };
        let account_balance_reliability = data
            .account_balance_reliabilities
            .get(&account.id)
            .cloned()
            .unwrap_or_else(BalanceReliability::finalized);
        let display_balance_reliability = BalanceReliability::from_reasons(
            account_balance_reliability
                .reasons()
                .iter()
                .copied()
                .filter(|reason| {
                    !matches!(
                        reason,
                        BalanceProvisionalReason::HistoricalBackfillInProgress
                            | BalanceProvisionalReason::HistoricalCoverageLimited
                            | BalanceProvisionalReason::PendingLedgerState
                    )
                }),
        );

        for scheme in schemes_to_render {
            let hd_keys: Vec<_> = account
                .hd_keys
                .iter()
                .filter(|key| key.address_scheme == scheme)
                .collect();

            let addresses_for_scheme: Vec<_> = account
                .addresses
                .iter()
                .filter(|addr| addr.address_scheme == scheme)
                .collect();

            if hd_keys.is_empty() && addresses_for_scheme.is_empty() {
                continue;
            }

            let derivation_path = hd_keys.first().map(|key| key.derivation_path.to_string());
            let account_reference_kind = if hd_keys.is_empty() {
                AccountReferenceKind::SingleAddress
            } else {
                AccountReferenceKind::ExtendedPubkey
            };
            let account_reference =
                if account_reference_kind == AccountReferenceKind::ExtendedPubkey {
                    hd_keys
                        .first()
                        .map(|key| key.extended_pubkey.as_str().to_string())
                        .unwrap_or_default()
                } else {
                    addresses_for_scheme
                        .first()
                        .map(|address| address.address.clone())
                        .unwrap_or_default()
                };

            let is_single_address =
                account.account_kind == crate::wallets::AccountKind::SingleAddress;

            let (receive, change) =
                if scheme == crate::wallets::AddressScheme::Standard || is_single_address {
                    let addresses: Vec<AddressView> = addresses_for_scheme
                        .iter()
                        .map(|addr| AddressView {
                            address: addr.address.clone(),
                            derivation_index: addr.derivation_index.unwrap_or(0),
                            balance: data.address_balances.get(&addr.address).cloned(),
                        })
                        .collect();
                    (addresses, Vec::new())
                } else {
                    let receive: Vec<AddressView> = addresses_for_scheme
                        .iter()
                        .filter(|addr| addr.is_receive())
                        .map(|addr| AddressView {
                            address: addr.address.clone(),
                            derivation_index: addr.derivation_index.unwrap_or(0),
                            balance: data.address_balances.get(&addr.address).cloned(),
                        })
                        .collect();

                    let change: Vec<AddressView> = addresses_for_scheme
                        .iter()
                        .filter(|addr| addr.is_change())
                        .map(|addr| AddressView {
                            address: addr.address.clone(),
                            derivation_index: addr.derivation_index.unwrap_or(0),
                            balance: data.address_balances.get(&addr.address).cloned(),
                        })
                        .collect();
                    (receive, change)
                };

            let (scheme_balance, scheme_balance_reliability) =
                if is_multi_scheme_btc_like && balance_scheme != Some(scheme) {
                    (
                        zero_balance_summary(account.asset_id)?,
                        BalanceReliability::finalized(),
                    )
                } else {
                    (account_balance.clone(), display_balance_reliability.clone())
                };

            let recent_transactions = data
                .account_transactions
                .and_then(|rows_by_account| rows_by_account.get(&account.id))
                .map(|account_rows| {
                    account_rows
                        .iter()
                        .map(|row| account_transaction_view(account.asset_id, row))
                        .collect::<Result<Vec<_>, WalletError>>()
                })
                .transpose()?
                .unwrap_or_default();

            let transaction_counts = match data.account_tx_counts.get(&account.id) {
                Some(db_counts) => AccountTransactionCountsView {
                    pending: db_counts.pending,
                    confirmed: db_counts.confirmed,
                    dropped: db_counts.dropped,
                    failed: db_counts.failed,
                    total: db_counts
                        .pending
                        .saturating_add(db_counts.confirmed)
                        .saturating_add(db_counts.dropped)
                        .saturating_add(db_counts.failed),
                },
                None => AccountTransactionCountsView::default(),
            };
            let has_derived_addresses =
                account_reference_kind == AccountReferenceKind::ExtendedPubkey;

            let mut manual_sync =
                native_account_manual_sync_view(account.id, transaction_counts.total, sync.clone());
            if account_state == AccountStateView::Inactive {
                manual_sync.mode = ManualSyncMode::Unavailable;
                manual_sync.slot_effect = ManualSyncSlotEffect::NoCapacity;
                manual_sync.disabled_reason = Some(ManualSyncDisabledReason::AccountInactive);
            }

            accounts_view.push(AccountView::Native(Box::new(NativeAccountView {
                account_id: account_view_id,
                native_account_id: account.id,
                account_number,
                account_state,
                asset: account.asset_id,
                scheme,
                label: account_label.clone(),
                derivation_path,
                account_reference_kind,
                account_reference,
                balance: wallet_balance_view(
                    account.asset_id,
                    account.network,
                    &scheme_balance,
                    scheme_balance_reliability,
                )?,
                transaction_counts,
                has_derived_addresses,
                sync_slot: native_account_sync_slot_view(
                    account.id,
                    sync.sync_slots,
                    sync.active_sync_slot_account_ids,
                    sync.slot_limit,
                    sync.free_balance_unavailable_account_ids,
                ),
                manual_sync,
                addresses: AddressesView { receive, change },
                transactions: recent_transactions,
            })));
        }
    }

    let mut wallet_manual_accounts = data
        .manual_asset_accounts
        .iter()
        .filter(|account| account.wallet_id == wallet.wallet.id)
        .cloned()
        .collect::<Vec<_>>();
    wallet_manual_accounts.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then(left.label.as_str().cmp(right.label.as_str()))
    });
    for manual_account in wallet_manual_accounts {
        let decimal_precision = manual_account.decimal_precision.as_u8();
        let balance_state = match data.custom_account_balances.get(&manual_account.account_id) {
            Some(ManualAssetBalanceState::Known(amount)) => AccountBalanceStateView::Known {
                amount: balance_amount_view(*amount, decimal_precision),
            },
            Some(ManualAssetBalanceState::Unknown) | None => AccountBalanceStateView::Unknown,
        };
        let account_state =
            classified_account_state(classified_accounts, manual_account.account_id);
        accounts_view.push(AccountView::Manual(ManualAssetAccountView {
            account_id: manual_account.account_id,
            account_state,
            label: crate::wallets::display_account_label(&manual_account.label),
            asset_instance_id: crate::asset_views::ManualAssetInstanceIdView {
                asset_id: manual_account.asset_id.as_str().to_string(),
                network_id: manual_account.network_id.as_str().to_string(),
            },
            unit_code: manual_account.unit_code.as_str().to_string(),
            asset_name: manual_account.asset_name,
            network_name: manual_account.network_name,
            decimal_precision,
            symbol: manual_account.symbol,
            balance_state,
            current_value: None,
        }));
    }

    accounts_view.sort_by(sort_account_views);

    let wallet_balances = projected_balances
        .iter()
        .map(|balance| {
            projected_wallet_balance_view(
                balance,
                projected_balance_symbol(balance, &wallet, data.manual_asset_accounts),
            )
        })
        .collect();

    Ok(WalletView {
        id: wallet.wallet.id,
        label: wallet_label,
        master_fingerprint: wallet
            .wallet
            .master_fingerprint
            .as_ref()
            .map(|fp| fp.as_str().to_string()),
        logical_account_count,
        has_accessors,
        balances: wallet_balances,
        accounts: accounts_view,
        value_summary: None,
    })
}

/// Sort key for account display: native before custom, then alphabetically by unit code,
/// then alphabetically by label.
fn compare_account_sort_keys(
    left_is_custom: bool,
    left_unit_code: &str,
    left_label: &str,
    right_is_custom: bool,
    right_unit_code: &str,
    right_label: &str,
) -> std::cmp::Ordering {
    left_is_custom
        .cmp(&right_is_custom)
        .then_with(|| left_unit_code.cmp(right_unit_code))
        .then_with(|| left_label.cmp(right_label))
}

fn account_view_is_custom(v: &AccountView) -> bool {
    matches!(v, AccountView::Custom(_) | AccountView::Manual(_))
}

fn account_view_unit_code(v: &AccountView) -> &str {
    match v {
        AccountView::Native(n) => {
            let synced = crate::asset_capabilities::synced_asset_instance(
                crate::asset_capabilities::synced_asset_instance_id(n.asset),
            );
            match crate::asset_capabilities::asset_instance(&synced.asset_instance_id) {
                Some(instance) => instance.unit_code.as_str(),
                None => "",
            }
        }
        AccountView::Custom(c) => &c.unit_code,
        AccountView::Manual(m) => &m.unit_code,
    }
}

fn account_view_label(v: &AccountView) -> &str {
    match v {
        AccountView::Native(n) => &n.label,
        AccountView::Custom(c) => &c.label,
        AccountView::Manual(m) => &m.label,
    }
}

pub(super) fn sort_account_views(left: &AccountView, right: &AccountView) -> std::cmp::Ordering {
    account_view_is_inactive_supported(left)
        .cmp(&account_view_is_inactive_supported(right))
        .then_with(|| {
            compare_account_sort_keys(
                account_view_is_custom(left),
                account_view_unit_code(left),
                account_view_label(left),
                account_view_is_custom(right),
                account_view_unit_code(right),
                account_view_label(right),
            )
        })
}

fn account_view_is_inactive_supported(v: &AccountView) -> bool {
    match v {
        AccountView::Native(native) => native.account_state == AccountStateView::Inactive,
        AccountView::Manual(manual) => manual.account_state == AccountStateView::Inactive,
        AccountView::Custom(_) => false,
    }
}

pub(super) fn sort_report_account_rows(
    left: &WalletReportAccountRow,
    right: &WalletReportAccountRow,
) -> std::cmp::Ordering {
    compare_account_sort_keys(
        left.catalog_asset_key.is_none(),
        &left.unit_code,
        &left.account_label,
        right.catalog_asset_key.is_none(),
        &right.unit_code,
        &right.account_label,
    )
}
