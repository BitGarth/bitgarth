use super::gate::MempoolHistoryPolicy;
use super::progress::publish_hd_account_progress_event;
use super::{
    AddressSyncExecutor, ChainTipCache, CycleAccumulator, CycleAccumulatorSnapshot, RunContext,
    SyncClients, SyncSingleAddressControlRequest, UserTransactionMonitorError, chain_tip_cache_key,
    sync_single_address_with_controls,
};
use crate::db::{
    AccountSyncBundle, BitcoinAccountCompletionPublication, BitcoinHdDiscoveryPublication,
    HdAccountChainFrontierPhase, HdAccountChainSyncState, SyncAddress,
    complete_hd_account_discovery, delete_hd_account_chain_sync_state,
    derive_next_derived_addresses_for_account, load_hd_account_chain_sync_state,
    publish_bitcoin_account_completion, upsert_account_sync_state,
    upsert_hd_account_chain_sync_state,
};
use crate::tasks::publish_transaction_sync_event;
use crate::transactions::{
    AddressCount, ChainTipHeight, SyncIntegrationId, TrackedAddress, TransactionSyncEvent,
};
use crate::wallets::{
    AddressScheme, DigitalAssetAccountId, DigitalAssetAddressId, Network, SyncedAssetId,
};
use chrono::{DateTime, Utc};
use dioxus::logger::tracing;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DerivedSyncAddress {
    pub(super) address_id: DigitalAssetAddressId,
    pub(super) address: TrackedAddress,
    pub(super) derivation_change: u32,
    pub(super) derivation_index: u32,
}

pub(super) struct AddressDerivationRequest {
    pub(super) user_id: crate::models::UserId,
    pub(super) account_id: DigitalAssetAccountId,
    pub(super) address_scheme: AddressScheme,
    pub(super) derivation_change: u32,
    pub(super) count: u32,
    pub(super) now: DateTime<Utc>,
}

pub(super) trait AddressDerivationProvider {
    fn derive_next_addresses(
        &mut self,
        request: AddressDerivationRequest,
    ) -> Result<Vec<DerivedSyncAddress>, UserTransactionMonitorError>;
}

pub(super) struct LiveAddressDerivationProvider;

impl AddressDerivationProvider for LiveAddressDerivationProvider {
    fn derive_next_addresses(
        &mut self,
        request: AddressDerivationRequest,
    ) -> Result<Vec<DerivedSyncAddress>, UserTransactionMonitorError> {
        let generated = derive_next_derived_addresses_for_account(
            request.user_id,
            request.account_id,
            request.address_scheme,
            request.derivation_change,
            request.count,
            request.now,
        )?;
        let mut result = Vec::with_capacity(generated.len());
        for generated_address in generated {
            let address =
                TrackedAddress::parse(generated_address.address.as_str()).map_err(|err| {
                    UserTransactionMonitorError::Parse(format!(
                        "generated address parse error: {err}"
                    ))
                })?;
            result.push(DerivedSyncAddress {
                address_id: generated_address.address_id,
                address,
                derivation_change: generated_address.derivation_change,
                derivation_index: generated_address.derivation_index,
            });
        }
        Ok(result)
    }
}

fn max_derived_index(addresses: &[SyncAddress]) -> Option<u32> {
    addresses
        .iter()
        .filter_map(|address| address.derivation_index)
        .max()
}

fn address_matches_hd_context(
    address: &SyncAddress,
    account_id: DigitalAssetAccountId,
    address_scheme: AddressScheme,
    derivation_change: u32,
) -> bool {
    address.account_id == Some(account_id)
        && address.address_scheme == Some(address_scheme)
        && address.derivation_change == Some(derivation_change)
}

fn required_derivation_index(address: &SyncAddress) -> Result<u32, UserTransactionMonitorError> {
    address.derivation_index.ok_or_else(|| {
        UserTransactionMonitorError::Parse(format!(
            "HD address {} missing derivation index",
            address.address_id
        ))
    })
}

fn next_unfinished_derivation_index(
    addresses: &[SyncAddress],
    known_activity: &HashSet<DigitalAssetAddressId>,
    account_id: DigitalAssetAccountId,
    address_scheme: AddressScheme,
    derivation_change: u32,
) -> u32 {
    addresses
        .iter()
        .find(|address| {
            address_matches_hd_context(address, account_id, address_scheme, derivation_change)
                && !known_activity.contains(&address.address_id)
        })
        .and_then(|address| address.derivation_index)
        .or_else(|| max_derived_index(addresses).map(|index| index.saturating_add(1)))
        .unwrap_or(0)
}

fn default_hd_chain_frontier_state(
    addresses: &[SyncAddress],
    known_activity: &HashSet<DigitalAssetAddressId>,
    account_id: DigitalAssetAccountId,
    address_scheme: AddressScheme,
    derivation_change: u32,
) -> HdAccountChainSyncState {
    HdAccountChainSyncState::ExistingAddresses {
        next_index_to_scan: next_unfinished_derivation_index(
            addresses,
            known_activity,
            account_id,
            address_scheme,
            derivation_change,
        ),
        consecutive_unused: 0,
    }
}

fn build_hd_chain_frontier_state(
    frontier_phase: HdAccountChainFrontierPhase,
    next_index_to_scan: u32,
    consecutive_unused: u32,
    active_rescan_from_index: Option<u32>,
) -> Result<HdAccountChainSyncState, UserTransactionMonitorError> {
    match frontier_phase {
        HdAccountChainFrontierPhase::ExistingAddresses => {
            Ok(HdAccountChainSyncState::ExistingAddresses {
                next_index_to_scan,
                consecutive_unused,
            })
        }
        HdAccountChainFrontierPhase::DerivedAddresses => {
            Ok(HdAccountChainSyncState::DerivedAddresses {
                next_index_to_scan,
                consecutive_unused,
            })
        }
        HdAccountChainFrontierPhase::ActiveRescan => {
            let active_rescan_from_index = active_rescan_from_index.ok_or_else(|| {
                UserTransactionMonitorError::Parse(
                    "HD frontier missing active rescan resume index".to_string(),
                )
            })?;
            Ok(HdAccountChainSyncState::ActiveRescan {
                next_index_to_scan,
                consecutive_unused,
                active_rescan_from_index,
            })
        }
    }
}

fn find_address_position_at_or_after_index(
    addresses: &[SyncAddress],
    account_id: DigitalAssetAccountId,
    address_scheme: AddressScheme,
    derivation_change: u32,
    next_index_to_scan: u32,
) -> Option<usize> {
    addresses.iter().enumerate().find_map(|(idx, address)| {
        let derivation_index = address.derivation_index?;
        if address_matches_hd_context(address, account_id, address_scheme, derivation_change)
            && derivation_index >= next_index_to_scan
        {
            Some(idx)
        } else {
            None
        }
    })
}

fn max_active_derivation_index(
    addresses: &[SyncAddress],
    known_activity: &HashSet<DigitalAssetAddressId>,
    account_id: DigitalAssetAccountId,
    address_scheme: AddressScheme,
    derivation_change: u32,
) -> Option<u32> {
    addresses
        .iter()
        .filter(|address| {
            known_activity.contains(&address.address_id)
                && address_matches_hd_context(
                    address,
                    account_id,
                    address_scheme,
                    derivation_change,
                )
        })
        .filter_map(|address| address.derivation_index)
        .max()
}

fn find_active_rescan_position(
    addresses: &[SyncAddress],
    known_activity: &HashSet<DigitalAssetAddressId>,
    completed_address_ids: &HashSet<DigitalAssetAddressId>,
    account_id: DigitalAssetAccountId,
    address_scheme: AddressScheme,
    derivation_change: u32,
    from_index: u32,
) -> Option<(usize, u32)> {
    addresses
        .iter()
        .enumerate()
        .filter_map(|(idx, address)| {
            let derivation_index = address.derivation_index?;
            if known_activity.contains(&address.address_id)
                && !completed_address_ids.contains(&address.address_id)
                && address_matches_hd_context(
                    address,
                    account_id,
                    address_scheme,
                    derivation_change,
                )
                && derivation_index <= from_index
            {
                Some((idx, derivation_index))
            } else {
                None
            }
        })
        .max_by_key(|(_, derivation_index)| *derivation_index)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct HdChainScanOutcome {
    pub(super) frontier_state: Option<HdAccountChainSyncState>,
    pub(super) interrupted: bool,
}

impl HdChainScanOutcome {
    fn completed() -> Self {
        Self {
            frontier_state: None,
            interrupted: false,
        }
    }

    fn interrupted(frontier_state: HdAccountChainSyncState) -> Self {
        Self {
            frontier_state: Some(frontier_state),
            interrupted: true,
        }
    }
}

pub(super) struct HdChainScanRequest<'a> {
    pub(super) run: RunContext<'a>,
    pub(super) clients: SyncClients<'a>,
    pub(super) pending_address_ids: &'a HashSet<DigitalAssetAddressId>,
    pub(super) frontier_state: Option<HdAccountChainSyncState>,
    pub(super) account_id: DigitalAssetAccountId,
    pub(super) asset_id: SyncedAssetId,
    pub(super) network: Network,
    pub(super) address_scheme: AddressScheme,
    pub(super) derivation_change: u32,
    pub(super) gap_limit: u32,
    pub(super) addresses: &'a mut Vec<SyncAddress>,
    pub(super) known_activity: &'a mut HashSet<DigitalAssetAddressId>,
    pub(super) chain_tip_cache: &'a mut ChainTipCache,
    pub(super) completed_address_ids: &'a mut HashSet<DigitalAssetAddressId>,
    pub(super) addresses_total: &'a mut u32,
    pub(super) accumulator: &'a mut CycleAccumulator,
    pub(super) processed_for_account: &'a mut u32,
    pub(super) sync_executor: &'a mut dyn AddressSyncExecutor,
    pub(super) derivation_provider: &'a mut dyn AddressDerivationProvider,
    pub(super) historical_backfill_enabled: bool,
}

struct HdAddressStepRequest<'a> {
    control: SyncSingleAddressControlRequest<'a>,
    account_id: DigitalAssetAccountId,
    asset_id: SyncedAssetId,
    completed_address_ids: &'a mut HashSet<DigitalAssetAddressId>,
    addresses_total: u32,
    known_activity: &'a mut HashSet<DigitalAssetAddressId>,
    consecutive_unused: Option<&'a mut u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HdAddressStepOutcome {
    interrupted: bool,
}

fn run_hd_address_step(
    request: HdAddressStepRequest<'_>,
) -> Result<HdAddressStepOutcome, UserTransactionMonitorError> {
    let HdAddressStepRequest {
        control,
        account_id,
        asset_id,
        completed_address_ids,
        addresses_total,
        known_activity,
        consecutive_unused,
    } = request;
    let SyncSingleAddressControlRequest {
        run,
        address,
        chain_tip_cache,
        pending_address_ids,
        clients,
        executor,
        accumulator,
        processed_for_account,
        single_address_progress,
        mempool_history_policy,
        mempool_history_page_frontier,
    } = control;
    let address_id = address.address_id;
    let before = CycleAccumulatorSnapshot::from_accumulator(accumulator);
    let (has_activity, interrupted) =
        sync_single_address_with_controls(SyncSingleAddressControlRequest {
            run,
            address,
            chain_tip_cache,
            pending_address_ids,
            clients,
            executor,
            accumulator,
            processed_for_account,
            single_address_progress,
            mempool_history_policy,
            mempool_history_page_frontier,
        })?;
    let after = CycleAccumulatorSnapshot::from_accumulator(accumulator);
    if interrupted {
        return Ok(HdAddressStepOutcome { interrupted: true });
    }

    let delta = after.delta_from(before);
    if delta.addresses_synced + delta.addresses_failed + delta.addresses_skipped > 0
        && completed_address_ids.insert(address_id)
    {
        publish_hd_account_progress_event(
            run,
            account_id,
            SyncIntegrationId::for_asset(asset_id),
            completed_address_ids.len(),
            addresses_total,
        );
    }
    if has_activity {
        known_activity.insert(address_id);
    }
    if let Some(consecutive_unused) = consecutive_unused {
        *consecutive_unused = if known_activity.contains(&address_id) {
            0
        } else {
            (*consecutive_unused).saturating_add(1)
        };
    }

    Ok(HdAddressStepOutcome { interrupted: false })
}

pub(super) fn run_hd_chain_scan(
    request: HdChainScanRequest<'_>,
) -> Result<HdChainScanOutcome, UserTransactionMonitorError> {
    let HdChainScanRequest {
        run,
        clients,
        pending_address_ids,
        frontier_state,
        account_id,
        asset_id,
        network,
        address_scheme,
        derivation_change,
        gap_limit,
        addresses,
        known_activity,
        chain_tip_cache,
        completed_address_ids,
        addresses_total,
        accumulator,
        processed_for_account,
        sync_executor,
        derivation_provider,
        historical_backfill_enabled,
    } = request;
    let initial_frontier_state = frontier_state.unwrap_or_else(|| {
        if historical_backfill_enabled {
            HdAccountChainSyncState::ExistingAddresses {
                next_index_to_scan: 0,
                consecutive_unused: 0,
            }
        } else {
            default_hd_chain_frontier_state(
                addresses,
                known_activity,
                account_id,
                address_scheme,
                derivation_change,
            )
        }
    });
    let (mut frontier_phase, mut next_index_to_scan, mut consecutive_unused, mut active_rescan) =
        match initial_frontier_state {
            HdAccountChainSyncState::ExistingAddresses {
                next_index_to_scan,
                consecutive_unused,
            } => (
                HdAccountChainFrontierPhase::ExistingAddresses,
                next_index_to_scan,
                consecutive_unused,
                None,
            ),
            HdAccountChainSyncState::DerivedAddresses {
                next_index_to_scan,
                consecutive_unused,
            } => (
                HdAccountChainFrontierPhase::DerivedAddresses,
                next_index_to_scan,
                consecutive_unused,
                None,
            ),
            HdAccountChainSyncState::ActiveRescan {
                next_index_to_scan,
                consecutive_unused,
                active_rescan_from_index,
            } => (
                HdAccountChainFrontierPhase::ActiveRescan,
                next_index_to_scan,
                consecutive_unused,
                Some(active_rescan_from_index),
            ),
        };

    if frontier_phase == HdAccountChainFrontierPhase::ExistingAddresses {
        let mut idx = find_address_position_at_or_after_index(
            addresses,
            account_id,
            address_scheme,
            derivation_change,
            next_index_to_scan,
        )
        .unwrap_or(addresses.len());
        while idx < addresses.len() {
            if *processed_for_account >= super::MAX_ADDRESSES_PER_ACCOUNT_PER_RUN {
                return Ok(HdChainScanOutcome::interrupted(
                    build_hd_chain_frontier_state(
                        frontier_phase,
                        next_index_to_scan,
                        consecutive_unused,
                        active_rescan,
                    )?,
                ));
            }
            if !address_matches_hd_context(
                &addresses[idx],
                account_id,
                address_scheme,
                derivation_change,
            ) {
                tracing::warn!(
                    user_id = %run.user_id,
                    run_id = %run.run_id,
                    account_id = %account_id,
                    address_id = %addresses[idx].address_id,
                    "transactions sync: skipping address with mismatched HD metadata"
                );
                idx = idx.saturating_add(1);
                continue;
            }
            let current_derivation_index = required_derivation_index(&addresses[idx])?;
            if historical_backfill_enabled
                && known_activity.contains(&addresses[idx].address_id)
                && completed_address_ids.contains(&addresses[idx].address_id)
            {
                consecutive_unused = 0;
                next_index_to_scan = current_derivation_index.saturating_add(1);
                idx = idx.saturating_add(1);
                continue;
            }
            let step_outcome = run_hd_address_step(HdAddressStepRequest {
                control: SyncSingleAddressControlRequest {
                    run,
                    address: &mut addresses[idx],
                    chain_tip_cache,
                    pending_address_ids,
                    clients,
                    executor: sync_executor,
                    accumulator,
                    processed_for_account,
                    single_address_progress: None,
                    mempool_history_policy: if historical_backfill_enabled {
                        MempoolHistoryPolicy::LegacyRepair
                    } else {
                        MempoolHistoryPolicy::CurrentOnly
                    },
                    mempool_history_page_frontier: None,
                },
                account_id,
                asset_id,
                completed_address_ids,
                addresses_total: *addresses_total,
                known_activity,
                consecutive_unused: Some(&mut consecutive_unused),
            })?;
            if step_outcome.interrupted {
                next_index_to_scan = current_derivation_index;
                return Ok(HdChainScanOutcome::interrupted(
                    build_hd_chain_frontier_state(
                        frontier_phase,
                        next_index_to_scan,
                        consecutive_unused,
                        active_rescan,
                    )?,
                ));
            }
            next_index_to_scan = current_derivation_index.saturating_add(1);
            idx = idx.saturating_add(1);
        }

        frontier_phase = HdAccountChainFrontierPhase::DerivedAddresses;
        next_index_to_scan = max_derived_index(addresses)
            .map(|index| index.saturating_add(1))
            .unwrap_or(next_index_to_scan);
    }

    if frontier_phase == HdAccountChainFrontierPhase::DerivedAddresses {
        while consecutive_unused < gap_limit {
            let mut resume_idx = find_address_position_at_or_after_index(
                addresses,
                account_id,
                address_scheme,
                derivation_change,
                next_index_to_scan,
            )
            .unwrap_or(addresses.len());
            while resume_idx < addresses.len() && consecutive_unused < gap_limit {
                if !address_matches_hd_context(
                    &addresses[resume_idx],
                    account_id,
                    address_scheme,
                    derivation_change,
                ) {
                    tracing::warn!(
                        user_id = %run.user_id,
                        run_id = %run.run_id,
                        account_id = %account_id,
                        address_id = %addresses[resume_idx].address_id,
                        "transactions sync: skipping resumed derived address with mismatched HD metadata"
                    );
                    resume_idx = resume_idx.saturating_add(1);
                    continue;
                }
                let current_derivation_index = required_derivation_index(&addresses[resume_idx])?;
                if current_derivation_index < next_index_to_scan {
                    resume_idx = resume_idx.saturating_add(1);
                    continue;
                }
                if *processed_for_account >= super::MAX_ADDRESSES_PER_ACCOUNT_PER_RUN {
                    next_index_to_scan = current_derivation_index;
                    return Ok(HdChainScanOutcome::interrupted(
                        build_hd_chain_frontier_state(
                            frontier_phase,
                            next_index_to_scan,
                            consecutive_unused,
                            active_rescan,
                        )?,
                    ));
                }
                let step_outcome = run_hd_address_step(HdAddressStepRequest {
                    control: SyncSingleAddressControlRequest {
                        run,
                        address: &mut addresses[resume_idx],
                        chain_tip_cache,
                        pending_address_ids,
                        clients,
                        executor: sync_executor,
                        accumulator,
                        processed_for_account,
                        single_address_progress: None,
                        mempool_history_policy: if historical_backfill_enabled {
                            MempoolHistoryPolicy::LegacyRepair
                        } else {
                            MempoolHistoryPolicy::CurrentOnly
                        },
                        mempool_history_page_frontier: None,
                    },
                    account_id,
                    asset_id,
                    completed_address_ids,
                    addresses_total: *addresses_total,
                    known_activity,
                    consecutive_unused: Some(&mut consecutive_unused),
                })?;
                if step_outcome.interrupted {
                    next_index_to_scan = current_derivation_index;
                    return Ok(HdChainScanOutcome::interrupted(
                        build_hd_chain_frontier_state(
                            frontier_phase,
                            next_index_to_scan,
                            consecutive_unused,
                            active_rescan,
                        )?,
                    ));
                }
                next_index_to_scan = current_derivation_index.saturating_add(1);
                resume_idx = resume_idx.saturating_add(1);
            }
            if consecutive_unused >= gap_limit {
                break;
            }
            if *processed_for_account >= super::MAX_ADDRESSES_PER_ACCOUNT_PER_RUN {
                return Ok(HdChainScanOutcome::interrupted(
                    build_hd_chain_frontier_state(
                        frontier_phase,
                        next_index_to_scan,
                        consecutive_unused,
                        active_rescan,
                    )?,
                ));
            }
            let remaining_budget =
                super::MAX_ADDRESSES_PER_ACCOUNT_PER_RUN.saturating_sub(*processed_for_account);
            let needed = gap_limit.saturating_sub(consecutive_unused);
            let to_derive = needed.min(remaining_budget);

            let generated =
                derivation_provider.derive_next_addresses(AddressDerivationRequest {
                    user_id: run.user_id,
                    account_id,
                    address_scheme,
                    derivation_change,
                    count: to_derive,
                    now: run.clock.utc_now(),
                })?;
            if generated.is_empty() {
                break;
            }
            accumulator.add_total(generated.len());
            *addresses_total =
                addresses_total.saturating_add(u32::try_from(generated.len()).unwrap_or(u32::MAX));
            let generated_len = generated.len();
            for generated_address in generated {
                addresses.push(SyncAddress {
                    address_id: generated_address.address_id,
                    address: generated_address.address,
                    asset_id,
                    network,
                    account_id: Some(account_id),
                    derivation_change: Some(generated_address.derivation_change),
                    derivation_index: Some(generated_address.derivation_index),
                    address_scheme: Some(address_scheme),
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
                    consecutive_failure_count: crate::transactions::ConsecutiveFailureCount::zero(),
                });
            }

            let mut new_idx = addresses.len().saturating_sub(generated_len);
            while new_idx < addresses.len() && consecutive_unused < gap_limit {
                if !address_matches_hd_context(
                    &addresses[new_idx],
                    account_id,
                    address_scheme,
                    derivation_change,
                ) {
                    tracing::warn!(
                        user_id = %run.user_id,
                        run_id = %run.run_id,
                        account_id = %account_id,
                        address_id = %addresses[new_idx].address_id,
                        "transactions sync: skipping generated address with mismatched HD metadata"
                    );
                    new_idx = new_idx.saturating_add(1);
                    continue;
                }
                let current_derivation_index = required_derivation_index(&addresses[new_idx])?;
                if *processed_for_account >= super::MAX_ADDRESSES_PER_ACCOUNT_PER_RUN {
                    next_index_to_scan = current_derivation_index;
                    return Ok(HdChainScanOutcome::interrupted(
                        build_hd_chain_frontier_state(
                            frontier_phase,
                            next_index_to_scan,
                            consecutive_unused,
                            active_rescan,
                        )?,
                    ));
                }
                let step_outcome = run_hd_address_step(HdAddressStepRequest {
                    control: SyncSingleAddressControlRequest {
                        run,
                        address: &mut addresses[new_idx],
                        chain_tip_cache,
                        pending_address_ids,
                        clients,
                        executor: sync_executor,
                        accumulator,
                        processed_for_account,
                        single_address_progress: None,
                        mempool_history_policy: if historical_backfill_enabled {
                            MempoolHistoryPolicy::LegacyRepair
                        } else {
                            MempoolHistoryPolicy::CurrentOnly
                        },
                        mempool_history_page_frontier: None,
                    },
                    account_id,
                    asset_id,
                    completed_address_ids,
                    addresses_total: *addresses_total,
                    known_activity,
                    consecutive_unused: Some(&mut consecutive_unused),
                })?;
                if step_outcome.interrupted {
                    next_index_to_scan = current_derivation_index;
                    return Ok(HdChainScanOutcome::interrupted(
                        build_hd_chain_frontier_state(
                            frontier_phase,
                            next_index_to_scan,
                            consecutive_unused,
                            active_rescan,
                        )?,
                    ));
                }
                next_index_to_scan = current_derivation_index.saturating_add(1);
                new_idx = new_idx.saturating_add(1);
            }
        }

        frontier_phase = HdAccountChainFrontierPhase::ActiveRescan;
        active_rescan = max_active_derivation_index(
            addresses,
            known_activity,
            account_id,
            address_scheme,
            derivation_change,
        );
    }

    if frontier_phase == HdAccountChainFrontierPhase::ActiveRescan {
        let mut current_from_index = active_rescan;
        while let Some(from_index) = current_from_index {
            let Some((idx, derivation_index)) = find_active_rescan_position(
                addresses,
                known_activity,
                completed_address_ids,
                account_id,
                address_scheme,
                derivation_change,
                from_index,
            ) else {
                break;
            };
            active_rescan = Some(derivation_index);
            if *processed_for_account >= super::MAX_ADDRESSES_PER_ACCOUNT_PER_RUN {
                return Ok(HdChainScanOutcome::interrupted(
                    build_hd_chain_frontier_state(
                        frontier_phase,
                        next_index_to_scan,
                        consecutive_unused,
                        active_rescan,
                    )?,
                ));
            }
            let step_outcome = run_hd_address_step(HdAddressStepRequest {
                control: SyncSingleAddressControlRequest {
                    run,
                    address: &mut addresses[idx],
                    chain_tip_cache,
                    pending_address_ids,
                    clients,
                    executor: sync_executor,
                    accumulator,
                    processed_for_account,
                    single_address_progress: None,
                    mempool_history_policy: if historical_backfill_enabled {
                        MempoolHistoryPolicy::LegacyRepair
                    } else {
                        MempoolHistoryPolicy::CurrentOnly
                    },
                    mempool_history_page_frontier: None,
                },
                account_id,
                asset_id,
                completed_address_ids,
                addresses_total: *addresses_total,
                known_activity,
                consecutive_unused: None,
            })?;
            if step_outcome.interrupted {
                return Ok(HdChainScanOutcome::interrupted(
                    build_hd_chain_frontier_state(
                        frontier_phase,
                        next_index_to_scan,
                        consecutive_unused,
                        active_rescan,
                    )?,
                ));
            }
            current_from_index = derivation_index.checked_sub(1);
        }
    }

    Ok(HdChainScanOutcome::completed())
}

pub(super) struct HdBundleScanRequest<'a> {
    pub(super) run: RunContext<'a>,
    pub(super) clients: SyncClients<'a>,
    pub(super) pending_address_ids: &'a HashSet<DigitalAssetAddressId>,
    pub(super) bundle: AccountSyncBundle,
    pub(super) completed_address_ids: HashSet<DigitalAssetAddressId>,
    pub(super) known_activity: &'a mut HashSet<DigitalAssetAddressId>,
    pub(super) chain_tip_cache: &'a mut ChainTipCache,
    pub(super) accumulator: &'a mut CycleAccumulator,
    pub(super) sync_executor: &'a mut dyn AddressSyncExecutor,
    pub(super) derivation_provider: &'a mut dyn AddressDerivationProvider,
    pub(super) historical_backfill_enabled: bool,
}

fn persist_hd_chain_frontier_state(
    run: RunContext<'_>,
    account_id: DigitalAssetAccountId,
    derivation_change: u32,
    frontier_state: Option<&HdAccountChainSyncState>,
    observed_at: DateTime<Utc>,
) -> Result<(), UserTransactionMonitorError> {
    match frontier_state {
        Some(frontier_state) => upsert_hd_account_chain_sync_state(
            run.user_id,
            account_id,
            derivation_change,
            frontier_state,
            observed_at,
        )?,
        None => delete_hd_account_chain_sync_state(run.user_id, account_id, derivation_change)?,
    }
    Ok(())
}

pub(super) fn run_hd_bundle_scan(
    request: HdBundleScanRequest<'_>,
) -> Result<(), UserTransactionMonitorError> {
    let HdBundleScanRequest {
        run,
        clients,
        pending_address_ids,
        bundle,
        mut completed_address_ids,
        known_activity,
        chain_tip_cache,
        accumulator,
        sync_executor,
        derivation_provider,
        historical_backfill_enabled,
    } = request;
    if let Some(sync_state) = bundle.sync_state.as_ref()
        && sync_state.account_id != bundle.account_id
    {
        tracing::warn!(
            user_id = %run.user_id,
            run_id = %run.run_id,
            account_id = %bundle.account_id,
            sync_state_account_id = %sync_state.account_id,
            "transactions sync: account sync state does not match bundle account"
        );
    }

    let gap_limit = bundle
        .sync_state
        .as_ref()
        .map(|state| state.gap_limit)
        .unwrap_or(crate::wallets::BIP44_GAP_LIMIT);
    let previous_last_scanned_time = bundle
        .sync_state
        .as_ref()
        .and_then(|state| state.last_scanned_time);
    let previous_external_index = bundle
        .sync_state
        .as_ref()
        .and_then(|state| state.last_derived_external_index);
    let previous_internal_index = bundle
        .sync_state
        .as_ref()
        .and_then(|state| state.last_derived_internal_index);
    let external_frontier_state =
        load_hd_account_chain_sync_state(run.user_id, bundle.account_id, 0)?
            .map(|state| state.frontier_state);
    let internal_frontier_state =
        load_hd_account_chain_sync_state(run.user_id, bundle.account_id, 1)?
            .map(|state| state.frontier_state);

    let mut external_addresses = bundle.external_addresses;
    let mut internal_addresses = bundle.internal_addresses;
    let mut addresses_total = u32::try_from(
        external_addresses
            .len()
            .saturating_add(internal_addresses.len()),
    )
    .unwrap_or(u32::MAX);
    let mut processed_for_account = 0_u32;
    let started_at_utc = run.clock.utc_now();
    tracing::info!(
        user_id = %run.user_id,
        run_id = %run.run_id,
        account_id = %bundle.account_id,
        asset_id = %bundle.asset_id.as_str(),
        network = %bundle.network.as_str(),
        address_scheme = %bundle.address_scheme.as_str(),
        gap_limit,
        previous_external_index = ?previous_external_index,
        previous_internal_index = ?previous_internal_index,
        external_addresses = external_addresses.len(),
        internal_addresses = internal_addresses.len(),
        "transactions sync: HD account scan started"
    );
    crate::db::debug_assert_user_db_unlocked(run.user_id, "hd account start publish");
    let hd_started_integration_id = SyncIntegrationId::for_asset(bundle.asset_id);
    publish_transaction_sync_event(
        run.user_id,
        TransactionSyncEvent::account_sync_started_hd_account(
            run.run_id,
            started_at_utc,
            bundle.account_id,
            AddressCount::from_u32(addresses_total),
        ),
    );
    publish_transaction_sync_event(
        run.user_id,
        TransactionSyncEvent::account_integration_sync_started_hd_account(
            run.run_id,
            started_at_utc,
            bundle.account_id,
            hd_started_integration_id,
            AddressCount::from_u32(addresses_total),
        ),
    );

    let external_outcome = run_hd_chain_scan(HdChainScanRequest {
        run,
        clients,
        pending_address_ids,
        frontier_state: external_frontier_state,
        account_id: bundle.account_id,
        asset_id: bundle.asset_id,
        network: bundle.network,
        address_scheme: bundle.address_scheme,
        derivation_change: 0,
        gap_limit,
        addresses: &mut external_addresses,
        known_activity,
        chain_tip_cache,
        completed_address_ids: &mut completed_address_ids,
        addresses_total: &mut addresses_total,
        accumulator,
        processed_for_account: &mut processed_for_account,
        sync_executor,
        derivation_provider,
        historical_backfill_enabled,
    })?;
    let observed_at = run.clock.utc_now();
    persist_hd_chain_frontier_state(
        run,
        bundle.account_id,
        0,
        external_outcome.frontier_state.as_ref(),
        observed_at,
    )?;

    let internal_outcome = run_hd_chain_scan(HdChainScanRequest {
        run,
        clients,
        pending_address_ids,
        frontier_state: internal_frontier_state,
        account_id: bundle.account_id,
        asset_id: bundle.asset_id,
        network: bundle.network,
        address_scheme: bundle.address_scheme,
        derivation_change: 1,
        gap_limit,
        addresses: &mut internal_addresses,
        known_activity,
        chain_tip_cache,
        completed_address_ids: &mut completed_address_ids,
        addresses_total: &mut addresses_total,
        accumulator,
        processed_for_account: &mut processed_for_account,
        sync_executor,
        derivation_provider,
        historical_backfill_enabled,
    })?;
    persist_hd_chain_frontier_state(
        run,
        bundle.account_id,
        1,
        internal_outcome.frontier_state.as_ref(),
        run.clock.utc_now(),
    )?;
    let observed_at = run.clock.utc_now();

    upsert_account_sync_state(
        run.user_id,
        bundle.account_id,
        gap_limit,
        max_derived_index(&external_addresses).or(previous_external_index),
        max_derived_index(&internal_addresses).or(previous_internal_index),
        observed_at,
    )?;
    let completed_tip = chain_tip_cache
        .tips
        .get(&chain_tip_cache_key(bundle.asset_id, bundle.network))
        .map(|cached| cached.height);
    if !external_outcome.interrupted
        && !internal_outcome.interrupted
        && let Some(completed_tip) = completed_tip
    {
        let external_last_index =
            max_derived_index(&external_addresses).or(previous_external_index);
        let internal_last_index =
            max_derived_index(&internal_addresses).or(previous_internal_index);
        if bundle.asset_id == SyncedAssetId::Bitcoin {
            publish_bitcoin_account_completion(
                run.user_id,
                BitcoinAccountCompletionPublication {
                    account_id: bundle.account_id,
                    final_address_proof: None,
                    completed_hd_discovery: Some(BitcoinHdDiscoveryPublication {
                        external_last_index,
                        internal_last_index,
                        completed_tip,
                        completed_at: observed_at,
                    }),
                    observed_at,
                },
            )?;
        } else {
            complete_hd_account_discovery(
                run.user_id,
                bundle.account_id,
                external_last_index,
                internal_last_index,
                completed_tip,
                observed_at,
            )?;
        }
    }
    tracing::info!(
        user_id = %run.user_id,
        run_id = %run.run_id,
        account_id = %bundle.account_id,
        previous_last_scanned_time = ?previous_last_scanned_time,
        last_scanned_height = ?completed_tip.map(ChainTipHeight::value),
        last_derived_external_index = ?max_derived_index(&external_addresses).or(previous_external_index),
        last_derived_internal_index = ?max_derived_index(&internal_addresses).or(previous_internal_index),
        external_addresses = external_addresses.len(),
        internal_addresses = internal_addresses.len(),
        "transactions sync: updated HD account sync state"
    );
    Ok(())
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use crate::transactions::TrackedAddress;

    fn make_hd_sync_address(
        account_id: DigitalAssetAccountId,
        derivation_change: u32,
        derivation_index: u32,
    ) -> SyncAddress {
        SyncAddress {
            address_id: DigitalAssetAddressId::new(),
            address: TrackedAddress::parse(&format!("bc1qhdfrontier{derivation_index:03}"))
                .expect("test address should parse"),
            asset_id: SyncedAssetId::Bitcoin,
            network: Network::Mainnet,
            account_id: Some(account_id),
            derivation_change: Some(derivation_change),
            derivation_index: Some(derivation_index),
            address_scheme: Some(AddressScheme::NativeSegwit),
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
            consecutive_failure_count: crate::transactions::ConsecutiveFailureCount::zero(),
        }
    }

    #[test]
    fn default_hd_chain_frontier_state_skips_known_activity_addresses() {
        let account_id = DigitalAssetAccountId::new();
        let addresses = vec![
            make_hd_sync_address(account_id, 0, 0),
            make_hd_sync_address(account_id, 0, 1),
            make_hd_sync_address(account_id, 0, 2),
        ];
        let known_activity = HashSet::from([addresses[0].address_id, addresses[1].address_id]);

        let frontier_state = default_hd_chain_frontier_state(
            &addresses,
            &known_activity,
            account_id,
            AddressScheme::NativeSegwit,
            0,
        );

        assert_eq!(
            frontier_state,
            HdAccountChainSyncState::ExistingAddresses {
                next_index_to_scan: 2,
                consecutive_unused: 0,
            }
        );
    }

    #[test]
    fn build_hd_chain_frontier_state_rejects_missing_active_rescan_index() {
        let error =
            build_hd_chain_frontier_state(HdAccountChainFrontierPhase::ActiveRescan, 5, 1, None)
                .expect_err("active rescan frontier requires a resume index");

        assert!(
            error
                .to_string()
                .contains("missing active rescan resume index")
        );
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod hd_scan_integration_tests {
    use super::super::chain_tip::CachedChainTip;
    use super::super::context::{SyncClock, SyncHttpCounters};
    use super::super::test_support::{
        DerivationRequestLog, FakeAddressDerivationProvider, FakeAddressSyncExecutor, FakeClock,
        FakeSyncOutcome, clear_rate_limiter_for_test, make_derived_sync_address, make_run_context,
        make_sync_address, persist_sync_addresses_for_test, test_utc_now,
        with_rate_limiter_isolated,
    };
    use super::super::{
        FAILED_ADDRESS_SYNC_COOLDOWN, LABEL_MEMPOOL, MAX_ADDRESSES_PER_ACCOUNT_PER_RUN,
        record_rate_limit,
    };
    use super::*;
    use crate::db::{
        acquire_test_runtime, complete_hd_account_discovery, create_eth_wallet_account_fixture,
        initialize_user_db_for_test,
    };
    use crate::models::UserId;
    use crate::tasks::TriggerSource;
    use chrono::Duration as ChronoDuration;
    use std::time::Duration;

    fn convert_fixture_to_bitcoin_hd(user_id: UserId, account_id: DigitalAssetAccountId) {
        crate::db::with_user_db_mut(user_id, |conn| {
            conn.execute(
                "UPDATE digital_asset_accounts
                 SET asset_id = 'bitcoin',
                     network = 'mainnet',
                     account_kind = 'hd_pubkey'
                 WHERE id = ?1",
                [account_id.to_string()],
            )
            .map_err(|err| {
                crate::db::DbError::new(format!("account fixture conversion failed: {err}"))
            })?;
            conn.execute(
                "UPDATE digital_asset_addresses
                 SET asset_id = 'bitcoin',
                     network = 'mainnet'
                 WHERE account_id = ?1",
                [account_id.to_string()],
            )
            .map_err(|err| {
                crate::db::DbError::new(format!("address fixture conversion failed: {err}"))
            })?;
            Ok::<(), crate::db::DbError>(())
        })
        .expect("fixture should convert to Bitcoin HD");
    }

    #[test]
    fn hd_history_discovery_completion_requires_both_branch_frontiers_to_clear() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user DB should initialize");
        let completed_at = test_utc_now();
        let previous_completed_at = completed_at - ChronoDuration::hours(1);
        let fixture = create_eth_wallet_account_fixture(
            user_id,
            &crate::ethereum::EthAddress::parse(&crate::ethereum::RawEthAddress::new(
                "0x52908400098527886E0F7030069857D2E4169EE7".to_string(),
            ))
            .expect("test address should parse"),
            "HD discovery",
            previous_completed_at,
        );
        let previous_tip = ChainTipHeight::try_new(99).expect("tip should parse");
        let completed_tip = ChainTipHeight::try_new(100).expect("tip should parse");
        upsert_account_sync_state(
            user_id,
            fixture.account_id,
            20,
            Some(3),
            Some(4),
            previous_completed_at,
        )
        .expect("initial checkpoint should persist");
        complete_hd_account_discovery(
            user_id,
            fixture.account_id,
            Some(3),
            Some(4),
            previous_tip,
            previous_completed_at,
        )
        .expect("initial completed checkpoint should persist");
        upsert_hd_account_chain_sync_state(
            user_id,
            fixture.account_id,
            1,
            &HdAccountChainSyncState::DerivedAddresses {
                next_index_to_scan: 5,
                consecutive_unused: 1,
            },
            completed_at,
        )
        .expect("internal frontier should persist");

        assert!(
            complete_hd_account_discovery(
                user_id,
                fixture.account_id,
                Some(6),
                Some(7),
                completed_tip,
                completed_at,
            )
            .is_err(),
            "a remaining branch frontier must block completed checkpoint publication"
        );

        let checkpoint = crate::db::with_user_db(user_id, |conn| {
            conn.query_row(
                "SELECT last_scanned_height, last_scanned_time
                 FROM account_sync_state
                 WHERE account_id = ?1",
                [fixture.account_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                    ))
                },
            )
            .map_err(|err| crate::db::DbError::new(format!("checkpoint query failed: {err}")))
        })
        .expect("checkpoint should load");
        assert_eq!(checkpoint.0, Some(99));
        assert_ne!(checkpoint.1, Some(completed_at.to_rfc3339()));

        delete_hd_account_chain_sync_state(user_id, fixture.account_id, 1)
            .expect("internal frontier should clear");
        complete_hd_account_discovery(
            user_id,
            fixture.account_id,
            Some(6),
            Some(7),
            completed_tip,
            completed_at,
        )
        .expect("frontier-free discovery should complete");
        let completed_checkpoint = crate::db::with_user_db(user_id, |conn| {
            conn.query_row(
                "SELECT last_scanned_height, last_scanned_time
                 FROM account_sync_state
                 WHERE account_id = ?1",
                [fixture.account_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                    ))
                },
            )
            .map_err(|err| crate::db::DbError::new(format!("checkpoint query failed: {err}")))
        })
        .expect("completed checkpoint should load");
        assert_eq!(completed_checkpoint.0, Some(100));
        assert_eq!(completed_checkpoint.1, Some(completed_at.to_rfc3339()));
    }

    fn assert_internal_interruption_preserves_checkpoint_and_frontier(
        interruption: FakeSyncOutcome,
    ) {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let run = make_run_context(&clock);
            let previous_completed_at = run.clock.utc_now() - ChronoDuration::hours(1);
            let fixture = create_eth_wallet_account_fixture(
                run.user_id,
                &crate::ethereum::EthAddress::parse(&crate::ethereum::RawEthAddress::new(
                    "0x52908400098527886E0F7030069857D2E4169EE7".to_string(),
                ))
                .expect("test address should parse"),
                "HD failure",
                previous_completed_at,
            );
            let previous_tip = ChainTipHeight::try_new(99).expect("tip should parse");
            upsert_account_sync_state(
                run.user_id,
                fixture.account_id,
                0,
                Some(0),
                Some(0),
                previous_completed_at,
            )
            .expect("sync state should seed");
            complete_hd_account_discovery(
                run.user_id,
                fixture.account_id,
                Some(0),
                Some(0),
                previous_tip,
                previous_completed_at,
            )
            .expect("checkpoint should seed");
            let external = make_sync_address(
                "bc1qbundlefailureexternal",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                Some(fixture.account_id),
                Some(AddressScheme::NativeSegwit),
                Some(0),
                Some(0),
            );
            let internal = make_sync_address(
                "bc1qbundlefailureinternal",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                Some(fixture.account_id),
                Some(AddressScheme::NativeSegwit),
                Some(1),
                Some(0),
            );
            persist_sync_addresses_for_test(run, &[external.clone(), internal.clone()]);
            let external_id = external.address_id;
            let internal_id = internal.address_id;
            let bundle = AccountSyncBundle {
                account_id: fixture.account_id,
                asset_id: SyncedAssetId::Bitcoin,
                network: Network::Mainnet,
                hd_key_extended_pubkey: "test".to_string(),
                address_scheme: AddressScheme::NativeSegwit,
                sync_state: Some(crate::db::AccountSyncStateRow {
                    account_id: fixture.account_id,
                    last_scanned_time: Some(previous_completed_at),
                    gap_limit: 0,
                    last_derived_external_index: Some(0),
                    last_derived_internal_index: Some(0),
                    mempool_history_next_address_id: None,
                }),
                external_addresses: vec![external],
                internal_addresses: vec![internal],
            };
            let http_counters = SyncHttpCounters::new();
            let mut chain_tip_cache = ChainTipCache::default();
            chain_tip_cache.tips.insert(
                chain_tip_cache_key(SyncedAssetId::Bitcoin, Network::Mainnet),
                CachedChainTip {
                    height: ChainTipHeight::try_new(100).expect("tip should parse"),
                    fetched_at: clock.instant_now(),
                },
            );
            let mut accumulator = CycleAccumulator::new(2);
            let mut known_activity = HashSet::new();
            let mut executor = FakeAddressSyncExecutor::new(vec![
                FakeSyncOutcome::Success {
                    new_tx_count: 0,
                    updated_tx_count: 0,
                },
                interruption,
            ]);
            let mut derivation_provider = FakeAddressDerivationProvider::new(Vec::new());
            let pending_address_ids = HashSet::new();

            run_hd_bundle_scan(HdBundleScanRequest {
                run,
                clients: SyncClients {
                    mempool_client: None,
                    etherscan_api_key: None,
                    etherscan_base_url: None,
                    http_counters: &http_counters,
                },
                pending_address_ids: &pending_address_ids,
                bundle,
                completed_address_ids: HashSet::new(),
                known_activity: &mut known_activity,
                chain_tip_cache: &mut chain_tip_cache,
                accumulator: &mut accumulator,
                sync_executor: &mut executor,
                derivation_provider: &mut derivation_provider,
                historical_backfill_enabled: false,
            })
            .expect("interrupted branch should persist a resumable bundle state");

            assert_eq!(executor.calls, vec![external_id, internal_id]);
            assert!(
                load_hd_account_chain_sync_state(run.user_id, fixture.account_id, 0)
                    .expect("external frontier should load")
                    .is_none()
            );
            assert_eq!(
                load_hd_account_chain_sync_state(run.user_id, fixture.account_id, 1)
                    .expect("frontier should load")
                    .map(|row| row.frontier_state),
                Some(HdAccountChainSyncState::ExistingAddresses {
                    next_index_to_scan: 0,
                    consecutive_unused: 0,
                })
            );
            assert!(derivation_provider.requests.is_empty());
            let checkpoint = crate::db::with_user_db(run.user_id, |conn| {
                conn.query_row(
                    "SELECT last_scanned_height, last_scanned_time
                     FROM account_sync_state
                     WHERE account_id = ?1",
                    [fixture.account_id.to_string()],
                    |row| {
                        Ok((
                            row.get::<_, Option<i64>>(0)?,
                            row.get::<_, Option<String>>(1)?,
                        ))
                    },
                )
                .map_err(|err| crate::db::DbError::new(format!("checkpoint query failed: {err}")))
            })
            .expect("checkpoint should load");
            assert_eq!(
                checkpoint,
                (Some(99), Some(previous_completed_at.to_rfc3339()))
            );
        });
    }

    #[test]
    fn hd_history_discovery_internal_rate_limit_preserves_checkpoint_and_frontier() {
        assert_internal_interruption_preserves_checkpoint_and_frontier(
            FakeSyncOutcome::RateLimited {
                integration: LABEL_MEMPOOL.to_string(),
            },
        );
    }

    #[test]
    fn hd_history_discovery_internal_failure_preserves_checkpoint_and_frontier() {
        assert_internal_interruption_preserves_checkpoint_and_frontier(FakeSyncOutcome::Failure {
            message: "provider failure".to_string(),
        });
    }

    #[test]
    fn hd_history_discovery_bundle_resets_independent_branch_gaps_and_completes_once() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let base_run = make_run_context(&clock);
            let run = RunContext {
                source: TriggerSource::AutoUpgrade,
                ..base_run
            };
            let previous_completed_at = run.clock.utc_now() - ChronoDuration::hours(1);
            let fixture = create_eth_wallet_account_fixture(
                run.user_id,
                &crate::ethereum::EthAddress::parse(&crate::ethereum::RawEthAddress::new(
                    "0xde709f2102306220921060314715629080e2fb77".to_string(),
                ))
                .expect("test address should parse"),
                "HD branch gaps",
                previous_completed_at,
            );
            convert_fixture_to_bitcoin_hd(run.user_id, fixture.account_id);
            let previous_tip = ChainTipHeight::try_new(99).expect("tip should parse");
            upsert_account_sync_state(
                run.user_id,
                fixture.account_id,
                2,
                Some(2),
                Some(3),
                previous_completed_at,
            )
            .expect("sync state should seed");
            complete_hd_account_discovery(
                run.user_id,
                fixture.account_id,
                Some(2),
                Some(3),
                previous_tip,
                previous_completed_at,
            )
            .expect("checkpoint should seed");
            for derivation_change in [0, 1] {
                upsert_hd_account_chain_sync_state(
                    run.user_id,
                    fixture.account_id,
                    derivation_change,
                    &HdAccountChainSyncState::ExistingAddresses {
                        next_index_to_scan: 0,
                        consecutive_unused: 0,
                    },
                    previous_completed_at,
                )
                .expect("branch frontier should seed");
            }

            let external_addresses = (0..3_u32)
                .map(|index| {
                    make_sync_address(
                        &format!("bc1qbundlegapexternal{index}"),
                        SyncedAssetId::Bitcoin,
                        Network::Mainnet,
                        Some(fixture.account_id),
                        Some(AddressScheme::NativeSegwit),
                        Some(0),
                        Some(index),
                    )
                })
                .collect::<Vec<_>>();
            let internal_addresses = (0..4_u32)
                .map(|index| {
                    make_sync_address(
                        &format!("bc1qbundlegapinternal{index}"),
                        SyncedAssetId::Bitcoin,
                        Network::Mainnet,
                        Some(fixture.account_id),
                        Some(AddressScheme::NativeSegwit),
                        Some(1),
                        Some(index),
                    )
                })
                .collect::<Vec<_>>();
            persist_sync_addresses_for_test(run, &external_addresses);
            persist_sync_addresses_for_test(run, &internal_addresses);
            let external_ids = external_addresses
                .iter()
                .map(|address| address.address_id)
                .collect::<Vec<_>>();
            let internal_ids = internal_addresses
                .iter()
                .map(|address| address.address_id)
                .collect::<Vec<_>>();
            let external_derived = make_derived_sync_address("bc1qbundlegapexternal3", 0, 3);
            let external_derived_id = external_derived.address_id;
            let bundle = AccountSyncBundle {
                account_id: fixture.account_id,
                asset_id: SyncedAssetId::Bitcoin,
                network: Network::Mainnet,
                hd_key_extended_pubkey: "test".to_string(),
                address_scheme: AddressScheme::NativeSegwit,
                sync_state: Some(crate::db::AccountSyncStateRow {
                    account_id: fixture.account_id,
                    last_scanned_time: Some(previous_completed_at),
                    gap_limit: 2,
                    last_derived_external_index: Some(2),
                    last_derived_internal_index: Some(3),
                    mempool_history_next_address_id: None,
                }),
                external_addresses,
                internal_addresses,
            };
            let http_counters = SyncHttpCounters::new();
            let mut chain_tip_cache = ChainTipCache::default();
            chain_tip_cache.tips.insert(
                chain_tip_cache_key(SyncedAssetId::Bitcoin, Network::Mainnet),
                CachedChainTip {
                    height: ChainTipHeight::try_new(100).expect("tip should parse"),
                    fetched_at: clock.instant_now(),
                },
            );
            let mut accumulator = CycleAccumulator::new(7);
            let mut known_activity = HashSet::from([external_ids[1], internal_ids[1]]);
            let mut executor = FakeAddressSyncExecutor::new(
                (0..6)
                    .map(|_| FakeSyncOutcome::Success {
                        new_tx_count: 0,
                        updated_tx_count: 0,
                    })
                    .collect(),
            );
            let mut derivation_provider =
                FakeAddressDerivationProvider::new(vec![vec![external_derived]]);
            let pending_address_ids = HashSet::new();

            run_hd_bundle_scan(HdBundleScanRequest {
                run,
                clients: SyncClients {
                    mempool_client: None,
                    etherscan_api_key: None,
                    etherscan_base_url: None,
                    http_counters: &http_counters,
                },
                pending_address_ids: &pending_address_ids,
                bundle,
                completed_address_ids: HashSet::from([external_ids[1], internal_ids[1]]),
                known_activity: &mut known_activity,
                chain_tip_cache: &mut chain_tip_cache,
                accumulator: &mut accumulator,
                sync_executor: &mut executor,
                derivation_provider: &mut derivation_provider,
                historical_backfill_enabled: true,
            })
            .expect("both branch scans should complete");

            assert_eq!(
                executor.calls,
                vec![
                    external_ids[0],
                    external_ids[2],
                    external_derived_id,
                    internal_ids[0],
                    internal_ids[2],
                    internal_ids[3],
                ],
                "repair should consume breadth visits, reset each branch gap, and visit each address once"
            );
            assert_eq!(
                derivation_provider.requests,
                vec![DerivationRequestLog {
                    account_id: fixture.account_id,
                    derivation_change: 0,
                    count: 1,
                }]
            );
            assert!(
                load_hd_account_chain_sync_state(run.user_id, fixture.account_id, 0)
                    .expect("external frontier should load")
                    .is_none()
            );
            assert!(
                load_hd_account_chain_sync_state(run.user_id, fixture.account_id, 1)
                    .expect("internal frontier should load")
                    .is_none()
            );
            let checkpoint = crate::db::with_user_db(run.user_id, |conn| {
                conn.query_row(
                    "SELECT last_scanned_height, last_scanned_time,
                            last_derived_external_index, last_derived_internal_index
                     FROM account_sync_state
                     WHERE account_id = ?1",
                    [fixture.account_id.to_string()],
                    |row| {
                        Ok((
                            row.get::<_, Option<i64>>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, Option<i64>>(2)?,
                            row.get::<_, Option<i64>>(3)?,
                        ))
                    },
                )
                .map_err(|err| crate::db::DbError::new(format!("checkpoint query failed: {err}")))
            })
            .expect("checkpoint should load");
            assert_eq!(
                checkpoint,
                (
                    Some(100),
                    Some(run.clock.utc_now().to_rfc3339()),
                    Some(3),
                    Some(3),
                )
            );
        });
    }

    #[test]
    fn hd_history_discovery_bundle_does_not_rescan_new_activity_in_same_cycle() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let base_run = make_run_context(&clock);
            let run = RunContext {
                source: TriggerSource::AutoUpgrade,
                ..base_run
            };
            let created_at = run.clock.utc_now();
            let fixture = create_eth_wallet_account_fixture(
                run.user_id,
                &crate::ethereum::EthAddress::parse(&crate::ethereum::RawEthAddress::new(
                    "0x27b1fdb04752bbc536007a920d24acb045561c26".to_string(),
                ))
                .expect("test address should parse"),
                "HD no duplicate",
                created_at,
            );
            convert_fixture_to_bitcoin_hd(run.user_id, fixture.account_id);
            let external = make_sync_address(
                "bc1qbundlenoduplicateexternal0",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                Some(fixture.account_id),
                Some(AddressScheme::NativeSegwit),
                Some(0),
                Some(0),
            );
            let internal = make_sync_address(
                "bc1qbundlenoduplicateinternal0",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                Some(fixture.account_id),
                Some(AddressScheme::NativeSegwit),
                Some(1),
                Some(0),
            );
            persist_sync_addresses_for_test(run, &[external.clone(), internal.clone()]);
            let external_id = external.address_id;
            let internal_id = internal.address_id;
            let derived = make_derived_sync_address("bc1qbundlenoduplicateexternal1", 0, 1);
            let derived_id = derived.address_id;
            let bundle = AccountSyncBundle {
                account_id: fixture.account_id,
                asset_id: SyncedAssetId::Bitcoin,
                network: Network::Mainnet,
                hd_key_extended_pubkey: "test".to_string(),
                address_scheme: AddressScheme::NativeSegwit,
                sync_state: Some(crate::db::AccountSyncStateRow {
                    account_id: fixture.account_id,
                    last_scanned_time: None,
                    gap_limit: 1,
                    last_derived_external_index: Some(0),
                    last_derived_internal_index: Some(0),
                    mempool_history_next_address_id: None,
                }),
                external_addresses: vec![external],
                internal_addresses: vec![internal],
            };
            let http_counters = SyncHttpCounters::new();
            let mut chain_tip_cache = ChainTipCache::default();
            chain_tip_cache.tips.insert(
                chain_tip_cache_key(SyncedAssetId::Bitcoin, Network::Mainnet),
                CachedChainTip {
                    height: ChainTipHeight::try_new(100).expect("tip should parse"),
                    fetched_at: clock.instant_now(),
                },
            );
            let mut accumulator = CycleAccumulator::new(2);
            let mut known_activity = HashSet::new();
            let mut executor = FakeAddressSyncExecutor::new(vec![
                FakeSyncOutcome::SuccessWithObservedActivity,
                FakeSyncOutcome::Success {
                    new_tx_count: 0,
                    updated_tx_count: 0,
                },
                FakeSyncOutcome::Success {
                    new_tx_count: 0,
                    updated_tx_count: 0,
                },
            ]);
            let mut derivation_provider = FakeAddressDerivationProvider::new(vec![vec![derived]]);
            let pending_address_ids = HashSet::new();

            run_hd_bundle_scan(HdBundleScanRequest {
                run,
                clients: SyncClients {
                    mempool_client: None,
                    etherscan_api_key: None,
                    etherscan_base_url: None,
                    http_counters: &http_counters,
                },
                pending_address_ids: &pending_address_ids,
                bundle,
                completed_address_ids: HashSet::new(),
                known_activity: &mut known_activity,
                chain_tip_cache: &mut chain_tip_cache,
                accumulator: &mut accumulator,
                sync_executor: &mut executor,
                derivation_provider: &mut derivation_provider,
                historical_backfill_enabled: true,
            })
            .expect("bundle scan should complete");

            assert_eq!(executor.calls, vec![external_id, derived_id, internal_id]);
            assert_eq!(
                derivation_provider.requests,
                vec![DerivationRequestLog {
                    account_id: fixture.account_id,
                    derivation_change: 0,
                    count: 1,
                }]
            );
        });
    }

    #[test]
    fn run_hd_chain_scan_derives_and_scans_until_gap_limit() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let run = make_run_context(&clock);
            let account_id = crate::wallets::DigitalAssetAccountId::new();
            let mut addresses = Vec::new();
            let mut known_activity = HashSet::new();
            let mut chain_tip_cache = ChainTipCache::default();
            let mut completed_address_ids = HashSet::new();
            let mut addresses_total = 0_u32;
            let mut accumulator = CycleAccumulator::new(0);
            let mut processed_for_account = 0_u32;
            let http_counters = SyncHttpCounters::new();
            let clients = SyncClients {
                mempool_client: None,
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            };
            let mut executor = FakeAddressSyncExecutor::new(vec![
                FakeSyncOutcome::Success {
                    new_tx_count: 0,
                    updated_tx_count: 0,
                },
                FakeSyncOutcome::Success {
                    new_tx_count: 0,
                    updated_tx_count: 0,
                },
            ]);
            let mut derivation_provider = FakeAddressDerivationProvider::new(vec![vec![
                make_derived_sync_address("bc1qderived000", 0, 0),
                make_derived_sync_address("bc1qderived001", 0, 1),
            ]]);
            let pending_address_ids = HashSet::new();

            run_hd_chain_scan(HdChainScanRequest {
                run,
                clients,
                pending_address_ids: &pending_address_ids,
                frontier_state: None,
                account_id,
                asset_id: SyncedAssetId::Bitcoin,
                network: crate::wallets::Network::Mainnet,
                address_scheme: crate::wallets::AddressScheme::NativeSegwit,
                derivation_change: 0,
                gap_limit: 2,
                addresses: &mut addresses,
                known_activity: &mut known_activity,
                chain_tip_cache: &mut chain_tip_cache,
                completed_address_ids: &mut completed_address_ids,
                addresses_total: &mut addresses_total,
                accumulator: &mut accumulator,
                processed_for_account: &mut processed_for_account,
                sync_executor: &mut executor,
                derivation_provider: &mut derivation_provider,
                historical_backfill_enabled: true,
            })
            .expect("HD scan should succeed");

            assert_eq!(processed_for_account, 2);
            assert_eq!(addresses.len(), 2);
            assert_eq!(accumulator.addresses_total, 2);
            assert_eq!(accumulator.addresses_synced, 2);
            assert_eq!(executor.calls.len(), 2);
            assert_eq!(completed_address_ids.len(), 2);
            assert!(known_activity.is_empty());
            assert_eq!(derivation_provider.requests.len(), 1);
            assert_eq!(
                derivation_provider.requests[0],
                DerivationRequestLog {
                    account_id,
                    derivation_change: 0,
                    count: 2,
                }
            );
            assert_eq!(clock.sleep_count(), 1);
        });
    }

    #[test]
    fn run_hd_chain_scan_respects_per_account_processing_cap() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let run = make_run_context(&clock);
            let account_id = crate::wallets::DigitalAssetAccountId::new();
            let mut addresses = (0..(MAX_ADDRESSES_PER_ACCOUNT_PER_RUN + 5))
                .map(|index| {
                    make_sync_address(
                        &format!("bc1qcap{index}"),
                        SyncedAssetId::Bitcoin,
                        crate::wallets::Network::Mainnet,
                        Some(account_id),
                        Some(crate::wallets::AddressScheme::NativeSegwit),
                        Some(0),
                        Some(index),
                    )
                })
                .collect::<Vec<SyncAddress>>();
            let mut known_activity = HashSet::new();
            let mut chain_tip_cache = ChainTipCache::default();
            let mut completed_address_ids = HashSet::new();
            let mut addresses_total =
                u32::try_from(addresses.len()).expect("address count should fit in u32");
            persist_sync_addresses_for_test(run, &addresses);
            let mut accumulator = CycleAccumulator::new(
                u32::try_from(addresses.len()).expect("address count should fit in u32"),
            );
            let mut processed_for_account = 0_u32;
            let http_counters = SyncHttpCounters::new();
            let clients = SyncClients {
                mempool_client: None,
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            };
            let mut executor = FakeAddressSyncExecutor::new(
                (0..addresses.len())
                    .map(|_| FakeSyncOutcome::Success {
                        new_tx_count: 0,
                        updated_tx_count: 0,
                    })
                    .collect(),
            );
            let mut derivation_provider = FakeAddressDerivationProvider::new(Vec::new());
            let pending_address_ids = HashSet::new();

            run_hd_chain_scan(HdChainScanRequest {
                run,
                clients,
                pending_address_ids: &pending_address_ids,
                frontier_state: None,
                account_id,
                asset_id: SyncedAssetId::Bitcoin,
                network: crate::wallets::Network::Mainnet,
                address_scheme: crate::wallets::AddressScheme::NativeSegwit,
                derivation_change: 0,
                gap_limit: 0,
                addresses: &mut addresses,
                known_activity: &mut known_activity,
                chain_tip_cache: &mut chain_tip_cache,
                completed_address_ids: &mut completed_address_ids,
                addresses_total: &mut addresses_total,
                accumulator: &mut accumulator,
                processed_for_account: &mut processed_for_account,
                sync_executor: &mut executor,
                derivation_provider: &mut derivation_provider,
                historical_backfill_enabled: true,
            })
            .expect("HD cap scan should succeed");

            assert_eq!(processed_for_account, MAX_ADDRESSES_PER_ACCOUNT_PER_RUN);
            assert_eq!(
                executor.calls.len(),
                usize::try_from(MAX_ADDRESSES_PER_ACCOUNT_PER_RUN)
                    .expect("cap should fit in usize")
            );
            assert!(
                executor
                    .observed_lock_free
                    .iter()
                    .all(|is_lock_free| *is_lock_free),
                "HD executor dispatch should observe no user-db locks"
            );
            assert!(derivation_provider.requests.is_empty());
        });
    }

    #[test]
    fn run_hd_chain_scan_resumes_existing_frontier_without_repeating_scanned_addresses() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let run = make_run_context(&clock);
            let account_id = crate::wallets::DigitalAssetAccountId::new();
            let mut addresses = (0..5_u32)
                .map(|index| {
                    make_sync_address(
                        &format!("bc1qresumeexisting{index}"),
                        SyncedAssetId::Bitcoin,
                        crate::wallets::Network::Mainnet,
                        Some(account_id),
                        Some(crate::wallets::AddressScheme::NativeSegwit),
                        Some(0),
                        Some(index),
                    )
                })
                .collect::<Vec<SyncAddress>>();
            let address_ids = addresses
                .iter()
                .map(|address| address.address_id)
                .collect::<Vec<_>>();
            let mut known_activity = HashSet::new();
            let mut chain_tip_cache = ChainTipCache::default();
            let mut completed_address_ids = HashSet::new();
            let mut addresses_total =
                u32::try_from(addresses.len()).expect("address count should fit in u32");
            persist_sync_addresses_for_test(run, &addresses);
            let http_counters = SyncHttpCounters::new();
            let clients = SyncClients {
                mempool_client: None,
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            };
            let pending_address_ids = HashSet::new();

            let mut first_accumulator = CycleAccumulator::new(addresses_total);
            let mut first_processed_for_account = 0_u32;
            let mut first_executor = FakeAddressSyncExecutor::new(vec![
                FakeSyncOutcome::Success {
                    new_tx_count: 0,
                    updated_tx_count: 0,
                },
                FakeSyncOutcome::Success {
                    new_tx_count: 0,
                    updated_tx_count: 0,
                },
                FakeSyncOutcome::RateLimited {
                    integration: LABEL_MEMPOOL.to_string(),
                },
            ]);
            let mut first_derivation_provider = FakeAddressDerivationProvider::new(Vec::new());

            let first_outcome = run_hd_chain_scan(HdChainScanRequest {
                run,
                clients,
                pending_address_ids: &pending_address_ids,
                frontier_state: None,
                account_id,
                asset_id: SyncedAssetId::Bitcoin,
                network: crate::wallets::Network::Mainnet,
                address_scheme: crate::wallets::AddressScheme::NativeSegwit,
                derivation_change: 0,
                gap_limit: 0,
                addresses: &mut addresses,
                known_activity: &mut known_activity,
                chain_tip_cache: &mut chain_tip_cache,
                completed_address_ids: &mut completed_address_ids,
                addresses_total: &mut addresses_total,
                accumulator: &mut first_accumulator,
                processed_for_account: &mut first_processed_for_account,
                sync_executor: &mut first_executor,
                derivation_provider: &mut first_derivation_provider,
                historical_backfill_enabled: true,
            })
            .expect("initial frontier scan should succeed");

            assert!(first_outcome.interrupted);
            assert_eq!(
                first_outcome.frontier_state,
                Some(crate::db::HdAccountChainSyncState::ExistingAddresses {
                    next_index_to_scan: 2,
                    consecutive_unused: 2,
                })
            );
            assert_eq!(completed_address_ids.len(), 2);
            clear_rate_limiter_for_test();
            clock.sleep(FAILED_ADDRESS_SYNC_COOLDOWN + Duration::from_secs(1));

            let mut second_accumulator = CycleAccumulator::new(addresses_total);
            let mut second_processed_for_account = 0_u32;
            let mut second_executor = FakeAddressSyncExecutor::new(vec![
                FakeSyncOutcome::Success {
                    new_tx_count: 0,
                    updated_tx_count: 0,
                },
                FakeSyncOutcome::Success {
                    new_tx_count: 0,
                    updated_tx_count: 0,
                },
                FakeSyncOutcome::Success {
                    new_tx_count: 0,
                    updated_tx_count: 0,
                },
            ]);
            let mut second_derivation_provider = FakeAddressDerivationProvider::new(Vec::new());
            let mut second_completed_address_ids = HashSet::new();

            let resumed_outcome = run_hd_chain_scan(HdChainScanRequest {
                run,
                clients,
                pending_address_ids: &pending_address_ids,
                frontier_state: first_outcome.frontier_state.clone(),
                account_id,
                asset_id: SyncedAssetId::Bitcoin,
                network: crate::wallets::Network::Mainnet,
                address_scheme: crate::wallets::AddressScheme::NativeSegwit,
                derivation_change: 0,
                gap_limit: 0,
                addresses: &mut addresses,
                known_activity: &mut known_activity,
                chain_tip_cache: &mut chain_tip_cache,
                completed_address_ids: &mut second_completed_address_ids,
                addresses_total: &mut addresses_total,
                accumulator: &mut second_accumulator,
                processed_for_account: &mut second_processed_for_account,
                sync_executor: &mut second_executor,
                derivation_provider: &mut second_derivation_provider,
                historical_backfill_enabled: true,
            })
            .expect("resumed frontier scan should succeed");

            assert!(!resumed_outcome.interrupted);
            assert_eq!(resumed_outcome.frontier_state, None);
            assert_eq!(second_executor.calls, address_ids[2..].to_vec());
            assert_eq!(second_completed_address_ids.len(), 3);
            assert!(second_derivation_provider.requests.is_empty());
        });
    }

    #[test]
    fn run_hd_chain_scan_resumes_derived_frontier_before_deriving_more() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let run = make_run_context(&clock);
            let account_id = crate::wallets::DigitalAssetAccountId::new();
            let mut addresses = Vec::<SyncAddress>::new();
            let mut known_activity = HashSet::new();
            let mut chain_tip_cache = ChainTipCache::default();
            let mut completed_address_ids = HashSet::new();
            let mut addresses_total = 0_u32;
            let http_counters = SyncHttpCounters::new();
            let clients = SyncClients {
                mempool_client: None,
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            };
            let pending_address_ids = HashSet::new();

            let mut first_accumulator = CycleAccumulator::new(0);
            let mut first_processed_for_account = 0_u32;
            let mut first_executor = FakeAddressSyncExecutor::new(vec![
                FakeSyncOutcome::Success {
                    new_tx_count: 0,
                    updated_tx_count: 0,
                },
                FakeSyncOutcome::RateLimited {
                    integration: LABEL_MEMPOOL.to_string(),
                },
            ]);
            let mut first_derivation_provider = FakeAddressDerivationProvider::new(vec![vec![
                make_derived_sync_address("bc1qderivedresume000", 0, 0),
                make_derived_sync_address("bc1qderivedresume001", 0, 1),
            ]]);

            let first_outcome = run_hd_chain_scan(HdChainScanRequest {
                run,
                clients,
                pending_address_ids: &pending_address_ids,
                frontier_state: None,
                account_id,
                asset_id: SyncedAssetId::Bitcoin,
                network: crate::wallets::Network::Mainnet,
                address_scheme: crate::wallets::AddressScheme::NativeSegwit,
                derivation_change: 0,
                gap_limit: 2,
                addresses: &mut addresses,
                known_activity: &mut known_activity,
                chain_tip_cache: &mut chain_tip_cache,
                completed_address_ids: &mut completed_address_ids,
                addresses_total: &mut addresses_total,
                accumulator: &mut first_accumulator,
                processed_for_account: &mut first_processed_for_account,
                sync_executor: &mut first_executor,
                derivation_provider: &mut first_derivation_provider,
                historical_backfill_enabled: true,
            })
            .expect("initial derived frontier scan should succeed");

            assert!(first_outcome.interrupted);
            assert_eq!(
                first_outcome.frontier_state,
                Some(crate::db::HdAccountChainSyncState::DerivedAddresses {
                    next_index_to_scan: 1,
                    consecutive_unused: 1,
                })
            );
            assert_eq!(completed_address_ids.len(), 1);
            assert_eq!(addresses.len(), 2);
            let derived_address_ids = addresses
                .iter()
                .map(|address| address.address_id)
                .collect::<Vec<_>>();
            clear_rate_limiter_for_test();
            clock.sleep(FAILED_ADDRESS_SYNC_COOLDOWN + Duration::from_secs(1));

            let mut second_accumulator = CycleAccumulator::new(addresses_total);
            let mut second_processed_for_account = 0_u32;
            let mut second_executor =
                FakeAddressSyncExecutor::new(vec![FakeSyncOutcome::Success {
                    new_tx_count: 0,
                    updated_tx_count: 0,
                }]);
            let mut second_derivation_provider =
                FakeAddressDerivationProvider::new(vec![vec![make_derived_sync_address(
                    "bc1qderivedresume002",
                    0,
                    2,
                )]]);
            let mut second_completed_address_ids = HashSet::new();

            let resumed_outcome = run_hd_chain_scan(HdChainScanRequest {
                run,
                clients,
                pending_address_ids: &pending_address_ids,
                frontier_state: first_outcome.frontier_state.clone(),
                account_id,
                asset_id: SyncedAssetId::Bitcoin,
                network: crate::wallets::Network::Mainnet,
                address_scheme: crate::wallets::AddressScheme::NativeSegwit,
                derivation_change: 0,
                gap_limit: 2,
                addresses: &mut addresses,
                known_activity: &mut known_activity,
                chain_tip_cache: &mut chain_tip_cache,
                completed_address_ids: &mut second_completed_address_ids,
                addresses_total: &mut addresses_total,
                accumulator: &mut second_accumulator,
                processed_for_account: &mut second_processed_for_account,
                sync_executor: &mut second_executor,
                derivation_provider: &mut second_derivation_provider,
                historical_backfill_enabled: true,
            })
            .expect("resumed derived frontier scan should succeed");

            assert!(!resumed_outcome.interrupted);
            assert_eq!(resumed_outcome.frontier_state, None);
            assert_eq!(second_executor.calls, vec![derived_address_ids[1]]);
            assert_eq!(second_completed_address_ids.len(), 1);
            assert!(
                second_derivation_provider.requests.is_empty(),
                "resume should scan the already-derived address before deriving more"
            );
            assert_eq!(addresses.len(), 2);
        });
    }

    #[test]
    fn run_hd_chain_scan_active_rescan_uses_shared_address_step() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let run = make_run_context(&clock);
            let account_id = DigitalAssetAccountId::new();
            let mut addresses = (0..2_u32)
                .map(|index| {
                    make_sync_address(
                        &format!("bc1qactiverescan{index}"),
                        SyncedAssetId::Bitcoin,
                        crate::wallets::Network::Mainnet,
                        Some(account_id),
                        Some(crate::wallets::AddressScheme::NativeSegwit),
                        Some(0),
                        Some(index),
                    )
                })
                .collect::<Vec<_>>();
            persist_sync_addresses_for_test(run, &addresses);
            let active_address_id = addresses[1].address_id;
            let mut known_activity = HashSet::from([active_address_id]);
            let mut chain_tip_cache = ChainTipCache::default();
            let mut completed_address_ids = HashSet::new();
            let mut addresses_total = 2_u32;
            let mut accumulator = CycleAccumulator::new(addresses_total);
            let mut processed_for_account = 0_u32;
            let http_counters = SyncHttpCounters::new();
            let clients = SyncClients {
                mempool_client: None,
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            };
            let pending_address_ids = HashSet::new();
            let mut executor = FakeAddressSyncExecutor::new(vec![FakeSyncOutcome::Success {
                new_tx_count: 0,
                updated_tx_count: 0,
            }]);
            let mut derivation_provider = FakeAddressDerivationProvider::new(Vec::new());

            let outcome = run_hd_chain_scan(HdChainScanRequest {
                run,
                clients,
                pending_address_ids: &pending_address_ids,
                frontier_state: Some(crate::db::HdAccountChainSyncState::ActiveRescan {
                    next_index_to_scan: 2,
                    consecutive_unused: 2,
                    active_rescan_from_index: 1,
                }),
                account_id,
                asset_id: SyncedAssetId::Bitcoin,
                network: crate::wallets::Network::Mainnet,
                address_scheme: crate::wallets::AddressScheme::NativeSegwit,
                derivation_change: 0,
                gap_limit: 2,
                addresses: &mut addresses,
                known_activity: &mut known_activity,
                chain_tip_cache: &mut chain_tip_cache,
                completed_address_ids: &mut completed_address_ids,
                addresses_total: &mut addresses_total,
                accumulator: &mut accumulator,
                processed_for_account: &mut processed_for_account,
                sync_executor: &mut executor,
                derivation_provider: &mut derivation_provider,
                historical_backfill_enabled: true,
            })
            .expect("active rescan should succeed");

            assert_eq!(outcome, HdChainScanOutcome::completed());
            assert_eq!(executor.calls, vec![active_address_id]);
            assert_eq!(completed_address_ids, HashSet::from([active_address_id]));
            assert!(derivation_provider.requests.is_empty());
        });
    }

    #[test]
    fn hd_address_step_publishes_progress_once_per_completed_address() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let run = make_run_context(&clock);
            let account_id = DigitalAssetAccountId::new();
            let mut address = make_sync_address(
                "bc1qaddressstepprogress",
                SyncedAssetId::Bitcoin,
                crate::wallets::Network::Mainnet,
                Some(account_id),
                Some(crate::wallets::AddressScheme::NativeSegwit),
                Some(0),
                Some(0),
            );
            persist_sync_addresses_for_test(run, std::slice::from_ref(&address));
            let mut receiver = crate::tasks::subscribe_transaction_sync_events(run.user_id)
                .expect("sync event subscription should succeed");
            let mut chain_tip_cache = ChainTipCache::default();
            let mut completed_address_ids = HashSet::new();
            let mut known_activity = HashSet::new();
            let mut consecutive_unused = 0_u32;
            let mut accumulator = CycleAccumulator::new(1);
            let mut processed_for_account = 0_u32;
            let http_counters = SyncHttpCounters::new();
            let clients = SyncClients {
                mempool_client: None,
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            };
            let pending_address_ids = HashSet::new();
            let mut executor = FakeAddressSyncExecutor::new(vec![
                FakeSyncOutcome::Success {
                    new_tx_count: 0,
                    updated_tx_count: 0,
                },
                FakeSyncOutcome::Success {
                    new_tx_count: 0,
                    updated_tx_count: 0,
                },
            ]);

            for attempt in 0..2 {
                run_hd_address_step(HdAddressStepRequest {
                    control: SyncSingleAddressControlRequest {
                        run,
                        address: &mut address,
                        chain_tip_cache: &mut chain_tip_cache,
                        pending_address_ids: &pending_address_ids,
                        clients,
                        executor: &mut executor,
                        accumulator: &mut accumulator,
                        processed_for_account: &mut processed_for_account,
                        single_address_progress: None,
                        mempool_history_policy: MempoolHistoryPolicy::CurrentOnly,
                        mempool_history_page_frontier: None,
                    },
                    account_id,
                    asset_id: SyncedAssetId::Bitcoin,
                    completed_address_ids: &mut completed_address_ids,
                    addresses_total: 1,
                    known_activity: &mut known_activity,
                    consecutive_unused: Some(&mut consecutive_unused),
                })
                .expect("address step should succeed");

                if attempt == 0 {
                    assert_eq!(completed_address_ids.len(), 1);
                    assert_eq!(accumulator.addresses_synced, 1);
                    assert_eq!(
                        [
                            receiver
                                .try_recv()
                                .expect("account progress event should publish")
                                .event_type,
                            receiver
                                .try_recv()
                                .expect("integration progress event should publish")
                                .event_type,
                        ],
                        [
                            crate::transactions::TransactionSyncEventType::AccountSyncProgress,
                            crate::transactions::TransactionSyncEventType::AccountIntegrationSyncProgress,
                        ]
                    );
                }
                assert!(matches!(
                    receiver.try_recv(),
                    Err(tokio::sync::broadcast::error::TryRecvError::Empty)
                ));
            }

            assert_eq!(completed_address_ids.len(), 1);
            assert_eq!(accumulator.addresses_skipped, 1);
            assert_eq!(processed_for_account, 1);
        });
    }

    #[test]
    fn hd_address_step_active_rescan_updates_activity_without_gap_state() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let run = make_run_context(&clock);
            let account_id = DigitalAssetAccountId::new();
            let mut address = make_sync_address(
                "bc1qaddressstepactivity",
                SyncedAssetId::Bitcoin,
                crate::wallets::Network::Mainnet,
                Some(account_id),
                Some(crate::wallets::AddressScheme::NativeSegwit),
                Some(0),
                Some(0),
            );
            persist_sync_addresses_for_test(run, std::slice::from_ref(&address));
            let address_id = address.address_id;
            let mut chain_tip_cache = ChainTipCache::default();
            let mut completed_address_ids = HashSet::new();
            let mut known_activity = HashSet::new();
            let consecutive_unused = 7_u32;
            let mut accumulator = CycleAccumulator::new(1);
            let mut processed_for_account = 0_u32;
            let http_counters = SyncHttpCounters::new();
            let clients = SyncClients {
                mempool_client: None,
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            };
            let pending_address_ids = HashSet::new();
            let mut executor = FakeAddressSyncExecutor::new(vec![FakeSyncOutcome::Success {
                new_tx_count: 1,
                updated_tx_count: 0,
            }]);

            let outcome = run_hd_address_step(HdAddressStepRequest {
                control: SyncSingleAddressControlRequest {
                    run,
                    address: &mut address,
                    chain_tip_cache: &mut chain_tip_cache,
                    pending_address_ids: &pending_address_ids,
                    clients,
                    executor: &mut executor,
                    accumulator: &mut accumulator,
                    processed_for_account: &mut processed_for_account,
                    single_address_progress: None,
                    mempool_history_policy: MempoolHistoryPolicy::LegacyRepair,
                    mempool_history_page_frontier: None,
                },
                account_id,
                asset_id: SyncedAssetId::Bitcoin,
                completed_address_ids: &mut completed_address_ids,
                addresses_total: 1,
                known_activity: &mut known_activity,
                consecutive_unused: None,
            })
            .expect("active-rescan address step should succeed");

            assert_eq!(outcome, HdAddressStepOutcome { interrupted: false });
            assert_eq!(known_activity, HashSet::from([address_id]));
            assert_eq!(consecutive_unused, 7);
        });
    }

    #[test]
    fn hd_history_discovery_pending_observation_marks_only_its_branch_used() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let run = make_run_context(&clock);
            let account_id = DigitalAssetAccountId::new();
            let mut address = make_sync_address(
                "bc1qpendingactivity",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                Some(account_id),
                Some(AddressScheme::NativeSegwit),
                Some(1),
                Some(0),
            );
            persist_sync_addresses_for_test(run, std::slice::from_ref(&address));
            let address_id = address.address_id;
            let mut chain_tip_cache = ChainTipCache::default();
            let mut completed_address_ids = HashSet::new();
            let mut known_activity = HashSet::new();
            let mut consecutive_unused = 4_u32;
            let mut accumulator = CycleAccumulator::new(1);
            let mut processed_for_account = 0_u32;
            let http_counters = SyncHttpCounters::new();
            let clients = SyncClients {
                mempool_client: None,
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            };
            let pending_address_ids = HashSet::new();
            let mut executor =
                FakeAddressSyncExecutor::new(vec![FakeSyncOutcome::SuccessWithObservedActivity]);

            run_hd_address_step(HdAddressStepRequest {
                control: SyncSingleAddressControlRequest {
                    run,
                    address: &mut address,
                    chain_tip_cache: &mut chain_tip_cache,
                    pending_address_ids: &pending_address_ids,
                    clients,
                    executor: &mut executor,
                    accumulator: &mut accumulator,
                    processed_for_account: &mut processed_for_account,
                    single_address_progress: None,
                    mempool_history_policy: MempoolHistoryPolicy::CurrentOnly,
                    mempool_history_page_frontier: None,
                },
                account_id,
                asset_id: SyncedAssetId::Bitcoin,
                completed_address_ids: &mut completed_address_ids,
                addresses_total: 1,
                known_activity: &mut known_activity,
                consecutive_unused: Some(&mut consecutive_unused),
            })
            .expect("pending observation should scan successfully");

            assert_eq!(known_activity, HashSet::from([address_id]));
            assert_eq!(consecutive_unused, 0);
        });
    }

    #[test]
    fn hd_scan_rate_limit_address_step_propagates_abort() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let run = make_run_context(&clock);
            let account_id = DigitalAssetAccountId::new();
            let mut address = make_sync_address(
                "bc1qaddressstepratelimit",
                SyncedAssetId::Bitcoin,
                crate::wallets::Network::Mainnet,
                Some(account_id),
                Some(crate::wallets::AddressScheme::NativeSegwit),
                Some(0),
                Some(0),
            );
            persist_sync_addresses_for_test(run, std::slice::from_ref(&address));
            let mut chain_tip_cache = ChainTipCache::default();
            let mut completed_address_ids = HashSet::new();
            let mut known_activity = HashSet::new();
            let mut consecutive_unused = 2_u32;
            let mut accumulator = CycleAccumulator::new(1);
            let mut processed_for_account = 0_u32;
            let http_counters = SyncHttpCounters::new();
            let clients = SyncClients {
                mempool_client: None,
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            };
            let pending_address_ids = HashSet::new();
            let mut executor = FakeAddressSyncExecutor::new(vec![FakeSyncOutcome::RateLimited {
                integration: LABEL_MEMPOOL.to_string(),
            }]);

            let outcome = run_hd_address_step(HdAddressStepRequest {
                control: SyncSingleAddressControlRequest {
                    run,
                    address: &mut address,
                    chain_tip_cache: &mut chain_tip_cache,
                    pending_address_ids: &pending_address_ids,
                    clients,
                    executor: &mut executor,
                    accumulator: &mut accumulator,
                    processed_for_account: &mut processed_for_account,
                    single_address_progress: None,
                    mempool_history_policy: MempoolHistoryPolicy::LegacyRepair,
                    mempool_history_page_frontier: None,
                },
                account_id,
                asset_id: SyncedAssetId::Bitcoin,
                completed_address_ids: &mut completed_address_ids,
                addresses_total: 1,
                known_activity: &mut known_activity,
                consecutive_unused: Some(&mut consecutive_unused),
            })
            .expect("rate-limited address step should return an outcome");

            assert_eq!(outcome, HdAddressStepOutcome { interrupted: true });
            assert!(completed_address_ids.is_empty());
            assert!(known_activity.is_empty());
            assert_eq!(consecutive_unused, 2);
        });
    }

    #[test]
    fn run_hd_chain_scan_does_not_derive_when_rate_limited_before_scan() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let run = make_run_context(&clock);
            let account_id = crate::wallets::DigitalAssetAccountId::new();
            let mut addresses = (0..5_u32)
                .map(|index| {
                    make_sync_address(
                        &format!("bc1qrl{index}"),
                        SyncedAssetId::Bitcoin,
                        crate::wallets::Network::Mainnet,
                        Some(account_id),
                        Some(crate::wallets::AddressScheme::NativeSegwit),
                        Some(0),
                        Some(index),
                    )
                })
                .collect::<Vec<SyncAddress>>();
            persist_sync_addresses_for_test(run, &addresses);
            let mut known_activity = HashSet::new();
            let mut chain_tip_cache = ChainTipCache::default();
            let mut completed_address_ids = HashSet::new();
            let mut addresses_total =
                u32::try_from(addresses.len()).expect("address count should fit in u32");
            persist_sync_addresses_for_test(run, &addresses);
            let mut accumulator = CycleAccumulator::new(
                u32::try_from(addresses.len()).expect("address count should fit in u32"),
            );
            let mut processed_for_account = 0_u32;
            let http_counters = SyncHttpCounters::new();
            let clients = SyncClients {
                mempool_client: None,
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            };

            // Pre-set a rate limit on the mempool integration so Phase A aborts immediately
            record_rate_limit(run.user_id, LABEL_MEMPOOL, clock.instant_now(), None);

            let mut executor = FakeAddressSyncExecutor::new(Vec::new());
            let mut derivation_provider = FakeAddressDerivationProvider::new(vec![
                (0..2_u32)
                    .map(|i| make_derived_sync_address(&format!("bc1qrlnew{i}"), 0, 5 + i))
                    .collect(),
            ]);
            let pending_address_ids = HashSet::new();

            run_hd_chain_scan(HdChainScanRequest {
                run,
                clients,
                pending_address_ids: &pending_address_ids,
                frontier_state: None,
                account_id,
                asset_id: SyncedAssetId::Bitcoin,
                network: crate::wallets::Network::Mainnet,
                address_scheme: crate::wallets::AddressScheme::NativeSegwit,
                derivation_change: 0,
                gap_limit: 2,
                addresses: &mut addresses,
                known_activity: &mut known_activity,
                chain_tip_cache: &mut chain_tip_cache,
                completed_address_ids: &mut completed_address_ids,
                addresses_total: &mut addresses_total,
                accumulator: &mut accumulator,
                processed_for_account: &mut processed_for_account,
                sync_executor: &mut executor,
                derivation_provider: &mut derivation_provider,
                historical_backfill_enabled: true,
            })
            .expect("HD scan should succeed");

            // Phase A was interrupted by rate limit — Phase B must NOT derive new addresses
            assert!(
                derivation_provider.requests.is_empty(),
                "no derivation should occur when Phase A was aborted by rate limit"
            );
            assert_eq!(
                addresses.len(),
                5,
                "address count should not change when scan was rate-limited"
            );
            assert!(
                executor.calls.is_empty(),
                "executor should not be called when rate-limited"
            );
        });
    }

    #[test]
    fn run_hd_chain_scan_does_not_derive_when_rate_limited_mid_scan() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let run = make_run_context(&clock);
            let account_id = crate::wallets::DigitalAssetAccountId::new();
            let mut addresses = (0..5_u32)
                .map(|index| {
                    make_sync_address(
                        &format!("bc1qrm{index}"),
                        SyncedAssetId::Bitcoin,
                        crate::wallets::Network::Mainnet,
                        Some(account_id),
                        Some(crate::wallets::AddressScheme::NativeSegwit),
                        Some(0),
                        Some(index),
                    )
                })
                .collect::<Vec<SyncAddress>>();
            let mut known_activity = HashSet::new();
            let mut chain_tip_cache = ChainTipCache::default();
            let mut completed_address_ids = HashSet::new();
            let mut addresses_total =
                u32::try_from(addresses.len()).expect("address count should fit in u32");
            persist_sync_addresses_for_test(run, &addresses);
            let mut accumulator = CycleAccumulator::new(
                u32::try_from(addresses.len()).expect("address count should fit in u32"),
            );
            let mut processed_for_account = 0_u32;
            let http_counters = SyncHttpCounters::new();
            let clients = SyncClients {
                mempool_client: None,
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            };

            // Executor succeeds for first 2, then rate-limits on the 3rd
            let mut executor = FakeAddressSyncExecutor::new(vec![
                FakeSyncOutcome::Success {
                    new_tx_count: 0,
                    updated_tx_count: 0,
                },
                FakeSyncOutcome::Success {
                    new_tx_count: 0,
                    updated_tx_count: 0,
                },
                FakeSyncOutcome::RateLimited {
                    integration: LABEL_MEMPOOL.to_string(),
                },
            ]);
            let mut derivation_provider = FakeAddressDerivationProvider::new(vec![
                (0..3_u32)
                    .map(|i| make_derived_sync_address(&format!("bc1qrmnew{i}"), 0, 5 + i))
                    .collect(),
            ]);
            let pending_address_ids = HashSet::new();

            run_hd_chain_scan(HdChainScanRequest {
                run,
                clients,
                pending_address_ids: &pending_address_ids,
                frontier_state: None,
                account_id,
                asset_id: SyncedAssetId::Bitcoin,
                network: crate::wallets::Network::Mainnet,
                address_scheme: crate::wallets::AddressScheme::NativeSegwit,
                derivation_change: 0,
                gap_limit: 5,
                addresses: &mut addresses,
                known_activity: &mut known_activity,
                chain_tip_cache: &mut chain_tip_cache,
                completed_address_ids: &mut completed_address_ids,
                addresses_total: &mut addresses_total,
                accumulator: &mut accumulator,
                processed_for_account: &mut processed_for_account,
                sync_executor: &mut executor,
                derivation_provider: &mut derivation_provider,
                historical_backfill_enabled: true,
            })
            .expect("HD scan should succeed");

            // Phase A was interrupted mid-scan by rate limit — Phase B must NOT derive
            assert!(
                derivation_provider.requests.is_empty(),
                "no derivation should occur when Phase A was interrupted by rate limit"
            );
            assert_eq!(
                addresses.len(),
                5,
                "address count should not change when scan was interrupted by rate limit"
            );
            // Only 3 executor calls: 2 successes + 1 rate-limited
            assert_eq!(executor.calls.len(), 3);
        });
    }
}
