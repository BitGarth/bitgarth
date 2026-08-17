#![cfg_attr(
    not(feature = "server"),
    allow(
        dead_code,
        reason = "Transaction sync domain types are primarily exercised on server paths"
    )
)]

use super::types::*;
use crate::amounts::{AmountError, UnsignedAmount};
use crate::wallets::{
    DigitalAssetAccountId, DigitalAssetAddressId, Network, SyncedAssetId, WalletId,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "amount", rename_all = "snake_case")]
pub(crate) enum NativeBalanceState {
    KnownAmount(UnsignedAmount),
    CanonicalZero,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AddressBalanceSummary {
    pub asset_id: SyncedAssetId,
    pub confirmed: NativeBalanceState,
}

impl AddressBalanceSummary {
    pub(crate) fn known(asset_id: SyncedAssetId, amount: UnsignedAmount) -> Self {
        Self {
            asset_id,
            confirmed: NativeBalanceState::KnownAmount(amount),
        }
    }

    pub(crate) fn unknown(asset_id: SyncedAssetId) -> Self {
        Self {
            asset_id,
            confirmed: NativeBalanceState::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AddressBalanceEntry {
    pub address_id: DigitalAssetAddressId,
    pub address: TrackedAddress,
    pub derivation_change: Option<u32>,
    pub derivation_index: Option<u32>,
    pub balance: AddressBalanceSummary,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AccountBalanceEntry {
    pub wallet_id: Option<WalletId>,
    pub account_id: DigitalAssetAccountId,
    pub asset_id: SyncedAssetId,
    pub network: Network,
    pub asset_linked_at: DateTime<Utc>,
    pub account_label: Option<String>,
    pub account_balance: AddressBalanceSummary,
    pub addresses: Vec<AddressBalanceEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AccountTransactionDirection {
    Incoming,
    Outgoing,
    SelfTransfer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AccountTransactionEntry {
    pub account_id: DigitalAssetAccountId,
    pub tx_hash: String,
    pub status: ChainTransactionStatus,
    pub direction: AccountTransactionDirection,
    pub transfer_kind: Option<String>,
    pub value: UnsignedAmount,
    pub fee: Option<UnsignedAmount>,
    pub from_address: Option<TrackedAddress>,
    pub to_address: Option<TrackedAddress>,
    pub block_time: Option<DateTime<Utc>>,
}

/// Transaction counts by status for an account, computed directly from the
/// database without any row limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct AccountTransactionCounts {
    pub pending: u32,
    pub confirmed: u32,
    pub dropped: u32,
    pub failed: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AssetNetworkTotal {
    pub asset_id: SyncedAssetId,
    pub network: Network,
    pub confirmed: NativeBalanceState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AccountBalancesResponse {
    pub accounts: Vec<AccountBalanceEntry>,
    pub totals: Vec<AssetNetworkTotal>,
}

pub(crate) fn sum_address_balances(
    asset_id: SyncedAssetId,
    balances: &[AddressBalanceSummary],
) -> Result<AddressBalanceSummary, AmountError> {
    for balance in balances {
        if balance.asset_id != asset_id {
            return Err(AmountError::ParseError {
                input: balance.asset_id.as_str().to_string(),
                reason: format!(
                    "sum_address_balances expected asset {}, got {}",
                    asset_id.as_str(),
                    balance.asset_id.as_str()
                ),
            });
        }
    }
    if balances
        .iter()
        .any(|balance| balance.confirmed == NativeBalanceState::CanonicalZero)
    {
        return Err(AmountError::ParseError {
            input: "canonical_zero".to_string(),
            reason: "canonical zero is only valid at historical boundaries".to_string(),
        });
    }
    if balances
        .iter()
        .any(|balance| balance.confirmed == NativeBalanceState::Unknown)
    {
        return Ok(AddressBalanceSummary::unknown(asset_id));
    }

    let mut confirmed_total = UnsignedAmount::zero();
    for balance in balances {
        match balance.confirmed {
            NativeBalanceState::KnownAmount(amount) => {
                confirmed_total = confirmed_total.checked_add(amount)?;
            }
            NativeBalanceState::CanonicalZero => {
                unreachable!("canonical zero was rejected before summation");
            }
            NativeBalanceState::Unknown => unreachable!("unknown was handled before summation"),
        }
    }

    Ok(AddressBalanceSummary::known(asset_id, confirmed_total))
}

pub(crate) fn aggregate_address_balances(
    accounts: &[AccountBalanceEntry],
) -> Result<Vec<AssetNetworkTotal>, AmountError> {
    for account in accounts {
        if account.account_balance.asset_id != account.asset_id {
            return Err(AmountError::ParseError {
                input: account.account_balance.asset_id.as_str().to_string(),
                reason: format!(
                    "account balance asset {} does not match account asset {}",
                    account.account_balance.asset_id.as_str(),
                    account.asset_id.as_str()
                ),
            });
        }
        if account.account_balance.confirmed == NativeBalanceState::CanonicalZero {
            return Err(AmountError::ParseError {
                input: "canonical_zero".to_string(),
                reason: "canonical zero is only valid at historical boundaries".to_string(),
            });
        }
    }

    let mut unknown_groups = Vec::new();
    for account in accounts {
        let group = (account.asset_id, account.network);
        if account.account_balance.confirmed == NativeBalanceState::Unknown
            && !unknown_groups.contains(&group)
        {
            unknown_groups.push(group);
        }
    }

    let mut totals: Vec<AssetNetworkTotal> = Vec::new();
    for account in accounts {
        let group_is_unknown = unknown_groups.contains(&(account.asset_id, account.network));
        if let Some(existing) = totals
            .iter_mut()
            .find(|total| total.asset_id == account.asset_id && total.network == account.network)
        {
            if !group_is_unknown {
                let (NativeBalanceState::KnownAmount(left), NativeBalanceState::KnownAmount(right)) =
                    (existing.confirmed, account.account_balance.confirmed)
                else {
                    unreachable!("non-known states were handled before current aggregation");
                };
                existing.confirmed = NativeBalanceState::KnownAmount(left.checked_add(right)?);
            }
        } else {
            totals.push(AssetNetworkTotal {
                asset_id: account.asset_id,
                network: account.network,
                confirmed: if group_is_unknown {
                    NativeBalanceState::Unknown
                } else {
                    account.account_balance.confirmed
                },
            });
        }
    }

    Ok(totals)
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn unsigned_amount_rejects_negative_values() {
        assert!(matches!(
            UnsignedAmount::try_from_i64(-1),
            Err(AmountError::NegativeNotAllowed { .. })
        ));
    }

    #[test]
    fn address_balance_summary_carries_confirmed_state() {
        let confirmed = match UnsignedAmount::try_from_i64(50_000) {
            Ok(value) => value,
            Err(err) => panic!("expected confirmed sats: {err}"),
        };
        let summary = AddressBalanceSummary::known(SyncedAssetId::Bitcoin, confirmed);

        assert_eq!(summary.asset_id, SyncedAssetId::Bitcoin);
        assert_eq!(
            summary.confirmed,
            NativeBalanceState::KnownAmount(confirmed)
        );
    }

    fn make_balance(asset_id: SyncedAssetId, confirmed: i64) -> AddressBalanceSummary {
        AddressBalanceSummary::known(
            asset_id,
            UnsignedAmount::try_from_i64(confirmed).expect("valid confirmed"),
        )
    }

    #[test]
    fn sum_address_balances_empty_returns_zero() {
        let result = sum_address_balances(SyncedAssetId::Bitcoin, &[]).expect("should succeed");
        assert_eq!(result.asset_id, SyncedAssetId::Bitcoin);
        assert_eq!(
            result.confirmed,
            NativeBalanceState::KnownAmount(UnsignedAmount::zero())
        );
    }

    #[test]
    fn sum_address_balances_multiple_entries() {
        let balances = vec![
            make_balance(SyncedAssetId::Bitcoin, 50_000),
            make_balance(SyncedAssetId::Bitcoin, 30_000),
        ];
        let result =
            sum_address_balances(SyncedAssetId::Bitcoin, &balances).expect("should succeed");
        assert_eq!(result.asset_id, SyncedAssetId::Bitcoin);
        assert_eq!(
            result.confirmed,
            NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(80_000))
        );
    }

    #[test]
    fn sum_address_balances_preserves_unknown() {
        let balances = vec![
            make_balance(SyncedAssetId::Bitcoin, 50_000),
            AddressBalanceSummary {
                asset_id: SyncedAssetId::Bitcoin,
                confirmed: NativeBalanceState::Unknown,
            },
        ];

        let result =
            sum_address_balances(SyncedAssetId::Bitcoin, &balances).expect("should succeed");
        assert_eq!(result.confirmed, NativeBalanceState::Unknown);
    }

    #[test]
    fn sum_address_balances_unknown_dominates_earlier_overflow() {
        let balances = vec![
            AddressBalanceSummary::known(
                SyncedAssetId::Bitcoin,
                UnsignedAmount::from_u128(u128::MAX),
            ),
            AddressBalanceSummary::known(SyncedAssetId::Bitcoin, UnsignedAmount::from_u128(1)),
            AddressBalanceSummary {
                asset_id: SyncedAssetId::Bitcoin,
                confirmed: NativeBalanceState::Unknown,
            },
        ];

        let result = sum_address_balances(SyncedAssetId::Bitcoin, &balances)
            .expect("unknown should dominate");
        assert_eq!(result.confirmed, NativeBalanceState::Unknown);
    }

    #[test]
    fn sum_address_balances_canonical_zero_before_unknown_is_error() {
        let balances = vec![
            AddressBalanceSummary {
                asset_id: SyncedAssetId::Bitcoin,
                confirmed: NativeBalanceState::CanonicalZero,
            },
            AddressBalanceSummary {
                asset_id: SyncedAssetId::Bitcoin,
                confirmed: NativeBalanceState::Unknown,
            },
        ];

        let result = sum_address_balances(SyncedAssetId::Bitcoin, &balances);
        assert!(matches!(result, Err(AmountError::ParseError { .. })));
    }

    #[test]
    fn sum_address_balances_unknown_before_canonical_zero_is_error() {
        let balances = vec![
            AddressBalanceSummary {
                asset_id: SyncedAssetId::Bitcoin,
                confirmed: NativeBalanceState::Unknown,
            },
            AddressBalanceSummary {
                asset_id: SyncedAssetId::Bitcoin,
                confirmed: NativeBalanceState::CanonicalZero,
            },
        ];

        let result = sum_address_balances(SyncedAssetId::Bitcoin, &balances);
        assert!(matches!(result, Err(AmountError::ParseError { .. })));
    }

    #[test]
    fn sum_address_balances_rejects_mismatched_asset() {
        let balances = vec![
            make_balance(SyncedAssetId::Bitcoin, 50_000),
            make_balance(SyncedAssetId::Ethereum, 30_000),
        ];
        let result = sum_address_balances(SyncedAssetId::Bitcoin, &balances);
        assert!(matches!(result, Err(AmountError::ParseError { .. })));
    }

    #[test]
    fn aggregate_address_balances_groups_by_asset_network() {
        let accounts = vec![
            AccountBalanceEntry {
                wallet_id: Some(WalletId::new()),
                account_id: DigitalAssetAccountId::new(),
                asset_id: SyncedAssetId::Bitcoin,
                network: Network::Mainnet,
                asset_linked_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
                account_label: None,
                account_balance: make_balance(SyncedAssetId::Bitcoin, 50_000),
                addresses: Vec::new(),
            },
            AccountBalanceEntry {
                wallet_id: Some(WalletId::new()),
                account_id: DigitalAssetAccountId::new(),
                asset_id: SyncedAssetId::Bitcoin,
                network: Network::Mainnet,
                asset_linked_at: "2026-02-16T10:01:00Z".parse().expect("valid datetime"),
                account_label: None,
                account_balance: make_balance(SyncedAssetId::Bitcoin, 30_000),
                addresses: Vec::new(),
            },
        ];

        let totals = aggregate_address_balances(&accounts).expect("aggregation should succeed");
        assert_eq!(totals.len(), 1);
        assert_eq!(
            totals[0].confirmed,
            NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(80_000))
        );
    }

    #[test]
    fn aggregate_address_balances_separates_different_networks() {
        let accounts = vec![
            AccountBalanceEntry {
                wallet_id: Some(WalletId::new()),
                account_id: DigitalAssetAccountId::new(),
                asset_id: SyncedAssetId::Bitcoin,
                network: Network::Mainnet,
                asset_linked_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
                account_label: None,
                account_balance: make_balance(SyncedAssetId::Bitcoin, 50_000),
                addresses: Vec::new(),
            },
            AccountBalanceEntry {
                wallet_id: Some(WalletId::new()),
                account_id: DigitalAssetAccountId::new(),
                asset_id: SyncedAssetId::Bitcoin,
                network: Network::Testnet,
                asset_linked_at: "2026-02-16T10:01:00Z".parse().expect("valid datetime"),
                account_label: None,
                account_balance: make_balance(SyncedAssetId::Bitcoin, 20_000),
                addresses: Vec::new(),
            },
        ];

        let totals = aggregate_address_balances(&accounts).expect("aggregation should succeed");
        assert_eq!(totals.len(), 2);
    }

    #[test]
    fn aggregate_address_balances_empty_returns_empty() {
        let totals = aggregate_address_balances(&[]).expect("aggregation should succeed");
        assert!(totals.is_empty());
    }

    #[test]
    fn aggregate_address_balances_rejects_mismatched_account_balance_asset() {
        let accounts = vec![AccountBalanceEntry {
            wallet_id: Some(WalletId::new()),
            account_id: DigitalAssetAccountId::new(),
            asset_id: SyncedAssetId::Bitcoin,
            network: Network::Mainnet,
            asset_linked_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
            account_label: None,
            account_balance: make_balance(SyncedAssetId::Ethereum, 1),
            addresses: Vec::new(),
        }];

        let result = aggregate_address_balances(&accounts);
        assert!(matches!(result, Err(AmountError::ParseError { .. })));
    }

    #[test]
    fn aggregate_address_balances_returns_overflow_error() {
        let max = UnsignedAmount::from_u128(u128::MAX);
        let accounts = vec![
            AccountBalanceEntry {
                wallet_id: Some(WalletId::new()),
                account_id: DigitalAssetAccountId::new(),
                asset_id: SyncedAssetId::Bitcoin,
                network: Network::Mainnet,
                asset_linked_at: "2026-02-16T10:00:00Z".parse().expect("valid datetime"),
                account_label: None,
                account_balance: AddressBalanceSummary::known(SyncedAssetId::Bitcoin, max),
                addresses: Vec::new(),
            },
            AccountBalanceEntry {
                wallet_id: Some(WalletId::new()),
                account_id: DigitalAssetAccountId::new(),
                asset_id: SyncedAssetId::Bitcoin,
                network: Network::Mainnet,
                asset_linked_at: "2026-02-16T10:01:00Z".parse().expect("valid datetime"),
                account_label: None,
                account_balance: AddressBalanceSummary::known(
                    SyncedAssetId::Bitcoin,
                    UnsignedAmount::from_u128(1),
                ),
                addresses: Vec::new(),
            },
        ];

        let result = aggregate_address_balances(&accounts);
        assert!(matches!(result, Err(AmountError::Overflow)));
    }

    #[test]
    fn aggregate_address_balances_unknown_dominates_earlier_overflow() {
        let wallet_id = Some(WalletId::new());
        let balance_entry = |confirmed, linked_at: &str| AccountBalanceEntry {
            wallet_id,
            account_id: DigitalAssetAccountId::new(),
            asset_id: SyncedAssetId::Bitcoin,
            network: Network::Mainnet,
            asset_linked_at: linked_at.parse().expect("valid datetime"),
            account_label: None,
            account_balance: AddressBalanceSummary {
                asset_id: SyncedAssetId::Bitcoin,
                confirmed,
            },
            addresses: Vec::new(),
        };
        let accounts = vec![
            balance_entry(
                NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(u128::MAX)),
                "2026-02-16T10:00:00Z",
            ),
            balance_entry(
                NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(1)),
                "2026-02-16T10:01:00Z",
            ),
            balance_entry(NativeBalanceState::Unknown, "2026-02-16T10:02:00Z"),
        ];

        let totals =
            aggregate_address_balances(&accounts).expect("unknown should dominate overflow");
        assert_eq!(totals[0].confirmed, NativeBalanceState::Unknown);
    }
}
