use super::client_config::fetch_mempool_chain_tip;
use super::error::preserve_iteration_error;
use super::{
    ChainTipCache, IntegrationIterationContext, RunContext, SingleAddressProgressPlan, SyncClients,
    SyncIterationResult, UserTransactionMonitorError, default_api_provider_for_asset,
    raw_sync_run_trigger_kind,
};
use crate::asset_capabilities::SyncProviderId;
use crate::db::SyncAddress;
use crate::db::raw_ingestion::{
    CompleteSyncRunRequest, IntegrationKind as RawIntegrationKind, OpaqueJsonText,
    StartSyncRunRequest, SyncRunScopeKind, SyncRunStatus as RawSyncRunStatus, complete_sync_run,
    start_sync_run,
};
use crate::tasks::jobs::sync::integrations::{
    AddressSyncIntegration,
    etherscan::{EtherscanAddressSyncIntegration, fetch_ethereum_chain_tip_height},
    mempool::MempoolAddressSyncIntegration,
};
use crate::wallets::DigitalAssetAddressId;
use chrono::{DateTime, Utc};
use dioxus::logger::tracing;
use std::collections::HashMap;
use std::time::Instant;

pub(super) struct AddressSyncExecutionRequest<'a> {
    pub(super) run: RunContext<'a>,
    pub(super) now_utc: DateTime<Utc>,
    pub(super) now_instant: Instant,
    pub(super) address: &'a SyncAddress,
    pub(super) chain_tip_cache: &'a mut ChainTipCache,
    pub(super) clients: SyncClients<'a>,
    pub(super) single_address_progress: Option<SingleAddressProgressPlan>,
    pub(super) allow_known_confirmed_early_exit: bool,
    pub(super) historical_backfill_enabled: bool,
    pub(super) legacy_mempool_history_repair: bool,
    pub(super) mempool_history_page_frontier: Option<crate::db::HdMempoolHistoryFrontierUpdate>,
}

pub(super) trait AddressSyncExecutor {
    fn sync_one_iteration(
        &mut self,
        request: AddressSyncExecutionRequest<'_>,
    ) -> Result<SyncIterationResult, UserTransactionMonitorError>;
}

pub(super) struct LiveAddressSyncExecutor {
    mempool: MempoolAddressSyncIntegration,
    etherscan: EtherscanAddressSyncIntegration,
    active_iteration_address_by_provider: HashMap<SyncProviderId, DigitalAssetAddressId>,
}

impl LiveAddressSyncExecutor {
    pub(super) fn new() -> Self {
        Self {
            mempool: MempoolAddressSyncIntegration::new(),
            etherscan: EtherscanAddressSyncIntegration::new(),
            active_iteration_address_by_provider: HashMap::new(),
        }
    }

    fn integration_mut(&mut self, provider: SyncProviderId) -> &mut dyn AddressSyncIntegration {
        match provider {
            SyncProviderId::MempoolSpace => &mut self.mempool,
            SyncProviderId::Etherscan => &mut self.etherscan,
        }
    }
}

pub(super) fn recover_interrupted_mempool_account(
    user_id: crate::models::UserId,
    account_id: crate::wallets::DigitalAssetAccountId,
    asset_id: crate::wallets::SyncedAssetId,
    start: crate::db::AccountIntegrationSyncStart,
) -> Result<crate::db::CoverageInvalidationTargets, crate::db::DbError> {
    if asset_id != crate::wallets::SyncedAssetId::Bitcoin || !start.was_interrupted {
        return Ok(crate::db::CoverageInvalidationTargets::default());
    }
    crate::db::invalidate_mempool_account_history_coverage(user_id, account_id)
}

fn failed_run_summary_json(
    integration: &dyn AddressSyncIntegration,
    request: &AddressSyncExecutionRequest<'_>,
    context: &'static str,
) -> Option<OpaqueJsonText> {
    match integration.current_run_summary_json() {
        Ok(summary_json) => summary_json,
        Err(err) => {
            tracing::error!(
                user_id = %request.run.user_id,
                run_id = %request.run.run_id,
                address_id = %request.address.address_id,
                error = %err,
                "transactions sync: failed to serialize raw sync run summary after {context}"
            );
            None
        }
    }
}

fn persist_iteration_chain_tip(
    address: &SyncAddress,
    result: SyncIterationResult,
) -> Result<SyncIterationResult, UserTransactionMonitorError> {
    crate::db::upsert_chain_tip_state(
        address.asset_id,
        address.network,
        result.tip_height,
        result.completed_at,
    )
    .map_err(|error| preserve_iteration_error(error, &result))?;
    Ok(result)
}

fn resolve_iteration_completion(
    sync_result: Result<SyncIterationResult, UserTransactionMonitorError>,
    raw_sync_completion: Result<(), crate::db::DbError>,
) -> (
    Result<SyncIterationResult, UserTransactionMonitorError>,
    Option<crate::db::DbError>,
) {
    match (sync_result, raw_sync_completion) {
        (Ok(summary), Ok(())) => (Ok(summary), None),
        (Err(error), Ok(())) => (Err(error), None),
        (Ok(summary), Err(error)) => {
            let error = preserve_iteration_error(error, &summary);
            (Err(error), None)
        }
        (Err(error), Err(completion_error)) => (Err(error), Some(completion_error)),
    }
}

fn should_reset_iteration_state(
    sync_result: &Result<SyncIterationResult, UserTransactionMonitorError>,
    raw_sync_completion: &Result<(), crate::db::DbError>,
) -> bool {
    raw_sync_completion.is_err() || !matches!(sync_result, Ok(result) if result.has_more_work)
}

fn raw_integration_kind(provider: SyncProviderId) -> RawIntegrationKind {
    match provider {
        SyncProviderId::MempoolSpace => RawIntegrationKind::Mempool,
        SyncProviderId::Etherscan => RawIntegrationKind::Etherscan,
    }
}

impl AddressSyncExecutor for LiveAddressSyncExecutor {
    fn sync_one_iteration(
        &mut self,
        request: AddressSyncExecutionRequest<'_>,
    ) -> Result<SyncIterationResult, UserTransactionMonitorError> {
        let provider = default_api_provider_for_asset(request.address.asset_id);
        let active_iteration_address = self
            .active_iteration_address_by_provider
            .get(&provider)
            .copied();
        if active_iteration_address != Some(request.address.address_id) {
            self.integration_mut(provider).reset_iteration_state();
            self.active_iteration_address_by_provider.remove(&provider);
        }
        let chain_tip = Some(match provider {
            SyncProviderId::MempoolSpace => {
                let client = request.clients.mempool_client.ok_or_else(|| {
                    UserTransactionMonitorError::Parse(format!(
                        "mempool client unavailable for {} sync",
                        request.address.asset_id.as_str()
                    ))
                })?;
                request.chain_tip_cache.get_or_fetch(
                    request.address.asset_id,
                    request.address.network,
                    request.now_instant,
                    request.now_utc,
                    || {
                        crate::db::debug_assert_user_db_unlocked(
                            request.run.user_id,
                            "mempool chain-tip fetch",
                        );
                        fetch_mempool_chain_tip(client)
                    },
                )?
            }
            SyncProviderId::Etherscan => request.chain_tip_cache.get_or_fetch(
                request.address.asset_id,
                request.address.network,
                request.now_instant,
                request.now_utc,
                || {
                    crate::db::debug_assert_user_db_unlocked(
                        request.run.user_id,
                        "etherscan chain-tip fetch",
                    );
                    fetch_ethereum_chain_tip_height(
                        request.run.user_id,
                        request.address.network,
                        request.clients.etherscan_api_key,
                        request.clients.etherscan_base_url,
                        request.clients.http_counters,
                    )
                },
            )?,
        });

        let (sync_result, raw_sync_completion, should_reset_iteration_state) = {
            let integration = self.integration_mut(provider);
            let sync_plan =
                integration.sync_plan(request.address, request.allow_known_confirmed_early_exit)?;
            let raw_sync_run = start_sync_run(
                request.run.user_id,
                StartSyncRunRequest {
                    integration: raw_integration_kind(provider),
                    scope_kind: SyncRunScopeKind::Address,
                    scope_address_id: request.address.address_id,
                    asset_id: request.address.asset_id,
                    network: request.address.network,
                    trigger_kind: raw_sync_run_trigger_kind(
                        request.run.source,
                        sync_plan.is_backfill_active,
                    ),
                    started_at: request.run.started_at,
                    summary_json: None,
                },
            )?;

            crate::db::debug_assert_user_db_unlocked(
                request.run.user_id,
                "integration iteration dispatch",
            );
            let sync_result = integration
                .sync_one_iteration(IntegrationIterationContext {
                    run: request.run,
                    now_utc: request.now_utc,
                    now_instant: request.now_instant,
                    address: request.address,
                    clients: request.clients,
                    single_address_progress: request.single_address_progress,
                    allow_known_confirmed_early_exit: request.allow_known_confirmed_early_exit,
                    chain_tip,
                    raw_sync_run_id: raw_sync_run.sync_run_id,
                    source_connection_id: &raw_sync_run.source_connection_id,
                    is_backfill_active: sync_plan.is_backfill_active,
                    historical_backfill_enabled: request.historical_backfill_enabled,
                    legacy_mempool_history_repair: request.legacy_mempool_history_repair,
                    mempool_history_page_frontier: request.mempool_history_page_frontier,
                })
                .and_then(|result| persist_iteration_chain_tip(request.address, result));
            let raw_run_summary_json = match &sync_result {
                Ok(summary) => summary.raw_run_summary_json.clone(),
                Err(_) => failed_run_summary_json(integration, &request, "iteration failure"),
            };
            let raw_sync_completion = complete_sync_run(
                request.run.user_id,
                CompleteSyncRunRequest {
                    sync_run_id: raw_sync_run.sync_run_id,
                    status: if sync_result.is_ok() {
                        RawSyncRunStatus::CompletedSuccess
                    } else {
                        RawSyncRunStatus::CompletedFailure
                    },
                    completed_at: request.run.clock.utc_now(),
                    summary_json: raw_run_summary_json,
                },
            );
            let should_reset_iteration_state =
                should_reset_iteration_state(&sync_result, &raw_sync_completion);
            if should_reset_iteration_state {
                integration.reset_iteration_state();
            }

            (
                sync_result,
                raw_sync_completion,
                should_reset_iteration_state,
            )
        };

        if should_reset_iteration_state {
            self.active_iteration_address_by_provider.remove(&provider);
        } else {
            self.active_iteration_address_by_provider
                .insert(provider, request.address.address_id);
        }

        let (result, secondary_completion_error) =
            resolve_iteration_completion(sync_result, raw_sync_completion);
        if let Some(error) = secondary_completion_error {
            tracing::error!(
                user_id = %request.run.user_id,
                run_id = %request.run.run_id,
                address_id = %request.address.address_id,
                error = %error,
                "transactions sync: failed to complete raw sync run after iteration failure"
            );
        }
        result
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;
    use crate::integrations::mempool::MempoolClient;
    use crate::tasks::jobs::sync::SyncHttpCounters;
    use crate::tasks::jobs::sync::chain_tip::{CachedChainTip, chain_tip_cache_key};
    use crate::tasks::jobs::sync::test_support::{
        FakeClock, make_run_context, make_sync_address, persist_sync_addresses_for_test,
        test_utc_now,
    };
    use crate::traces::client::{IntegrationLabel, TracedBlockingClient};
    use crate::transactions::ChainTipHeight;
    use crate::wallets::{Network, SyncedAssetId};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;
    use url::Url;

    const MEMPOOL_STATS_JSON: &str = r#"{"chain_stats":{"tx_count":2,"funded_txo_sum":50000,"spent_txo_sum":50000},"mempool_stats":{"tx_count":0}}"#;
    const MEMPOOL_PAGE_JSON: &str = r#"[{"txid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","vin":[],"vout":[],"fee":0,"status":{"confirmed":true,"block_height":1,"block_hash":"block","block_time":1}}]"#;

    struct MempoolPageServer {
        base_url: String,
        handle: thread::JoinHandle<Vec<String>>,
    }

    impl MempoolPageServer {
        fn join(self) -> Vec<String> {
            self.handle
                .join()
                .expect("test mempool server thread should join")
        }
    }

    fn start_mempool_page_server(page_response_body: &'static str) -> MempoolPageServer {
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
            let deadline = Instant::now() + Duration::from_secs(3);
            while request_lines.len() < 2 && Instant::now() < deadline {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(error) => panic!("test mempool server should accept request: {error}"),
                };
                stream
                    .set_nonblocking(false)
                    .expect("test mempool connection should be blocking");
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .expect("test mempool connection should have a read timeout");
                let mut buf = [0_u8; 4096];
                let read = stream
                    .read(&mut buf)
                    .expect("test mempool server should read request");
                let first_line = String::from_utf8_lossy(&buf[..read])
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_string();
                let response_body = if first_line.contains("/txs") {
                    page_response_body
                } else {
                    MEMPOOL_STATS_JSON
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
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

        MempoolPageServer { base_url, handle }
    }

    fn run_live_mempool_iteration(
        executor: &mut LiveAddressSyncExecutor,
        run: RunContext<'_>,
        address: &SyncAddress,
        chain_tip_cache: &mut ChainTipCache,
        page_response_body: &'static str,
    ) -> (
        Result<SyncIterationResult, UserTransactionMonitorError>,
        Vec<String>,
    ) {
        let server = start_mempool_page_server(page_response_body);
        let http_counters = SyncHttpCounters::new();
        let traced_client = TracedBlockingClient::builder(
            IntegrationLabel::new(super::super::context::LABEL_MEMPOOL),
            run.user_id,
        )
        .configure(|builder| builder.timeout(Duration::from_secs(2)))
        .build_for_tests_with_tracing(false)
        .expect("traced blocking client should build");
        let mempool_client = MempoolClient::new(
            traced_client,
            Url::parse(&server.base_url).expect("test mempool URL should parse"),
        );
        let result = executor.sync_one_iteration(AddressSyncExecutionRequest {
            run,
            now_utc: run.clock.utc_now(),
            now_instant: run.clock.instant_now(),
            address,
            chain_tip_cache,
            clients: SyncClients {
                mempool_client: Some(&mempool_client),
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            },
            single_address_progress: None,
            allow_known_confirmed_early_exit: false,
            historical_backfill_enabled: true,
            legacy_mempool_history_repair: false,
            mempool_history_page_frontier: None,
        });
        (result, server.join())
    }

    fn assert_first_page_request(request_lines: &[String], address: &SyncAddress) {
        assert_eq!(
            request_lines[1],
            format!("GET /api/address/{}/txs HTTP/1.1", address.address.as_str())
        );
    }

    #[test]
    fn live_executor_persists_nonterminal_state_and_resumes_with_fresh_executor() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let clock = FakeClock::new(test_utc_now());
        let run = make_run_context(&clock);
        let address = make_sync_address(
            "bc1qnonterminalsummary",
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            None,
            None,
            None,
            None,
        );
        persist_sync_addresses_for_test(run, std::slice::from_ref(&address));
        crate::db::mark_address_sync_started(
            run.user_id,
            address.address_id,
            run.run_id,
            run.started_at,
        )
        .expect("sync state row should exist");
        let mut chain_tip_cache = ChainTipCache::default();
        chain_tip_cache.tips.insert(
            chain_tip_cache_key(SyncedAssetId::Bitcoin, Network::Mainnet),
            CachedChainTip {
                height: ChainTipHeight::try_new(100).expect("tip should be valid"),
                fetched_at: run.clock.instant_now(),
            },
        );

        let cursor = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let (result, first_requests) = run_live_mempool_iteration(
            &mut LiveAddressSyncExecutor::new(),
            run,
            &address,
            &mut chain_tip_cache,
            MEMPOOL_PAGE_JSON,
        );
        assert!(result.expect("first page should succeed").has_more_work);
        assert_first_page_request(&first_requests, &address);

        let summary_json: String = crate::db::with_user_db(run.user_id, |conn| {
            conn.query_row(
                "SELECT summary_json FROM sync_runs WHERE scope_address_id = ?1 ORDER BY rowid DESC LIMIT 1",
                [address.address_id.to_string()],
                |row| row.get(0),
            )
            .map_err(|error| {
                crate::db::DbError::from_rusqlite_error(
                    "Failed to load persisted sync run summary",
                    error,
                )
            })
        })
        .expect("persisted sync run summary should load");
        assert!(summary_json.contains("\"backfill_budget_exhausted\":true"));

        let persisted_address = crate::db::get_non_hd_sync_addresses(run.user_id)
            .expect("persisted sync addresses should load")
            .into_iter()
            .find(|candidate| candidate.address_id == address.address_id)
            .expect("persisted sync address should exist");
        assert_eq!(
            persisted_address
                .mempool_backfill_cursor_txid
                .as_ref()
                .map(|stored| stored.as_str()),
            Some(cursor)
        );

        let (resumed, resumed_requests) = run_live_mempool_iteration(
            &mut LiveAddressSyncExecutor::new(),
            run,
            &persisted_address,
            &mut chain_tip_cache,
            "[]",
        );
        assert!(
            !resumed
                .expect("persisted cursor should resume and exhaust")
                .has_more_work
        );
        assert_eq!(
            resumed_requests[1],
            format!(
                "GET /api/address/{}/txs/chain/{cursor} HTTP/1.1",
                address.address.as_str()
            )
        );
    }

    #[test]
    fn live_executor_retains_and_resets_provider_iteration_state() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let clock = FakeClock::new(test_utc_now());
        let run = make_run_context(&clock);
        let address_a = make_sync_address(
            "bc1qstateaddressa",
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            None,
            None,
            None,
            None,
        );
        let address_b = make_sync_address(
            "bc1qstateaddressb",
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            None,
            None,
            None,
            None,
        );
        persist_sync_addresses_for_test(run, &[address_a.clone(), address_b.clone()]);
        for address in [&address_a, &address_b] {
            crate::db::mark_address_sync_started(
                run.user_id,
                address.address_id,
                run.run_id,
                run.started_at,
            )
            .expect("sync state row should exist");
        }
        let mut chain_tip_cache = ChainTipCache::default();
        chain_tip_cache.tips.insert(
            chain_tip_cache_key(SyncedAssetId::Bitcoin, Network::Mainnet),
            CachedChainTip {
                height: ChainTipHeight::try_new(100).expect("tip should be valid"),
                fetched_at: run.clock.instant_now(),
            },
        );
        let provider = SyncProviderId::MempoolSpace;
        let cursor = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

        let mut executor = LiveAddressSyncExecutor::new();
        let (first, first_requests) = run_live_mempool_iteration(
            &mut executor,
            run,
            &address_a,
            &mut chain_tip_cache,
            MEMPOOL_PAGE_JSON,
        );
        assert!(first.expect("first page should succeed").has_more_work);
        assert_eq!(
            executor.active_iteration_address_by_provider.get(&provider),
            Some(&address_a.address_id)
        );
        assert_first_page_request(&first_requests, &address_a);

        let (exhausted, exhausted_requests) =
            run_live_mempool_iteration(&mut executor, run, &address_a, &mut chain_tip_cache, "[]");
        assert!(
            !exhausted
                .expect("empty next page should exhaust")
                .has_more_work
        );
        let expected_paginated_request = format!(
            "GET /api/address/{}/txs/chain/{cursor} HTTP/1.1",
            address_a.address.as_str()
        );
        assert_eq!(exhausted_requests.last(), Some(&expected_paginated_request));
        assert!(
            !executor
                .active_iteration_address_by_provider
                .contains_key(&provider)
        );
        let (after_exhaustion, after_exhaustion_requests) =
            run_live_mempool_iteration(&mut executor, run, &address_a, &mut chain_tip_cache, "[]");
        assert!(
            !after_exhaustion
                .expect("restart after exhaustion should succeed")
                .has_more_work
        );
        assert_first_page_request(&after_exhaustion_requests, &address_a);

        let (before_error, _) = run_live_mempool_iteration(
            &mut executor,
            run,
            &address_a,
            &mut chain_tip_cache,
            MEMPOOL_PAGE_JSON,
        );
        assert!(
            before_error
                .expect("error setup page should succeed")
                .has_more_work
        );
        let (provider_error, error_requests) = run_live_mempool_iteration(
            &mut executor,
            run,
            &address_a,
            &mut chain_tip_cache,
            "unexpected",
        );
        provider_error.expect_err("invalid provider response should fail");
        assert!(
            error_requests
                .last()
                .is_some_and(|request| request.contains("/txs/chain/"))
        );
        assert!(
            !executor
                .active_iteration_address_by_provider
                .contains_key(&provider)
        );
        let (after_error, after_error_requests) =
            run_live_mempool_iteration(&mut executor, run, &address_a, &mut chain_tip_cache, "[]");
        assert!(
            !after_error
                .expect("restart after provider error should succeed")
                .has_more_work
        );
        assert_first_page_request(&after_error_requests, &address_a);

        let (before_address_change, _) = run_live_mempool_iteration(
            &mut executor,
            run,
            &address_a,
            &mut chain_tip_cache,
            MEMPOOL_PAGE_JSON,
        );
        assert!(
            before_address_change
                .expect("address-change setup page should succeed")
                .has_more_work
        );
        let (changed_address, changed_address_requests) =
            run_live_mempool_iteration(&mut executor, run, &address_b, &mut chain_tip_cache, "[]");
        assert!(
            !changed_address
                .expect("changed address should start fresh")
                .has_more_work
        );
        assert_first_page_request(&changed_address_requests, &address_b);
        assert!(
            !executor
                .active_iteration_address_by_provider
                .contains_key(&provider)
        );
    }

    #[test]
    fn address_change_resets_provider_state_before_chain_tip_failure() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let clock = FakeClock::new(test_utc_now());
        let run = make_run_context(&clock);
        let address_a = make_sync_address(
            "bc1qsetupfailureaddressa",
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            None,
            None,
            None,
            None,
        );
        let address_b = make_sync_address(
            "bc1qsetupfailureaddressb",
            SyncedAssetId::Bitcoin,
            Network::Testnet,
            None,
            None,
            None,
            None,
        );
        persist_sync_addresses_for_test(run, &[address_a.clone(), address_b.clone()]);
        for address in [&address_a, &address_b] {
            crate::db::mark_address_sync_started(
                run.user_id,
                address.address_id,
                run.run_id,
                run.started_at,
            )
            .expect("sync state row should exist");
        }
        let mut chain_tip_cache = ChainTipCache::default();
        chain_tip_cache.tips.insert(
            chain_tip_cache_key(SyncedAssetId::Bitcoin, Network::Mainnet),
            CachedChainTip {
                height: ChainTipHeight::try_new(100).expect("tip should be valid"),
                fetched_at: run.clock.instant_now(),
            },
        );
        let mut executor = LiveAddressSyncExecutor::new();

        let (first, _) = run_live_mempool_iteration(
            &mut executor,
            run,
            &address_a,
            &mut chain_tip_cache,
            MEMPOOL_PAGE_JSON,
        );
        assert!(first.expect("first page should succeed").has_more_work);

        chain_tip_cache.tips.clear();
        let (setup_failure, _) =
            run_live_mempool_iteration(&mut executor, run, &address_b, &mut chain_tip_cache, "[]");
        setup_failure.expect_err("address B chain-tip fetch should fail");

        chain_tip_cache.tips.insert(
            chain_tip_cache_key(SyncedAssetId::Bitcoin, Network::Mainnet),
            CachedChainTip {
                height: ChainTipHeight::try_new(100).expect("tip should be valid"),
                fetched_at: run.clock.instant_now(),
            },
        );
        let (restarted, restarted_requests) =
            run_live_mempool_iteration(&mut executor, run, &address_a, &mut chain_tip_cache, "[]");
        assert!(
            !restarted
                .expect("address A should restart after address B setup failure")
                .has_more_work
        );
        assert_first_page_request(&restarted_requests, &address_a);
    }

    #[test]
    fn persist_iteration_chain_tip_for_both_providers() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let completed_at = crate::tasks::jobs::sync::test_support::test_utc_now();

        for (raw_address, asset_id, tip_value) in [
            (
                "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4",
                SyncedAssetId::Bitcoin,
                101_i64,
            ),
            (
                "0x1111111111111111111111111111111111111111",
                SyncedAssetId::Ethereum,
                202_i64,
            ),
        ] {
            let address = crate::tasks::jobs::sync::test_support::make_sync_address(
                raw_address,
                asset_id,
                Network::Mainnet,
                None,
                None,
                None,
                None,
            );
            let tip_height = ChainTipHeight::try_new(tip_value).expect("tip should be valid");
            let result = SyncIterationResult::exhausted(tip_height, completed_at);

            let returned = persist_iteration_chain_tip(&address, result.clone())
                .expect("iteration tip should persist");
            let stored = crate::db::load_chain_tip_state(asset_id, Network::Mainnet)
                .expect("chain tip should load")
                .expect("chain tip should exist");

            assert_eq!(returned, result);
            assert_eq!(stored.chain_tip_height, tip_height);
            assert_eq!(stored.updated_at, completed_at);
        }
    }

    #[test]
    fn chain_tip_failure_preserves_successful_iteration_targets() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let completed_at = test_utc_now();
        let account_id = crate::wallets::DigitalAssetAccountId::new();
        let address = make_sync_address(
            "bc1qchainpersistfailure",
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            Some(account_id),
            None,
            None,
            None,
        );
        let mut result = SyncIterationResult::exhausted(
            ChainTipHeight::try_new(909_090).expect("tip should parse"),
            completed_at,
        );
        result.coverage_invalidation.account_ids.insert(account_id);
        crate::db::with_db_mut(|conn| {
            conn.execute_batch(
                "CREATE TRIGGER test_reject_chain_tip_persistence
                 BEFORE INSERT ON chain_state
                 WHEN NEW.chain_height = 909090
                 BEGIN
                   SELECT RAISE(ABORT, 'injected chain-tip persistence failure');
                 END;",
            )
            .map_err(|err| {
                crate::db::DbError::new(format!(
                    "Failed to install chain-tip persistence failure: {err}"
                ))
            })
        })
        .expect("chain-tip persistence failure should install");

        let error = persist_iteration_chain_tip(&address, result)
            .expect_err("injected chain-tip persistence should fail");
        crate::db::with_db_mut(|conn| {
            conn.execute_batch("DROP TRIGGER test_reject_chain_tip_persistence;")
                .map_err(|err| {
                    crate::db::DbError::new(format!(
                        "Failed to remove chain-tip persistence failure: {err}"
                    ))
                })
        })
        .expect("chain-tip persistence failure should be removed");

        assert_eq!(
            error
                .coverage_invalidation()
                .expect("chain-tip error should preserve iteration targets")
                .account_ids,
            std::collections::HashSet::from([account_id])
        );
    }

    #[test]
    fn interrupted_account_recovery_invalidates_proof_when_replay_has_no_changes() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let clock = FakeClock::new(test_utc_now());
        let run = make_run_context(&clock);
        let account_id = crate::wallets::DigitalAssetAccountId::new();
        let address = make_sync_address(
            "bc1qinterruptedrecovery",
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            Some(account_id),
            None,
            None,
            None,
        );
        persist_sync_addresses_for_test(run, std::slice::from_ref(&address));
        crate::db::mark_address_sync_started(
            run.user_id,
            address.address_id,
            run.run_id,
            run.started_at,
        )
        .expect("address start should persist");
        crate::db::publish_mempool_history_proof(
            run.user_id,
            address.address_id,
            crate::db::MempoolHistoryProof {
                confirmed_tx_count: crate::transactions::TransactionCount::from_u32(1),
                complete_height: ChainTipHeight::try_new(100).expect("valid height"),
            },
        )
        .expect("proof should publish");
        crate::db::mark_account_integration_sync_started(
            run.user_id,
            account_id,
            crate::transactions::SyncIntegrationId::Mempool,
            run.started_at,
        )
        .expect("interrupted start should persist");
        let recovery_start = crate::db::mark_account_integration_sync_started(
            run.user_id,
            account_id,
            crate::transactions::SyncIntegrationId::Mempool,
            run.started_at + chrono::Duration::seconds(1),
        )
        .expect("recovery start should persist");
        let replay = crate::db::reconcile_address_transactions(
            run.user_id,
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            &[],
            run.started_at + chrono::Duration::seconds(1),
        )
        .expect("empty replay should reconcile");
        assert_eq!(replay.new_tx_count.value(), 0);
        assert_eq!(replay.updated_tx_count.value(), 0);

        let targets = recover_interrupted_mempool_account(
            run.user_id,
            account_id,
            SyncedAssetId::Bitcoin,
            recovery_start,
        )
        .expect("interrupted account should recover");

        assert_eq!(
            targets.account_ids,
            std::collections::HashSet::from([account_id])
        );
        assert_eq!(
            crate::db::get_sync_addresses_for_account(run.user_id, account_id)
                .expect("address should reload")[0]
                .mempool_history_proof,
            None
        );
    }

    #[test]
    fn resolve_iteration_completion_preserves_error_precedence() {
        let completed_at = crate::tasks::jobs::sync::test_support::test_utc_now();
        let account_id = crate::wallets::DigitalAssetAccountId::new();
        let mut result = SyncIterationResult::exhausted(
            ChainTipHeight::try_new(303).expect("tip should be valid"),
            completed_at,
        );
        result.coverage_invalidation.account_ids.insert(account_id);

        let (provider_failure, secondary_failure) = resolve_iteration_completion(
            Err(UserTransactionMonitorError::Http(
                "provider failed".to_string(),
            )),
            Err(crate::db::DbError::new("raw completion failed")),
        );
        assert!(
            provider_failure
                .expect_err("provider error should remain primary")
                .to_string()
                .contains("provider failed")
        );
        assert!(
            secondary_failure
                .expect("raw completion error should be retained for logging")
                .to_string()
                .contains("raw completion failed")
        );

        let (completion_failure, secondary_failure) = resolve_iteration_completion(
            Ok(result),
            Err(crate::db::DbError::new("raw completion failed")),
        );
        let completion_failure = completion_failure
            .expect_err("completion error should fail a successful provider result");
        assert!(
            completion_failure
                .to_string()
                .contains("raw completion failed")
        );
        assert_eq!(
            completion_failure
                .coverage_invalidation()
                .expect("raw completion error should preserve iteration targets")
                .account_ids,
            std::collections::HashSet::from([account_id])
        );
        assert!(secondary_failure.is_none());
    }

    #[test]
    fn raw_completion_failure_resets_nonterminal_iteration_state() {
        let mut result = SyncIterationResult::exhausted(
            ChainTipHeight::try_new(404).expect("tip should be valid"),
            test_utc_now(),
        );
        result.has_more_work = true;

        assert!(should_reset_iteration_state(
            &Ok(result),
            &Err(crate::db::DbError::new("raw completion failed")),
        ));
    }
}
