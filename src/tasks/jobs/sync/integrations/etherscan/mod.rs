mod mapper;

use super::{AddressSyncIntegration, IntegrationEstimateContext};
use crate::asset_capabilities::SyncProviderId;
use crate::db::raw_ingestion::{EtherscanChainId, EtherscanRequestKind, OpaqueJsonText, SyncRunId};
use crate::db::{
    SyncAddress, reconcile_account_transactions, update_address_etherscan_backfill_cursor,
    update_address_etherscan_history_status,
};
use crate::ethereum::{EthAddress, RawEthAddress};
use crate::integrations::etherscan::{
    EtherscanClient, EtherscanError, EtherscanFetchedPage, EtherscanInternalTx, EtherscanNetwork,
    EtherscanNormalTx, EtherscanRequestMetadata,
};
use crate::models::{EtherscanBaseUrl, RawEtherscanApiKey, UserId};
use crate::tasks::jobs::raw_ingestion_executor::{
    EtherscanPageIngestionRequest, EtherscanPageIngestionSummary, EtherscanRequestFailureRecord,
    IngestedEtherscanPage, ingest_etherscan_internal_page, ingest_etherscan_normal_page,
    record_etherscan_request_failure,
};
use crate::tasks::jobs::sync::{
    IntegrationIterationContext, IntegrationSyncPlan, LABEL_ETHERSCAN, RunContext,
    SyncIterationResult, UserTransactionMonitorError, is_first_sync,
};
use crate::tasks::publish_transaction_sync_event;
use crate::traces::client::{IntegrationLabel, TracedBlockingClient};
use crate::transactions::{
    AddressBackfillCursor, AddressBackfillState, ApiConfirmedBalance, ChainTipHeight,
    ChainTipHeightError, EthereumBlockNumber, EtherscanHistoryStatus, TransactionCount,
    TransactionSyncEvent, TxCountEstimate,
};
use crate::wallets::Network;
use dioxus::logger::tracing;
use std::collections::HashSet;
use std::time::Duration;

pub(crate) use self::mapper::map_etherscan_transactions;

const ETHERSCAN_REQUEST_TIMEOUT_SECONDS: u64 = 15;
const ETHERSCAN_PAGE_SIZE: u64 = 1000;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, Default)]
struct EtherscanRunSummary {
    pages_fetched: u32,
    items_seen: u32,
    versions_inserted: u32,
    versions_reused: u32,
    parse_success_count: u32,
    parse_failure_count: u32,
    http_failure_count: u32,
    transport_failure_count: u32,
    backfill_active: bool,
    backfill_complete: bool,
    backfill_budget_exhausted: bool,
    requested_start_block: u64,
    requested_end_block: u64,
    resume_cursor_end_block: Option<u64>,
}

impl EtherscanRunSummary {
    fn record_page(&mut self, page: &EtherscanPageIngestionSummary) {
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
                "failed to serialize etherscan sync run summary: {err}"
            ))
        })?;
        OpaqueJsonText::parse(json).map_err(Into::into)
    }

    fn record_request_failure(&mut self, error: &EtherscanError) {
        match error {
            EtherscanError::Http { .. } => {
                self.transport_failure_count = self.transport_failure_count.saturating_add(1);
            }
            EtherscanError::UpstreamStatus { .. }
            | EtherscanError::Deserialize { .. }
            | EtherscanError::ApiError { .. } => {
                self.http_failure_count = self.http_failure_count.saturating_add(1);
            }
        }
    }
}

type EtherscanPageSet = (
    Vec<EtherscanNormalTx>,
    Vec<EtherscanInternalTx>,
    Option<u64>,
);

#[derive(Clone, Copy)]
struct EtherscanFetchRange {
    start_block: u64,
    end_block: u64,
}

pub(crate) struct EtherscanAddressSyncIntegration {
    iteration_state: Option<EtherscanIterationState>,
}

struct EtherscanIterationState {
    client: EtherscanClient,
    tracked_address: EthAddress,
    range_start_block: u64,
    current_end_block: u64,
    chain_tip_u64: u64,
    backfill_cursor_active: bool,
    run_summary: EtherscanRunSummary,
    fetched_normal_count: u32,
    done: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EtherscanFetchInitialization {
    range_start_block: u64,
    current_end_block: u64,
    backfill_cursor_active: bool,
}

fn etherscan_fetch_initialization(
    address: &SyncAddress,
    chain_tip_u64: u64,
    historical_backfill_enabled: bool,
) -> Result<EtherscanFetchInitialization, UserTransactionMonitorError> {
    if historical_backfill_enabled && let Some(cursor) = address.etherscan_backfill_end_block {
        let end_block = cursor.as_u64().map_err(|err| {
            UserTransactionMonitorError::Parse(format!(
                "invalid persisted etherscan backfill end block: {err}"
            ))
        })?;
        return Ok(EtherscanFetchInitialization {
            range_start_block: 0,
            current_end_block: end_block,
            backfill_cursor_active: true,
        });
    }

    if historical_backfill_enabled && !address.etherscan_history_checkpoint_verified {
        return Ok(EtherscanFetchInitialization {
            range_start_block: 0,
            current_end_block: chain_tip_u64,
            backfill_cursor_active: false,
        });
    }

    let range_start_block = match address.last_tip_height {
        Some(height) if height.value() > 0 => {
            u64::try_from(height.value() - 1_i64).unwrap_or(0_u64)
        }
        _ => 0_u64,
    };
    Ok(EtherscanFetchInitialization {
        range_start_block,
        current_end_block: chain_tip_u64,
        backfill_cursor_active: false,
    })
}

impl EtherscanAddressSyncIntegration {
    pub(crate) const fn new() -> Self {
        Self {
            iteration_state: None,
        }
    }

    fn ensure_initialized(
        &mut self,
        context: &IntegrationIterationContext<'_>,
        chain_tip_height: ChainTipHeight,
    ) -> Result<&mut EtherscanIterationState, UserTransactionMonitorError> {
        if self.iteration_state.is_none() {
            let address = context.address;

            if !matches!(address.network, Network::Mainnet | Network::Testnet) {
                return Err(UserTransactionMonitorError::UnsupportedEthereumNetwork(
                    address.network,
                ));
            }

            let api_key = context
                .clients
                .etherscan_api_key
                .ok_or(UserTransactionMonitorError::MissingEtherscanApiKey)?;
            let tracked_raw = RawEthAddress::new(address.address.as_str().to_string());
            let tracked_address = EthAddress::parse(&tracked_raw).map_err(|err| {
                UserTransactionMonitorError::Parse(format!("ethereum address parse error: {err}"))
            })?;

            let client = build_etherscan_client(
                context.run.user_id,
                api_key,
                address.network,
                context.clients.etherscan_base_url,
                context.clients.http_counters,
            )?;

            let chain_tip_u64 = u64::try_from(chain_tip_height.value()).map_err(|_| {
                UserTransactionMonitorError::Parse(format!(
                    "chain tip exceeds supported range: {}",
                    chain_tip_height.value()
                ))
            })?;
            let fetch_initialization = etherscan_fetch_initialization(
                address,
                chain_tip_u64,
                context.historical_backfill_enabled,
            )?;
            let range_start_block = fetch_initialization.range_start_block;
            let current_end_block = fetch_initialization.current_end_block;

            if range_start_block > current_end_block {
                return Err(UserTransactionMonitorError::Parse(format!(
                    "etherscan range start block {range_start_block} exceeds end block {current_end_block}"
                )));
            }

            self.iteration_state = Some(EtherscanIterationState {
                client,
                tracked_address,
                range_start_block,
                current_end_block,
                chain_tip_u64,
                backfill_cursor_active: fetch_initialization.backfill_cursor_active,
                run_summary: EtherscanRunSummary {
                    backfill_active: context.is_backfill_active
                        && context.historical_backfill_enabled,
                    requested_start_block: range_start_block,
                    requested_end_block: current_end_block,
                    ..EtherscanRunSummary::default()
                },
                fetched_normal_count: 0,
                done: false,
            });
        }
        self.iteration_state.as_mut().ok_or_else(|| {
            UserTransactionMonitorError::Parse(
                "etherscan iteration state not initialized".to_string(),
            )
        })
    }
}

impl AddressSyncIntegration for EtherscanAddressSyncIntegration {
    fn sync_plan(
        &self,
        address: &SyncAddress,
        _allow_known_confirmed_early_exit: bool,
    ) -> Result<IntegrationSyncPlan, UserTransactionMonitorError> {
        Ok(IntegrationSyncPlan {
            is_backfill_active: is_first_sync(address.last_tip_height)
                || address.etherscan_backfill_end_block.is_some()
                || !address.etherscan_history_checkpoint_verified,
        })
    }

    fn estimate_first_sync_tx_count(
        &self,
        context: IntegrationEstimateContext<'_>,
    ) -> Result<Option<TxCountEstimate>, UserTransactionMonitorError> {
        let Some(api_key) = context.clients.etherscan_api_key else {
            return Ok(None);
        };
        let client = build_etherscan_client(
            context.run.user_id,
            api_key,
            context.address.network,
            context.clients.etherscan_base_url,
            context.clients.http_counters,
        )?;
        crate::db::debug_assert_user_db_unlocked(
            context.run.user_id,
            "etherscan tx-count estimate fetch",
        );
        let estimate = client.quick_estimate_tx_count(context.address.address.as_str())?;
        Ok(Some(estimate))
    }

    fn unfinished_backfill_state(&self, address: &SyncAddress) -> Option<AddressBackfillState> {
        address.etherscan_backfill_end_block.map(|end_block| {
            AddressBackfillState::new(AddressBackfillCursor::Etherscan { end_block }, None)
        })
    }

    fn sync_one_iteration(
        &mut self,
        context: IntegrationIterationContext<'_>,
    ) -> Result<SyncIterationResult, UserTransactionMonitorError> {
        tracing::trace!(
            provider = ?SyncProviderId::Etherscan,
            address_id = %context.address.address_id,
            synced_at = %context.now_utc,
            sync_instant = ?context.now_instant,
            "sync integration: dispatching etherscan address sync"
        );
        let chain_tip_height = context.chain_tip.ok_or_else(|| {
            UserTransactionMonitorError::Parse(
                "etherscan integration requires a chain tip height".to_string(),
            )
        })?;

        {
            let state = self.ensure_initialized(&context, chain_tip_height)?;
            if state.done {
                let raw_run_summary_json = Some(state.run_summary.to_summary_json()?);
                self.reset_iteration_state();
                return Ok(SyncIterationResult {
                    raw_run_summary_json,
                    ..SyncIterationResult::exhausted(chain_tip_height, context.run.clock.utc_now())
                });
            }
        }

        if !context.historical_backfill_enabled {
            let state = self.iteration_state.as_mut().ok_or_else(|| {
                UserTransactionMonitorError::Parse(
                    "etherscan iteration state not initialized".to_string(),
                )
            })?;
            let api_confirmed_balance = fetch_etherscan_api_confirmed_balance(state, &context)?;
            self.reset_iteration_state();
            return Ok(SyncIterationResult {
                new_tx_count: TransactionCount::zero(),
                updated_tx_count: TransactionCount::zero(),
                coverage_invalidation: crate::db::CoverageInvalidationTargets::default(),
                tip_height: chain_tip_height,
                completed_at: context.run.clock.utc_now(),
                has_more_work: false,
                early_exited: false,
                observed_activity: false,
                ledger_rebuild_required: false,
                raw_run_summary_json: Some(EtherscanRunSummary::default().to_summary_json()?),
                api_confirmed_balance: Some(api_confirmed_balance),
            });
        }
        let state = self.iteration_state.as_mut().ok_or_else(|| {
            UserTransactionMonitorError::Parse(
                "etherscan iteration state not initialized".to_string(),
            )
        })?;
        let result = run_etherscan_iteration(state, &context)?;
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

/// Execute one etherscan window iteration: fetch normal + internal, map, reconcile.
fn run_etherscan_iteration(
    state: &mut EtherscanIterationState,
    context: &IntegrationIterationContext<'_>,
) -> Result<SyncIterationResult, UserTransactionMonitorError> {
    let run = context.run;
    let address = context.address;
    let normalized_address = state.tracked_address.normalized();

    let persistence_context = EtherscanPersistenceContext {
        run,
        raw_sync_run_id: context.raw_sync_run_id,
        source_connection_id: context.source_connection_id,
        address,
    };

    let mut on_normal_page_fetched = |page_len: usize| {
        let Some(progress) = context.single_address_progress else {
            return;
        };
        let page_len_u32 = u32::try_from(page_len).unwrap_or(u32::MAX);
        state.fetched_normal_count = state.fetched_normal_count.saturating_add(page_len_u32);
        crate::db::debug_assert_user_db_unlocked(run.user_id, "etherscan progress publish");
        let now_utc = run.clock.utc_now();
        let fetched_count = TransactionCount::from_u32(state.fetched_normal_count);
        publish_transaction_sync_event(
            run.user_id,
            TransactionSyncEvent::account_sync_progress_single_address(
                run.run_id,
                now_utc,
                progress.account_id,
                progress.is_first_sync,
                fetched_count,
                progress.expected_tx_count,
                progress.expected_tx_count_is_lower_bound,
            ),
        );
        publish_transaction_sync_event(
            run.user_id,
            TransactionSyncEvent::account_integration_sync_progress_single_address(
                run.run_id,
                now_utc,
                progress.account_id,
                crate::transactions::SyncIntegrationId::Etherscan,
                progress.is_first_sync,
                fetched_count,
                progress.expected_tx_count,
                progress.expected_tx_count_is_lower_bound,
            ),
        );
    };

    let (normal_txs, internal_txs, resume_cursor) = fetch_etherscan_page_set(
        persistence_context,
        &state.client,
        &normalized_address,
        EtherscanFetchRange {
            start_block: state.range_start_block,
            end_block: state.current_end_block,
        },
        &mut state.run_summary,
        &mut on_normal_page_fetched,
    )?;

    let mapped_transactions = map_etherscan_transactions(normal_txs, internal_txs)?;
    let observed_at = run.clock.utc_now();
    let reconcile_summary = reconcile_account_transactions(
        run.user_id,
        address.asset_id,
        address.network,
        &mapped_transactions,
        observed_at,
    )?;

    // Update cursor for next iteration.
    match resume_cursor {
        Some(cursor_block) => state.current_end_block = cursor_block,
        None => state.done = true,
    }

    persist_etherscan_backfill_cursor_transition(
        run.user_id,
        address.address_id,
        &mut state.backfill_cursor_active,
        resume_cursor,
    )?;
    if resume_cursor.is_none() {
        update_address_etherscan_history_status(
            run.user_id,
            address.address_id,
            EtherscanHistoryStatus::Continuous,
        )?;
    }
    set_etherscan_run_summary_backfill_state(
        &mut state.run_summary,
        context.is_backfill_active,
        resume_cursor,
    );

    let has_more_work = !state.done;

    Ok(SyncIterationResult {
        new_tx_count: reconcile_summary.new_tx_count,
        updated_tx_count: reconcile_summary.updated_tx_count,
        coverage_invalidation: reconcile_summary.coverage_invalidation,
        tip_height: ChainTipHeight::try_new(i64::try_from(state.chain_tip_u64).unwrap_or(i64::MAX))
            .unwrap_or_else(|err| match err {
                ChainTipHeightError::Negative(v) => {
                    unreachable!("chain_tip_u64 cast to i64 should be non-negative, got {v}")
                }
            }),
        completed_at: run.clock.utc_now(),
        has_more_work,
        early_exited: false,
        observed_activity: false,
        ledger_rebuild_required: !mapped_transactions.is_empty(),
        raw_run_summary_json: Some(state.run_summary.to_summary_json()?),
        api_confirmed_balance: None,
    })
}

#[derive(Clone, Copy)]
struct EtherscanPersistenceContext<'a> {
    run: RunContext<'a>,
    raw_sync_run_id: SyncRunId,
    source_connection_id: &'a crate::db::raw_ingestion::SourceConnectionId,
    address: &'a SyncAddress,
}

#[derive(Clone, Copy)]
struct EtherscanRequestAttemptContext<'a> {
    run: RunContext<'a>,
    raw_sync_run_id: SyncRunId,
    address: &'a SyncAddress,
    request_kind: EtherscanRequestKind,
}

trait EtherscanTx {
    fn block_number_str(&self) -> &str;
    fn dedup_key(&self) -> String;
}

impl EtherscanTx for EtherscanNormalTx {
    fn block_number_str(&self) -> &str {
        &self.block_number
    }

    fn dedup_key(&self) -> String {
        self.hash.clone()
    }
}

impl EtherscanTx for EtherscanInternalTx {
    fn block_number_str(&self) -> &str {
        &self.block_number
    }

    fn dedup_key(&self) -> String {
        format!("{}:{}", self.hash, self.trace_id)
    }
}

fn dedup_etherscan_txs<T: EtherscanTx>(items: &mut Vec<T>) {
    let mut seen = HashSet::new();
    items.retain(|item| seen.insert(item.dedup_key()));
}

fn fetch_etherscan_page<T, F, M, P>(
    request_context: EtherscanRequestAttemptContext<'_>,
    fetch_page: F,
    request_metadata: M,
    fetch_range: EtherscanFetchRange,
    raw_run_summary: &mut EtherscanRunSummary,
    mut persist_page: P,
) -> Result<(Vec<T>, Option<u64>), UserTransactionMonitorError>
where
    T: EtherscanTx,
    F: Fn(u64, u64, u64, u64) -> Result<EtherscanFetchedPage<T>, EtherscanError>,
    M: Fn(u64, u64, u64, u64) -> Result<EtherscanRequestMetadata, EtherscanError>,
    P: FnMut(
        u32,
        u64,
        u64,
        u64,
        u64,
        EtherscanFetchedPage<T>,
    ) -> Result<IngestedEtherscanPage<T>, UserTransactionMonitorError>,
{
    let attempted_at = request_context.run.clock.utc_now();
    crate::db::debug_assert_user_db_unlocked(request_context.run.user_id, "etherscan page fetch");
    let page_data = match fetch_page(
        fetch_range.start_block,
        fetch_range.end_block,
        1,
        ETHERSCAN_PAGE_SIZE,
    ) {
        Ok(page_data) => page_data,
        Err(error) => {
            raw_run_summary.record_request_failure(&error);
            if let Ok(request) = request_metadata(
                fetch_range.start_block,
                fetch_range.end_block,
                1,
                ETHERSCAN_PAGE_SIZE,
            ) {
                record_etherscan_request_failure(
                    EtherscanRequestFailureRecord {
                        user_id: request_context.run.user_id,
                        raw_sync_run_id: request_context.raw_sync_run_id,
                        scope_address_id: request_context.address.address_id,
                        request_kind: request_context.request_kind,
                        request_metadata: &request,
                        attempted_at,
                    },
                    &error,
                )?;
            }
            return Err(UserTransactionMonitorError::from(error));
        }
    };
    let persisted = persist_page(
        0,
        fetch_range.start_block,
        fetch_range.end_block,
        1,
        ETHERSCAN_PAGE_SIZE,
        page_data,
    )?;
    raw_run_summary.record_page(&persisted.summary);
    let mut transactions = persisted.transactions;
    let cursor = if u64::try_from(transactions.len()).unwrap_or(u64::MAX) < ETHERSCAN_PAGE_SIZE {
        None
    } else {
        Some(etherscan_resume_cursor_from_full_page(
            &transactions,
            fetch_range.end_block,
        )?)
    };
    dedup_etherscan_txs(&mut transactions);
    Ok((transactions, cursor))
}

fn etherscan_resume_cursor_from_full_page<T: EtherscanTx>(
    page_items: &[T],
    current_end: u64,
) -> Result<u64, UserTransactionMonitorError> {
    let last_block_str = page_items
        .last()
        .map(|tx| tx.block_number_str())
        .ok_or_else(|| {
            UserTransactionMonitorError::Parse(
                "etherscan pagination: hit page limit with no results".to_string(),
            )
        })?;
    let last_block = last_block_str.parse::<u64>().map_err(|e| {
        UserTransactionMonitorError::Parse(format!(
            "etherscan pagination: invalid block number '{last_block_str}': {e}"
        ))
    })?;

    if last_block >= current_end {
        return Err(UserTransactionMonitorError::Parse(format!(
            "etherscan pagination: too many transactions in block {last_block} \
             to fetch within one bounded iteration"
        )));
    }

    Ok(last_block)
}

fn fetch_etherscan_page_set(
    context: EtherscanPersistenceContext<'_>,
    client: &EtherscanClient,
    address: &str,
    fetch_range: EtherscanFetchRange,
    raw_run_summary: &mut EtherscanRunSummary,
    on_normal_page_fetched: &mut dyn FnMut(usize),
) -> Result<EtherscanPageSet, UserTransactionMonitorError> {
    let chain_id = EtherscanChainId::try_new(client.chain_id())?;
    let normal_request_context = EtherscanRequestAttemptContext {
        run: context.run,
        raw_sync_run_id: context.raw_sync_run_id,
        address: context.address,
        request_kind: EtherscanRequestKind::NormalTransactionsPage,
    };
    let (normal, normal_cursor) = fetch_etherscan_page(
        normal_request_context,
        |sb, eb, page, offset| client.fetch_normal_transactions_page(address, sb, eb, page, offset),
        |sb, eb, page, offset| {
            client.normal_transactions_request_metadata(address, sb, eb, page, offset)
        },
        fetch_range,
        raw_run_summary,
        |_group_index, _sb, _eb, _page_number, _page_size, page| {
            let persisted = ingest_etherscan_normal_page(
                EtherscanPageIngestionRequest {
                    user_id: context.run.user_id,
                    raw_sync_run_id: context.raw_sync_run_id,
                    source_connection_id: context.source_connection_id,
                    chain_id,
                    network: context.address.network,
                    observed_at: context.run.clock.utc_now(),
                },
                page,
            )?;
            if !persisted.transactions.is_empty() {
                on_normal_page_fetched(persisted.transactions.len());
            }
            Ok(persisted)
        },
    )?;

    let internal_request_context = EtherscanRequestAttemptContext {
        run: context.run,
        raw_sync_run_id: context.raw_sync_run_id,
        address: context.address,
        request_kind: EtherscanRequestKind::InternalTransactionsPage,
    };
    let (internal, internal_cursor) = fetch_etherscan_page(
        internal_request_context,
        |sb, eb, page, offset| {
            client.fetch_internal_transactions_page(address, sb, eb, page, offset)
        },
        |sb, eb, page, offset| {
            client.internal_transactions_request_metadata(address, sb, eb, page, offset)
        },
        fetch_range,
        raw_run_summary,
        |_group_index, _sb, _eb, _page_number, _page_size, page| {
            ingest_etherscan_internal_page(
                EtherscanPageIngestionRequest {
                    user_id: context.run.user_id,
                    raw_sync_run_id: context.raw_sync_run_id,
                    source_connection_id: context.source_connection_id,
                    chain_id,
                    network: context.address.network,
                    observed_at: context.run.clock.utc_now(),
                },
                page,
            )
        },
    )?;

    let resume_cursor = align_recent_first_resume_cursor(normal_cursor, internal_cursor);

    Ok((normal, internal, resume_cursor))
}

fn align_recent_first_resume_cursor(
    normal_cursor: Option<u64>,
    internal_cursor: Option<u64>,
) -> Option<u64> {
    match (normal_cursor, internal_cursor) {
        (Some(n), Some(i)) => Some(n.max(i)),
        (Some(n), None) => Some(n),
        (None, Some(i)) => Some(i),
        (None, None) => None,
    }
}

fn network_to_etherscan(network: Network) -> Result<EtherscanNetwork, UserTransactionMonitorError> {
    match network {
        Network::Mainnet => Ok(EtherscanNetwork::EthereumMainnet),
        Network::Testnet => Ok(EtherscanNetwork::Sepolia),
        _ => Err(UserTransactionMonitorError::UnsupportedEthereumNetwork(
            network,
        )),
    }
}

pub(crate) fn build_etherscan_client(
    user_id: UserId,
    api_key: &RawEtherscanApiKey,
    network: Network,
    base_url_override: Option<&EtherscanBaseUrl>,
    http_counters: &crate::tasks::jobs::sync::SyncHttpCounters,
) -> Result<EtherscanClient, UserTransactionMonitorError> {
    let etherscan_network = network_to_etherscan(network)?;
    let base_url = match base_url_override {
        Some(url) => url.as_str(),
        None => etherscan_network.base_url(),
    };
    let client = TracedBlockingClient::builder(IntegrationLabel::new(LABEL_ETHERSCAN), user_id)
        .configure(|builder| {
            builder.timeout(Duration::from_secs(ETHERSCAN_REQUEST_TIMEOUT_SECONDS))
        })
        .redact_query_params(&["apikey"])
        .redact_headers(&["authorization"])
        .build()
        .map_err(|err| {
            UserTransactionMonitorError::Http(format!(
                "failed to build etherscan HTTP client: {err}"
            ))
        })?;
    Ok(EtherscanClient::new(
        client,
        api_key.as_str(),
        base_url,
        etherscan_network.chain_id(),
    )
    .with_total_api_call_counter(http_counters.total_api_calls_counter()))
}

pub(crate) fn fetch_ethereum_chain_tip_height(
    user_id: UserId,
    network: Network,
    etherscan_api_key: Option<&RawEtherscanApiKey>,
    etherscan_base_url: Option<&EtherscanBaseUrl>,
    http_counters: &crate::tasks::jobs::sync::SyncHttpCounters,
) -> Result<ChainTipHeight, UserTransactionMonitorError> {
    let api_key = etherscan_api_key.ok_or(UserTransactionMonitorError::MissingEtherscanApiKey)?;
    let client =
        build_etherscan_client(user_id, api_key, network, etherscan_base_url, http_counters)?;
    let chain_tip_u64 = client
        .fetch_block_number()
        .map_err(UserTransactionMonitorError::from)?;
    let chain_tip_i64 = i64::try_from(chain_tip_u64).map_err(|_| {
        UserTransactionMonitorError::Parse(format!(
            "chain tip exceeds supported range: {chain_tip_u64}"
        ))
    })?;
    ChainTipHeight::try_new(chain_tip_i64).map_err(|err| {
        UserTransactionMonitorError::Parse(format!("invalid chain tip height: {err}"))
    })
}

fn etherscan_backfill_cursor_update(
    backfill_cursor_active: bool,
    resume_cursor: Option<u64>,
) -> Result<Option<Option<EthereumBlockNumber>>, UserTransactionMonitorError> {
    match resume_cursor {
        Some(cursor) => EthereumBlockNumber::from_u64(cursor)
            .map(|end_block| Some(Some(end_block)))
            .map_err(|err| {
                UserTransactionMonitorError::Parse(format!(
                    "etherscan backfill resume cursor out of range: {err}"
                ))
            }),
        None if backfill_cursor_active => Ok(Some(None)),
        None => Ok(None),
    }
}

fn persist_etherscan_backfill_cursor_transition(
    user_id: UserId,
    address_id: crate::wallets::DigitalAssetAddressId,
    backfill_cursor_active: &mut bool,
    resume_cursor: Option<u64>,
) -> Result<(), UserTransactionMonitorError> {
    let Some(cursor_update) =
        etherscan_backfill_cursor_update(*backfill_cursor_active, resume_cursor)?
    else {
        return Ok(());
    };

    update_address_etherscan_backfill_cursor(user_id, address_id, cursor_update)?;
    *backfill_cursor_active = cursor_update.is_some();

    if let Some(cursor) = resume_cursor {
        tracing::debug!(
            user_id = %user_id,
            address_id = %address_id,
            resume_block = cursor,
            "transactions sync: ethereum backfill cursor updated"
        );
    }

    Ok(())
}

fn fetch_etherscan_api_confirmed_balance(
    state: &mut EtherscanIterationState,
    context: &IntegrationIterationContext<'_>,
) -> Result<ApiConfirmedBalance, UserTransactionMonitorError> {
    let normalized_address = state.tracked_address.normalized();
    let attempted_at = context.run.clock.utc_now();
    crate::db::debug_assert_user_db_unlocked(context.run.user_id, "etherscan balance fetch");

    match state.client.fetch_native_balance(&normalized_address) {
        Ok(balance) => Ok(balance),
        Err(error) => {
            state.run_summary.record_request_failure(&error);
            if let Ok(request_metadata) = state
                .client
                .native_balance_request_metadata(&normalized_address)
            {
                record_etherscan_request_failure(
                    EtherscanRequestFailureRecord {
                        user_id: context.run.user_id,
                        raw_sync_run_id: context.raw_sync_run_id,
                        scope_address_id: context.address.address_id,
                        request_kind: EtherscanRequestKind::NativeBalance,
                        request_metadata: &request_metadata,
                        attempted_at,
                    },
                    &error,
                )?;
            }
            Err(error.into())
        }
    }
}

fn set_etherscan_run_summary_backfill_state(
    run_summary: &mut EtherscanRunSummary,
    is_backfill_active: bool,
    resume_cursor: Option<u64>,
) {
    run_summary.resume_cursor_end_block = resume_cursor;
    run_summary.backfill_budget_exhausted = resume_cursor.is_some();
    run_summary.backfill_complete = is_backfill_active && resume_cursor.is_none();
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod pure_tests {
    use super::*;
    use crate::transactions::TrackedAddress;
    use crate::wallets::{DigitalAssetAddressId, SyncedAssetId};

    fn dummy_sync_address() -> SyncAddress {
        SyncAddress {
            address_id: DigitalAssetAddressId::new(),
            address: TrackedAddress::parse("0x1111111111111111111111111111111111111111")
                .expect("test address should parse"),
            asset_id: SyncedAssetId::Ethereum,
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

    fn make_normal_tx(hash: &str, block_number: &str) -> EtherscanNormalTx {
        EtherscanNormalTx {
            hash: hash.to_string(),
            block_number: block_number.to_string(),
            time_stamp: "1609459200".to_string(),
            from: "0x1111111111111111111111111111111111111111".to_string(),
            to: "0x2222222222222222222222222222222222222222".to_string(),
            value: "0".to_string(),
            gas_price: "0".to_string(),
            gas_used: "0".to_string(),
            is_error: "0".to_string(),
            txreceipt_status: "1".to_string(),
            nonce: "0".to_string(),
        }
    }

    fn make_internal_tx(hash: &str, trace_id: &str, block_number: &str) -> EtherscanInternalTx {
        EtherscanInternalTx {
            hash: hash.to_string(),
            block_number: block_number.to_string(),
            time_stamp: "1609459200".to_string(),
            from: "0x1111111111111111111111111111111111111111".to_string(),
            to: "0x2222222222222222222222222222222222222222".to_string(),
            value: "0".to_string(),
            is_error: "0".to_string(),
            call_type: "call".to_string(),
            trace_id: trace_id.to_string(),
        }
    }

    #[test]
    fn sync_plan_marks_backfill_when_resume_cursor_exists() {
        let integration = EtherscanAddressSyncIntegration::new();
        let mut address = dummy_sync_address();
        address.etherscan_backfill_end_block =
            Some(EthereumBlockNumber::try_new(42).expect("block should be valid"));

        let plan = integration
            .sync_plan(&address, true)
            .expect("sync plan should compute");

        assert!(plan.is_backfill_active);
    }

    #[test]
    fn free_etherscan_fetch_initialization_ignores_persisted_backfill_cursor() {
        let mut address = dummy_sync_address();
        address.last_tip_height = Some(ChainTipHeight::try_new(100).expect("tip should be valid"));
        address.etherscan_backfill_end_block =
            Some(EthereumBlockNumber::try_new(42).expect("block should be valid"));

        let initialization = etherscan_fetch_initialization(&address, 150, false)
            .expect("fetch initialization should compute");

        assert_eq!(
            initialization,
            EtherscanFetchInitialization {
                range_start_block: 99,
                current_end_block: 150,
                backfill_cursor_active: false,
            }
        );
    }

    #[test]
    fn paid_etherscan_fetch_initialization_uses_persisted_backfill_cursor() {
        let mut address = dummy_sync_address();
        address.last_tip_height = Some(ChainTipHeight::try_new(100).expect("tip should be valid"));
        address.etherscan_backfill_end_block =
            Some(EthereumBlockNumber::try_new(42).expect("block should be valid"));

        let initialization = etherscan_fetch_initialization(&address, 150, true)
            .expect("fetch initialization should compute");

        assert_eq!(
            initialization,
            EtherscanFetchInitialization {
                range_start_block: 0,
                current_end_block: 42,
                backfill_cursor_active: true,
            }
        );
    }

    #[test]
    fn paid_etherscan_fetch_initialization_backfills_after_balance_only_sync() {
        let mut address = dummy_sync_address();
        address.last_tip_height = Some(ChainTipHeight::try_new(100).expect("tip should be valid"));

        let plan = EtherscanAddressSyncIntegration::new()
            .sync_plan(&address, true)
            .expect("sync plan should compute");
        assert!(plan.is_backfill_active);

        let initialization = etherscan_fetch_initialization(&address, 150, true)
            .expect("fetch initialization should compute");

        assert_eq!(
            initialization,
            EtherscanFetchInitialization {
                range_start_block: 0,
                current_end_block: 150,
                backfill_cursor_active: false,
            }
        );
    }

    #[test]
    fn paid_etherscan_fetch_initialization_uses_tip_after_verified_backfill() {
        let mut address = dummy_sync_address();
        address.last_tip_height = Some(ChainTipHeight::try_new(100).expect("tip should be valid"));
        address.etherscan_history_checkpoint_verified = true;

        let plan = EtherscanAddressSyncIntegration::new()
            .sync_plan(&address, true)
            .expect("sync plan should compute");
        assert!(!plan.is_backfill_active);

        let initialization = etherscan_fetch_initialization(&address, 150, true)
            .expect("fetch initialization should compute");
        assert_eq!(
            initialization,
            EtherscanFetchInitialization {
                range_start_block: 99,
                current_end_block: 150,
                backfill_cursor_active: false,
            }
        );
    }

    #[test]
    fn etherscan_backfill_cursor_update_saves_resume_cursor() {
        let saved_cursor = etherscan_backfill_cursor_update(true, Some(123))
            .expect("cursor update should succeed");
        assert_eq!(
            saved_cursor,
            Some(Some(
                EthereumBlockNumber::try_new(123).expect("block should be valid")
            ))
        );
        let result = etherscan_backfill_cursor_update(false, Some(u64::MAX));
        assert!(matches!(
            result,
            Err(UserTransactionMonitorError::Parse(message))
                if message
                    == "etherscan backfill resume cursor out of range: ethereum block number exceeds supported range: 18446744073709551615"
        ));
    }

    #[test]
    fn etherscan_backfill_cursor_update_clears_only_when_a_cursor_was_active() {
        assert_eq!(
            etherscan_backfill_cursor_update(true, None).expect("cursor clear should succeed"),
            Some(None)
        );
        assert_eq!(
            etherscan_backfill_cursor_update(false, None)
                .expect("no-op cursor update should succeed"),
            None
        );
    }

    #[test]
    fn persist_etherscan_backfill_cursor_transition_tracks_cursor_set_and_clear() {
        let mut backfill_cursor_active = false;
        let saved_cursor = EthereumBlockNumber::try_new(123).expect("block should be valid");
        let cursor_update = etherscan_backfill_cursor_update(backfill_cursor_active, Some(123))
            .expect("cursor update should succeed")
            .expect("cursor should be saved");
        backfill_cursor_active = cursor_update.is_some();
        assert_eq!(cursor_update, Some(saved_cursor));
        assert!(backfill_cursor_active);

        let clear_update = etherscan_backfill_cursor_update(backfill_cursor_active, None)
            .expect("cursor clear should succeed");
        assert_eq!(clear_update, Some(None));
    }

    #[test]
    fn dedup_normal_txs_removes_duplicates_by_hash() {
        let mut txs = vec![
            make_normal_tx("0xaaa", "100"),
            make_normal_tx("0xbbb", "100"),
            make_normal_tx("0xaaa", "100"),
            make_normal_tx("0xccc", "200"),
        ];

        dedup_etherscan_txs(&mut txs);

        let hashes: Vec<&str> = txs.iter().map(|tx| tx.hash.as_str()).collect();
        assert_eq!(hashes, vec!["0xaaa", "0xbbb", "0xccc"]);
    }

    #[test]
    fn dedup_internal_txs_keeps_distinct_traces_same_hash() {
        let mut txs = vec![
            make_internal_tx("0xaaa", "0", "100"),
            make_internal_tx("0xaaa", "1", "100"),
            make_internal_tx("0xaaa", "0", "100"),
            make_internal_tx("0xbbb", "0", "200"),
        ];

        dedup_etherscan_txs(&mut txs);

        let keys: Vec<String> = txs
            .iter()
            .map(|tx| format!("{}:{}", tx.hash, tx.trace_id))
            .collect();
        assert_eq!(keys, vec!["0xaaa:0", "0xaaa:1", "0xbbb:0"]);
    }

    #[test]
    fn align_recent_first_resume_cursor_uses_safer_newer_boundary() {
        assert_eq!(
            align_recent_first_resume_cursor(Some(100), Some(200)),
            Some(200)
        );
        assert_eq!(align_recent_first_resume_cursor(Some(100), None), Some(100));
        assert_eq!(align_recent_first_resume_cursor(None, Some(200)), Some(200));
        assert_eq!(align_recent_first_resume_cursor(None, None), None);
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;
    use crate::db::SyncAddress;
    use crate::db::raw_ingestion::{
        IntegrationKind as RawIntegrationKind, StartSyncRunRequest, SyncRunTriggerKind,
        start_sync_run,
    };
    use crate::db::{
        AddressSyncSuccess, DbError, acquire_test_runtime, get_non_hd_sync_addresses,
        mark_address_sync_completed_success, mark_address_sync_started,
        persist_sync_address_fixture, setup_test_user, unique_user_id, with_user_db,
    };
    use crate::models::{EtherscanBaseUrl, RawEtherscanApiKey};
    use crate::tasks::TriggerSource;
    use crate::tasks::jobs::sync::{SyncClients, SyncClock, SyncHttpCounters};
    use crate::traces::client::{IntegrationLabel, TracedBlockingClient};
    use crate::transactions::{TrackedAddress, TransactionSyncRunId};
    use crate::wallets::{DigitalAssetAddressId, SyncedAssetId};
    use chrono::{DateTime, TimeZone, Utc};
    use serde_json::Value;
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    struct FixedClock {
        now_utc: DateTime<Utc>,
        now_instant: Instant,
    }

    impl FixedClock {
        fn new(now_utc: DateTime<Utc>) -> Self {
            Self {
                now_utc,
                now_instant: Instant::now(),
            }
        }
    }

    impl SyncClock for FixedClock {
        fn utc_now(&self) -> DateTime<Utc> {
            self.now_utc
        }

        fn instant_now(&self) -> Instant {
            self.now_instant
        }

        fn sleep(&self, _duration: Duration) {}
    }

    fn test_now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, 13, 12, 0, 0)
            .single()
            .expect("fixed test time should be valid")
    }

    fn dummy_sync_address() -> SyncAddress {
        SyncAddress {
            address_id: DigitalAssetAddressId::new(),
            address: TrackedAddress::parse("0x1111111111111111111111111111111111111111")
                .expect("test address should parse"),
            asset_id: SyncedAssetId::Ethereum,
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

    fn make_run_context<'a>(clock: &'a FixedClock) -> RunContext<'a> {
        RunContext {
            user_id: UserId::new(),
            run_id: TransactionSyncRunId::new(),
            source: TriggerSource::Schedule,
            started_at: clock.utc_now(),
            clock,
        }
    }

    fn make_run_context_for_user<'a>(clock: &'a FixedClock, user_id: UserId) -> RunContext<'a> {
        RunContext {
            user_id,
            run_id: TransactionSyncRunId::new(),
            source: TriggerSource::Schedule,
            started_at: clock.utc_now(),
            clock,
        }
    }

    fn make_request_context<'a>(
        run: RunContext<'a>,
        address: &'a SyncAddress,
    ) -> EtherscanRequestAttemptContext<'a> {
        EtherscanRequestAttemptContext {
            run,
            raw_sync_run_id: SyncRunId::new(),
            address,
            request_kind: EtherscanRequestKind::NormalTransactionsPage,
        }
    }

    fn start_etherscan_sync_run(
        user_id: UserId,
        address_id: DigitalAssetAddressId,
        started_at: DateTime<Utc>,
    ) -> crate::db::raw_ingestion::StartedSyncRun {
        start_sync_run(
            user_id,
            StartSyncRunRequest {
                integration: RawIntegrationKind::Etherscan,
                scope_kind: crate::db::raw_ingestion::SyncRunScopeKind::Address,
                scope_address_id: address_id,
                asset_id: SyncedAssetId::Ethereum,
                network: Network::Mainnet,
                trigger_kind: SyncRunTriggerKind::Manual,
                started_at,
                summary_json: None,
            },
        )
        .expect("etherscan sync run should insert")
    }

    fn collect_files(root: &std::path::Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let entries = match fs::read_dir(root) {
            Ok(entries) => entries,
            Err(_) => return files,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(collect_files(&path));
            } else {
                files.push(path);
            }
        }
        files
    }

    struct SingleResponseServer {
        base_url: String,
        handle: JoinHandle<()>,
    }

    impl SingleResponseServer {
        fn join(self) {
            self.handle
                .join()
                .expect("single-response etherscan server thread should finish");
        }
    }

    fn start_single_response_etherscan_server(response_body: &'static str) -> SingleResponseServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("test etherscan server should bind");
        let base_url = format!(
            "http://{}/v2/api",
            listener
                .local_addr()
                .expect("test etherscan server local address should load")
        );
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener
                .accept()
                .expect("test etherscan server should accept one request");
            let mut buffer = [0_u8; 4096];
            let read = stream
                .read(&mut buffer)
                .expect("test etherscan server should read request");
            let request = String::from_utf8_lossy(&buffer[..read]);
            assert!(
                request.contains("action=balance"),
                "expected native balance request, got: {request}"
            );
            assert!(
                request.contains("apikey=test-api-key"),
                "balance request should include API key on the wire"
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            stream
                .write_all(response.as_bytes())
                .expect("test etherscan server should write response");
        });

        SingleResponseServer { base_url, handle }
    }

    #[test]
    fn etherscan_balance_fetch_result_persists_as_api_confirmed_balance() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let address = dummy_sync_address();
        let now = test_now();
        persist_sync_address_fixture(user_id, &address, now)
            .expect("sync address fixture should persist");
        let run_id = TransactionSyncRunId::new();
        mark_address_sync_started(user_id, address.address_id, run_id, now)
            .expect("sync state row should exist");
        let raw_sync_run = start_etherscan_sync_run(user_id, address.address_id, now);

        let server = start_single_response_etherscan_server(
            r#"{"status":"1","message":"OK","result":"321000"}"#,
        );
        let base_url = EtherscanBaseUrl::parse(&server.base_url)
            .expect("test etherscan base URL should parse");
        let api_key = RawEtherscanApiKey::new("test-api-key".to_string());
        let http_counters = SyncHttpCounters::new();
        let traced_client =
            TracedBlockingClient::builder(IntegrationLabel::new(LABEL_ETHERSCAN), user_id)
                .configure(|builder| builder.timeout(Duration::from_secs(2)))
                .redact_query_params(&["apikey"])
                .redact_headers(&["authorization"])
                .build_for_tests_with_tracing(false)
                .expect("traced blocking client should build");
        let client = EtherscanClient::new(
            traced_client,
            api_key.as_str(),
            base_url.as_str(),
            EtherscanNetwork::EthereumMainnet.chain_id(),
        )
        .with_total_api_call_counter(http_counters.total_api_calls_counter());
        let tracked_address =
            EthAddress::parse(&RawEthAddress::new(address.address.as_str().to_string()))
                .expect("test address should parse");
        let mut state = EtherscanIterationState {
            client,
            tracked_address,
            range_start_block: 0,
            current_end_block: 1,
            chain_tip_u64: 1,
            backfill_cursor_active: false,
            run_summary: EtherscanRunSummary::default(),
            fetched_normal_count: 0,
            done: false,
        };
        let clock = FixedClock::new(now);
        let run = make_run_context_for_user(&clock, user_id);
        let clients = SyncClients {
            mempool_client: None,
            etherscan_api_key: Some(&api_key),
            etherscan_base_url: Some(&base_url),
            http_counters: &http_counters,
        };
        let context = IntegrationIterationContext {
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
            is_backfill_active: false,
            historical_backfill_enabled: true,
            legacy_mempool_history_repair: false,
            mempool_history_page_frontier: None,
        };

        let balance = fetch_etherscan_api_confirmed_balance(&mut state, &context)
            .expect("balance fetch should succeed");
        server.join();
        assert_eq!(
            balance,
            ApiConfirmedBalance::from_smallest_unit_i64(321_000)
                .expect("test balance should parse")
        );
        mark_address_sync_completed_success(
            user_id,
            &AddressSyncSuccess {
                address_id: address.address_id,
                run_id,
                started_at: now,
                completed_at: now,
                last_tip_height: ChainTipHeight::try_new(1).expect("tip should be valid"),
                new_tx_count: TransactionCount::zero(),
                updated_tx_count: TransactionCount::zero(),
                api_confirmed_balance: Some(balance),
            },
        )
        .expect("balance sync success should persist");

        let stored_balance: (Option<i64>, Option<i64>) =
            with_user_db(user_id, |conn| -> Result<_, DbError> {
                conn.query_row(
                    "SELECT api_confirmed_balance_hi, api_confirmed_balance_lo
                     FROM transaction_sync_state
                     WHERE address_id = ?1",
                    [address.address_id.to_string()],
                    |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
                )
                .map_err(|err| {
                    DbError::from_rusqlite_error(
                        "Failed to load persisted api_confirmed_balance",
                        err,
                    )
                })
            })
            .expect("persisted sync state should load");
        assert_eq!(stored_balance, (Some(0), Some(321_000)));
    }

    fn make_normal_tx(hash: &str, block_number: &str) -> EtherscanNormalTx {
        EtherscanNormalTx {
            hash: hash.to_string(),
            block_number: block_number.to_string(),
            time_stamp: "1609459200".to_string(),
            from: "0x1111111111111111111111111111111111111111".to_string(),
            to: "0x2222222222222222222222222222222222222222".to_string(),
            value: "0".to_string(),
            gas_price: "0".to_string(),
            gas_used: "0".to_string(),
            is_error: "0".to_string(),
            txreceipt_status: "1".to_string(),
            nonce: "0".to_string(),
        }
    }

    fn make_full_etherscan_page(
        page_num: u64,
        count: u64,
        block: &str,
    ) -> EtherscanFetchedPage<EtherscanNormalTx> {
        use crate::integrations::etherscan::{EtherscanFetchedItem, EtherscanRequestMetadata};

        EtherscanFetchedPage {
            request: EtherscanRequestMetadata {
                request_url_without_api_key: String::new(),
                request_query_json: String::new(),
            },
            items: (0..count)
                .map(|i| EtherscanFetchedItem {
                    parsed: make_normal_tx(&format!("0x{page_num:04x}{i:04x}"), block),
                    raw_json_bytes: Vec::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn persist_etherscan_backfill_cursor_transition_clears_active_cursor() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let mut address = dummy_sync_address();
        address.etherscan_backfill_end_block =
            Some(EthereumBlockNumber::try_new(42).expect("block should be valid"));
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

        let mut backfill_cursor_active = true;
        persist_etherscan_backfill_cursor_transition(
            user_id,
            address.address_id,
            &mut backfill_cursor_active,
            None,
        )
        .expect("cursor transition should clear persisted cursor");

        assert!(!backfill_cursor_active);
        let persisted_address = get_non_hd_sync_addresses(user_id)
            .expect("sync addresses should load")
            .into_iter()
            .find(|candidate| candidate.address_id == address.address_id)
            .expect("persisted sync address should exist");
        assert_eq!(persisted_address.etherscan_backfill_end_block, None);
    }

    #[test]
    fn fetch_etherscan_page_full_page_returns_resume_cursor() {
        let clock = FixedClock::new(test_now());
        let run = make_run_context(&clock);
        let address = dummy_sync_address();
        let request_context = make_request_context(run, &address);
        let fetch_calls = std::cell::RefCell::new(Vec::new());

        let fetch_page = |sb, eb, page_num, page_size| {
            fetch_calls.borrow_mut().push((sb, eb, page_num, page_size));
            Ok(make_full_etherscan_page(page_num, page_size, "100"))
        };
        let request_metadata = |sb, eb, page, offset| {
            Ok(EtherscanRequestMetadata {
                request_url_without_api_key: format!("https://fake/{sb}/{eb}/{page}/{offset}"),
                request_query_json: String::new(),
            })
        };
        let mut raw_run_summary = EtherscanRunSummary::default();
        let mut persist_page =
            |_group_index: u32,
             _sb,
             _eb,
             _page,
             _ps,
             page_data: EtherscanFetchedPage<EtherscanNormalTx>| {
                Ok(IngestedEtherscanPage {
                    transactions: page_data
                        .items
                        .into_iter()
                        .map(|item| item.parsed)
                        .collect::<Vec<_>>(),
                    summary: EtherscanPageIngestionSummary::default(),
                })
            };

        let (txs, cursor) = fetch_etherscan_page(
            request_context,
            fetch_page,
            request_metadata,
            EtherscanFetchRange {
                start_block: 0,
                end_block: 999,
            },
            &mut raw_run_summary,
            &mut persist_page,
        )
        .expect("full page fetch should succeed");

        assert_eq!(cursor, Some(100_u64));
        assert_eq!(txs.len(), 1_000);
        assert_eq!(fetch_calls.into_inner(), vec![(0, 999, 1, 1_000)]);
    }

    #[test]
    fn fetch_etherscan_page_partial_page_completes_without_cursor() {
        let clock = FixedClock::new(test_now());
        let run = make_run_context(&clock);
        let address = dummy_sync_address();
        let request_context = make_request_context(run, &address);
        let fetch_calls = std::cell::RefCell::new(Vec::new());
        let fetch_page = |start_block, end_block, page, page_size| {
            fetch_calls
                .borrow_mut()
                .push((start_block, end_block, page, page_size));
            Ok(make_full_etherscan_page(page, 5, "50"))
        };
        let request_metadata = |sb, eb, page, offset| {
            Ok(EtherscanRequestMetadata {
                request_url_without_api_key: format!("https://fake/{sb}/{eb}/{page}/{offset}"),
                request_query_json: String::new(),
            })
        };
        let mut summary = EtherscanRunSummary::default();
        let (transactions, cursor) = fetch_etherscan_page(
            request_context,
            fetch_page,
            request_metadata,
            EtherscanFetchRange {
                start_block: 0,
                end_block: 999,
            },
            &mut summary,
            |_group_index, _start, _end, _page, _size, page| {
                Ok(IngestedEtherscanPage {
                    transactions: page.items.into_iter().map(|item| item.parsed).collect(),
                    summary: EtherscanPageIngestionSummary::default(),
                })
            },
        )
        .expect("partial page should succeed");

        assert_eq!(transactions.len(), 5);
        assert_eq!(cursor, None);
        assert_eq!(fetch_calls.into_inner(), vec![(0, 999, 1, 1_000)]);
    }

    #[test]
    fn fetch_etherscan_page_persists_transport_failure_row_and_har_trace() {
        let runtime = acquire_test_runtime().expect("test runtime should initialize");
        let project_dir = runtime.runtime_context().project_dir().to_path_buf();
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let now = test_now();
        let address = dummy_sync_address();
        persist_sync_address_fixture(user_id, &address, now)
            .expect("sync address fixture should persist");
        let sync_run = start_etherscan_sync_run(user_id, address.address_id, now);

        let clock = FixedClock::new(now);
        let run = make_run_context_for_user(&clock, user_id);
        let request_context = make_request_context(run, &address);
        let port = 0_u16;
        let base_url = EtherscanBaseUrl::parse(&format!("http://127.0.0.1:{port}/v2/api"))
            .expect("local etherscan base URL should parse");
        let api_key = RawEtherscanApiKey::new("test-api-key".to_string());
        let http_counters = SyncHttpCounters::new();
        let error = {
            let traced_client =
                TracedBlockingClient::builder(IntegrationLabel::new(LABEL_ETHERSCAN), user_id)
                    .configure(|builder| builder.timeout(Duration::from_millis(200)))
                    .redact_query_params(&["apikey"])
                    .redact_headers(&["authorization"])
                    .build_for_tests_with_tracing(true)
                    .expect("traced blocking client should build");
            let client = EtherscanClient::new(
                traced_client,
                api_key.as_str(),
                base_url.as_str(),
                EtherscanNetwork::EthereumMainnet.chain_id(),
            )
            .with_total_api_call_counter(http_counters.total_api_calls_counter());
            let mut raw_run_summary = EtherscanRunSummary::default();

            fetch_etherscan_page(
                EtherscanRequestAttemptContext {
                    raw_sync_run_id: sync_run.sync_run_id,
                    ..request_context
                },
                |start_block, end_block, page, offset| {
                    client.fetch_normal_transactions_page(
                        address.address.as_str(),
                        start_block,
                        end_block,
                        page,
                        offset,
                    )
                },
                |start_block, end_block, page, offset| {
                    client.normal_transactions_request_metadata(
                        address.address.as_str(),
                        start_block,
                        end_block,
                        page,
                        offset,
                    )
                },
                EtherscanFetchRange {
                    start_block: 0,
                    end_block: 100,
                },
                &mut raw_run_summary,
                |_group_index, _sb, _eb, _page_number, _page_size, _page| {
                    panic!("persist_page should not run when the fetch fails before any response");
                },
            )
            .expect_err("unused localhost port should trigger transport failure")
        };

        let error_message = error.to_string();
        assert!(
            error_message.contains("Etherscan HTTP error"),
            "expected surfaced sync error to mention etherscan HTTP failure, got: {error_message}"
        );
        assert!(
            error_message.contains("send_failed:"),
            "expected surfaced sync error to include failure stage, got: {error_message}"
        );
        assert!(
            error_message.contains(&format!("127.0.0.1:{port}")),
            "expected surfaced sync error to retain request target context, got: {error_message}"
        );

        type RequestAttemptRow = (String, String, String);
        let rows: Vec<RequestAttemptRow> = with_user_db(user_id, |conn| -> Result<_, DbError> {
            let mut stmt = conn
                .prepare(
                    "SELECT outcome_kind, request_url, transport_error_message
                     FROM request_attempts
                     ORDER BY created_at ASC",
                )
                .map_err(|err| {
                    DbError::from_rusqlite_error("Failed to prepare request attempt query", err)
                })?;
            stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|err| DbError::from_rusqlite_error("Failed to query request attempts", err))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| DbError::from_rusqlite_error("Failed to read request attempts", err))
        })
        .expect("request attempts should load");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "transport_error");
        assert!(
            rows[0].1.contains(&format!("127.0.0.1:{port}")),
            "expected persisted request URL to retain localhost target, got: {}",
            rows[0].1
        );
        assert!(
            !rows[0].1.contains("test-api-key"),
            "persisted request URL should not retain the raw API key: {}",
            rows[0].1
        );
        assert!(
            rows[0].2.starts_with("send_failed: "),
            "expected persisted transport error to include failure stage, got: {}",
            rows[0].2
        );

        let trace_root = project_dir
            .join("users")
            .join(user_id.to_string())
            .join("traces");
        let trace_files = collect_files(&trace_root);

        assert_eq!(trace_files.len(), 1);
        assert_eq!(
            trace_files[0].extension().and_then(|ext| ext.to_str()),
            Some("har"),
            "transport failure should write exactly one HAR artifact"
        );

        let har_json = std::fs::read_to_string(&trace_files[0]).expect("HAR file should read");
        let har: Value = serde_json::from_str(&har_json).expect("HAR file should parse");
        let entry = &har["log"]["entries"][0];
        assert_eq!(entry["response"]["status"].as_u64(), Some(0));
        assert_eq!(
            entry["response"]["statusText"].as_str(),
            Some("No Response")
        );
        assert_eq!(entry["_bitgarthFailureStage"].as_str(), Some("send_failed"));
        assert_eq!(
            entry["_bitgarthTransportErrorKind"].as_str(),
            Some("connect")
        );
        assert!(
            entry["_bitgarthTransportErrorMessage"]
                .as_str()
                .is_some_and(|message| message.contains("error sending request")),
            "expected HAR failure metadata to retain the reqwest send error text"
        );
        assert!(
            entry["request"]["url"]
                .as_str()
                .is_some_and(|url| url.contains("apikey=***REDACTED***")),
            "HAR request URL should redact the API key"
        );
    }
}
