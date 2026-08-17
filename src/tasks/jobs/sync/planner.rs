use crate::db::{AccountSyncBundle, SyncAddress};
use crate::transactions::{TransactionCount, TransactionSyncResult};
use crate::wallets::{DigitalAssetAccountId, DigitalAssetAddressId};
use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};

use super::context::{
    ADDRESS_FAILURE_THRESHOLD, SyncIterationStopReason, SyncPlannerPriorityTier, is_first_sync,
    is_successful_balance_refresh_fresh,
};
use super::gate::{MempoolHistoryPolicy, mempool_history_requires_first_page_restart};
use super::integrations::unfinished_backfill_state;

#[derive(Debug, Clone, Copy)]
pub(super) struct SyncPlannerInput<'a> {
    pub(super) now_utc: DateTime<Utc>,
    pub(super) mempool_history_policy: MempoolHistoryPolicy,
    pub(super) account_transaction_counts: &'a HashMap<DigitalAssetAccountId, TransactionCount>,
    pub(super) pending_address_ids: &'a HashSet<DigitalAssetAddressId>,
    pub(super) known_activity_address_ids: &'a HashSet<DigitalAssetAddressId>,
    pub(super) bitcoin_history_repair_account_ids: &'a HashSet<DigitalAssetAccountId>,
    pub(super) run_excluded_address_ids: &'a HashSet<DigitalAssetAddressId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PlannedSyncIteration {
    Execute,
    DeriveHdAddresses,
    Stop { reason: SyncIterationStopReason },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlannerCandidate {
    priority: SyncPlannerPriorityTier,
    last_attempted_at: Option<DateTime<Utc>>,
    stable_tie_breaker: String,
    is_hd_derivation: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AddressPlanState {
    Candidate,
    Blocked,
    FreshBalance,
    Ineligible,
}

pub(super) fn priority_tier_for_address(
    address: &SyncAddress,
    input: &SyncPlannerInput<'_>,
) -> SyncPlannerPriorityTier {
    if unfinished_backfill_state(address).is_some() {
        return SyncPlannerPriorityTier::ActiveUnfinishedBackfill;
    }

    if input.pending_address_ids.contains(&address.address_id) {
        return SyncPlannerPriorityTier::PendingTransactionRefresh;
    }

    if matches!(address.last_result, Some(TransactionSyncResult::Failure)) {
        return SyncPlannerPriorityTier::RetryableFailedAttempt;
    }

    if should_plan_balance_refresh(address, input) {
        return SyncPlannerPriorityTier::BalanceRefresh;
    }
    if is_first_sync(address.last_tip_height) {
        return SyncPlannerPriorityTier::NeverAttemptedFirstSync;
    }

    if input
        .known_activity_address_ids
        .contains(&address.address_id)
    {
        return SyncPlannerPriorityTier::KnownActivityRefresh;
    }

    SyncPlannerPriorityTier::ColdRefresh
}

pub(super) fn sort_addresses_by_planner_priority(
    addresses: &mut [SyncAddress],
    input: &SyncPlannerInput<'_>,
) {
    addresses.sort_by_key(|address| {
        (
            priority_tier_for_address(address, input),
            address.last_completed_at,
            address.address_id.to_string(),
        )
    });
}

pub(super) fn sort_hd_bundles_by_planner_priority(
    bundles: &mut [AccountSyncBundle],
    input: &SyncPlannerInput<'_>,
) {
    bundles.sort_by_key(|bundle| {
        (
            hd_bundle_priority_tier(bundle, input),
            bundle.account_id.to_string(),
        )
    });
}

pub(super) fn pick_next_address_index(
    addresses: &[SyncAddress],
    input: &SyncPlannerInput<'_>,
) -> Option<usize> {
    addresses
        .iter()
        .enumerate()
        .filter_map(|(idx, address)| {
            let (candidate, state) = address_candidate(address, input);
            (state == AddressPlanState::Candidate).then_some((idx, candidate?))
        })
        .min_by_key(|(_, candidate)| {
            (
                candidate.priority,
                candidate.last_attempted_at,
                candidate.stable_tie_breaker.clone(),
            )
        })
        .map(|(idx, _)| idx)
}

pub(super) fn plan_next_iteration(
    addresses: &[SyncAddress],
    hd_bundles: &[AccountSyncBundle],
    input: &SyncPlannerInput<'_>,
) -> PlannedSyncIteration {
    let mut candidates = Vec::<PlannerCandidate>::new();
    let mut blocked_count = 0_u32;
    let mut fresh_balance_count = 0_u32;
    let mut ineligible_count = 0_u32;

    for address in addresses {
        collect_address_candidate(
            address,
            input,
            &mut candidates,
            &mut blocked_count,
            &mut fresh_balance_count,
            &mut ineligible_count,
        );
    }

    for bundle in hd_bundles {
        for address in bundle
            .external_addresses
            .iter()
            .chain(bundle.internal_addresses.iter())
        {
            collect_address_candidate(
                address,
                input,
                &mut candidates,
                &mut blocked_count,
                &mut fresh_balance_count,
                &mut ineligible_count,
            );
        }
        candidates.extend(hd_derivation_candidates(bundle));
    }

    if let Some(candidate) = candidates.into_iter().min_by_key(|candidate| {
        (
            candidate.priority,
            candidate.last_attempted_at,
            candidate.stable_tie_breaker.clone(),
        )
    }) {
        if candidate.is_hd_derivation {
            return PlannedSyncIteration::DeriveHdAddresses;
        }
        return PlannedSyncIteration::Execute;
    }

    if blocked_count > 0 {
        PlannedSyncIteration::Stop {
            reason: SyncIterationStopReason::OnlyBlockedActions,
        }
    } else if fresh_balance_count > 0 {
        PlannedSyncIteration::Stop {
            reason: SyncIterationStopReason::BalanceRefreshesFresh,
        }
    } else {
        let _ = ineligible_count;
        PlannedSyncIteration::Stop {
            reason: SyncIterationStopReason::NoEligibleAction,
        }
    }
}

fn collect_address_candidate(
    address: &SyncAddress,
    input: &SyncPlannerInput<'_>,
    candidates: &mut Vec<PlannerCandidate>,
    blocked_count: &mut u32,
    fresh_balance_count: &mut u32,
    ineligible_count: &mut u32,
) {
    let (candidate, state) = address_candidate(address, input);
    match state {
        AddressPlanState::Candidate => {
            if let Some(candidate) = candidate {
                candidates.push(candidate);
            }
        }
        AddressPlanState::Blocked => *blocked_count = blocked_count.saturating_add(1),
        AddressPlanState::FreshBalance => {
            *fresh_balance_count = fresh_balance_count.saturating_add(1)
        }
        AddressPlanState::Ineligible => *ineligible_count = ineligible_count.saturating_add(1),
    }
}

fn address_candidate(
    address: &SyncAddress,
    input: &SyncPlannerInput<'_>,
) -> (Option<PlannerCandidate>, AddressPlanState) {
    if address.account_id.is_none() {
        return (None, AddressPlanState::Ineligible);
    }

    let repair_owned = address.account_id.is_some_and(|account_id| {
        input
            .bitcoin_history_repair_account_ids
            .contains(&account_id)
    });
    if input.run_excluded_address_ids.contains(&address.address_id)
        || (!repair_owned && address.consecutive_failure_count.value() >= ADDRESS_FAILURE_THRESHOLD)
    {
        return (None, AddressPlanState::Blocked);
    }

    let priority = priority_tier_for_address(address, input);
    if priority == SyncPlannerPriorityTier::BalanceRefresh
        && is_successful_balance_refresh_fresh(
            address.last_result,
            address.last_completed_at,
            input.now_utc,
        )
    {
        return (None, AddressPlanState::FreshBalance);
    }

    (
        Some(PlannerCandidate {
            priority,
            last_attempted_at: address.last_completed_at,
            stable_tie_breaker: address.address_id.to_string(),
            is_hd_derivation: false,
        }),
        AddressPlanState::Candidate,
    )
}

fn should_plan_balance_refresh(address: &SyncAddress, input: &SyncPlannerInput<'_>) -> bool {
    if input.mempool_history_policy == MempoolHistoryPolicy::CurrentOnly {
        return true;
    }

    let Some(account_id) = address.account_id else {
        return false;
    };
    input
        .account_transaction_counts
        .get(&account_id)
        .is_some_and(|count| {
            !input
                .mempool_history_policy
                .permits_transaction_page(*count)
        })
}

pub(super) fn address_is_blocked_for_planner(
    address: &SyncAddress,
    input: &SyncPlannerInput<'_>,
) -> bool {
    let (_, state) = address_candidate(address, input);
    state == AddressPlanState::Blocked
}

fn hd_bundle_priority_tier(
    bundle: &AccountSyncBundle,
    input: &SyncPlannerInput<'_>,
) -> SyncPlannerPriorityTier {
    bundle
        .external_addresses
        .iter()
        .chain(bundle.internal_addresses.iter())
        .map(|address| priority_tier_for_address(address, input))
        .min()
        .unwrap_or(SyncPlannerPriorityTier::HdDerivation)
}

fn hd_derivation_candidates(bundle: &AccountSyncBundle) -> Vec<PlannerCandidate> {
    let mut candidates = Vec::new();

    if bundle.external_addresses.is_empty() {
        candidates.push(PlannerCandidate {
            priority: SyncPlannerPriorityTier::HdDerivation,
            last_attempted_at: None,
            stable_tie_breaker: format!("{}:0", bundle.account_id),
            is_hd_derivation: true,
        });
    }

    if bundle.internal_addresses.is_empty() {
        candidates.push(PlannerCandidate {
            priority: SyncPlannerPriorityTier::HdDerivation,
            last_attempted_at: None,
            stable_tie_breaker: format!("{}:1", bundle.account_id),
            is_hd_derivation: true,
        });
    }

    candidates
}

pub(super) fn ordered_active_mempool_history_address_ids<'a>(
    addresses: impl IntoIterator<Item = &'a SyncAddress>,
    active_address_ids: &HashSet<DigitalAssetAddressId>,
    frontier_address_id: Option<DigitalAssetAddressId>,
) -> Vec<DigitalAssetAddressId> {
    let mut addresses = addresses.into_iter().collect::<Vec<_>>();
    addresses.sort_by_key(|address| {
        (
            address.derivation_index,
            address.derivation_change,
            address.address_id.to_string(),
        )
    });
    let frontier_key = frontier_address_id.and_then(|frontier_id| {
        addresses
            .iter()
            .find(|address| address.address_id == frontier_id)
            .map(|address| {
                (
                    address.derivation_index,
                    address.derivation_change,
                    address.address_id.to_string(),
                )
            })
    });
    let active = addresses
        .into_iter()
        .filter(|address| {
            active_address_ids.contains(&address.address_id)
                || (address.mempool_history_proof.is_none()
                    && mempool_history_requires_first_page_restart(address))
        })
        .collect::<Vec<_>>();
    let start = frontier_address_id
        .and_then(|frontier_id| {
            active
                .iter()
                .position(|address| address.address_id == frontier_id)
        })
        .or_else(|| {
            frontier_key.and_then(|frontier_key| {
                active.iter().position(|address| {
                    (
                        address.derivation_index,
                        address.derivation_change,
                        address.address_id.to_string(),
                    ) > frontier_key
                })
            })
        })
        .unwrap_or(0);
    let mut ordered = active
        .iter()
        .filter(|address| {
            address.mempool_backfill_cursor_txid.is_none()
                && matches!(
                    (
                        address.mempool_history_proof,
                        address.mempool_expected_tx_count
                    ),
                    (Some(proof), Some(expected))
                        if expected.value() > proof.confirmed_tx_count.value()
                )
        })
        .map(|address| address.address_id)
        .collect::<Vec<_>>();
    let restart_address_ids = ordered.iter().copied().collect::<HashSet<_>>();
    ordered.extend(
        active
            .into_iter()
            .skip(start)
            .filter(|address| !restart_address_ids.contains(&address.address_id))
            .map(|address| address.address_id),
    );
    ordered
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use crate::transactions::{ChainTipHeight, ConsecutiveFailureCount, TrackedAddress};
    use crate::wallets::{AddressScheme, DigitalAssetAccountId, Network, SyncedAssetId};
    use chrono::{Duration, TimeZone};

    fn empty_set() -> HashSet<DigitalAssetAddressId> {
        HashSet::new()
    }

    fn empty_account_set() -> &'static HashSet<DigitalAssetAccountId> {
        static EMPTY: std::sync::LazyLock<HashSet<DigitalAssetAccountId>> =
            std::sync::LazyLock::new(HashSet::new);
        &EMPTY
    }

    fn test_utc_now() -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000, 0)
            .single()
            .expect("valid timestamp")
    }

    fn make_sync_address(
        address: &str,
        asset_id: SyncedAssetId,
        network: Network,
        account_id: Option<DigitalAssetAccountId>,
        address_scheme: Option<AddressScheme>,
        derivation_change: Option<u32>,
        derivation_index: Option<u32>,
    ) -> SyncAddress {
        SyncAddress {
            address_id: DigitalAssetAddressId::new(),
            address: TrackedAddress::parse(address).expect("valid test address"),
            asset_id,
            network,
            account_id,
            derivation_change,
            derivation_index,
            address_scheme,
            last_completed_at: None,
            last_result: None,
            last_tip_height: None,
            mempool_backfill_cursor_txid: None,
            mempool_expected_tx_count: None,
            mempool_history_proof: None,
            mempool_history_scan_start_run_id: None,
            etherscan_backfill_end_block: None,
            etherscan_history_checkpoint_verified: false,
            has_api_confirmed_balance: false,
            consecutive_failure_count: ConsecutiveFailureCount::zero(),
        }
    }

    fn planner_input<'a>(
        now_utc: DateTime<Utc>,
        pending_address_ids: &'a HashSet<DigitalAssetAddressId>,
        known_activity_address_ids: &'a HashSet<DigitalAssetAddressId>,
        account_transaction_counts: &'a HashMap<DigitalAssetAccountId, TransactionCount>,
        run_excluded_address_ids: &'a HashSet<DigitalAssetAddressId>,
    ) -> SyncPlannerInput<'a> {
        SyncPlannerInput {
            now_utc,
            mempool_history_policy: MempoolHistoryPolicy::Normal {
                cap: TransactionCount::from_u32(1_000),
            },
            account_transaction_counts,
            pending_address_ids,
            known_activity_address_ids,
            bitcoin_history_repair_account_ids: empty_account_set(),
            run_excluded_address_ids,
        }
    }

    fn btc_address(account_id: DigitalAssetAccountId, suffix: &str) -> SyncAddress {
        make_sync_address(
            &format!("bc1qplanner{suffix}000000000000000000000000000000000000000"),
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            Some(account_id),
            None,
            None,
            None,
        )
    }

    fn hd_bundle(
        account_id: DigitalAssetAccountId,
        external_addresses: Vec<SyncAddress>,
        internal_addresses: Vec<SyncAddress>,
    ) -> AccountSyncBundle {
        AccountSyncBundle {
            account_id,
            asset_id: SyncedAssetId::Bitcoin,
            network: Network::Mainnet,
            hd_key_extended_pubkey: "xpub-test".to_string(),
            address_scheme: AddressScheme::NativeSegwit,
            sync_state: None,
            external_addresses,
            internal_addresses,
        }
    }

    #[test]
    fn breadth_cap_boundary_keeps_crossing_page_and_resumes_next_address() {
        let account_id = DigitalAssetAccountId::new();
        let mut address_1 = make_sync_address(
            "bc1qbreadth100000000000000000000000000000000000000000000",
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            Some(account_id),
            Some(AddressScheme::NativeSegwit),
            Some(0),
            Some(1),
        );
        let mut address_2 = make_sync_address(
            "bc1qbreadth200000000000000000000000000000000000000000000",
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            Some(account_id),
            Some(AddressScheme::NativeSegwit),
            Some(1),
            Some(1),
        );
        let mut address_3 = make_sync_address(
            "bc1qbreadth300000000000000000000000000000000000000000000",
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            Some(account_id),
            Some(AddressScheme::NativeSegwit),
            Some(0),
            Some(2),
        );
        let address_12 = make_sync_address(
            "bc1qbreadth12000000000000000000000000000000000000000000",
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            Some(account_id),
            Some(AddressScheme::NativeSegwit),
            Some(0),
            Some(12),
        );
        address_1.address_id = DigitalAssetAddressId::new();
        address_2.address_id = DigitalAssetAddressId::new();
        address_3.address_id = DigitalAssetAddressId::new();
        address_1.mempool_history_proof = Some(crate::db::MempoolHistoryProof {
            confirmed_tx_count: TransactionCount::from_u32(1),
            complete_height: ChainTipHeight::try_new(10).expect("height should parse"),
        });
        address_1.mempool_expected_tx_count = Some(TransactionCount::from_u32(2));
        let active = HashSet::from([
            address_1.address_id,
            address_2.address_id,
            address_3.address_id,
            address_12.address_id,
        ]);
        let addresses = [&address_12, &address_3, &address_2, &address_1];

        let ordered = ordered_active_mempool_history_address_ids(
            addresses,
            &active,
            Some(address_3.address_id),
        );

        assert_eq!(
            ordered,
            vec![
                address_1.address_id,
                address_3.address_id,
                address_12.address_id
            ]
        );

        let policy = MempoolHistoryPolicy::Normal {
            cap: TransactionCount::from_u32(3),
        };
        assert!(policy.permits_transaction_page(TransactionCount::from_u32(2)));
        assert!(!policy.permits_transaction_page(TransactionCount::from_u32(4)));
        assert_eq!(ordered[0], address_1.address_id);
        assert_eq!(ordered[1], address_3.address_id);
    }

    #[test]
    fn planner_selects_oldest_never_synced_address_with_stable_tiebreaker() {
        let account_id = DigitalAssetAccountId::new();
        let now = test_utc_now();
        let mut newer = btc_address(account_id, "newer");
        newer.last_completed_at = Some(now);
        let older = btc_address(account_id, "older");
        let counts = HashMap::new();
        let pending = empty_set();
        let activity = empty_set();
        let excluded = empty_set();
        let input = planner_input(now, &pending, &activity, &counts, &excluded);

        let planned = plan_next_iteration(&[newer.clone(), older.clone()], &[], &input);

        assert_eq!(planned, PlannedSyncIteration::Execute);
        assert_eq!(pick_next_address_index(&[newer, older], &input), Some(1));
    }

    #[test]
    fn pending_refresh_outranks_first_sync() {
        let account_id = DigitalAssetAccountId::new();
        let now = test_utc_now();
        let first_sync = btc_address(account_id, "first");
        let mut pending_address = btc_address(account_id, "pending");
        pending_address.last_tip_height = Some(ChainTipHeight::try_new(100).expect("valid tip"));
        let pending = HashSet::from([pending_address.address_id]);
        let counts = HashMap::new();
        let activity = empty_set();
        let excluded = empty_set();
        let input = planner_input(now, &pending, &activity, &counts, &excluded);

        let planned =
            plan_next_iteration(&[first_sync.clone(), pending_address.clone()], &[], &input);

        assert_eq!(planned, PlannedSyncIteration::Execute);
        assert_eq!(
            pick_next_address_index(&[first_sync, pending_address], &input),
            Some(1)
        );
    }

    #[test]
    fn transaction_cap_switches_to_balance_refresh() {
        let account_id = DigitalAssetAccountId::new();
        let now = test_utc_now();
        let address = btc_address(account_id, "cap");
        let counts = HashMap::from([(account_id, TransactionCount::from_u32(1_000))]);
        let pending = empty_set();
        let activity = empty_set();
        let excluded = empty_set();
        let input = planner_input(now, &pending, &activity, &counts, &excluded);

        let planned = plan_next_iteration(std::slice::from_ref(&address), &[], &input);

        assert_eq!(planned, PlannedSyncIteration::Execute);
        assert!(should_plan_balance_refresh(&address, &input));
    }

    #[test]
    fn fresh_balance_refresh_stops_when_only_balance_work_exists() {
        let account_id = DigitalAssetAccountId::new();
        let now = test_utc_now();
        let mut address = btc_address(account_id, "fresh");
        address.last_completed_at = Some(now - Duration::minutes(5));
        address.last_result = Some(TransactionSyncResult::Success);
        let counts = HashMap::new();
        let pending = empty_set();
        let activity = empty_set();
        let excluded = empty_set();
        let input = SyncPlannerInput {
            mempool_history_policy: MempoolHistoryPolicy::CurrentOnly,
            ..planner_input(now, &pending, &activity, &counts, &excluded)
        };

        let planned = plan_next_iteration(&[address], &[], &input);

        assert_eq!(
            planned,
            PlannedSyncIteration::Stop {
                reason: SyncIterationStopReason::BalanceRefreshesFresh,
            }
        );
    }

    #[test]
    fn failed_balance_refresh_is_retryable_not_fresh() {
        let account_id = DigitalAssetAccountId::new();
        let now = test_utc_now();
        let mut address = btc_address(account_id, "balfail");
        address.last_completed_at = Some(now - Duration::minutes(5));
        address.last_result = Some(TransactionSyncResult::Failure);
        let counts = HashMap::new();
        let pending = empty_set();
        let activity = empty_set();
        let excluded = empty_set();
        let input = SyncPlannerInput {
            mempool_history_policy: MempoolHistoryPolicy::CurrentOnly,
            ..planner_input(now, &pending, &activity, &counts, &excluded)
        };

        let planned = plan_next_iteration(std::slice::from_ref(&address), &[], &input);

        assert_eq!(planned, PlannedSyncIteration::Execute);
        assert!(should_plan_balance_refresh(&address, &input));
        assert_eq!(
            priority_tier_for_address(&address, &input),
            SyncPlannerPriorityTier::RetryableFailedAttempt
        );
    }

    #[test]
    fn failed_address_at_threshold_is_blocked_for_run() {
        let account_id = DigitalAssetAccountId::new();
        let now = test_utc_now();
        let mut address = btc_address(account_id, "fail");
        address.consecutive_failure_count =
            ConsecutiveFailureCount::try_new(2).expect("failure count should parse");
        let counts = HashMap::new();
        let pending = empty_set();
        let activity = empty_set();
        let excluded = empty_set();
        let input = planner_input(now, &pending, &activity, &counts, &excluded);

        let planned = plan_next_iteration(&[address], &[], &input);

        assert_eq!(
            planned,
            PlannedSyncIteration::Stop {
                reason: SyncIterationStopReason::OnlyBlockedActions,
            }
        );
    }

    #[test]
    fn repair_owned_failed_address_at_threshold_remains_retryable() {
        let account_id = DigitalAssetAccountId::new();
        let now = test_utc_now();
        let mut address = btc_address(account_id, "repairfail");
        address.last_result = Some(TransactionSyncResult::Failure);
        address.consecutive_failure_count =
            ConsecutiveFailureCount::try_new(2).expect("failure count should parse");
        let counts = HashMap::new();
        let pending = empty_set();
        let activity = empty_set();
        let repair_accounts = HashSet::from([account_id]);
        let excluded = empty_set();
        let input = SyncPlannerInput {
            bitcoin_history_repair_account_ids: &repair_accounts,
            ..planner_input(now, &pending, &activity, &counts, &excluded)
        };

        assert_eq!(
            plan_next_iteration(std::slice::from_ref(&address), &[], &input),
            PlannedSyncIteration::Execute
        );
        assert!(!address_is_blocked_for_planner(&address, &input));
    }

    #[test]
    fn failed_address_does_not_block_other_candidate_in_same_run() {
        let account_id = DigitalAssetAccountId::new();
        let now = test_utc_now();
        let mut failed = btc_address(account_id, "failed");
        failed.consecutive_failure_count =
            ConsecutiveFailureCount::try_new(2).expect("failure count should parse");
        let healthy = btc_address(account_id, "healthy");
        let counts = HashMap::new();
        let pending = empty_set();
        let activity = empty_set();
        let excluded = empty_set();
        let input = planner_input(now, &pending, &activity, &counts, &excluded);

        let planned = plan_next_iteration(&[failed.clone(), healthy.clone()], &[], &input);

        assert_eq!(planned, PlannedSyncIteration::Execute);
        assert_eq!(pick_next_address_index(&[failed, healthy], &input), Some(1));
    }

    #[test]
    fn hd_derivation_interleaves_external_before_internal_for_empty_bundle() {
        let account_id = DigitalAssetAccountId::new();
        let now = test_utc_now();
        let bundle = hd_bundle(account_id, Vec::new(), Vec::new());
        let counts = HashMap::new();
        let pending = empty_set();
        let activity = empty_set();
        let excluded = empty_set();
        let input = planner_input(now, &pending, &activity, &counts, &excluded);

        let planned = plan_next_iteration(&[], &[bundle], &input);

        assert_eq!(planned, PlannedSyncIteration::DeriveHdAddresses);
    }

    #[test]
    fn hd_derivation_for_empty_bundle_is_after_existing_address_work() {
        let account_id = DigitalAssetAccountId::new();
        let now = test_utc_now();
        let address = btc_address(account_id, "existing");
        let empty_bundle = hd_bundle(DigitalAssetAccountId::new(), Vec::new(), Vec::new());
        let counts = HashMap::new();
        let pending = empty_set();
        let activity = empty_set();
        let excluded = empty_set();
        let input = planner_input(now, &pending, &activity, &counts, &excluded);

        let planned = plan_next_iteration(std::slice::from_ref(&address), &[empty_bundle], &input);

        assert_eq!(planned, PlannedSyncIteration::Execute);
    }

    #[test]
    fn existing_hd_addresses_are_planner_candidates() {
        let account_id = DigitalAssetAccountId::new();
        let now = test_utc_now();
        let external = btc_address(account_id, "hdext");
        let internal = btc_address(account_id, "hdint");
        let bundle = hd_bundle(account_id, vec![external.clone()], vec![internal.clone()]);
        let counts = HashMap::new();
        let pending = empty_set();
        let activity = empty_set();
        let excluded = empty_set();
        let input = planner_input(now, &pending, &activity, &counts, &excluded);

        let planned = plan_next_iteration(&[], &[bundle], &input);

        assert_eq!(planned, PlannedSyncIteration::Execute);
    }

    #[test]
    fn never_attempted_first_sync_outranks_failed_first_sync() {
        let account_id = DigitalAssetAccountId::new();
        let now = test_utc_now();
        let never_attempted = btc_address(account_id, "never");
        // last_result stays None, last_tip_height stays None
        let mut attempted_failed = btc_address(account_id, "failed");
        attempted_failed.last_result = Some(TransactionSyncResult::Failure);
        let counts = HashMap::new();
        let pending = empty_set();
        let activity = empty_set();
        let excluded = empty_set();
        let input = planner_input(now, &pending, &activity, &counts, &excluded);

        let planned = plan_next_iteration(
            &[attempted_failed.clone(), never_attempted.clone()],
            &[],
            &input,
        );

        assert_eq!(planned, PlannedSyncIteration::Execute);
        assert_eq!(
            pick_next_address_index(&[attempted_failed, never_attempted], &input),
            Some(1)
        );
    }

    #[test]
    fn failed_first_sync_outranks_cold_refresh() {
        let account_id = DigitalAssetAccountId::new();
        let now = test_utc_now();
        let mut failed_first = btc_address(account_id, "failed");
        failed_first.last_result = Some(TransactionSyncResult::Failure);
        let mut cold = btc_address(account_id, "cold");
        cold.last_tip_height = Some(ChainTipHeight::try_new(500_000).expect("valid tip"));
        let counts = HashMap::new();
        let pending = empty_set();
        let activity = empty_set();
        let excluded = empty_set();
        let input = planner_input(now, &pending, &activity, &counts, &excluded);

        let planned = plan_next_iteration(&[cold.clone(), failed_first.clone()], &[], &input);

        assert_eq!(planned, PlannedSyncIteration::Execute);
        assert_eq!(
            pick_next_address_index(&[cold, failed_first], &input),
            Some(1)
        );
    }

    #[test]
    fn failed_first_sync_outranks_known_activity_refresh() {
        let account_id = DigitalAssetAccountId::new();
        let now = test_utc_now();
        let mut failed_first = btc_address(account_id, "failed");
        failed_first.last_result = Some(TransactionSyncResult::Failure);
        let mut known = btc_address(account_id, "known");
        known.last_tip_height = Some(ChainTipHeight::try_new(500_000).expect("valid tip"));
        let activity = HashSet::from([known.address_id]);
        let counts = HashMap::new();
        let pending = empty_set();
        let excluded = empty_set();
        let input = planner_input(now, &pending, &activity, &counts, &excluded);

        let planned = plan_next_iteration(&[known.clone(), failed_first.clone()], &[], &input);

        assert_eq!(planned, PlannedSyncIteration::Execute);
        assert_eq!(
            pick_next_address_index(&[known, failed_first], &input),
            Some(1)
        );
    }

    #[test]
    fn failed_first_sync_at_threshold_is_blocked() {
        let account_id = DigitalAssetAccountId::new();
        let now = test_utc_now();
        let mut failed_first = btc_address(account_id, "failed");
        failed_first.last_result = Some(TransactionSyncResult::Failure);
        failed_first.consecutive_failure_count =
            ConsecutiveFailureCount::try_new(2).expect("failure count should parse");
        let counts = HashMap::new();
        let pending = empty_set();
        let activity = empty_set();
        let excluded = empty_set();
        let input = planner_input(now, &pending, &activity, &counts, &excluded);

        let planned = plan_next_iteration(&[failed_first], &[], &input);

        assert_eq!(
            planned,
            PlannedSyncIteration::Stop {
                reason: SyncIterationStopReason::OnlyBlockedActions,
            }
        );
    }

    #[test]
    fn regression_never_attempted_before_failed_before_cold() {
        let account_id = DigitalAssetAccountId::new();
        let now = test_utc_now();

        let never_a = btc_address(account_id, "neverA");
        let never_b = btc_address(account_id, "neverB");

        let mut failed = btc_address(account_id, "failed");
        failed.last_result = Some(TransactionSyncResult::Failure);
        // Give it an older last_completed_at so it ranks earlier among
        // retryable failed candidates.
        failed.last_completed_at = Some(now - Duration::hours(2));

        let mut cold = btc_address(account_id, "cold");
        cold.last_tip_height = Some(ChainTipHeight::try_new(500_000).expect("valid tip"));

        let counts = HashMap::new();
        let pending = empty_set();
        let activity = empty_set();
        let excluded = empty_set();
        let input = planner_input(now, &pending, &activity, &counts, &excluded);

        let addresses = [
            never_a.clone(),
            never_b.clone(),
            failed.clone(),
            cold.clone(),
        ];

        // First pick should be one of the never-attempted addresses.
        let idx0 = pick_next_address_index(&addresses, &input).unwrap();
        assert!(
            idx0 < 2,
            "first pick should be a never-attempted address, got index {idx0}"
        );

        // Remove that address and pick again. Second should be the other
        // never-attempted address.
        let remaining: Vec<_> = addresses
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != idx0)
            .map(|(_, a)| a.clone())
            .collect();
        let idx1 = pick_next_address_index(&remaining, &input).unwrap();
        assert!(
            remaining[idx1].address_id == never_a.address_id
                || remaining[idx1].address_id == never_b.address_id,
            "second pick should be the other never-attempted address"
        );

        // Remove both never-attempted addresses.
        let remaining2: Vec<_> = remaining
            .into_iter()
            .enumerate()
            .filter(|(i, _)| *i != idx1)
            .map(|(_, a)| a)
            .collect();
        let idx2 = pick_next_address_index(&remaining2, &input).unwrap();
        assert_eq!(
            remaining2[idx2].address_id, failed.address_id,
            "third pick should be the failed first-sync address"
        );

        // Remove the failed address.
        let remaining3: Vec<_> = remaining2
            .into_iter()
            .enumerate()
            .filter(|(i, _)| *i != idx2)
            .map(|(_, a)| a)
            .collect();
        let idx3 = pick_next_address_index(&remaining3, &input).unwrap();
        assert_eq!(
            remaining3[idx3].address_id, cold.address_id,
            "fourth pick should be the cold address"
        );
    }

    #[test]
    fn balance_refresh_gating_preserved_for_failed_first_sync() {
        let account_id = DigitalAssetAccountId::new();
        let now = test_utc_now();
        let mut address = btc_address(account_id, "capfail");
        address.last_result = Some(TransactionSyncResult::Failure);
        let counts = HashMap::from([(account_id, TransactionCount::from_u32(1_000))]);
        let pending = empty_set();
        let activity = empty_set();
        let excluded = empty_set();
        let input = planner_input(now, &pending, &activity, &counts, &excluded);

        let planned = plan_next_iteration(std::slice::from_ref(&address), &[], &input);

        assert_eq!(planned, PlannedSyncIteration::Execute);
        assert!(should_plan_balance_refresh(&address, &input));
    }
}
