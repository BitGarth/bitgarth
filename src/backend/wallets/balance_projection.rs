#![cfg(feature = "server")]

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use chrono::Utc;

use crate::account_limits::{AccountActivationState, ClassifiedAccount};
use crate::amounts::UnsignedAmount;
use crate::asset_capabilities::{
    asset_instance, network_slug, sync_provider, synced_asset_instance, synced_asset_instance_id,
};
use crate::balance_reliability::{BalanceProvisionalReason, BalanceReliability};
use crate::db::{ManualAssetBalanceState, WalletSummaryBundle};
use crate::models::UserId;
use crate::payments::types::{EntitlementTier, FeatureEntitlements};
use crate::transactions::NativeBalanceState;
use crate::wallets::{WalletAccountId, WalletWithDetails};

use super::helpers::internal_error;
use super::types::WalletError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WalletBalanceProjection {
    pub(crate) wallets: Vec<ProjectedWalletBalance>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectedWalletBalance {
    pub(crate) id: crate::wallets::WalletId,
    pub(crate) name: String,
    pub(crate) balances: Vec<ProjectedAssetBalance>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectedAssetBalance {
    pub(crate) asset_id: String,
    pub(crate) network_id: String,
    pub(crate) unit: String,
    pub(crate) amount: Option<UnsignedAmount>,
    pub(crate) decimal_precision: u8,
    pub(crate) status: ProjectedBalanceStatus,
    pub(crate) reasons: Vec<ProjectedBalanceReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectedBalanceStatus {
    Final,
    Provisional,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ProjectedBalanceReason {
    FirstSuccessfulSyncPending,
    InactiveAccountNotSyncing,
}

#[derive(Debug, Clone)]
pub(super) struct BalanceContributor {
    pub(super) asset_id: String,
    pub(super) network_id: String,
    pub(super) unit: String,
    pub(super) amount: Option<UnsignedAmount>,
    pub(super) decimal_precision: u8,
    pub(super) inactive: bool,
    pub(super) reliability: BalanceReliability,
}

#[derive(Debug)]
struct BalanceAccumulator {
    unit: String,
    contributors: Vec<BalanceContributor>,
}

pub(super) fn aggregate_contributors(
    contributors: Vec<BalanceContributor>,
) -> Result<Vec<ProjectedAssetBalance>, WalletError> {
    let mut grouped = BTreeMap::<(String, String), BalanceAccumulator>::new();
    for contributor in contributors {
        let key = (contributor.asset_id.clone(), contributor.network_id.clone());
        match grouped.entry(key) {
            std::collections::btree_map::Entry::Occupied(mut occupied) => {
                if occupied.get().unit != contributor.unit {
                    return Err(internal_error(
                        "wallet_balance_projection_identity",
                        "one asset identity has conflicting unit codes",
                    ));
                }
                occupied.get_mut().contributors.push(contributor);
            }
            std::collections::btree_map::Entry::Vacant(vacant) => {
                vacant.insert(BalanceAccumulator {
                    unit: contributor.unit.clone(),
                    contributors: vec![contributor],
                });
            }
        }
    }

    grouped
        .into_iter()
        .map(|((asset_id, network_id), group)| {
            let decimal_precision = group
                .contributors
                .iter()
                .map(|contributor| contributor.decimal_precision)
                .max()
                .unwrap_or(0);
            if group
                .contributors
                .iter()
                .any(|contributor| contributor.amount.is_none())
            {
                return Ok(ProjectedAssetBalance {
                    asset_id,
                    network_id,
                    unit: group.unit,
                    amount: None,
                    decimal_precision,
                    status: ProjectedBalanceStatus::Unknown,
                    reasons: Vec::new(),
                });
            }

            let mut amount = UnsignedAmount::zero();
            let mut reasons = BTreeSet::new();
            for contributor in group.contributors {
                let Some(value) = contributor.amount else {
                    return Err(internal_error(
                        "wallet_balance_projection_state",
                        "unknown contributor reached known aggregation",
                    ));
                };
                let scale = u32::from(decimal_precision - contributor.decimal_precision);
                let multiplier = 10_u128.checked_pow(scale).ok_or_else(|| {
                    internal_error("wallet_balance_projection_scale", "decimal scale overflow")
                })?;
                let normalized = value.value().checked_mul(multiplier).ok_or_else(|| {
                    internal_error(
                        "wallet_balance_projection_scale",
                        "balance multiplication overflow",
                    )
                })?;
                amount = amount
                    .checked_add(UnsignedAmount::from_u128(normalized))
                    .map_err(|error| internal_error("wallet_balance_projection_add", error))?;

                if contributor
                    .reliability
                    .reasons()
                    .contains(&BalanceProvisionalReason::FirstSuccessfulSyncPending)
                {
                    reasons.insert(ProjectedBalanceReason::FirstSuccessfulSyncPending);
                }
                if contributor.inactive {
                    reasons.insert(ProjectedBalanceReason::InactiveAccountNotSyncing);
                }
            }
            let reasons = reasons.into_iter().collect::<Vec<_>>();
            Ok(ProjectedAssetBalance {
                asset_id,
                network_id,
                unit: group.unit,
                amount: Some(amount),
                decimal_precision,
                status: if reasons.is_empty() {
                    ProjectedBalanceStatus::Final
                } else {
                    ProjectedBalanceStatus::Provisional
                },
                reasons,
            })
        })
        .collect()
}

pub(super) fn free_balance_unavailable_account_ids(
    wallets: &[WalletWithDetails],
    tier: &EntitlementTier,
) -> HashSet<crate::wallets::DigitalAssetAccountId> {
    if *tier != EntitlementTier::Free {
        return HashSet::new();
    }

    wallets
        .iter()
        .flat_map(|wallet| wallet.accounts.iter())
        .filter(|account| {
            let synced = synced_asset_instance(synced_asset_instance_id(account.asset_id));
            !sync_provider(synced.default_sync_provider)
                .capabilities
                .supports_balance_only_sync
        })
        .map(|account| account.id)
        .collect()
}

fn account_is_inactive(
    classified_accounts: &HashMap<WalletAccountId, AccountActivationState>,
    account_id: WalletAccountId,
) -> bool {
    classified_accounts.get(&account_id) == Some(&AccountActivationState::Inactive)
}

fn project_loaded_balances(
    summary: WalletSummaryBundle,
    manual_balances: HashMap<WalletAccountId, ManualAssetBalanceState>,
    entitlements: &FeatureEntitlements,
    classifications: Vec<ClassifiedAccount>,
) -> Result<WalletBalanceProjection, WalletError> {
    let classifications = classifications
        .into_iter()
        .map(|account| (account.account_id, account.state))
        .collect::<HashMap<_, _>>();
    let unavailable_native =
        free_balance_unavailable_account_ids(&summary.wallets, &entitlements.tier);

    let mut wallets = summary
        .wallets
        .iter()
        .map(|wallet| {
            let mut contributors = Vec::new();
            for account in &wallet.accounts {
                let synced = synced_asset_instance(synced_asset_instance_id(account.asset_id));
                let instance = asset_instance(&synced.asset_instance_id).ok_or_else(|| {
                    internal_error(
                        "wallet_balance_projection_asset_instance",
                        "synced asset instance is not registered",
                    )
                })?;
                let amount = if unavailable_native.contains(&account.id) {
                    None
                } else {
                    match summary
                        .account_balances
                        .get(&account.id)
                        .map(|row| row.confirmed)
                    {
                        Some(NativeBalanceState::KnownAmount(amount)) => Some(amount),
                        Some(NativeBalanceState::Unknown) | None => None,
                        Some(NativeBalanceState::CanonicalZero) => {
                            return Err(internal_error(
                                "wallet_balance_projection_state",
                                "canonical zero is only valid at historical boundaries",
                            ));
                        }
                    }
                };
                contributors.push(BalanceContributor {
                    asset_id: instance.id.asset_id.as_str().to_string(),
                    network_id: network_slug(instance.id.network_id).to_string(),
                    unit: instance.unit_code.as_str().to_string(),
                    amount,
                    decimal_precision: instance.decimal_precision,
                    inactive: account_is_inactive(
                        &classifications,
                        WalletAccountId::from(account.id),
                    ),
                    reliability: summary
                        .account_balance_reliabilities
                        .get(&account.id)
                        .cloned()
                        .unwrap_or_else(BalanceReliability::finalized),
                });
            }

            for account in summary
                .manual_asset_accounts
                .iter()
                .filter(|account| account.wallet_id == wallet.wallet.id)
            {
                contributors.push(BalanceContributor {
                    asset_id: account.asset_id.as_str().to_string(),
                    network_id: account.network_id.as_str().to_string(),
                    unit: account.unit_code.as_str().to_string(),
                    amount: match manual_balances.get(&account.account_id) {
                        Some(ManualAssetBalanceState::Known(amount)) => Some(*amount),
                        Some(ManualAssetBalanceState::Unknown) | None => None,
                    },
                    decimal_precision: account.decimal_precision.as_u8(),
                    inactive: account_is_inactive(&classifications, account.account_id),
                    reliability: BalanceReliability::finalized(),
                });
            }

            Ok(ProjectedWalletBalance {
                id: wallet.wallet.id,
                name: crate::wallets::display_wallet_label(&wallet.wallet.label),
                balances: aggregate_contributors(contributors)?,
            })
        })
        .collect::<Result<Vec<_>, WalletError>>()?;
    wallets.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.id.to_string().cmp(&right.id.to_string()))
    });
    Ok(WalletBalanceProjection { wallets })
}

#[cfg(any(
    all(not(test), not(feature = "desktop")),
    all(test, not(bitgarth_db_unit_only))
))]
pub(crate) fn load_wallet_balance_projection(
    user_id: UserId,
) -> Result<WalletBalanceProjection, WalletError> {
    let summary = crate::db::load_wallet_summary_bundle(user_id)
        .map_err(|error| internal_error("wallet_balance_projection", error))?;
    load_wallet_balance_projection_from_summary(user_id, summary)
}

pub(crate) fn load_wallet_balance_projection_from_summary(
    user_id: UserId,
    summary: WalletSummaryBundle,
) -> Result<WalletBalanceProjection, WalletError> {
    let manual_balances = crate::db::load_manual_asset_current_balances(user_id)
        .map_err(|error| internal_error("wallet_balance_projection", error))?;
    let entitlements =
        crate::payments::entitlements::load_feature_entitlements(user_id, Utc::now())
            .map_err(|error| internal_error("wallet_balance_projection", error))?;
    let classifications = crate::db::account_limits::classify_supported_accounts_for_user(
        user_id,
        usize::from(entitlements.sync_account_slots_limit),
    )
    .map_err(|error| internal_error("wallet_balance_projection", error))?;

    project_loaded_balances(summary, manual_balances, &entitlements, classifications)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::balance_reliability::BalanceProvisionalReason;

    fn contributor(
        asset_id: &str,
        network_id: &str,
        unit: &str,
        value: Option<u128>,
        decimal_precision: u8,
    ) -> BalanceContributor {
        BalanceContributor {
            asset_id: asset_id.to_string(),
            network_id: network_id.to_string(),
            unit: unit.to_string(),
            amount: value.map(UnsignedAmount::from_u128),
            decimal_precision,
            inactive: false,
            reliability: BalanceReliability::finalized(),
        }
    }

    #[test]
    fn aggregates_native_and_manual_contributors_with_the_same_identity() {
        let balances = aggregate_contributors(vec![
            contributor("bitcoin", "bitcoin-mainnet", "BTC", Some(4), 0),
            contributor("bitcoin", "bitcoin-mainnet", "BTC", Some(6), 0),
        ])
        .unwrap();

        assert_eq!(balances.len(), 1);
        assert_eq!(balances[0].amount, Some(UnsignedAmount::from_u128(10)));
    }

    #[test]
    fn normalizes_to_the_greatest_precision_before_adding() {
        let balances = aggregate_contributors(vec![
            contributor("asset", "network", "UNIT", Some(12), 1),
            contributor("asset", "network", "UNIT", Some(345), 2),
        ])
        .unwrap();

        assert_eq!(balances[0].decimal_precision, 2);
        assert_eq!(balances[0].amount, Some(UnsignedAmount::from_u128(465)));
    }

    #[test]
    fn rejects_checked_scaling_and_addition_overflow() {
        assert!(
            aggregate_contributors(vec![
                contributor("asset", "network", "UNIT", Some(u128::MAX), 0),
                contributor("asset", "network", "UNIT", Some(0), 1),
            ])
            .is_err()
        );
        assert!(
            aggregate_contributors(vec![
                contributor("asset", "network", "UNIT", Some(u128::MAX), 0),
                contributor("asset", "network", "UNIT", Some(1), 0),
            ])
            .is_err()
        );
    }

    #[test]
    fn rejects_conflicting_units_for_one_identity() {
        assert!(
            aggregate_contributors(vec![
                contributor("asset", "network", "ONE", Some(1), 0),
                contributor("asset", "network", "TWO", Some(1), 0),
            ])
            .is_err()
        );
    }

    #[test]
    fn unknown_dominates_known_values_and_clears_reasons() {
        let mut inactive = contributor("asset", "network", "UNIT", Some(1), 0);
        inactive.inactive = true;
        let balances = aggregate_contributors(vec![
            inactive,
            contributor("asset", "network", "UNIT", None, 0),
        ])
        .unwrap();

        assert_eq!(balances[0].amount, None);
        assert_eq!(balances[0].status, ProjectedBalanceStatus::Unknown);
        assert!(balances[0].reasons.is_empty());
    }

    #[test]
    fn filters_and_orders_public_reasons() {
        let mut row = contributor("asset", "network", "UNIT", Some(1), 0);
        row.inactive = true;
        row.reliability = BalanceReliability::from_reasons([
            BalanceProvisionalReason::PendingLedgerState,
            BalanceProvisionalReason::HistoricalCoverageLimited,
            BalanceProvisionalReason::FirstSuccessfulSyncPending,
        ]);
        let balances = aggregate_contributors(vec![row]).unwrap();

        assert_eq!(balances[0].status, ProjectedBalanceStatus::Provisional);
        assert_eq!(
            balances[0].reasons,
            vec![
                ProjectedBalanceReason::FirstSuccessfulSyncPending,
                ProjectedBalanceReason::InactiveAccountNotSyncing,
            ]
        );
    }

    #[test]
    fn sorts_balances_by_stable_identity() {
        let balances = aggregate_contributors(vec![
            contributor("zeta", "a-network", "Z", Some(1), 0),
            contributor("alpha", "z-network", "A", Some(1), 0),
            contributor("alpha", "a-network", "A", Some(1), 0),
        ])
        .unwrap();

        let keys = balances
            .iter()
            .map(|balance| (balance.asset_id.as_str(), balance.network_id.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            vec![
                ("alpha", "a-network"),
                ("alpha", "z-network"),
                ("zeta", "a-network"),
            ]
        );
    }

    #[test]
    fn empty_contributors_produce_no_balances() {
        assert!(aggregate_contributors(Vec::new()).unwrap().is_empty());
    }

    #[cfg(feature = "db-tests")]
    #[test]
    fn over_limit_database_fixture_keeps_web_and_projection_balances_in_parity() {
        use chrono::TimeZone;

        use crate::account_limits::{ClassifiedAccount, SupportedAccountKind};
        use crate::backend::wallets::conversions::{
            NativeAccountManualSyncContext, WalletAccountData, convert_wallet_to_view,
        };
        use crate::db::{DbError, setup_test_user, unique_user_id, with_user_db_mut};
        use crate::ethereum::{EthAddress, RawEthAddress};
        use crate::payments::types::EntitlementTier;
        use crate::wallets::WalletAccountId;

        let user_id = unique_user_id();
        setup_test_user(user_id);
        let now = chrono::Utc.with_ymd_and_hms(2026, 7, 31, 12, 0, 0).unwrap();
        let address = EthAddress::parse(&RawEthAddress::new(
            "0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed".to_string(),
        ))
        .unwrap();
        let native = crate::db::create_eth_wallet_account_fixture(
            user_id,
            &address,
            "Projection Wallet",
            now,
        );
        let manual_account_id = WalletAccountId::new();
        with_user_db_mut(user_id, |connection| -> Result<(), DbError> {
            let timestamp = now.to_rfc3339();
            connection
                .execute(
                    "INSERT INTO manual_asset_accounts
                     (id, wallet_id, label, label_key, asset_id, network_id, decimal_precision,
                      unit_code, symbol, asset_name, network_name, coingecko_id, asset_source,
                      precision_source, coingecko_platform_id, provider_platform_asset_ref,
                      created_at, updated_at)
                     VALUES (?1, ?2, 'Manual ETH', 'manual eth', 'ethereum',
                             'ethereum-mainnet', 18, 'ETH', NULL, 'Ethereum', 'Ethereum',
                             'ethereum', 'bitgarth_catalog', 'bitgarth_catalog', NULL, NULL,
                             ?3, ?3)",
                    rusqlite::params![
                        manual_account_id.to_string(),
                        native.wallet_id.to_string(),
                        timestamp,
                    ],
                )
                .map_err(|error| DbError::new(error.to_string()))?;
            Ok(())
        })
        .unwrap();

        let mut summary = crate::db::load_wallet_summary_bundle(user_id).unwrap();
        let native_account_id = summary.wallets[0].accounts[0].id;
        summary.account_balances.insert(
            native_account_id,
            crate::transactions::AddressBalanceSummary::known(
                crate::wallets::SyncedAssetId::Ethereum,
                UnsignedAmount::from_u128(1_000_000_000_000_000_000),
            ),
        );
        let manual_balances = HashMap::from([(
            manual_account_id,
            ManualAssetBalanceState::Known(UnsignedAmount::from_u128(2_000_000_000_000_000_000)),
        )]);
        let classifications = vec![
            ClassifiedAccount {
                account_id: WalletAccountId::from(native_account_id),
                kind: SupportedAccountKind::Native,
                state: AccountActivationState::Active,
            },
            ClassifiedAccount {
                account_id: manual_account_id,
                kind: SupportedAccountKind::ManualAsset,
                state: AccountActivationState::Inactive,
            },
        ];
        let mut entitlements = FeatureEntitlements::free();
        entitlements.tier = EntitlementTier::Premium;
        let projection = project_loaded_balances(
            summary.clone(),
            manual_balances.clone(),
            &entitlements,
            classifications.clone(),
        )
        .unwrap();
        let projected_wallet = &projection.wallets[0];
        assert_eq!(projected_wallet.balances.len(), 1);
        let projected = &projected_wallet.balances[0];
        assert_eq!(projected.asset_id, "ethereum");
        assert_eq!(projected.network_id, "ethereum-mainnet");
        assert_eq!(
            projected.amount,
            Some(UnsignedAmount::from_u128(3_000_000_000_000_000_000))
        );
        assert_eq!(projected.status, ProjectedBalanceStatus::Provisional);
        assert_eq!(
            projected.reasons,
            vec![
                ProjectedBalanceReason::FirstSuccessfulSyncPending,
                ProjectedBalanceReason::InactiveAccountNotSyncing,
            ]
        );

        let wallet = summary.wallets[0].clone();
        let web = convert_wallet_to_view(
            wallet,
            &projected_wallet.balances,
            &WalletAccountData {
                manual_asset_accounts: &summary.manual_asset_accounts,
                address_balances: &summary.address_balances,
                account_balances: &summary.account_balances,
                account_balance_reliabilities: &summary.account_balance_reliabilities,
                custom_account_balances: &manual_balances,
                account_transactions: None,
                account_tx_counts: &summary.account_tx_counts,
            },
            &NativeAccountManualSyncContext {
                sync_slots: &HashMap::new(),
                active_sync_slot_account_ids: &HashSet::new(),
                slot_limit: 1,
                tier: EntitlementTier::Premium,
                historical_backfill_enabled: false,
                historical_backfill_transactions_per_account: 0,
                free_balance_unavailable_account_ids: &HashSet::new(),
            },
            &classifications,
        )
        .unwrap();
        assert_eq!(web.balances[0].asset_id, projected.asset_id);
        assert_eq!(web.balances[0].network_id, projected.network_id);
        assert_eq!(
            web.balances[0].balance_reliability,
            BalanceReliability::from_reasons([
                BalanceProvisionalReason::FirstSuccessfulSyncPending,
                BalanceProvisionalReason::InactiveAccountNotSyncing,
            ])
        );
        assert_eq!(
            web.balances[0].balance_state,
            super::super::types::AccountBalanceStateView::Known {
                amount: super::super::types::BalanceAmountView {
                    raw_value: "3000000000000000000".to_string(),
                    formatted_value: "3".to_string(),
                },
            }
        );
    }
}
