mod mapper;
mod paginator;

use super::{AddressSyncIntegration, IntegrationEstimateContext};
use crate::asset_capabilities::SyncProviderId;
use crate::db::raw_ingestion::{MempoolPageKind as RawMempoolPageKind, OpaqueJsonText, SyncRunId};
use crate::db::{
    BitcoinAccountCompletionPublication, BitcoinAddressProofPublication,
    MempoolAddressObservationSuccess, MempoolHistoryPageWorkUpdate, MempoolHistoryProof,
    StrictMempoolScanValidation, SyncAddress, address_has_pending_txs, begin_mempool_history_scan,
    commit_mempool_history_page_work, load_account_reported_tx_counts,
    load_canonical_confirmed_account_transaction_count, load_confirmed_tx_hashes_for_address,
    load_known_tx_hashes_for_address, persist_mempool_address_observation_success,
    publish_bitcoin_account_completion, publish_mempool_history_proof,
    publish_strict_mempool_history_proof, reconcile_address_transactions_preserving_invalidation,
    restart_strict_mempool_history_scan, update_address_mempool_backfill_cursor,
    update_address_mempool_expected_tx_count, validate_strict_mempool_history_scan,
};
use crate::integrations::mempool::{
    AddressStats, MempoolAddressTransaction, MempoolClient, MempoolError, MempoolTransactionPage,
};
use crate::tasks::jobs::raw_ingestion_executor::{
    IngestedMempoolPage, MempoolPageIngestionRequest, MempoolPageIngestionSummary,
    MempoolRequestFailureRecord, ingest_mempool_page, record_mempool_request_failure,
};
use crate::tasks::jobs::sync::error::preserve_iteration_error;
use crate::tasks::jobs::sync::progress::approximate_account_unsynced_count;
use crate::tasks::jobs::sync::{
    IntegrationIterationContext, IntegrationSyncPlan, RunContext, SyncIterationResult,
    UserTransactionMonitorError, is_first_sync,
};
use crate::tasks::publish_transaction_sync_event;
use crate::transactions::{
    AddressBackfillCursor, AddressBackfillState, ChainTipHeight, MempoolCursorTxid,
    TransactionCount, TransactionSyncEvent, TxCountEstimate,
};
use chrono::{DateTime, Utc};
use dioxus::logger::tracing;
use serde::Serialize;
use std::collections::HashSet;

pub(crate) use self::mapper::map_mempool_transactions;
pub(crate) use self::paginator::{confirmed_mempool_tx_count_in_page, last_confirmed_txid_in_page};

pub(crate) struct MempoolAddressSyncIntegration {
    iteration_state: Option<MempoolIterationState>,
}

struct MempoolIterationState {
    visit: MempoolAddressVisit,
    backfill_active: bool,
    proof_publication_allowed: bool,
    proof_published: bool,
    /// Whether the first page has been fetched for this sync run.
    first_page_done: bool,
    /// Cursor for the next paginated page.
    cursor: Option<String>,
    /// Known confirmed txids for early-exit detection (incremental only, loaded once).
    known_confirmed: HashSet<String>,
    /// Distinct confirmed txids observed before/during backfill for expected-count completion.
    observed_confirmed_txids: HashSet<String>,
    /// Exact provider-reported expected count for the active mempool backfill.
    backfill_expected_tx_count: Option<TransactionCount>,
    /// Whether pending transaction state makes confirmed-count completion unsafe.
    backfill_has_pending_transactions: bool,
    strict_scan_start_run_id: Option<SyncRunId>,
    /// Run summary accumulated across pages.
    run_summary: MempoolRunSummary,
}

#[derive(Debug, Clone, Copy)]
struct MempoolAddressVisit {
    stats: AddressStats,
    tip_height: ChainTipHeight,
    account_progress: Option<MempoolAccountProgressObservation>,
}

#[derive(Debug, Clone, Copy)]
struct MempoolAccountProgressObservation {
    known_transaction_count: TransactionCount,
    approximate_unsynced_count: TransactionCount,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MempoolHistoryProofTransition {
    Publish(MempoolHistoryProof),
    PreserveAndRestart,
    InvalidateAndRestart,
    Restart,
}

fn mempool_history_proof_transition(
    existing: Option<MempoolHistoryProof>,
    stats: &AddressStats,
    tip_height: ChainTipHeight,
) -> MempoolHistoryProofTransition {
    match existing {
        Some(proof) if stats.tx_count == proof.confirmed_tx_count => {
            MempoolHistoryProofTransition::Publish(MempoolHistoryProof {
                confirmed_tx_count: stats.tx_count,
                complete_height: tip_height,
            })
        }
        Some(proof) if stats.tx_count.value() > proof.confirmed_tx_count.value() => {
            MempoolHistoryProofTransition::PreserveAndRestart
        }
        Some(_) => MempoolHistoryProofTransition::InvalidateAndRestart,
        None if stats.tx_count.value() == 0 && stats.mempool_tx_count.value() == 0 => {
            MempoolHistoryProofTransition::Publish(MempoolHistoryProof {
                confirmed_tx_count: TransactionCount::zero(),
                complete_height: tip_height,
            })
        }
        None => MempoolHistoryProofTransition::Restart,
    }
}

fn zero_stats_skip_transaction_page(stats: &AddressStats) -> bool {
    stats.tx_count.value() == 0 && stats.mempool_tx_count.value() == 0
}

fn publish_address_history_proof(
    context: &IntegrationIterationContext<'_>,
    scan_start_run_id: Option<SyncRunId>,
    proof: MempoolHistoryProof,
) -> Result<(), UserTransactionMonitorError> {
    if let Some(account_id) = context.address.account_id {
        publish_bitcoin_account_completion(
            context.run.user_id,
            BitcoinAccountCompletionPublication {
                account_id,
                final_address_proof: Some(BitcoinAddressProofPublication {
                    address_id: context.address.address_id,
                    proof,
                    scan_start_run_id,
                }),
                completed_hd_discovery: None,
                observed_at: context.run.clock.utc_now(),
            },
        )?;
    } else if let Some(scan_start_run_id) = scan_start_run_id {
        publish_strict_mempool_history_proof(
            context.run.user_id,
            context.address.address_id,
            scan_start_run_id,
            proof,
        )?;
    } else {
        publish_mempool_history_proof(context.run.user_id, context.address.address_id, proof)?;
    }
    Ok(())
}

impl MempoolAddressSyncIntegration {
    pub(crate) const fn new() -> Self {
        Self {
            iteration_state: None,
        }
    }

    fn ensure_initialized(
        &mut self,
        context: &IntegrationIterationContext<'_>,
        tip_height: ChainTipHeight,
        mempool_client: &MempoolClient,
    ) -> Result<&mut MempoolIterationState, UserTransactionMonitorError> {
        if self.iteration_state.is_none() {
            let stats = fetch_mempool_address_stats(context, mempool_client)?;
            persist_mempool_observation(context, stats, tip_height)?;
            let account_progress = load_mempool_account_progress_observation(context)?;
            let proof_transition = mempool_history_proof_transition(
                context.address.mempool_history_proof,
                &stats,
                tip_height,
            );
            let restart_from_first_page = matches!(
                proof_transition,
                MempoolHistoryProofTransition::PreserveAndRestart
                    | MempoolHistoryProofTransition::InvalidateAndRestart
            );
            let backfill_active = context.is_backfill_active
                || !matches!(proof_transition, MempoolHistoryProofTransition::Publish(_));
            let proof_publication_allowed = !matches!(
                proof_transition,
                MempoolHistoryProofTransition::InvalidateAndRestart
            ) && !context.legacy_mempool_history_repair;
            let proof_published = match proof_transition {
                MempoolHistoryProofTransition::Publish(proof) => {
                    publish_address_history_proof(context, None, proof)?;
                    true
                }
                MempoolHistoryProofTransition::InvalidateAndRestart => {
                    crate::db::invalidate_mempool_history_proof(
                        context.run.user_id,
                        context.address.address_id,
                    )?;
                    false
                }
                MempoolHistoryProofTransition::PreserveAndRestart
                | MempoolHistoryProofTransition::Restart => false,
            };
            let mut persisted_resume_cursor = if restart_from_first_page {
                update_address_mempool_backfill_cursor(
                    context.run.user_id,
                    context.address.address_id,
                    None,
                )?;
                None
            } else {
                active_mempool_resume_cursor(context)
            };
            let strict_scan_start_run_id = if context.legacy_mempool_history_repair
                && !proof_published
                && !zero_stats_skip_transaction_page(&stats)
            {
                match context.address.mempool_history_scan_start_run_id {
                    Some(start_run_id) => Some(start_run_id),
                    None => {
                        begin_mempool_history_scan(
                            context.run.user_id,
                            context.address.address_id,
                            context.raw_sync_run_id,
                        )?;
                        persisted_resume_cursor = None;
                        Some(context.raw_sync_run_id)
                    }
                }
            } else {
                None
            };
            let backfill_expected_tx_count = backfill_active.then_some(stats.tx_count);
            let known_confirmed = if backfill_active {
                HashSet::new()
            } else {
                let known_tx_hashes = load_known_tx_hashes_for_address(
                    context.run.user_id,
                    context.address.address_id,
                    context.address.asset_id,
                    context.address.network,
                )?;
                known_tx_hashes
                    .iter()
                    .map(|hash| hash.as_str().to_string())
                    .collect()
            };
            let observed_confirmed_txids = if backfill_active {
                let confirmed_tx_hashes = load_confirmed_tx_hashes_for_address(
                    context.run.user_id,
                    context.address.address_id,
                    context.address.asset_id,
                    context.address.network,
                )?;
                confirmed_tx_hashes
                    .iter()
                    .map(|hash| hash.as_str().to_string())
                    .collect()
            } else {
                HashSet::new()
            };
            let backfill_has_pending_transactions = if backfill_active {
                address_has_pending_txs(
                    context.run.user_id,
                    context.address.address_id,
                    context.address.asset_id,
                    context.address.network,
                )?
            } else {
                false
            } || stats.mempool_tx_count.value() > 0;

            if backfill_active {
                update_address_mempool_expected_tx_count(
                    context.run.user_id,
                    context.address.address_id,
                    backfill_expected_tx_count,
                )?;
            }

            self.iteration_state = Some(MempoolIterationState {
                visit: MempoolAddressVisit {
                    stats,
                    tip_height,
                    account_progress,
                },
                backfill_active,
                proof_publication_allowed,
                proof_published,
                first_page_done: persisted_resume_cursor.is_some(),
                cursor: persisted_resume_cursor,
                known_confirmed,
                observed_confirmed_txids,
                backfill_expected_tx_count,
                backfill_has_pending_transactions,
                strict_scan_start_run_id,
                run_summary: MempoolRunSummary {
                    backfill_active,
                    ..MempoolRunSummary::default()
                },
            });
        }
        self.iteration_state.as_mut().ok_or_else(|| {
            UserTransactionMonitorError::Parse(
                "mempool iteration state not initialized".to_string(),
            )
        })
    }

    fn finalize_terminal_run(
        &mut self,
        user_id: crate::models::UserId,
        address_id: crate::wallets::DigitalAssetAddressId,
        early_exited: bool,
    ) -> Result<Option<OpaqueJsonText>, UserTransactionMonitorError> {
        let state = self.iteration_state.as_mut().ok_or_else(|| {
            UserTransactionMonitorError::Parse(
                "mempool iteration state not initialized".to_string(),
            )
        })?;

        let backfill_complete = state.backfill_active && state.cursor.is_none();
        if state.backfill_active {
            state.run_summary.backfill_complete = backfill_complete;
            state.run_summary.backfill_budget_exhausted =
                !backfill_complete && state.first_page_done;
        } else {
            state.run_summary.early_exit_known_confirmed = early_exited;
            state.run_summary.incremental_budget_exhausted =
                state.cursor.is_some() && !early_exited;
        }

        if backfill_complete && state.proof_published {
            update_address_mempool_backfill_cursor(user_id, address_id, None)?;
            update_address_mempool_expected_tx_count(user_id, address_id, None)?;
        }

        Ok(Some(state.run_summary.to_summary_json()?))
    }
}

fn active_mempool_resume_cursor(context: &IntegrationIterationContext<'_>) -> Option<String> {
    context
        .is_backfill_active
        .then(|| {
            context
                .address
                .mempool_backfill_cursor_txid
                .as_ref()
                .map(|cursor| cursor.as_str().to_string())
        })
        .flatten()
}

fn fetch_mempool_address_stats(
    context: &IntegrationIterationContext<'_>,
    mempool_client: &MempoolClient,
) -> Result<AddressStats, UserTransactionMonitorError> {
    crate::db::debug_assert_user_db_unlocked(
        context.run.user_id,
        "mempool address statistics fetch",
    );
    mempool_client
        .get_address_stats(context.address.address.as_str())
        .map_err(Into::into)
}

fn persist_mempool_observation(
    context: &IntegrationIterationContext<'_>,
    stats: AddressStats,
    tip_height: ChainTipHeight,
) -> Result<(), UserTransactionMonitorError> {
    persist_mempool_address_observation_success(
        context.run.user_id,
        MempoolAddressObservationSuccess {
            address_id: context.address.address_id,
            confirmed_tx_count: stats.tx_count,
            confirmed_balance: stats.confirmed_balance,
            tip_height,
            observed_at: context.run.clock.utc_now(),
        },
    )?;
    Ok(())
}

fn load_mempool_account_progress_observation(
    context: &IntegrationIterationContext<'_>,
) -> Result<Option<MempoolAccountProgressObservation>, UserTransactionMonitorError> {
    load_mempool_account_progress_for_publication(context.single_address_progress.is_some(), || {
        let Some(account_id) = context.address.account_id else {
            return Ok(None);
        };
        let reported_address_counts =
            load_account_reported_tx_counts(context.run.user_id, account_id)?;
        let known_transaction_count =
            load_canonical_confirmed_account_transaction_count(context.run.user_id, account_id)?;
        let approximate_unsynced_count =
            approximate_account_unsynced_count(reported_address_counts, known_transaction_count);
        Ok(Some(MempoolAccountProgressObservation {
            known_transaction_count,
            approximate_unsynced_count,
        }))
    })
}

fn load_mempool_account_progress_for_publication(
    should_publish: bool,
    load: impl FnOnce()
        -> Result<Option<MempoolAccountProgressObservation>, UserTransactionMonitorError>,
) -> Result<Option<MempoolAccountProgressObservation>, UserTransactionMonitorError> {
    if !should_publish {
        return Ok(None);
    }
    load()
}

impl AddressSyncIntegration for MempoolAddressSyncIntegration {
    fn sync_plan(
        &self,
        address: &SyncAddress,
        allow_known_confirmed_early_exit: bool,
    ) -> Result<IntegrationSyncPlan, UserTransactionMonitorError> {
        Ok(IntegrationSyncPlan {
            is_backfill_active: should_use_mempool_backfill_lane(
                address.last_tip_height,
                address.mempool_backfill_cursor_txid.as_ref(),
                allow_known_confirmed_early_exit,
            ),
        })
    }

    fn estimate_first_sync_tx_count(
        &self,
        _context: IntegrationEstimateContext<'_>,
    ) -> Result<Option<TxCountEstimate>, UserTransactionMonitorError> {
        Ok(None)
    }

    fn unfinished_backfill_state(&self, address: &SyncAddress) -> Option<AddressBackfillState> {
        address
            .mempool_backfill_cursor_txid
            .clone()
            .map(|cursor_txid| {
                AddressBackfillState::new(
                    AddressBackfillCursor::Mempool { cursor_txid },
                    address.mempool_expected_tx_count,
                )
            })
    }

    fn sync_one_iteration(
        &mut self,
        context: IntegrationIterationContext<'_>,
    ) -> Result<SyncIterationResult, UserTransactionMonitorError> {
        tracing::trace!(
            provider = ?SyncProviderId::MempoolSpace,
            address_id = %context.address.address_id,
            synced_at = %context.now_utc,
            sync_instant = ?context.now_instant,
            "sync integration: dispatching mempool address sync"
        );
        let tip_height = context.chain_tip.ok_or_else(|| {
            UserTransactionMonitorError::Parse(
                "mempool integration requires a chain tip height".to_string(),
            )
        })?;
        let mempool_client = context.clients.mempool_client.ok_or_else(|| {
            UserTransactionMonitorError::Parse(format!(
                "mempool client unavailable for {} sync",
                context.address.asset_id.as_str()
            ))
        })?;
        let state = self.ensure_initialized(&context, tip_height, mempool_client)?;
        let visit = state.visit;
        publish_mempool_progress(state, &context);
        if !context.historical_backfill_enabled {
            return Ok(SyncIterationResult {
                new_tx_count: TransactionCount::zero(),
                updated_tx_count: TransactionCount::zero(),
                coverage_invalidation: crate::db::CoverageInvalidationTargets::default(),
                tip_height,
                completed_at: context.run.clock.utc_now(),
                has_more_work: false,
                early_exited: false,
                observed_activity: !zero_stats_skip_transaction_page(&visit.stats),
                ledger_rebuild_required: false,
                raw_run_summary_json: Some(MempoolRunSummary::default().to_summary_json()?),
                api_confirmed_balance: visit.stats.confirmed_balance,
            });
        }
        if zero_stats_skip_transaction_page(&visit.stats) {
            let raw_run_summary_json =
                self.finalize_terminal_run(context.run.user_id, context.address.address_id, false)?;
            self.reset_iteration_state();
            return Ok(SyncIterationResult {
                new_tx_count: TransactionCount::zero(),
                updated_tx_count: TransactionCount::zero(),
                coverage_invalidation: crate::db::CoverageInvalidationTargets::default(),
                tip_height: visit.tip_height,
                completed_at: context.run.clock.utc_now(),
                has_more_work: false,
                early_exited: false,
                observed_activity: false,
                ledger_rebuild_required: false,
                raw_run_summary_json,
                api_confirmed_balance: visit.stats.confirmed_balance,
            });
        }

        let mut result = run_mempool_iteration(state, &context, tip_height, mempool_client)?;
        result.observed_activity = !zero_stats_skip_transaction_page(&visit.stats);
        result.api_confirmed_balance = visit.stats.confirmed_balance;
        result.raw_run_summary_json = self
            .finalize_terminal_run(
                context.run.user_id,
                context.address.address_id,
                result.early_exited,
            )
            .map_err(|error| preserve_iteration_error(error, &result))?;
        if !result.has_more_work {
            self.reset_iteration_state();
        }
        Ok(result)
    }

    fn current_run_summary_json(
        &self,
    ) -> Result<Option<OpaqueJsonText>, UserTransactionMonitorError> {
        self.iteration_state
            .as_ref()
            .map(|state| state.run_summary.to_summary_json())
            .transpose()
    }

    fn reset_iteration_state(&mut self) {
        self.iteration_state = None;
    }
}

/// Execute one mempool page iteration: fetch, persist, map, reconcile.
fn run_mempool_iteration(
    state: &mut MempoolIterationState,
    context: &IntegrationIterationContext<'_>,
    _tip_height: ChainTipHeight,
    mempool_client: &MempoolClient,
) -> Result<SyncIterationResult, UserTransactionMonitorError> {
    let run = context.run;
    let address = context.address;

    // Determine what page to fetch.
    let (page_kind, page_cursor_str) = if !state.first_page_done {
        (RawMempoolPageKind::FirstPage, None)
    } else {
        match state.cursor.as_deref() {
            Some(cursor) => (
                RawMempoolPageKind::PaginatedAfterConfirmed,
                Some(cursor.to_string()),
            ),
            None => {
                return Ok(SyncIterationResult {
                    new_tx_count: TransactionCount::zero(),
                    updated_tx_count: TransactionCount::zero(),
                    coverage_invalidation: crate::db::CoverageInvalidationTargets::default(),
                    tip_height: _tip_height,
                    completed_at: run.clock.utc_now(),
                    has_more_work: false,
                    early_exited: false,
                    observed_activity: false,
                    ledger_rebuild_required: false,
                    raw_run_summary_json: Some(state.run_summary.to_summary_json()?),
                    api_confirmed_balance: None,
                });
            }
        }
    };

    let persistence_context = MempoolPagePersistenceContext {
        run,
        raw_sync_run_id: context.raw_sync_run_id,
        source_connection_id: context.source_connection_id,
        address,
        scan_start_run_id: state.strict_scan_start_run_id,
    };

    let observed_at = run.clock.utc_now();
    let ingested = fetch_and_persist_mempool_transaction_page(
        persistence_context,
        mempool_client,
        page_kind,
        page_cursor_str.as_deref(),
        observed_at,
        &mut state.run_summary,
    )?;
    state.run_summary.record_page(&ingested.summary);
    let page = ingested.transactions;

    if let (true, Some(scan_start_run_id)) = (page.is_empty(), state.strict_scan_start_run_id) {
        let expected_count = state
            .backfill_expected_tx_count
            .unwrap_or(TransactionCount::zero());
        match validate_strict_mempool_history_scan(
            run.user_id,
            address.address_id,
            scan_start_run_id,
            expected_count,
        )? {
            StrictMempoolScanValidation::Exact => {
                publish_address_history_proof(
                    context,
                    Some(scan_start_run_id),
                    MempoolHistoryProof {
                        confirmed_tx_count: expected_count,
                        complete_height: _tip_height,
                    },
                )?;
                state.proof_published = true;
                state.cursor = None;
            }
            StrictMempoolScanValidation::Restart { reason } => {
                restart_strict_mempool_history_scan(run.user_id, address.address_id)?;
                return Err(UserTransactionMonitorError::Db(crate::db::DbError::new(
                    reason,
                )));
            }
        }
        return Ok(SyncIterationResult {
            new_tx_count: TransactionCount::zero(),
            updated_tx_count: TransactionCount::zero(),
            coverage_invalidation: crate::db::CoverageInvalidationTargets::default(),
            tip_height: _tip_height,
            completed_at: run.clock.utc_now(),
            has_more_work: false,
            early_exited: false,
            observed_activity: false,
            ledger_rebuild_required: false,
            raw_run_summary_json: Some(state.run_summary.to_summary_json()?),
            api_confirmed_balance: None,
        });
    }

    if page.is_empty() && state.first_page_done {
        state.cursor = None;
        return Ok(SyncIterationResult {
            new_tx_count: TransactionCount::zero(),
            updated_tx_count: TransactionCount::zero(),
            coverage_invalidation: crate::db::CoverageInvalidationTargets::default(),
            tip_height: _tip_height,
            completed_at: run.clock.utc_now(),
            has_more_work: false,
            early_exited: false,
            observed_activity: false,
            ledger_rebuild_required: false,
            raw_run_summary_json: Some(state.run_summary.to_summary_json()?),
            api_confirmed_balance: None,
        });
    }

    // Publish progress.
    let confirmed_in_page = confirmed_mempool_tx_count_in_page(&page);
    if confirmed_in_page > 0 {
        publish_mempool_progress(state, context);
    }

    // Map and reconcile.
    let mapped = map_mempool_transactions(&page)?;
    let ledger_rebuild_required = !mapped.is_empty();
    let reconcile_summary = reconcile_mempool_transactions(
        run.user_id,
        address.asset_id,
        address.network,
        &mapped,
        observed_at,
    )?;
    let coverage_invalidation = reconcile_summary.coverage_invalidation.clone();

    let result = (|| {
        observe_backfill_expected_count_page(state, &page);

        // Update cursor for the next iteration.
        if !state.first_page_done {
            state.cursor = last_confirmed_txid_in_page(&page);
            state.first_page_done = true;
        } else {
            let next_cursor = last_confirmed_txid_in_page(&page);
            let page_cursor = page_cursor_str.as_deref().ok_or_else(|| {
                UserTransactionMonitorError::Parse(
                    "paginated mempool page missing fetched cursor".to_string(),
                )
            })?;
            update_cursor_after_paginated_page(
                state,
                context,
                page_cursor,
                next_cursor,
                page.len(),
                confirmed_in_page,
            );
        }
        let completed_backfill = if state.backfill_active && state.proof_publication_allowed {
            complete_backfill_if_expected_count_reached(state)
        } else {
            None
        };

        // Persist address and HD breadth progress atomically so interruption cannot skip work.
        if state.backfill_active || context.mempool_history_page_frontier.is_some() {
            let next_cursor_typed = if state.backfill_active {
                state
                    .cursor
                    .as_ref()
                    .map(|c| MempoolCursorTxid::parse(c))
                    .transpose()
                    .map_err(|err| {
                        UserTransactionMonitorError::Parse(format!(
                            "invalid mempool resume cursor: {err}"
                        ))
                    })?
            } else {
                None
            };
            commit_mempool_history_page_work(
                run.user_id,
                MempoolHistoryPageWorkUpdate {
                    address_id: address.address_id,
                    next_cursor: next_cursor_typed,
                    hd_frontier: context.mempool_history_page_frontier,
                },
            )?;
        }

        if state.strict_scan_start_run_id.is_some()
            && (state.run_summary.duplicate_cursor_page_detected || state.cursor.is_none())
        {
            restart_strict_mempool_history_scan(run.user_id, address.address_id)?;
            return Err(UserTransactionMonitorError::Db(crate::db::DbError::new(
                "Strict Mempool history scan ended without an empty terminal page",
            )));
        }

        if let Some(completion) = completed_backfill {
            publish_address_history_proof(
                context,
                None,
                MempoolHistoryProof {
                    confirmed_tx_count: completion,
                    complete_height: _tip_height,
                },
            )?;
            state.proof_published = true;
            tracing::debug!(
                user_id = %context.run.user_id,
                run_id = %context.run.run_id,
                address_id = %context.address.address_id,
                confirmed_tx_count = completion.value(),
                "transactions sync: mempool expected transaction count reached, treating backfill as complete"
            );
        }

        // Early-exit check (incremental only).
        let early_exited = if !state.backfill_active {
            should_early_exit_for_known_confirmed_page(
                context.allow_known_confirmed_early_exit,
                &page,
                &state.known_confirmed,
            )
        } else {
            false
        };

        let has_more_work = state.cursor.is_some() && !early_exited;

        Ok(SyncIterationResult {
            new_tx_count: reconcile_summary.new_tx_count,
            updated_tx_count: reconcile_summary.updated_tx_count,
            coverage_invalidation: reconcile_summary.coverage_invalidation,
            tip_height: _tip_height,
            completed_at: run.clock.utc_now(),
            has_more_work,
            early_exited,
            observed_activity: false,
            ledger_rebuild_required,
            raw_run_summary_json: Some(state.run_summary.to_summary_json()?),
            api_confirmed_balance: None,
        })
    })();
    result.map_err(|error: UserTransactionMonitorError| {
        error.with_coverage_invalidation(coverage_invalidation)
    })
}

fn reconcile_mempool_transactions(
    user_id: crate::models::UserId,
    asset_id: crate::wallets::SyncedAssetId,
    network: crate::wallets::Network,
    records: &[crate::db::SyncTransactionRecord],
    observed_at: DateTime<Utc>,
) -> Result<crate::db::TransactionSyncReconcileSummary, UserTransactionMonitorError> {
    let summary =
        match reconcile_address_transactions_preserving_invalidation(
            user_id,
            asset_id,
            network,
            records,
            observed_at,
        ) {
            Ok(summary) => summary,
            Err(failure) => {
                let targets = failure.summary.coverage_invalidation;
                return match crate::db::invalidate_mempool_history_coverage(user_id, &targets) {
                    Ok(()) => Err(UserTransactionMonitorError::from(*failure.error)
                        .with_coverage_invalidation(targets)),
                    Err(error) => Err(UserTransactionMonitorError::from(error)
                        .with_coverage_invalidation(targets)),
                };
            }
        };
    if let Err(error) =
        crate::db::invalidate_mempool_history_coverage(user_id, &summary.coverage_invalidation)
    {
        return Err(UserTransactionMonitorError::from(error)
            .with_coverage_invalidation(summary.coverage_invalidation));
    }
    Ok(summary)
}

fn publish_mempool_progress(
    state: &MempoolIterationState,
    context: &IntegrationIterationContext<'_>,
) {
    let Some(progress) = context.single_address_progress.as_ref() else {
        return;
    };
    let Some(observation) = state.visit.account_progress else {
        return;
    };
    crate::db::debug_assert_user_db_unlocked(context.run.user_id, "mempool progress publish");
    let now_utc = context.run.clock.utc_now();
    let fetched_count = observation.known_transaction_count;
    let expected_count = fetched_count.saturating_add(observation.approximate_unsynced_count);
    publish_transaction_sync_event(
        context.run.user_id,
        TransactionSyncEvent::account_sync_progress_single_address(
            context.run.run_id,
            now_utc,
            progress.account_id,
            progress.is_first_sync,
            fetched_count,
            Some(expected_count),
            Some(false),
        ),
    );
    publish_transaction_sync_event(
        context.run.user_id,
        TransactionSyncEvent::account_integration_sync_progress_single_address(
            context.run.run_id,
            now_utc,
            progress.account_id,
            crate::transactions::SyncIntegrationId::Mempool,
            progress.is_first_sync,
            fetched_count,
            Some(expected_count),
            Some(false),
        ),
    );
}

fn update_cursor_after_paginated_page(
    state: &mut MempoolIterationState,
    context: &IntegrationIterationContext<'_>,
    fetched_cursor: &str,
    next_cursor: Option<String>,
    items_seen: usize,
    confirmed_items_seen: usize,
) {
    if next_cursor.as_deref() == Some(fetched_cursor) {
        state.cursor = Some(fetched_cursor.to_string());
        state.proof_publication_allowed = false;
        state.run_summary.duplicate_cursor_page_detected = true;
        tracing::warn!(
            user_id = %context.run.user_id,
            run_id = %context.run.run_id,
            address_id = %context.address.address_id,
            cursor_txid = %fetched_cursor,
            items_seen = items_seen,
            confirmed_items_seen = confirmed_items_seen,
            "transactions sync: duplicate mempool cursor page detected, preserving retry cursor"
        );
        return;
    }

    state.cursor = next_cursor;
}

fn observe_backfill_expected_count_page(
    state: &mut MempoolIterationState,
    page: &[MempoolAddressTransaction],
) {
    if state.backfill_expected_tx_count.is_none() {
        return;
    }

    for tx in page {
        if tx.status.confirmed {
            state.observed_confirmed_txids.insert(tx.txid.clone());
        } else {
            state.backfill_has_pending_transactions = true;
        }
    }
}

fn complete_backfill_if_expected_count_reached(
    state: &mut MempoolIterationState,
) -> Option<TransactionCount> {
    let expected_tx_count = state.backfill_expected_tx_count?;
    if state.backfill_has_pending_transactions {
        return None;
    }

    let observed_confirmed_tx_count =
        u32::try_from(state.observed_confirmed_txids.len()).unwrap_or(u32::MAX);
    if observed_confirmed_tx_count != expected_tx_count.value() {
        return None;
    }

    state.cursor = None;
    Some(expected_tx_count)
}

#[derive(Clone, Copy)]
struct MempoolPagePersistenceContext<'a> {
    run: RunContext<'a>,
    raw_sync_run_id: SyncRunId,
    source_connection_id: &'a crate::db::raw_ingestion::SourceConnectionId,
    address: &'a SyncAddress,
    scan_start_run_id: Option<SyncRunId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
struct MempoolRunSummary {
    pages_fetched: u32,
    items_seen: u32,
    versions_inserted: u32,
    versions_reused: u32,
    parse_success_count: u32,
    parse_failure_count: u32,
    http_failure_count: u32,
    transport_failure_count: u32,
    duplicate_cursor_page_detected: bool,
    backfill_active: bool,
    backfill_complete: bool,
    backfill_budget_exhausted: bool,
    incremental_budget_exhausted: bool,
    early_exit_known_confirmed: bool,
}

impl MempoolRunSummary {
    fn record_page(&mut self, page: &MempoolPageIngestionSummary) {
        self.pages_fetched = self.pages_fetched.saturating_add(1);
        self.items_seen = self.items_seen.saturating_add(page.items_seen);
        self.versions_inserted = self
            .versions_inserted
            .saturating_add(page.versions_inserted);
        self.versions_reused = self.versions_reused.saturating_add(page.versions_reused);
        self.parse_success_count = self
            .parse_success_count
            .saturating_add(page.parse_success_count);
        self.parse_failure_count = self
            .parse_failure_count
            .saturating_add(page.parse_failure_count);
    }

    fn to_summary_json(&self) -> Result<OpaqueJsonText, UserTransactionMonitorError> {
        let json = serde_json::to_string(self).map_err(|err| {
            UserTransactionMonitorError::Parse(format!(
                "failed to serialize mempool sync run summary: {err}"
            ))
        })?;
        OpaqueJsonText::parse(json).map_err(Into::into)
    }

    fn record_request_failure(&mut self, error: &MempoolError) {
        match error {
            MempoolError::Http { .. } | MempoolError::UrlJoin(_) => {
                self.transport_failure_count = self.transport_failure_count.saturating_add(1);
            }
            MempoolError::UpstreamStatus { .. }
            | MempoolError::RateLimited { .. }
            | MempoolError::Deserialize { .. } => {
                self.http_failure_count = self.http_failure_count.saturating_add(1);
            }
        }
    }
}

fn should_use_mempool_backfill_lane(
    last_tip_height: Option<ChainTipHeight>,
    mempool_backfill_cursor_txid: Option<&MempoolCursorTxid>,
    allow_known_confirmed_early_exit: bool,
) -> bool {
    is_first_sync(last_tip_height)
        || mempool_backfill_cursor_txid.is_some()
        || !allow_known_confirmed_early_exit
}

fn should_early_exit_for_known_confirmed_page(
    allow_known_confirmed_early_exit: bool,
    page: &[MempoolAddressTransaction],
    known_confirmed_txids: &HashSet<String>,
) -> bool {
    if !allow_known_confirmed_early_exit || known_confirmed_txids.is_empty() {
        return false;
    }

    let mut saw_confirmed = false;
    for tx in page {
        if !tx.status.confirmed {
            continue;
        }
        saw_confirmed = true;
        if !known_confirmed_txids.contains(&tx.txid) {
            return false;
        }
    }

    saw_confirmed
}

fn fetch_and_persist_mempool_transaction_page(
    context: MempoolPagePersistenceContext<'_>,
    mempool_client: &MempoolClient,
    page_kind: RawMempoolPageKind,
    page_cursor: Option<&str>,
    observed_at: DateTime<Utc>,
    run_summary: &mut MempoolRunSummary,
) -> Result<IngestedMempoolPage, UserTransactionMonitorError> {
    crate::db::debug_assert_user_db_unlocked(context.run.user_id, "mempool transaction page fetch");
    let fetch_result = match (page_kind, page_cursor) {
        (RawMempoolPageKind::FirstPage, None) => {
            mempool_client.fetch_first_page_raw(context.address.address.as_str())
        }
        (RawMempoolPageKind::PaginatedAfterConfirmed, Some(page_cursor)) => mempool_client
            .fetch_page_after_confirmed_raw(context.address.address.as_str(), page_cursor),
        (RawMempoolPageKind::FirstPage, Some(_)) => {
            return Err(UserTransactionMonitorError::Parse(
                "first mempool page cannot have a page cursor".to_string(),
            ));
        }
        (RawMempoolPageKind::PaginatedAfterConfirmed, None) => {
            return Err(UserTransactionMonitorError::Parse(
                "paginated mempool page requires a page cursor".to_string(),
            ));
        }
    };

    match fetch_result {
        Ok(page) => {
            persist_mempool_transaction_page(context, page, page_kind, page_cursor, observed_at)
        }
        Err(error) => {
            run_summary.record_request_failure(&error);
            record_mempool_request_failure(
                MempoolRequestFailureRecord {
                    user_id: context.run.user_id,
                    raw_sync_run_id: context.raw_sync_run_id,
                    scope_address_id: context.address.address_id,
                    page_kind,
                    page_cursor,
                    attempted_at: observed_at,
                },
                &error,
            )?;
            Err(UserTransactionMonitorError::from(error))
        }
    }
}

fn persist_mempool_transaction_page(
    context: MempoolPagePersistenceContext<'_>,
    page: MempoolTransactionPage,
    page_kind: RawMempoolPageKind,
    page_cursor: Option<&str>,
    observed_at: DateTime<Utc>,
) -> Result<IngestedMempoolPage, UserTransactionMonitorError> {
    ingest_mempool_page(
        MempoolPageIngestionRequest {
            user_id: context.run.user_id,
            raw_sync_run_id: context.raw_sync_run_id,
            source_connection_id: context.source_connection_id,
            scope_address_id: context.address.address_id,
            page_kind,
            page_cursor,
            scan_start_run_id: context.scan_start_run_id,
            network: context.address.network,
            observed_at,
        },
        page,
    )
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod pure_tests {
    use super::*;
    use crate::db::SyncAddress;
    use crate::integrations::mempool::MempoolTransactionStatus;
    use crate::transactions::TrackedAddress;
    use crate::wallets::{DigitalAssetAddressId, Network, SyncedAssetId};
    use std::collections::HashSet;

    fn test_sync_address() -> SyncAddress {
        SyncAddress {
            address_id: DigitalAssetAddressId::new(),
            address: TrackedAddress::parse(
                "bc1qtestaddress000000000000000000000000000000000000000000000",
            )
            .expect("test address should parse"),
            asset_id: SyncedAssetId::Bitcoin,
            network: Network::Mainnet,
            account_id: None,
            derivation_change: None,
            derivation_index: None,
            address_scheme: None,
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

    fn make_transaction(txid: &str, confirmed: bool) -> MempoolAddressTransaction {
        MempoolAddressTransaction {
            txid: txid.to_string(),
            vin: Vec::new(),
            vout: Vec::new(),
            fee: None,
            status: MempoolTransactionStatus {
                confirmed,
                block_height: confirmed.then_some(1),
                block_hash: confirmed.then_some("block".to_string()),
                block_time: confirmed.then_some(1),
            },
        }
    }

    fn address_stats(confirmed: u32, pending: u32) -> crate::integrations::mempool::AddressStats {
        crate::integrations::mempool::AddressStats {
            tx_count: TransactionCount::from_u32(confirmed),
            mempool_tx_count: TransactionCount::from_u32(pending),
            confirmed_balance: None,
        }
    }

    fn test_visit() -> MempoolAddressVisit {
        MempoolAddressVisit {
            stats: address_stats(0, 0),
            tip_height: ChainTipHeight::try_new(1).expect("tip should parse"),
            account_progress: None,
        }
    }

    #[test]
    fn account_progress_load_runs_only_for_single_address_publication() {
        let load_count = std::cell::Cell::new(0_u32);
        let observation = MempoolAccountProgressObservation {
            known_transaction_count: TransactionCount::from_u32(3),
            approximate_unsynced_count: TransactionCount::from_u32(2),
        };

        let skipped = load_mempool_account_progress_for_publication(false, || {
            load_count.set(load_count.get() + 1);
            Ok(Some(observation))
        })
        .expect("HD progress should skip the load");

        assert!(skipped.is_none());
        assert_eq!(load_count.get(), 0);

        let loaded = load_mempool_account_progress_for_publication(true, || {
            load_count.set(load_count.get() + 1);
            Ok(Some(observation))
        })
        .expect("single-address progress should load")
        .expect("single-address progress should be published");

        assert_eq!(load_count.get(), 1);
        assert_eq!(
            loaded.known_transaction_count,
            observation.known_transaction_count
        );
        assert_eq!(
            loaded.approximate_unsynced_count,
            observation.approximate_unsynced_count
        );
    }

    #[test]
    fn sync_plan_marks_backfill_when_resume_cursor_exists() {
        let integration = MempoolAddressSyncIntegration::new();
        let mut address = test_sync_address();
        address.mempool_backfill_cursor_txid = Some(
            MempoolCursorTxid::parse(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .expect("cursor should parse"),
        );

        let plan = integration
            .sync_plan(&address, true)
            .expect("sync plan should compute");

        assert!(plan.is_backfill_active);
    }

    #[test]
    fn expected_count_completion_clears_first_run_cursor_when_distinct_confirmed_reach_expected() {
        let mut state = MempoolIterationState {
            visit: test_visit(),
            backfill_active: true,
            proof_publication_allowed: true,
            proof_published: false,
            first_page_done: true,
            cursor: Some(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
            ),
            known_confirmed: HashSet::new(),
            observed_confirmed_txids: HashSet::new(),
            backfill_expected_tx_count: Some(TransactionCount::from_u32(2)),
            backfill_has_pending_transactions: false,
            strict_scan_start_run_id: None,
            run_summary: MempoolRunSummary {
                backfill_active: true,
                ..MempoolRunSummary::default()
            },
        };
        let page = vec![
            make_transaction(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                true,
            ),
            make_transaction(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                true,
            ),
        ];

        observe_backfill_expected_count_page(&mut state, &page);
        let completion = complete_backfill_if_expected_count_reached(&mut state)
            .expect("expected count should complete backfill");

        assert_eq!(completion, TransactionCount::from_u32(2));
        assert_eq!(state.cursor, None);
    }

    #[test]
    fn expected_count_completion_includes_confirmed_history_for_resume() {
        let mut state = MempoolIterationState {
            visit: test_visit(),
            backfill_active: true,
            proof_publication_allowed: true,
            proof_published: false,
            first_page_done: true,
            cursor: Some(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
            ),
            known_confirmed: HashSet::new(),
            observed_confirmed_txids: HashSet::from([String::from(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )]),
            backfill_expected_tx_count: Some(TransactionCount::from_u32(2)),
            backfill_has_pending_transactions: false,
            strict_scan_start_run_id: None,
            run_summary: MempoolRunSummary {
                backfill_active: true,
                ..MempoolRunSummary::default()
            },
        };
        let page = vec![make_transaction(
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            true,
        )];

        observe_backfill_expected_count_page(&mut state, &page);
        let completion = complete_backfill_if_expected_count_reached(&mut state)
            .expect("expected count should complete resumed backfill");

        assert_eq!(completion, TransactionCount::from_u32(2));
        assert_eq!(state.cursor, None);
    }

    #[test]
    fn expected_count_completion_waits_when_pending_state_exists() {
        let mut state = MempoolIterationState {
            visit: test_visit(),
            backfill_active: true,
            proof_publication_allowed: true,
            proof_published: false,
            first_page_done: true,
            cursor: Some(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
            ),
            known_confirmed: HashSet::new(),
            observed_confirmed_txids: HashSet::new(),
            backfill_expected_tx_count: Some(TransactionCount::from_u32(1)),
            backfill_has_pending_transactions: false,
            strict_scan_start_run_id: None,
            run_summary: MempoolRunSummary {
                backfill_active: true,
                ..MempoolRunSummary::default()
            },
        };
        let page = vec![
            make_transaction(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                true,
            ),
            make_transaction(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                false,
            ),
        ];

        observe_backfill_expected_count_page(&mut state, &page);
        let completion = complete_backfill_if_expected_count_reached(&mut state);

        assert_eq!(completion, None);
        assert_eq!(
            state.cursor,
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string())
        );
        assert!(state.backfill_has_pending_transactions);
    }

    #[test]
    fn mempool_history_proof_completion_requires_exact_distinct_count() {
        let mut state = MempoolIterationState {
            visit: test_visit(),
            backfill_active: true,
            proof_publication_allowed: true,
            proof_published: false,
            first_page_done: true,
            cursor: Some(
                "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".to_string(),
            ),
            known_confirmed: HashSet::new(),
            observed_confirmed_txids: HashSet::from([
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
            ]),
            backfill_expected_tx_count: Some(TransactionCount::from_u32(1)),
            backfill_has_pending_transactions: false,
            strict_scan_start_run_id: None,
            run_summary: MempoolRunSummary {
                backfill_active: true,
                ..MempoolRunSummary::default()
            },
        };

        assert_eq!(
            complete_backfill_if_expected_count_reached(&mut state),
            None
        );
        assert!(state.cursor.is_some());
    }

    #[test]
    fn mempool_history_proof_transitions_follow_one_fresh_observation() {
        let old_tip = ChainTipHeight::try_new(800_000).expect("old tip should parse");
        let new_tip = ChainTipHeight::try_new(800_001).expect("new tip should parse");
        let proof = crate::db::MempoolHistoryProof {
            confirmed_tx_count: TransactionCount::from_u32(2),
            complete_height: old_tip,
        };

        assert_eq!(
            mempool_history_proof_transition(Some(proof), &address_stats(2, 0), new_tip),
            MempoolHistoryProofTransition::Publish(crate::db::MempoolHistoryProof {
                confirmed_tx_count: TransactionCount::from_u32(2),
                complete_height: new_tip,
            })
        );
        assert_eq!(
            mempool_history_proof_transition(Some(proof), &address_stats(3, 0), new_tip),
            MempoolHistoryProofTransition::PreserveAndRestart
        );
        assert_eq!(
            mempool_history_proof_transition(Some(proof), &address_stats(1, 0), new_tip),
            MempoolHistoryProofTransition::InvalidateAndRestart
        );
        assert_eq!(
            mempool_history_proof_transition(None, &address_stats(0, 0), new_tip),
            MempoolHistoryProofTransition::Publish(crate::db::MempoolHistoryProof {
                confirmed_tx_count: TransactionCount::zero(),
                complete_height: new_tip,
            })
        );
        assert_eq!(
            mempool_history_proof_transition(None, &address_stats(0, 1), new_tip),
            MempoolHistoryProofTransition::Restart
        );
    }

    #[test]
    fn mempool_history_proof_zero_zero_skips_transaction_page() {
        assert!(zero_stats_skip_transaction_page(&address_stats(0, 0)));
        assert!(!zero_stats_skip_transaction_page(&address_stats(0, 1)));
        assert!(!zero_stats_skip_transaction_page(&address_stats(1, 0)));
    }
}

#[cfg(all(test, feature = "db-tests"))]
pub(crate) mod tests {
    use super::*;
    use crate::db::SyncAddress;
    use crate::db::raw_ingestion::{
        IntegrationKind, StartSyncRunRequest, SyncRunScopeKind, SyncRunTriggerKind, start_sync_run,
    };
    use crate::db::{
        acquire_test_runtime, get_non_hd_sync_addresses, mark_address_sync_started,
        persist_sync_address_fixture, setup_test_user, unique_user_id,
    };
    use crate::tasks::TriggerSource;
    use crate::tasks::jobs::sync::{
        LABEL_MEMPOOL, RunContext, SyncClients, SyncClock, SyncHttpCounters,
    };
    use crate::traces::client::{IntegrationLabel, TracedBlockingClient};
    use crate::transactions::{TrackedAddress, TransactionSyncRunId};
    use crate::wallets::{DigitalAssetAddressId, Network, SyncedAssetId};
    use chrono::{TimeZone, Utc};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::{Duration, Instant};
    use url::Url;

    struct FixedClock {
        now_utc: chrono::DateTime<Utc>,
        now_instant: Instant,
    }

    impl FixedClock {
        fn new(now_utc: chrono::DateTime<Utc>) -> Self {
            Self {
                now_utc,
                now_instant: Instant::now(),
            }
        }
    }

    impl SyncClock for FixedClock {
        fn utc_now(&self) -> chrono::DateTime<Utc> {
            self.now_utc
        }

        fn instant_now(&self) -> Instant {
            self.now_instant
        }

        fn sleep(&self, _duration: Duration) {}
    }

    fn test_sync_address() -> SyncAddress {
        SyncAddress {
            address_id: DigitalAssetAddressId::new(),
            address: TrackedAddress::parse(
                "bc1qtestaddress000000000000000000000000000000000000000000000",
            )
            .expect("test address should parse"),
            asset_id: SyncedAssetId::Bitcoin,
            network: Network::Mainnet,
            account_id: None,
            derivation_change: None,
            derivation_index: None,
            address_scheme: None,
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

    fn test_now() -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000, 0)
            .single()
            .expect("valid timestamp")
    }

    fn test_visit() -> MempoolAddressVisit {
        MempoolAddressVisit {
            stats: AddressStats {
                tx_count: TransactionCount::zero(),
                mempool_tx_count: TransactionCount::zero(),
                confirmed_balance: None,
            },
            tip_height: ChainTipHeight::try_new(1).expect("tip should parse"),
            account_progress: None,
        }
    }

    fn make_run_context<'a>(
        clock: &'a FixedClock,
        user_id: crate::models::UserId,
    ) -> RunContext<'a> {
        RunContext {
            user_id,
            run_id: TransactionSyncRunId::new(),
            source: TriggerSource::ManualInternal,
            started_at: clock.utc_now(),
            clock,
        }
    }

    #[test]
    fn account_progress_excludes_pending_canonical_rows_from_confirmed_known_count() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let account_id = crate::wallets::DigitalAssetAccountId::new();
        let mut address = test_sync_address();
        address.account_id = Some(account_id);
        let now = test_now();
        persist_sync_address_fixture(user_id, &address, now)
            .expect("sync address fixture should persist");
        mark_address_sync_started(
            user_id,
            address.address_id,
            TransactionSyncRunId::new(),
            now,
        )
        .expect("sync state row should exist");

        let records = (0..10_u32)
            .map(|index| {
                let status = if index < 8 {
                    crate::transactions::ChainTransactionStatus::Confirmed
                } else {
                    crate::transactions::ChainTransactionStatus::Pending
                };
                crate::db::SyncTransactionRecord {
                    tx_hash: crate::transactions::TxHash::parse(&format!("{:064x}", index + 1))
                        .expect("tx hash should parse"),
                    status,
                    block_height: (status
                        == crate::transactions::ChainTransactionStatus::Confirmed)
                        .then_some(i64::from(index) + 1),
                    block_hash: (status == crate::transactions::ChainTransactionStatus::Confirmed)
                        .then(|| format!("block-{index}")),
                    block_time: (status == crate::transactions::ChainTransactionStatus::Confirmed)
                        .then_some(now),
                    fee_amount: Some(0),
                    inputs: Vec::new(),
                    outputs: vec![crate::db::SyncTransactionOutputRecord {
                        output_index: 0,
                        raw_address: Some(address.address.clone()),
                        script_pubkey_hex: "00".to_string(),
                        value_amount: 1,
                    }],
                }
            })
            .collect::<Vec<_>>();
        crate::db::reconcile_address_transactions(
            user_id,
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            &records,
            now,
        )
        .expect("canonical transaction fixtures should persist");
        persist_mempool_address_observation_success(
            user_id,
            MempoolAddressObservationSuccess {
                address_id: address.address_id,
                confirmed_tx_count: TransactionCount::from_u32(10),
                confirmed_balance: None,
                tip_height: ChainTipHeight::try_new(800_000).expect("height should parse"),
                observed_at: now,
            },
        )
        .expect("provider observation should persist");

        let clock = FixedClock::new(now);
        let http_counters = SyncHttpCounters::new();
        let source_connection_id = crate::db::raw_ingestion::SourceConnectionId::new();
        let progress = load_mempool_account_progress_observation(&IntegrationIterationContext {
            run: make_run_context(&clock, user_id),
            now_utc: now,
            now_instant: clock.instant_now(),
            address: &address,
            clients: SyncClients {
                mempool_client: None,
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            },
            single_address_progress: Some(crate::tasks::jobs::sync::SingleAddressProgressPlan {
                account_id,
                is_first_sync: true,
                expected_tx_count: None,
                expected_tx_count_is_lower_bound: None,
            }),
            allow_known_confirmed_early_exit: false,
            chain_tip: None,
            raw_sync_run_id: SyncRunId::new(),
            source_connection_id: &source_connection_id,
            is_backfill_active: false,
            historical_backfill_enabled: true,
            legacy_mempool_history_repair: false,
            mempool_history_page_frontier: None,
        })
        .expect("account progress should load")
        .expect("account progress should exist");

        assert_eq!(
            progress.known_transaction_count,
            TransactionCount::from_u32(8)
        );
        assert_eq!(
            progress.approximate_unsynced_count,
            TransactionCount::from_u32(2)
        );
        assert_eq!(
            crate::db::load_canonical_account_transaction_count_bounded(
                user_id,
                account_id,
                TransactionCount::from_u32(20),
            )
            .expect("cap count should load"),
            TransactionCount::from_u32(10)
        );
    }

    pub(crate) struct HistoricalSyncMempoolServer {
        pub(crate) base_url: String,
        handle: thread::JoinHandle<Vec<String>>,
    }

    impl HistoricalSyncMempoolServer {
        pub(crate) fn join(self) -> Vec<String> {
            self.handle
                .join()
                .expect("test mempool server thread should join")
        }
    }

    pub(crate) fn start_historical_sync_mempool_server(
        response_bodies: Vec<String>,
    ) -> HistoricalSyncMempoolServer {
        start_historical_sync_mempool_server_with_statuses(
            response_bodies
                .into_iter()
                .map(|body| (200_u16, body))
                .collect(),
        )
    }

    fn start_historical_sync_mempool_server_with_statuses(
        responses: Vec<(u16, String)>,
    ) -> HistoricalSyncMempoolServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("test mempool server should bind");
        listener
            .set_nonblocking(true)
            .expect("test mempool server should become nonblocking");
        let addr = listener
            .local_addr()
            .expect("test mempool server should expose address");
        let base_url = format!("http://{addr}/");
        let handle = thread::spawn(move || {
            let mut request_lines = Vec::new();
            for (status, response_body) in responses {
                let deadline = Instant::now() + Duration::from_secs(3);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            if Instant::now() >= deadline {
                                return request_lines;
                            }
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => panic!("test mempool server should accept request: {error}"),
                    }
                };
                stream
                    .set_nonblocking(false)
                    .expect("test mempool stream should become blocking");
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .expect("test mempool stream should have a read timeout");
                let mut buf = [0_u8; 4096];
                let read = stream
                    .read(&mut buf)
                    .expect("test mempool server should read request");
                let request = String::from_utf8_lossy(&buf[..read]);
                let first_line = request.lines().next().unwrap_or_default().to_string();
                let reason = if status == 429 {
                    "Too Many Requests"
                } else {
                    "OK"
                };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\nretry-after: 1\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    response_body.len(),
                    response_body
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("test mempool server should write response");
                request_lines.push(first_line);
            }
            request_lines
        });

        HistoricalSyncMempoolServer { base_url, handle }
    }

    fn run_stats_only_visit(
        user_id: crate::models::UserId,
        address: &SyncAddress,
        stats_json: &str,
        tip_height: ChainTipHeight,
    ) -> Result<SyncIterationResult, UserTransactionMonitorError> {
        let server = start_historical_sync_mempool_server(vec![stats_json.to_string()]);
        let http_counters = SyncHttpCounters::new();
        let traced_client =
            TracedBlockingClient::builder(IntegrationLabel::new(LABEL_MEMPOOL), user_id)
                .configure(|builder| builder.timeout(Duration::from_secs(2)))
                .build_for_tests_with_tracing(false)
                .expect("traced blocking client should build");
        let mempool_client = MempoolClient::new(
            traced_client,
            Url::parse(&server.base_url).expect("test mempool URL should parse"),
        );
        let clients = SyncClients {
            mempool_client: Some(&mempool_client),
            etherscan_api_key: None,
            etherscan_base_url: None,
            http_counters: &http_counters,
        };
        let now = test_now();
        let clock = FixedClock::new(now);
        let source_connection_id = crate::db::raw_ingestion::SourceConnectionId::new();
        let result =
            MempoolAddressSyncIntegration::new().sync_one_iteration(IntegrationIterationContext {
                run: make_run_context(&clock, user_id),
                now_utc: now,
                now_instant: clock.instant_now(),
                address,
                clients,
                single_address_progress: None,
                allow_known_confirmed_early_exit: false,
                chain_tip: Some(tip_height),
                raw_sync_run_id: SyncRunId::new(),
                source_connection_id: &source_connection_id,
                is_backfill_active: false,
                historical_backfill_enabled: false,
                legacy_mempool_history_repair: false,
                mempool_history_page_frontier: None,
            });
        let requests = server.join();
        assert_eq!(requests.len(), 1);
        result
    }

    fn run_backfill_visit(
        user_id: crate::models::UserId,
        address: &SyncAddress,
        stats_json: &str,
        page_json: &str,
    ) -> SyncIterationResult {
        let server = start_historical_sync_mempool_server(vec![
            stats_json.to_string(),
            page_json.to_string(),
        ]);
        let http_counters = SyncHttpCounters::new();
        let traced_client =
            TracedBlockingClient::builder(IntegrationLabel::new(LABEL_MEMPOOL), user_id)
                .configure(|builder| builder.timeout(Duration::from_secs(2)))
                .build_for_tests_with_tracing(false)
                .expect("traced blocking client should build");
        let mempool_client = MempoolClient::new(
            traced_client,
            Url::parse(&server.base_url).expect("test mempool URL should parse"),
        );
        let clients = SyncClients {
            mempool_client: Some(&mempool_client),
            etherscan_api_key: None,
            etherscan_base_url: None,
            http_counters: &http_counters,
        };
        let now = test_now();
        let clock = FixedClock::new(now);
        let raw_sync_run = start_sync_run(
            user_id,
            StartSyncRunRequest {
                integration: IntegrationKind::Mempool,
                scope_kind: SyncRunScopeKind::Address,
                scope_address_id: address.address_id,
                asset_id: address.asset_id,
                network: address.network,
                trigger_kind: SyncRunTriggerKind::Backfill,
                started_at: now,
                summary_json: None,
            },
        )
        .expect("raw sync run should start");
        let result = MempoolAddressSyncIntegration::new()
            .sync_one_iteration(IntegrationIterationContext {
                run: make_run_context(&clock, user_id),
                now_utc: now,
                now_instant: clock.instant_now(),
                address,
                clients,
                single_address_progress: None,
                allow_known_confirmed_early_exit: false,
                chain_tip: Some(ChainTipHeight::try_new(800_001).expect("tip should parse")),
                raw_sync_run_id: raw_sync_run.sync_run_id,
                source_connection_id: &raw_sync_run.source_connection_id,
                is_backfill_active: false,
                historical_backfill_enabled: true,
                legacy_mempool_history_repair: false,
                mempool_history_page_frontier: None,
            })
            .expect("backfill visit should succeed");
        server.join();
        result
    }

    #[test]
    fn capped_stats_only_visit_does_not_require_a_ledger_rebuild() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let address = test_sync_address();
        persist_sync_address_fixture(user_id, &address, test_now())
            .expect("sync address fixture should persist");
        mark_address_sync_started(
            user_id,
            address.address_id,
            TransactionSyncRunId::new(),
            test_now(),
        )
        .expect("sync state should exist");

        let result = run_stats_only_visit(
            user_id,
            &address,
            r#"{"chain_stats":{"tx_count":4,"funded_txo_sum":130,"spent_txo_sum":0},"mempool_stats":{"tx_count":0}}"#,
            ChainTipHeight::try_new(800_001).expect("tip should parse"),
        )
        .expect("stats-only visit should succeed");

        assert!(
            result.observed_activity,
            "an address with provider history still reports activity"
        );
        assert!(
            result.api_confirmed_balance.is_some(),
            "a stats-only visit still refreshes the current provider balance"
        );
        assert_eq!(result.new_tx_count.value(), 0);
        assert_eq!(result.updated_tx_count.value(), 0);
        assert!(
            !result.ledger_rebuild_required,
            "a stats-only visit must not request a ledger rebuild"
        );
    }

    #[test]
    fn already_known_page_still_requires_a_ledger_rebuild_despite_zero_counts() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let address = test_sync_address();
        persist_sync_address_fixture(user_id, &address, test_now())
            .expect("sync address fixture should persist");
        mark_address_sync_started(
            user_id,
            address.address_id,
            TransactionSyncRunId::new(),
            test_now(),
        )
        .expect("sync state should exist");

        let stats = r#"{"chain_stats":{"tx_count":1,"funded_txo_sum":7,"spent_txo_sum":0},"mempool_stats":{"tx_count":0}}"#;
        let page = format!(
            r#"[{{"txid":"7777777777777777777777777777777777777777777777777777777777777777","vin":[],"vout":[{{"scriptpubkey":"00","scriptpubkey_address":"{}","value":7}}],"fee":0,"status":{{"confirmed":true,"block_height":5,"block_hash":"block5","block_time":1700000005}}}}]"#,
            address.address.as_str()
        );

        let first = run_backfill_visit(user_id, &address, stats, &page);
        assert!(
            first.ledger_rebuild_required,
            "a first non-empty reconciliation requires a rebuild"
        );

        let second = run_backfill_visit(user_id, &address, stats, &page);
        assert_eq!(second.new_tx_count.value(), 0);
        assert_eq!(second.updated_tx_count.value(), 0);
        assert!(
            second.ledger_rebuild_required,
            "a non-empty reconciliation requires a rebuild even at zero counts"
        );
    }

    #[test]
    fn mempool_history_proof_higher_and_lower_stats_persist_durable_transitions() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let old_tip = ChainTipHeight::try_new(800_000).expect("old tip should parse");
        let new_tip = ChainTipHeight::try_new(800_001).expect("new tip should parse");
        let old_proof = MempoolHistoryProof {
            confirmed_tx_count: TransactionCount::from_u32(2),
            complete_height: old_tip,
        };

        let higher_user_id = unique_user_id();
        setup_test_user(higher_user_id);
        let mut higher_address = test_sync_address();
        higher_address.mempool_history_proof = Some(old_proof);
        persist_sync_address_fixture(higher_user_id, &higher_address, test_now())
            .expect("higher-count fixture should persist");
        mark_address_sync_started(
            higher_user_id,
            higher_address.address_id,
            TransactionSyncRunId::new(),
            test_now(),
        )
        .expect("higher-count sync state should exist");
        publish_mempool_history_proof(higher_user_id, higher_address.address_id, old_proof)
            .expect("older proof should seed");
        let cursor = MempoolCursorTxid::parse(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .expect("cursor should parse");
        update_address_mempool_backfill_cursor(
            higher_user_id,
            higher_address.address_id,
            Some(&cursor),
        )
        .expect("old cursor should seed");

        run_stats_only_visit(
            higher_user_id,
            &higher_address,
            r#"{"chain_stats":{"tx_count":3,"funded_txo_sum":3,"spent_txo_sum":0},"mempool_stats":{"tx_count":0}}"#,
            new_tip,
        )
        .expect("higher count visit should succeed");
        let durable_higher = get_non_hd_sync_addresses(higher_user_id)
            .expect("higher-count address should load")
            .into_iter()
            .find(|candidate| candidate.address_id == higher_address.address_id)
            .expect("higher-count address should exist");
        assert_eq!(durable_higher.mempool_history_proof, Some(old_proof));
        assert_eq!(durable_higher.mempool_backfill_cursor_txid, None);
        assert_eq!(
            durable_higher.mempool_expected_tx_count,
            Some(TransactionCount::from_u32(3))
        );

        let lower_user_id = unique_user_id();
        setup_test_user(lower_user_id);
        let mut lower_address = test_sync_address();
        lower_address.mempool_history_proof = Some(old_proof);
        persist_sync_address_fixture(lower_user_id, &lower_address, test_now())
            .expect("lower-count fixture should persist");
        mark_address_sync_started(
            lower_user_id,
            lower_address.address_id,
            TransactionSyncRunId::new(),
            test_now(),
        )
        .expect("lower-count sync state should exist");
        publish_mempool_history_proof(lower_user_id, lower_address.address_id, old_proof)
            .expect("older proof should seed");

        run_stats_only_visit(
            lower_user_id,
            &lower_address,
            r#"{"chain_stats":{"tx_count":1,"funded_txo_sum":1,"spent_txo_sum":0},"mempool_stats":{"tx_count":0}}"#,
            new_tip,
        )
        .expect("lower count visit should succeed");
        let durable_lower = get_non_hd_sync_addresses(lower_user_id)
            .expect("lower-count address should load")
            .into_iter()
            .find(|candidate| candidate.address_id == lower_address.address_id)
            .expect("lower-count address should exist");
        assert_eq!(durable_lower.mempool_history_proof, None);
        assert_eq!(durable_lower.mempool_backfill_cursor_txid, None);
        assert_eq!(
            durable_lower.mempool_expected_tx_count,
            Some(TransactionCount::from_u32(1))
        );
    }

    #[test]
    fn mempool_history_proof_malformed_stats_does_not_publish() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let address = test_sync_address();
        persist_sync_address_fixture(user_id, &address, test_now())
            .expect("sync address fixture should persist");
        mark_address_sync_started(
            user_id,
            address.address_id,
            TransactionSyncRunId::new(),
            test_now(),
        )
        .expect("sync state row should exist");

        assert!(
            run_stats_only_visit(
                user_id,
                &address,
                r#"{"chain_stats":{"tx_count":"invalid"},"mempool_stats":{"tx_count":0}}"#,
                ChainTipHeight::try_new(800_001).expect("tip should parse"),
            )
            .is_err()
        );
        assert_eq!(
            get_non_hd_sync_addresses(user_id)
                .expect("address should load")
                .into_iter()
                .find(|candidate| candidate.address_id == address.address_id)
                .expect("address should exist")
                .mempool_history_proof,
            None
        );
    }

    #[test]
    fn mempool_history_proof_nonzero_publishes_after_one_cached_stats_observation() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let mut address = test_sync_address();
        address.mempool_expected_tx_count = Some(TransactionCount::from_u32(2));
        let now = test_now();
        persist_sync_address_fixture(user_id, &address, now)
            .expect("sync address fixture should persist");
        mark_address_sync_started(
            user_id,
            address.address_id,
            TransactionSyncRunId::new(),
            now,
        )
        .expect("sync state row should exist");
        let raw_sync_run = start_sync_run(
            user_id,
            StartSyncRunRequest {
                integration: IntegrationKind::Mempool,
                scope_kind: SyncRunScopeKind::Address,
                scope_address_id: address.address_id,
                asset_id: address.asset_id,
                network: address.network,
                trigger_kind: SyncRunTriggerKind::Backfill,
                started_at: now,
                summary_json: None,
            },
        )
        .expect("raw sync run should start");

        let stats_json = r#"{"chain_stats":{"tx_count":2,"funded_txo_sum":50000,"spent_txo_sum":50000},"mempool_stats":{"tx_count":0}}"#;
        let first_page_json = format!(
            r#"[{{"txid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","vin":[],"vout":[{{"scriptpubkey":"00","scriptpubkey_address":"{}","value":50000}}],"fee":0,"status":{{"confirmed":true,"block_height":1,"block_hash":"block","block_time":1}}}}]"#,
            address.address.as_str()
        );
        let second_page_json = format!(
            r#"[{{"txid":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","vin":[],"vout":[{{"scriptpubkey":"00","scriptpubkey_address":"{}","value":1}}],"fee":0,"status":{{"confirmed":true,"block_height":2,"block_hash":"block-2","block_time":2}}}}]"#,
            address.address.as_str()
        );
        let server = start_historical_sync_mempool_server(vec![
            stats_json.to_string(),
            first_page_json,
            second_page_json,
        ]);
        let http_counters = SyncHttpCounters::new();
        let traced_client =
            TracedBlockingClient::builder(IntegrationLabel::new(LABEL_MEMPOOL), user_id)
                .configure(|builder| builder.timeout(Duration::from_secs(2)))
                .build_for_tests_with_tracing(false)
                .expect("traced blocking client should build");
        let mempool_client = MempoolClient::new(
            traced_client,
            Url::parse(&server.base_url).expect("test mempool URL should parse"),
        )
        .with_total_api_call_counter(http_counters.total_api_calls_counter());
        let clients = SyncClients {
            mempool_client: Some(&mempool_client),
            etherscan_api_key: None,
            etherscan_base_url: None,
            http_counters: &http_counters,
        };
        let clock = FixedClock::new(now);
        let run = make_run_context(&clock, user_id);
        let mut integration = MempoolAddressSyncIntegration::new();

        let first_result = integration
            .sync_one_iteration(IntegrationIterationContext {
                run,
                now_utc: now,
                now_instant: clock.instant_now(),
                address: &address,
                clients,
                single_address_progress: None,
                allow_known_confirmed_early_exit: false,
                chain_tip: Some(ChainTipHeight::try_new(1).expect("tip should be valid")),
                raw_sync_run_id: raw_sync_run.sync_run_id,
                source_connection_id: &raw_sync_run.source_connection_id,
                is_backfill_active: true,
                historical_backfill_enabled: true,
                legacy_mempool_history_repair: false,
                mempool_history_page_frontier: None,
            })
            .expect("historical sync should fetch the first transaction page");
        assert!(first_result.has_more_work);
        let result = integration
            .sync_one_iteration(IntegrationIterationContext {
                run,
                now_utc: now,
                now_instant: clock.instant_now(),
                address: &address,
                clients,
                single_address_progress: None,
                allow_known_confirmed_early_exit: false,
                chain_tip: Some(ChainTipHeight::try_new(1).expect("tip should be valid")),
                raw_sync_run_id: raw_sync_run.sync_run_id,
                source_connection_id: &raw_sync_run.source_connection_id,
                is_backfill_active: true,
                historical_backfill_enabled: true,
                legacy_mempool_history_repair: false,
                mempool_history_page_frontier: None,
            })
            .expect("historical sync should fetch the second transaction page");
        let request_lines = server.join();

        assert_eq!(
            request_lines,
            vec![
                format!("GET /api/address/{} HTTP/1.1", address.address.as_str()),
                format!("GET /api/address/{}/txs HTTP/1.1", address.address.as_str()),
                format!(
                    "GET /api/address/{}/txs/chain/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa HTTP/1.1",
                    address.address.as_str()
                ),
            ]
        );
        assert_eq!(
            result
                .api_confirmed_balance
                .map(|balance| balance.amount().value()),
            Some(0)
        );
        assert!(
            result
                .raw_run_summary_json
                .as_ref()
                .expect("summary should serialize")
                .as_str()
                .contains("\"pages_fetched\":2")
        );
        assert!(!result.has_more_work);
        let observation = crate::db::with_user_db(user_id, |conn| {
            conn.query_row(
                "SELECT reported_tx_count, last_tip_height, last_completed_at,
                        api_confirmed_balance_hi, api_confirmed_balance_lo
                 FROM transaction_sync_state
                 WHERE scope = 'address' AND address_id = ?1",
                [address.address_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                    ))
                },
            )
            .map_err(|err| crate::db::DbError::new(format!("observation query failed: {err}")))
        })
        .expect("coherent observation should load");
        assert_eq!(
            observation,
            (Some(2), Some(1), Some(now.to_rfc3339()), Some(0), Some(0),)
        );
        assert_eq!(
            get_non_hd_sync_addresses(user_id)
                .expect("proof should load")
                .into_iter()
                .find(|candidate| candidate.address_id == address.address_id)
                .expect("address should load")
                .mempool_history_proof,
            Some(MempoolHistoryProof {
                confirmed_tx_count: TransactionCount::from_u32(2),
                complete_height: ChainTipHeight::try_new(1).expect("tip should parse"),
            })
        );
    }

    #[test]
    fn mempool_page_writes_stay_bounded_by_page_items() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let mut address = test_sync_address();
        address.mempool_expected_tx_count = Some(TransactionCount::from_u32(3));
        let now = test_now();
        persist_sync_address_fixture(user_id, &address, now)
            .expect("sync address fixture should persist");
        mark_address_sync_started(
            user_id,
            address.address_id,
            TransactionSyncRunId::new(),
            now,
        )
        .expect("sync state row should exist");
        let raw_sync_run = start_sync_run(
            user_id,
            StartSyncRunRequest {
                integration: IntegrationKind::Mempool,
                scope_kind: SyncRunScopeKind::Address,
                scope_address_id: address.address_id,
                asset_id: address.asset_id,
                network: address.network,
                trigger_kind: SyncRunTriggerKind::Backfill,
                started_at: now,
                summary_json: None,
            },
        )
        .expect("raw sync run should start");
        let page_json = (1_u32..=3)
            .map(|index| {
                format!(
                    r#"{{"txid":"{index:064x}","vin":[],"vout":[{{"scriptpubkey":"00","scriptpubkey_address":"{}","value":1}}],"fee":0,"status":{{"confirmed":true,"block_height":{index},"block_hash":"block-{index}","block_time":{index}}}}}"#,
                    address.address.as_str()
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let server = start_historical_sync_mempool_server(vec![
            r#"{"chain_stats":{"tx_count":3,"funded_txo_sum":3,"spent_txo_sum":0},"mempool_stats":{"tx_count":0}}"#
                .to_string(),
            format!("[{page_json}]"),
        ]);
        let http_counters = SyncHttpCounters::new();
        let mempool_client = MempoolClient::new(
            TracedBlockingClient::builder(IntegrationLabel::new(LABEL_MEMPOOL), user_id)
                .configure(|builder| builder.timeout(Duration::from_secs(2)))
                .build_for_tests_with_tracing(false)
                .expect("traced blocking client should build"),
            Url::parse(&server.base_url).expect("test mempool URL should parse"),
        );
        let clients = SyncClients {
            mempool_client: Some(&mempool_client),
            etherscan_api_key: None,
            etherscan_base_url: None,
            http_counters: &http_counters,
        };
        let clock = FixedClock::new(now);
        let result = MempoolAddressSyncIntegration::new()
            .sync_one_iteration(IntegrationIterationContext {
                run: make_run_context(&clock, user_id),
                now_utc: now,
                now_instant: clock.instant_now(),
                address: &address,
                clients,
                single_address_progress: None,
                allow_known_confirmed_early_exit: false,
                chain_tip: Some(ChainTipHeight::try_new(1).expect("tip should parse")),
                raw_sync_run_id: raw_sync_run.sync_run_id,
                source_connection_id: &raw_sync_run.source_connection_id,
                is_backfill_active: true,
                historical_backfill_enabled: true,
                legacy_mempool_history_repair: false,
                mempool_history_page_frontier: None,
            })
            .expect("single transaction page should persist");
        assert!(!result.has_more_work);
        assert_eq!(server.join().len(), 2);

        let counts = crate::db::with_user_db(user_id, |conn| {
            let raw_versions = conn
                .query_row(
                    "SELECT COUNT(*) FROM raw_mempool_transaction_versions",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|error| crate::db::DbError::new(format!("raw count failed: {error}")))?;
            let raw_memberships = conn
                .query_row(
                    "SELECT COUNT(*)
                     FROM raw_mempool_transaction_observations
                     WHERE sync_run_id = ?1",
                    [raw_sync_run.sync_run_id.to_string()],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|error| {
                    crate::db::DbError::new(format!("membership count failed: {error}"))
                })?;
            let declared_page_items = conn
                .query_row(
                    "SELECT json_extract(grouping_metadata_json, '$.item_count')
                     FROM raw_observation_sets
                     WHERE sync_run_id = ?1",
                    [raw_sync_run.sync_run_id.to_string()],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|error| {
                    crate::db::DbError::new(format!("page item count failed: {error}"))
                })?;
            let canonical_transactions = conn
                .query_row("SELECT COUNT(*) FROM chain_transactions", [], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(|error| {
                    crate::db::DbError::new(format!("canonical count failed: {error}"))
                })?;
            Ok::<_, crate::db::DbError>((
                declared_page_items,
                raw_versions,
                raw_memberships,
                canonical_transactions,
            ))
        })
        .expect("page write counts should load");

        assert_eq!(counts, (3, 3, 3, 3));
    }

    #[test]
    fn bitcoin_history_full_resync_requires_empty_terminal_before_strict_proof() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let address = test_sync_address();
        let now = test_now();
        persist_sync_address_fixture(user_id, &address, now)
            .expect("sync address fixture should persist");
        mark_address_sync_started(
            user_id,
            address.address_id,
            TransactionSyncRunId::new(),
            now,
        )
        .expect("sync state row should exist");
        let raw_sync_run = start_sync_run(
            user_id,
            StartSyncRunRequest {
                integration: IntegrationKind::Mempool,
                scope_kind: SyncRunScopeKind::Address,
                scope_address_id: address.address_id,
                asset_id: address.asset_id,
                network: address.network,
                trigger_kind: SyncRunTriggerKind::Backfill,
                started_at: now,
                summary_json: None,
            },
        )
        .expect("raw sync run should start");
        let page = |txid: &str, height: u32| {
            format!(
                r#"[{{"txid":"{txid}","vin":[],"vout":[{{"scriptpubkey":"00","scriptpubkey_address":"{}","value":1}}],"fee":0,"status":{{"confirmed":true,"block_height":{height},"block_hash":"block-{height}","block_time":{height}}}}}]"#,
                address.address.as_str()
            )
        };
        let server = start_historical_sync_mempool_server(vec![
            r#"{"chain_stats":{"tx_count":2,"funded_txo_sum":2,"spent_txo_sum":0},"mempool_stats":{"tx_count":0}}"#
                .to_string(),
            page(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                1,
            ),
            page(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                2,
            ),
            "[]".to_string(),
        ]);
        let http_counters = SyncHttpCounters::new();
        let traced_client =
            TracedBlockingClient::builder(IntegrationLabel::new(LABEL_MEMPOOL), user_id)
                .configure(|builder| builder.timeout(Duration::from_secs(2)))
                .build_for_tests_with_tracing(false)
                .expect("traced blocking client should build");
        let mempool_client = MempoolClient::new(
            traced_client,
            Url::parse(&server.base_url).expect("test mempool URL should parse"),
        );
        let clients = SyncClients {
            mempool_client: Some(&mempool_client),
            etherscan_api_key: None,
            etherscan_base_url: None,
            http_counters: &http_counters,
        };
        let clock = FixedClock::new(now);
        let run = make_run_context(&clock, user_id);
        let tip = ChainTipHeight::try_new(800_001).expect("tip should parse");
        let mut integration = MempoolAddressSyncIntegration::new();

        for expected_more_work in [true, true, false] {
            let result = integration
                .sync_one_iteration(IntegrationIterationContext {
                    run,
                    now_utc: now,
                    now_instant: clock.instant_now(),
                    address: &address,
                    clients,
                    single_address_progress: None,
                    allow_known_confirmed_early_exit: false,
                    chain_tip: Some(tip),
                    raw_sync_run_id: raw_sync_run.sync_run_id,
                    source_connection_id: &raw_sync_run.source_connection_id,
                    is_backfill_active: true,
                    historical_backfill_enabled: true,
                    legacy_mempool_history_repair: true,
                    mempool_history_page_frontier: None,
                })
                .expect("strict repair page should succeed");
            assert_eq!(result.has_more_work, expected_more_work);
        }
        let requests = server.join();
        assert_eq!(requests.len(), 4, "stats plus three pages are required");
        let persisted = get_non_hd_sync_addresses(user_id)
            .expect("address should load")
            .into_iter()
            .find(|candidate| candidate.address_id == address.address_id)
            .expect("address should exist");
        assert_eq!(
            persisted
                .mempool_history_proof
                .map(|proof| proof.confirmed_tx_count),
            Some(TransactionCount::from_u32(2))
        );
        assert_eq!(persisted.mempool_history_scan_start_run_id, None);
    }

    #[test]
    fn bitcoin_history_full_resync_mismatch_clears_retained_proof_and_resume_state() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let now = test_now();
        let old_proof = MempoolHistoryProof {
            confirmed_tx_count: TransactionCount::from_u32(1),
            complete_height: ChainTipHeight::try_new(800_000).expect("tip should parse"),
        };
        let mut address = test_sync_address();
        address.mempool_history_proof = Some(old_proof);
        persist_sync_address_fixture(user_id, &address, now)
            .expect("sync address fixture should persist");
        mark_address_sync_started(
            user_id,
            address.address_id,
            TransactionSyncRunId::new(),
            now,
        )
        .expect("sync state row should exist");
        publish_mempool_history_proof(user_id, address.address_id, old_proof)
            .expect("old proof should seed");
        let raw_sync_run = start_sync_run(
            user_id,
            StartSyncRunRequest {
                integration: IntegrationKind::Mempool,
                scope_kind: SyncRunScopeKind::Address,
                scope_address_id: address.address_id,
                asset_id: address.asset_id,
                network: address.network,
                trigger_kind: SyncRunTriggerKind::Backfill,
                started_at: now,
                summary_json: None,
            },
        )
        .expect("raw sync run should start");
        let txid = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let server = start_historical_sync_mempool_server(vec![
            r#"{"chain_stats":{"tx_count":2,"funded_txo_sum":1,"spent_txo_sum":0},"mempool_stats":{"tx_count":0}}"#
                .to_string(),
            format!(
                r#"[{{"txid":"{txid}","vin":[],"vout":[{{"scriptpubkey":"00","scriptpubkey_address":"{}","value":1}}],"fee":0,"status":{{"confirmed":true,"block_height":1,"block_hash":"block-1","block_time":1}}}}]"#,
                address.address.as_str()
            ),
            "[]".to_string(),
        ]);
        let http_counters = SyncHttpCounters::new();
        let traced_client =
            TracedBlockingClient::builder(IntegrationLabel::new(LABEL_MEMPOOL), user_id)
                .configure(|builder| builder.timeout(Duration::from_secs(2)))
                .build_for_tests_with_tracing(false)
                .expect("traced blocking client should build");
        let mempool_client = MempoolClient::new(
            traced_client,
            Url::parse(&server.base_url).expect("test mempool URL should parse"),
        );
        let clients = SyncClients {
            mempool_client: Some(&mempool_client),
            etherscan_api_key: None,
            etherscan_base_url: None,
            http_counters: &http_counters,
        };
        let clock = FixedClock::new(now);
        let run = make_run_context(&clock, user_id);
        let context = || IntegrationIterationContext {
            run,
            now_utc: now,
            now_instant: clock.instant_now(),
            address: &address,
            clients,
            single_address_progress: None,
            allow_known_confirmed_early_exit: false,
            chain_tip: Some(ChainTipHeight::try_new(800_001).expect("tip should parse")),
            raw_sync_run_id: raw_sync_run.sync_run_id,
            source_connection_id: &raw_sync_run.source_connection_id,
            is_backfill_active: true,
            historical_backfill_enabled: true,
            legacy_mempool_history_repair: true,
            mempool_history_page_frontier: None,
        };
        let mut integration = MempoolAddressSyncIntegration::new();
        assert!(
            integration
                .sync_one_iteration(context())
                .expect("first page should succeed")
                .has_more_work
        );
        integration
            .sync_one_iteration(context())
            .expect_err("count mismatch should restart the strict scan");
        server.join();

        let persisted = get_non_hd_sync_addresses(user_id)
            .expect("address should load")
            .into_iter()
            .find(|candidate| candidate.address_id == address.address_id)
            .expect("address should exist");
        assert_eq!(persisted.mempool_history_proof, None);
        assert_eq!(persisted.mempool_backfill_cursor_txid, None);
        assert_eq!(persisted.mempool_history_scan_start_run_id, None);

        let retry_run = start_sync_run(
            user_id,
            StartSyncRunRequest {
                integration: IntegrationKind::Mempool,
                scope_kind: SyncRunScopeKind::Address,
                scope_address_id: address.address_id,
                asset_id: address.asset_id,
                network: address.network,
                trigger_kind: SyncRunTriggerKind::Backfill,
                started_at: now,
                summary_json: None,
            },
        )
        .expect("retry raw sync run should start");
        let retry_server = start_historical_sync_mempool_server(vec![
            r#"{"chain_stats":{"tx_count":2,"funded_txo_sum":1,"spent_txo_sum":0},"mempool_stats":{"tx_count":0}}"#
                .to_string(),
            format!(
                r#"[{{"txid":"{txid}","vin":[],"vout":[{{"scriptpubkey":"00","scriptpubkey_address":"{}","value":1}}],"fee":0,"status":{{"confirmed":true,"block_height":1,"block_hash":"block-1","block_time":1}}}}]"#,
                address.address.as_str()
            ),
        ]);
        let retry_counters = SyncHttpCounters::new();
        let retry_client = MempoolClient::new(
            TracedBlockingClient::builder(IntegrationLabel::new(LABEL_MEMPOOL), user_id)
                .configure(|builder| builder.timeout(Duration::from_secs(2)))
                .build_for_tests_with_tracing(false)
                .expect("traced blocking client should build"),
            Url::parse(&retry_server.base_url).expect("test mempool URL should parse"),
        );
        let retry_clients = SyncClients {
            mempool_client: Some(&retry_client),
            etherscan_api_key: None,
            etherscan_base_url: None,
            http_counters: &retry_counters,
        };
        MempoolAddressSyncIntegration::new()
            .sync_one_iteration(IntegrationIterationContext {
                run,
                now_utc: now,
                now_instant: clock.instant_now(),
                address: &persisted,
                clients: retry_clients,
                single_address_progress: None,
                allow_known_confirmed_early_exit: false,
                chain_tip: Some(ChainTipHeight::try_new(800_001).expect("tip should parse")),
                raw_sync_run_id: retry_run.sync_run_id,
                source_connection_id: &retry_run.source_connection_id,
                is_backfill_active: true,
                historical_backfill_enabled: true,
                legacy_mempool_history_repair: true,
                mempool_history_page_frontier: None,
            })
            .expect("retry first page should succeed");
        let retry_requests = retry_server.join();
        assert!(retry_requests[1].ends_with("/txs HTTP/1.1"));
        let restarted = get_non_hd_sync_addresses(user_id)
            .expect("address should load")
            .into_iter()
            .find(|candidate| candidate.address_id == address.address_id)
            .expect("address should exist");
        assert_eq!(
            restarted.mempool_history_scan_start_run_id,
            Some(retry_run.sync_run_id)
        );
    }

    #[test]
    fn bitcoin_history_full_resync_rate_limit_resumes_tagged_scan_and_cursor() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let address = test_sync_address();
        let now = test_now();
        persist_sync_address_fixture(user_id, &address, now)
            .expect("sync address fixture should persist");
        mark_address_sync_started(
            user_id,
            address.address_id,
            TransactionSyncRunId::new(),
            now,
        )
        .expect("sync state row should exist");
        let start_raw_run = || {
            start_sync_run(
                user_id,
                StartSyncRunRequest {
                    integration: IntegrationKind::Mempool,
                    scope_kind: SyncRunScopeKind::Address,
                    scope_address_id: address.address_id,
                    asset_id: address.asset_id,
                    network: address.network,
                    trigger_kind: SyncRunTriggerKind::Backfill,
                    started_at: now,
                    summary_json: None,
                },
            )
            .expect("raw sync run should start")
        };
        let first_run = start_raw_run();
        let first_txid = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let second_txid = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let page = |txid: &str, height: u32| {
            format!(
                r#"[{{"txid":"{txid}","vin":[],"vout":[{{"scriptpubkey":"00","scriptpubkey_address":"{}","value":1}}],"fee":0,"status":{{"confirmed":true,"block_height":{height},"block_hash":"block-{height}","block_time":{height}}}}}]"#,
                address.address.as_str()
            )
        };
        let stats = r#"{"chain_stats":{"tx_count":2,"funded_txo_sum":2,"spent_txo_sum":0},"mempool_stats":{"tx_count":0}}"#;
        let first_server = start_historical_sync_mempool_server_with_statuses(vec![
            (200, stats.to_string()),
            (200, page(first_txid, 1)),
            (429, "{}".to_string()),
        ]);
        let first_counters = SyncHttpCounters::new();
        let first_client = MempoolClient::new(
            TracedBlockingClient::builder(IntegrationLabel::new(LABEL_MEMPOOL), user_id)
                .configure(|builder| builder.timeout(Duration::from_secs(2)))
                .build_for_tests_with_tracing(false)
                .expect("traced blocking client should build"),
            Url::parse(&first_server.base_url).expect("test mempool URL should parse"),
        );
        let first_clients = SyncClients {
            mempool_client: Some(&first_client),
            etherscan_api_key: None,
            etherscan_base_url: None,
            http_counters: &first_counters,
        };
        let clock = FixedClock::new(now);
        let run = make_run_context(&clock, user_id);
        let tip = ChainTipHeight::try_new(800_001).expect("tip should parse");
        let mut first_integration = MempoolAddressSyncIntegration::new();
        let first_context = || IntegrationIterationContext {
            run,
            now_utc: now,
            now_instant: clock.instant_now(),
            address: &address,
            clients: first_clients,
            single_address_progress: None,
            allow_known_confirmed_early_exit: false,
            chain_tip: Some(tip),
            raw_sync_run_id: first_run.sync_run_id,
            source_connection_id: &first_run.source_connection_id,
            is_backfill_active: true,
            historical_backfill_enabled: true,
            legacy_mempool_history_repair: true,
            mempool_history_page_frontier: None,
        };
        assert!(
            first_integration
                .sync_one_iteration(first_context())
                .expect("first page should succeed")
                .has_more_work
        );
        assert!(matches!(
            first_integration.sync_one_iteration(first_context()),
            Err(UserTransactionMonitorError::RateLimited { .. })
        ));
        let first_requests = first_server.join();
        assert!(first_requests[2].contains(first_txid));

        let persisted = get_non_hd_sync_addresses(user_id)
            .expect("address should load")
            .into_iter()
            .find(|candidate| candidate.address_id == address.address_id)
            .expect("address should exist");
        assert_eq!(
            persisted
                .mempool_backfill_cursor_txid
                .as_ref()
                .map(MempoolCursorTxid::as_str),
            Some(first_txid)
        );
        assert_eq!(
            persisted.mempool_history_scan_start_run_id,
            Some(first_run.sync_run_id)
        );

        let second_run = start_raw_run();
        let second_server = start_historical_sync_mempool_server(vec![
            stats.to_string(),
            page(second_txid, 2),
            "[]".to_string(),
        ]);
        let second_counters = SyncHttpCounters::new();
        let second_client = MempoolClient::new(
            TracedBlockingClient::builder(IntegrationLabel::new(LABEL_MEMPOOL), user_id)
                .configure(|builder| builder.timeout(Duration::from_secs(2)))
                .build_for_tests_with_tracing(false)
                .expect("traced blocking client should build"),
            Url::parse(&second_server.base_url).expect("test mempool URL should parse"),
        );
        let second_clients = SyncClients {
            mempool_client: Some(&second_client),
            etherscan_api_key: None,
            etherscan_base_url: None,
            http_counters: &second_counters,
        };
        let second_context = || IntegrationIterationContext {
            run,
            now_utc: now,
            now_instant: clock.instant_now(),
            address: &persisted,
            clients: second_clients,
            single_address_progress: None,
            allow_known_confirmed_early_exit: false,
            chain_tip: Some(tip),
            raw_sync_run_id: second_run.sync_run_id,
            source_connection_id: &second_run.source_connection_id,
            is_backfill_active: true,
            historical_backfill_enabled: true,
            legacy_mempool_history_repair: true,
            mempool_history_page_frontier: None,
        };
        let mut second_integration = MempoolAddressSyncIntegration::new();
        assert!(
            second_integration
                .sync_one_iteration(second_context())
                .expect("resumed page should succeed")
                .has_more_work
        );
        assert!(
            !second_integration
                .sync_one_iteration(second_context())
                .expect("empty terminal should complete")
                .has_more_work
        );
        let second_requests = second_server.join();
        assert!(second_requests[1].contains(first_txid));

        let completed = get_non_hd_sync_addresses(user_id)
            .expect("address should load")
            .into_iter()
            .find(|candidate| candidate.address_id == address.address_id)
            .expect("address should exist");
        assert_eq!(
            completed
                .mempool_history_proof
                .map(|proof| proof.confirmed_tx_count),
            Some(TransactionCount::from_u32(2))
        );
        assert_eq!(completed.mempool_history_scan_start_run_id, None);
        assert_eq!(completed.mempool_backfill_cursor_txid, None);
    }

    #[test]
    fn mempool_history_proof_page_work_failure_does_not_publish() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let account_id = crate::wallets::DigitalAssetAccountId::new();
        let address = crate::tasks::jobs::sync::test_support::make_sync_address(
            "bc1qtestaddress000000000000000000000000000000000000000000000",
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            Some(account_id),
            None,
            None,
            None,
        );
        let now = test_now();
        persist_sync_address_fixture(user_id, &address, now)
            .expect("sync address fixture should persist");
        crate::db::reconcile_address_transactions(
            user_id,
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            &[crate::db::SyncTransactionRecord {
                tx_hash: crate::transactions::TxHash::parse(
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                )
                .expect("tx hash should parse"),
                status: crate::transactions::ChainTransactionStatus::Confirmed,
                block_height: Some(1),
                block_hash: Some("old-block".to_string()),
                block_time: Some(
                    Utc.timestamp_opt(1, 0)
                        .single()
                        .expect("block time should parse"),
                ),
                fee_amount: Some(0),
                inputs: Vec::new(),
                outputs: vec![crate::db::SyncTransactionOutputRecord {
                    output_index: 0,
                    raw_address: Some(address.address.clone()),
                    script_pubkey_hex: "00".to_string(),
                    value_amount: 1,
                }],
            }],
            now,
        )
        .expect("canonical transaction fixture should persist");
        crate::db::rebuild_account_transaction_ledger(user_id, account_id, now)
            .expect("ledger fixture should build");
        mark_address_sync_started(
            user_id,
            address.address_id,
            TransactionSyncRunId::new(),
            now,
        )
        .expect("sync state row should exist");
        crate::db::publish_mempool_history_proof(
            user_id,
            address.address_id,
            MempoolHistoryProof {
                confirmed_tx_count: TransactionCount::from_u32(1),
                complete_height: ChainTipHeight::try_new(1).expect("tip should parse"),
            },
        )
        .expect("proof fixture should publish");
        crate::db::with_user_db_mut(user_id, |conn| {
            conn.execute_batch(
                "CREATE TRIGGER reject_mempool_page_work
                 BEFORE UPDATE OF mempool_backfill_cursor_txid
                 ON transaction_sync_state
                 BEGIN
                   SELECT RAISE(ABORT, 'injected page-work failure');
                 END;",
            )
            .map_err(|err| {
                crate::db::DbError::new(format!("failed to install test trigger: {err}"))
            })
        })
        .expect("page-work failure trigger should install");
        let raw_sync_run = start_sync_run(
            user_id,
            StartSyncRunRequest {
                integration: IntegrationKind::Mempool,
                scope_kind: SyncRunScopeKind::Address,
                scope_address_id: address.address_id,
                asset_id: address.asset_id,
                network: address.network,
                trigger_kind: SyncRunTriggerKind::Backfill,
                started_at: now,
                summary_json: None,
            },
        )
        .expect("raw sync run should start");
        let page_json = format!(
            r#"[{{"txid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","vin":[],"vout":[{{"scriptpubkey":"00","scriptpubkey_address":"{}","value":1}}],"fee":0,"status":{{"confirmed":true,"block_height":1,"block_hash":"block","block_time":1}}}}]"#,
            address.address.as_str()
        );
        let server = start_historical_sync_mempool_server(vec![
            r#"{"chain_stats":{"tx_count":1,"funded_txo_sum":1,"spent_txo_sum":0},"mempool_stats":{"tx_count":0}}"#
                .to_string(),
            page_json,
        ]);
        let http_counters = SyncHttpCounters::new();
        let traced_client =
            TracedBlockingClient::builder(IntegrationLabel::new(LABEL_MEMPOOL), user_id)
                .configure(|builder| builder.timeout(Duration::from_secs(2)))
                .build_for_tests_with_tracing(false)
                .expect("traced blocking client should build");
        let mempool_client = MempoolClient::new(
            traced_client,
            Url::parse(&server.base_url).expect("test mempool URL should parse"),
        );
        let clients = SyncClients {
            mempool_client: Some(&mempool_client),
            etherscan_api_key: None,
            etherscan_base_url: None,
            http_counters: &http_counters,
        };
        let clock = FixedClock::new(now);

        let error = MempoolAddressSyncIntegration::new()
            .sync_one_iteration(IntegrationIterationContext {
                run: make_run_context(&clock, user_id),
                now_utc: now,
                now_instant: clock.instant_now(),
                address: &address,
                clients,
                single_address_progress: None,
                allow_known_confirmed_early_exit: false,
                chain_tip: Some(ChainTipHeight::try_new(1).expect("tip should be valid")),
                raw_sync_run_id: raw_sync_run.sync_run_id,
                source_connection_id: &raw_sync_run.source_connection_id,
                is_backfill_active: true,
                historical_backfill_enabled: true,
                legacy_mempool_history_repair: false,
                mempool_history_page_frontier: None,
            })
            .expect_err("injected page-work failure should abort the visit");
        let _requests = server.join();

        assert_eq!(
            error
                .coverage_invalidation()
                .expect("error should preserve invalidation targets")
                .account_ids,
            HashSet::from([account_id])
        );
        assert_eq!(
            get_non_hd_sync_addresses(user_id)
                .expect("address should load")
                .into_iter()
                .find(|candidate| candidate.address_id == address.address_id)
                .expect("persisted address should exist")
                .mempool_history_proof,
            None
        );
        let null_closing_count = crate::db::with_user_db(user_id, |conn| {
            conn.query_row(
                "SELECT COUNT(*)
                 FROM account_transaction_ledger
                 WHERE account_id = ?1
                   AND (
                     closing_balance_hi IS NULL
                     OR closing_balance_lo IS NULL
                   )",
                [account_id.to_string()],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|err| {
                crate::db::DbError::new(format!(
                    "failed to load invalidated closing balances: {err}"
                ))
            })
        })
        .expect("invalidated closing balances should load");
        assert_eq!(null_closing_count, 1);
    }

    #[test]
    fn mempool_partial_reconciliation_failure_invalidates_committed_contradiction() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let account_id = crate::wallets::DigitalAssetAccountId::new();
        let address = crate::tasks::jobs::sync::test_support::make_sync_address(
            "bc1qtestaddress000000000000000000000000000000000000000000000",
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            Some(account_id),
            None,
            None,
            None,
        );
        let now = test_now();
        persist_sync_address_fixture(user_id, &address, now)
            .expect("sync address fixture should persist");
        let original = crate::db::SyncTransactionRecord {
            tx_hash: crate::transactions::TxHash::parse(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .expect("tx hash should parse"),
            status: crate::transactions::ChainTransactionStatus::Confirmed,
            block_height: Some(1),
            block_hash: Some("old-block".to_string()),
            block_time: Some(now),
            fee_amount: Some(0),
            inputs: Vec::new(),
            outputs: vec![crate::db::SyncTransactionOutputRecord {
                output_index: 0,
                raw_address: Some(address.address.clone()),
                script_pubkey_hex: "00".to_string(),
                value_amount: 1,
            }],
        };
        crate::db::reconcile_address_transactions(
            user_id,
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            std::slice::from_ref(&original),
            now,
        )
        .expect("canonical transaction fixture should persist");
        crate::db::rebuild_account_transaction_ledger(user_id, account_id, now)
            .expect("ledger fixture should build");
        mark_address_sync_started(
            user_id,
            address.address_id,
            TransactionSyncRunId::new(),
            now,
        )
        .expect("sync state row should exist");
        crate::db::publish_mempool_history_proof(
            user_id,
            address.address_id,
            MempoolHistoryProof {
                confirmed_tx_count: TransactionCount::from_u32(1),
                complete_height: ChainTipHeight::try_new(1).expect("tip should parse"),
            },
        )
        .expect("proof fixture should publish");

        let mut contradicted = original;
        contradicted.block_hash = Some("replacement-block".to_string());
        let invalid = crate::db::SyncTransactionRecord {
            tx_hash: crate::transactions::TxHash::parse(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            )
            .expect("tx hash should parse"),
            status: crate::transactions::ChainTransactionStatus::Confirmed,
            block_height: Some(2),
            block_hash: Some("block-2".to_string()),
            block_time: Some(now),
            fee_amount: Some(0),
            inputs: Vec::new(),
            outputs: vec![crate::db::SyncTransactionOutputRecord {
                output_index: 0,
                raw_address: Some(address.address.clone()),
                script_pubkey_hex: "00".to_string(),
                value_amount: -1,
            }],
        };

        let error = reconcile_mempool_transactions(
            user_id,
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            &[contradicted, invalid],
            now + chrono::Duration::seconds(1),
        )
        .expect_err("later invalid record should fail after the contradiction commits");

        assert_eq!(
            error
                .coverage_invalidation()
                .expect("error should preserve committed invalidation targets")
                .account_ids,
            HashSet::from([account_id])
        );
        assert_eq!(
            get_non_hd_sync_addresses(user_id)
                .expect("address should load")
                .into_iter()
                .find(|candidate| candidate.address_id == address.address_id)
                .expect("persisted address should exist")
                .mempool_history_proof,
            None
        );
        let (stored_block_hash, null_closing_count) = crate::db::with_user_db(user_id, |conn| {
            let stored_block_hash = conn
                .query_row(
                    "SELECT block_hash
                         FROM chain_transactions
                         WHERE tx_hash = ?1",
                    ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],
                    |row| row.get::<_, Option<String>>(0),
                )
                .map_err(|err| {
                    crate::db::DbError::new(format!(
                        "failed to load committed contradiction: {err}"
                    ))
                })?;
            let null_closing_count = conn
                .query_row(
                    "SELECT COUNT(*)
                         FROM account_transaction_ledger
                         WHERE account_id = ?1
                           AND (
                             closing_balance_hi IS NULL
                             OR closing_balance_lo IS NULL
                           )",
                    [account_id.to_string()],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|err| {
                    crate::db::DbError::new(format!(
                        "failed to load invalidated closing balances: {err}"
                    ))
                })?;
            Ok::<_, crate::db::DbError>((stored_block_hash, null_closing_count))
        })
        .expect("partial reconciliation state should load");
        assert_eq!(stored_block_hash.as_deref(), Some("replacement-block"));
        assert_eq!(null_closing_count, 1);
    }

    #[test]
    fn mempool_history_proof_zero_zero_publishes_without_transaction_request() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let address = test_sync_address();
        let now = test_now();
        persist_sync_address_fixture(user_id, &address, now)
            .expect("sync address fixture should persist");
        mark_address_sync_started(
            user_id,
            address.address_id,
            TransactionSyncRunId::new(),
            now,
        )
        .expect("sync state row should exist");
        crate::db::mark_address_sync_completed_success(
            user_id,
            &crate::db::AddressSyncSuccess {
                address_id: address.address_id,
                run_id: TransactionSyncRunId::new(),
                started_at: now,
                completed_at: now,
                last_tip_height: ChainTipHeight::try_new(800_000).expect("tip should parse"),
                new_tx_count: TransactionCount::zero(),
                updated_tx_count: TransactionCount::zero(),
                api_confirmed_balance: Some(
                    crate::transactions::ApiConfirmedBalance::from_smallest_unit_i64(5)
                        .expect("balance should parse"),
                ),
            },
        )
        .expect("old balance should seed");
        let raw_sync_run = start_sync_run(
            user_id,
            StartSyncRunRequest {
                integration: IntegrationKind::Mempool,
                scope_kind: SyncRunScopeKind::Address,
                scope_address_id: address.address_id,
                asset_id: address.asset_id,
                network: address.network,
                trigger_kind: SyncRunTriggerKind::Backfill,
                started_at: now,
                summary_json: None,
            },
        )
        .expect("raw sync run should start");
        let server = start_historical_sync_mempool_server(vec![
            r#"{"chain_stats":{"tx_count":0,"funded_txo_sum":0,"spent_txo_sum":1},"mempool_stats":{"tx_count":0}}"#
                .to_string(),
        ]);
        let http_counters = SyncHttpCounters::new();
        let traced_client =
            TracedBlockingClient::builder(IntegrationLabel::new(LABEL_MEMPOOL), user_id)
                .configure(|builder| builder.timeout(Duration::from_secs(2)))
                .build_for_tests_with_tracing(false)
                .expect("traced blocking client should build");
        let mempool_client = MempoolClient::new(
            traced_client,
            Url::parse(&server.base_url).expect("test mempool URL should parse"),
        );
        let clients = SyncClients {
            mempool_client: Some(&mempool_client),
            etherscan_api_key: None,
            etherscan_base_url: None,
            http_counters: &http_counters,
        };
        let clock = FixedClock::new(now);
        let run = make_run_context(&clock, user_id);
        let tip_height = ChainTipHeight::try_new(800_001).expect("tip should parse");

        let result = MempoolAddressSyncIntegration::new()
            .sync_one_iteration(IntegrationIterationContext {
                run,
                now_utc: now,
                now_instant: clock.instant_now(),
                address: &address,
                clients,
                single_address_progress: None,
                allow_known_confirmed_early_exit: false,
                chain_tip: Some(tip_height),
                raw_sync_run_id: raw_sync_run.sync_run_id,
                source_connection_id: &raw_sync_run.source_connection_id,
                is_backfill_active: true,
                historical_backfill_enabled: true,
                legacy_mempool_history_repair: false,
                mempool_history_page_frontier: None,
            })
            .expect("zero address visit should succeed");
        let requests = server.join();

        assert!(!result.has_more_work);
        assert_eq!(requests.len(), 1);
        assert!(!requests[0].contains("/txs"));
        let balance_limbs = crate::db::with_user_db(user_id, |conn| {
            conn.query_row(
                "SELECT api_confirmed_balance_hi, api_confirmed_balance_lo
                 FROM transaction_sync_state
                 WHERE scope = 'address' AND address_id = ?1",
                [address.address_id.to_string()],
                |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
            )
            .map_err(|err| crate::db::DbError::new(format!("balance query failed: {err}")))
        })
        .expect("balance limbs should load");
        assert_eq!(balance_limbs, (None, None));
        assert_eq!(
            get_non_hd_sync_addresses(user_id)
                .expect("proof should load")
                .into_iter()
                .find(|candidate| candidate.address_id == address.address_id)
                .expect("address should load")
                .mempool_history_proof,
            Some(MempoolHistoryProof {
                confirmed_tx_count: TransactionCount::zero(),
                complete_height: tip_height,
            })
        );
    }

    #[test]
    fn finalize_terminal_run_clears_expected_count_when_backfill_completes() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let address = test_sync_address();
        let now = test_now();
        persist_sync_address_fixture(user_id, &address, now)
            .expect("sync address fixture should persist");
        mark_address_sync_started(
            user_id,
            address.address_id,
            TransactionSyncRunId::new(),
            now,
        )
        .expect("sync state row should exist");

        let cursor = MempoolCursorTxid::parse(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .expect("cursor should parse");
        update_address_mempool_backfill_cursor(user_id, address.address_id, Some(&cursor))
            .expect("cursor should persist");
        update_address_mempool_expected_tx_count(
            user_id,
            address.address_id,
            Some(TransactionCount::from_u32(2)),
        )
        .expect("expected tx count should persist");

        let mut integration = MempoolAddressSyncIntegration {
            iteration_state: Some(MempoolIterationState {
                visit: test_visit(),
                backfill_active: true,
                proof_publication_allowed: true,
                proof_published: true,
                first_page_done: true,
                cursor: None,
                known_confirmed: HashSet::new(),
                observed_confirmed_txids: HashSet::new(),
                backfill_expected_tx_count: None,
                backfill_has_pending_transactions: false,
                strict_scan_start_run_id: None,
                run_summary: MempoolRunSummary {
                    backfill_active: true,
                    ..MempoolRunSummary::default()
                },
            }),
        };

        let raw_run_summary_json = integration
            .finalize_terminal_run(user_id, address.address_id, false)
            .expect("backfill finalization should succeed")
            .expect("summary json should be present");
        assert_eq!(
            integration
                .current_run_summary_json()
                .expect("summary json should still serialize"),
            Some(raw_run_summary_json)
        );

        let persisted_address = get_non_hd_sync_addresses(user_id)
            .expect("sync addresses should load")
            .into_iter()
            .find(|candidate| candidate.address_id == address.address_id)
            .expect("persisted sync address should exist");
        assert_eq!(persisted_address.mempool_backfill_cursor_txid, None);
        assert_eq!(persisted_address.mempool_expected_tx_count, None);
    }

    #[test]
    fn terminal_cursor_cleanup_failure_preserves_iteration_targets() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let account_id = crate::wallets::DigitalAssetAccountId::new();
        let address = crate::tasks::jobs::sync::test_support::make_sync_address(
            "bc1qterminalcleanupfailure",
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            Some(account_id),
            None,
            None,
            None,
        );
        let now = test_now();
        persist_sync_address_fixture(user_id, &address, now)
            .expect("sync address fixture should persist");
        mark_address_sync_started(
            user_id,
            address.address_id,
            TransactionSyncRunId::new(),
            now,
        )
        .expect("sync state row should exist");
        update_address_mempool_expected_tx_count(
            user_id,
            address.address_id,
            Some(TransactionCount::from_u32(1)),
        )
        .expect("expected transaction count should persist");
        crate::db::with_user_db_mut(user_id, |conn| {
            conn.execute_batch(
                "CREATE TRIGGER test_reject_terminal_expected_count_cleanup
                 BEFORE UPDATE OF mempool_expected_tx_count
                 ON transaction_sync_state
                 WHEN NEW.mempool_expected_tx_count IS NULL
                 BEGIN
                   SELECT RAISE(ABORT, 'injected terminal cursor cleanup failure');
                 END;",
            )
            .map_err(|err| {
                crate::db::DbError::new(format!(
                    "failed to install terminal cleanup failure: {err}"
                ))
            })
        })
        .expect("terminal cleanup failure should install");
        let mut integration = MempoolAddressSyncIntegration {
            iteration_state: Some(MempoolIterationState {
                visit: test_visit(),
                backfill_active: true,
                proof_publication_allowed: true,
                proof_published: true,
                first_page_done: true,
                cursor: None,
                known_confirmed: HashSet::new(),
                observed_confirmed_txids: HashSet::new(),
                backfill_expected_tx_count: None,
                backfill_has_pending_transactions: false,
                strict_scan_start_run_id: None,
                run_summary: MempoolRunSummary {
                    backfill_active: true,
                    ..MempoolRunSummary::default()
                },
            }),
        };
        let mut iteration = SyncIterationResult::exhausted(
            ChainTipHeight::try_new(1).expect("tip should parse"),
            now,
        );
        iteration
            .coverage_invalidation
            .account_ids
            .insert(account_id);

        let finalization = integration.finalize_terminal_run(user_id, address.address_id, false);
        let error = finalization
            .map_err(|error| {
                crate::tasks::jobs::sync::error::preserve_iteration_error(error, &iteration)
            })
            .expect_err("terminal cleanup should fail");

        assert!(
            error
                .to_string()
                .contains("terminal cursor cleanup failure")
        );
        assert_eq!(
            error
                .coverage_invalidation()
                .expect("terminal cleanup error should preserve iteration targets")
                .account_ids,
            HashSet::from([account_id])
        );
    }

    #[test]
    fn mempool_history_proof_duplicate_cursor_does_not_complete_backfill() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let mut address = test_sync_address();
        let cursor = MempoolCursorTxid::parse(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .expect("cursor should parse");
        address.mempool_backfill_cursor_txid = Some(cursor.clone());
        address.mempool_expected_tx_count = Some(TransactionCount::from_u32(2));
        let now = test_now();
        persist_sync_address_fixture(user_id, &address, now)
            .expect("sync address fixture should persist");
        mark_address_sync_started(
            user_id,
            address.address_id,
            TransactionSyncRunId::new(),
            now,
        )
        .expect("sync state row should exist");
        update_address_mempool_backfill_cursor(user_id, address.address_id, Some(&cursor))
            .expect("cursor should persist");
        update_address_mempool_expected_tx_count(
            user_id,
            address.address_id,
            Some(TransactionCount::from_u32(2)),
        )
        .expect("expected count should persist");

        let clock = FixedClock::new(now);
        let run = make_run_context(&clock, user_id);
        let http_counters = SyncHttpCounters::new();
        let clients = SyncClients {
            mempool_client: None,
            etherscan_api_key: None,
            etherscan_base_url: None,
            http_counters: &http_counters,
        };
        let source_connection_id = crate::db::raw_ingestion::SourceConnectionId::new();
        let context = IntegrationIterationContext {
            run,
            now_utc: now,
            now_instant: clock.instant_now(),
            address: &address,
            clients,
            single_address_progress: None,
            allow_known_confirmed_early_exit: false,
            chain_tip: Some(ChainTipHeight::try_new(1).expect("tip should be valid")),
            raw_sync_run_id: SyncRunId::new(),
            source_connection_id: &source_connection_id,
            is_backfill_active: true,
            historical_backfill_enabled: true,
            legacy_mempool_history_repair: false,
            mempool_history_page_frontier: None,
        };
        let mut integration = MempoolAddressSyncIntegration {
            iteration_state: Some(MempoolIterationState {
                visit: test_visit(),
                backfill_active: true,
                proof_publication_allowed: true,
                proof_published: false,
                first_page_done: true,
                cursor: Some(cursor.as_str().to_string()),
                known_confirmed: HashSet::new(),
                observed_confirmed_txids: HashSet::new(),
                backfill_expected_tx_count: Some(TransactionCount::from_u32(2)),
                backfill_has_pending_transactions: false,
                strict_scan_start_run_id: None,
                run_summary: MempoolRunSummary {
                    backfill_active: true,
                    ..MempoolRunSummary::default()
                },
            }),
        };

        {
            let state = integration
                .iteration_state
                .as_mut()
                .expect("iteration state should exist");
            update_cursor_after_paginated_page(
                state,
                &context,
                cursor.as_str(),
                Some(cursor.as_str().to_string()),
                1,
                1,
            );
            assert_eq!(state.cursor, Some(cursor.as_str().to_string()));
            assert!(state.run_summary.duplicate_cursor_page_detected);
        }

        let raw_run_summary_json = integration
            .current_run_summary_json()
            .expect("summary should serialize")
            .expect("summary json should be present");
        assert!(
            raw_run_summary_json
                .as_str()
                .contains("\"duplicate_cursor_page_detected\":true")
        );

        let persisted_address = get_non_hd_sync_addresses(user_id)
            .expect("sync addresses should load")
            .into_iter()
            .find(|candidate| candidate.address_id == address.address_id)
            .expect("persisted sync address should exist");
        assert_eq!(persisted_address.mempool_backfill_cursor_txid, Some(cursor));
        assert_eq!(
            persisted_address.mempool_expected_tx_count,
            Some(TransactionCount::from_u32(2))
        );
    }

    #[test]
    fn ensure_initialized_resumes_backfill_from_persisted_cursor() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let mut address = test_sync_address();
        let cursor = MempoolCursorTxid::parse(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .expect("cursor should parse");
        address.mempool_backfill_cursor_txid = Some(cursor.clone());
        let now = test_now();
        persist_sync_address_fixture(user_id, &address, now)
            .expect("sync address fixture should persist");
        mark_address_sync_started(
            user_id,
            address.address_id,
            TransactionSyncRunId::new(),
            now,
        )
        .expect("sync state row should exist");

        let clock = FixedClock::new(now);
        let run = make_run_context(&clock, user_id);
        let http_counters = SyncHttpCounters::new();
        let server = start_historical_sync_mempool_server(vec![
            r#"{"chain_stats":{"tx_count":2,"funded_txo_sum":0,"spent_txo_sum":0},"mempool_stats":{"tx_count":0}}"#
                .to_string(),
        ]);
        let traced_client =
            TracedBlockingClient::builder(IntegrationLabel::new(LABEL_MEMPOOL), user_id)
                .configure(|builder| builder.timeout(Duration::from_secs(2)))
                .build_for_tests_with_tracing(false)
                .expect("traced blocking client should build");
        let mempool_client = MempoolClient::new(
            traced_client,
            Url::parse(&server.base_url).expect("test mempool URL should parse"),
        );
        let clients = SyncClients {
            mempool_client: Some(&mempool_client),
            etherscan_api_key: None,
            etherscan_base_url: None,
            http_counters: &http_counters,
        };
        let mut integration = MempoolAddressSyncIntegration::new();
        let source_connection_id = crate::db::raw_ingestion::SourceConnectionId::new();
        let state = integration
            .ensure_initialized(
                &IntegrationIterationContext {
                    run,
                    now_utc: now,
                    now_instant: clock.instant_now(),
                    address: &address,
                    clients,
                    single_address_progress: None,
                    allow_known_confirmed_early_exit: false,
                    chain_tip: Some(ChainTipHeight::try_new(1).expect("tip should be valid")),
                    raw_sync_run_id: SyncRunId::new(),
                    source_connection_id: &source_connection_id,
                    is_backfill_active: true,
                    historical_backfill_enabled: true,
                    legacy_mempool_history_repair: false,
                    mempool_history_page_frontier: None,
                },
                ChainTipHeight::try_new(1).expect("tip should parse"),
                &mempool_client,
            )
            .expect("backfill initialization should succeed");
        let request_lines = server.join();

        assert!(state.first_page_done);
        assert_eq!(state.cursor, Some(cursor.as_str().to_string()));
        assert_eq!(request_lines.len(), 1);
    }
}
