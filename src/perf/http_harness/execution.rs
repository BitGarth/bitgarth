use super::PerfError;
use super::datasets::{
    APP_DB_WRITE_ITERATIONS, EXPORT_WORKLOAD_ITERATIONS, PerfDatasetManifest, PerfSyncWorkloadKind,
    PerfSyncWorkloadProfile, SETTINGS_SYNC_CURRENCY_CODE, build_large_account_transaction_records,
    build_large_utxo_transaction_records, perf_seed_timestamp,
};
use super::http_client::{
    PerfRequestSpec, PerfSession, build_clients, build_perf_client, endpoint_url,
    fetch_account_transactions, fetch_settings, fetch_sync_state, fetch_wallet_counts,
    run_request_batch, send_authenticated_post_json,
};
use super::measurement::RequestOutcome;
use super::scenarios::{
    PerfScenarioDefinition, SCENARIO_ACCOUNT_TRANSACTIONS_LARGE_READ,
    SCENARIO_AUTH_RESTORE_DURING_APP_DB_WRITES, SCENARIO_AUTH_RESTORE_DURING_USER_DB_LOAD,
    SCENARIO_READS_DURING_EXPORT, SCENARIO_READS_DURING_HEAVY_SYNC, SCENARIO_READS_DURING_SYNC,
    SCENARIO_SETTINGS_WRITES_DURING_SYNC, SCENARIO_SYNC_STATE_LARGE_READ,
    SCENARIO_UTXO_READS_DURING_SYNC, SCENARIO_UTXO_TRANSACTIONS_LARGE_READ,
    SCENARIO_WALLETS_EMPTY_READ, SCENARIO_WALLETS_LARGE_READ, sync_workload_kind_for_scenario,
    sync_workload_profile_for_scenario,
};
use super::server::spawn_with_current_runtime_context;
use crate::db::{
    rebuild_account_transaction_ledger, reconcile_account_transactions,
    reconcile_address_transactions, upsert_chain_tip_state,
};
use crate::models::UserId;
use crate::transactions::{ChainTipHeight, TrackedAddress};
use crate::wallets::{DigitalAssetAccountId, Network, SyncedAssetId};
use chrono::Duration as ChronoDuration;
use serde_json::json;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ScenarioExecutionResult {
    pub(super) outcomes: Vec<RequestOutcome>,
    pub(super) notes: Vec<String>,
}

pub(super) fn execute_scenario(
    session: &PerfSession,
    scenario: &PerfScenarioDefinition,
    manifest: &PerfDatasetManifest,
    concurrency: u32,
    warmup_iterations: u32,
    measured_iterations: u32,
) -> Result<ScenarioExecutionResult, PerfError> {
    match scenario.id {
        SCENARIO_WALLETS_EMPTY_READ
        | SCENARIO_WALLETS_LARGE_READ
        | SCENARIO_ACCOUNT_TRANSACTIONS_LARGE_READ
        | SCENARIO_UTXO_TRANSACTIONS_LARGE_READ
        | SCENARIO_SYNC_STATE_LARGE_READ => execute_simple_request_scenario(
            session,
            scenario,
            manifest,
            concurrency,
            warmup_iterations,
            measured_iterations,
        ),
        SCENARIO_AUTH_RESTORE_DURING_USER_DB_LOAD => execute_auth_restore_overlap_scenario(
            session,
            scenario,
            manifest,
            concurrency,
            warmup_iterations,
            measured_iterations,
        ),
        SCENARIO_AUTH_RESTORE_DURING_APP_DB_WRITES => execute_app_db_overlap_scenario(
            session,
            scenario,
            manifest,
            concurrency,
            warmup_iterations,
            measured_iterations,
        ),
        SCENARIO_READS_DURING_EXPORT => execute_export_overlap_scenario(
            session,
            scenario,
            manifest,
            concurrency,
            warmup_iterations,
            measured_iterations,
        ),
        SCENARIO_READS_DURING_SYNC
        | SCENARIO_READS_DURING_HEAVY_SYNC
        | SCENARIO_SETTINGS_WRITES_DURING_SYNC => execute_sync_overlap_scenario(
            session,
            scenario,
            manifest,
            concurrency,
            warmup_iterations,
            measured_iterations,
        ),
        SCENARIO_UTXO_READS_DURING_SYNC => execute_sync_overlap_scenario(
            session,
            scenario,
            manifest,
            concurrency,
            warmup_iterations,
            measured_iterations,
        ),
        other => Err(PerfError::usage(format!(
            "unsupported scenario execution for '{other}'"
        ))),
    }
}

fn concurrency_as_usize(concurrency: u32) -> Result<usize, PerfError> {
    let concurrency_usize = usize::try_from(concurrency)
        .map_err(|_| PerfError::usage("concurrency does not fit into usize"))?;
    if concurrency_usize == 0 {
        return Err(PerfError::usage("concurrency must be greater than zero"));
    }
    Ok(concurrency_usize)
}

fn execute_simple_request_scenario(
    session: &PerfSession,
    scenario: &PerfScenarioDefinition,
    manifest: &PerfDatasetManifest,
    concurrency: u32,
    warmup_iterations: u32,
    measured_iterations: u32,
) -> Result<ScenarioExecutionResult, PerfError> {
    let request_specs = scenario_request_specs(session, scenario, manifest)?;
    let concurrency_usize = concurrency_as_usize(concurrency)?;
    let clients = build_clients(session.user_id, concurrency_usize)?;
    for _ in 0..warmup_iterations {
        let _ = run_request_batch(&clients, &request_specs, &session.cookie_header)?;
    }

    let mut outcomes = Vec::new();
    for _ in 0..measured_iterations {
        outcomes.extend(run_request_batch(
            &clients,
            &request_specs,
            &session.cookie_header,
        )?);
    }
    let notes = scenario_validation_notes(session, scenario, manifest)?;

    Ok(ScenarioExecutionResult { outcomes, notes })
}

fn run_export_workload(
    client: crate::traces::client::TracedBlockingClient,
    base_url: String,
    cookie_header: String,
) -> Result<Vec<String>, PerfError> {
    let export_url = endpoint_url(&base_url, "_app/user/exports/hledger/download");
    let request_body = serde_json::json!({ "encrypted": false });
    let mut last_byte_count = 0_usize;

    for _ in 0..EXPORT_WORKLOAD_ITERATIONS {
        let response = send_authenticated_post_json(
            &client,
            export_url.clone(),
            &cookie_header,
            &request_body,
            "perf export POST",
        )?;
        let status = response.status();
        let response_url = response.url().to_string();
        let bytes = response.into_bytes();
        if !status.is_success() {
            return Err(PerfError::HttpClient(format!(
                "perf export POST returned {status} from {response_url} ({} bytes)",
                bytes.len()
            )));
        }
        last_byte_count = bytes.len();
    }

    Ok(vec![
        format!("export_iterations={EXPORT_WORKLOAD_ITERATIONS}"),
        format!("export_zip_bytes={last_byte_count}"),
    ])
}

fn run_app_db_write_workload() -> Result<Vec<String>, PerfError> {
    let base_time = perf_seed_timestamp()?;
    let mut last_height = 0_i64;

    for iteration in 0..APP_DB_WRITE_ITERATIONS {
        let height =
            ChainTipHeight::try_new(1_000_000_i64 + i64::from(iteration)).map_err(|err| {
                PerfError::io(format!("failed to build app-db chain tip height: {err}"))
            })?;
        let updated_at = base_time + ChronoDuration::seconds(i64::from(iteration));
        upsert_chain_tip_state(SyncedAssetId::Bitcoin, Network::Mainnet, height, updated_at)
            .map_err(|err| {
                PerfError::io(format!(
                    "failed to upsert app-db chain tip for iteration {}: {err}",
                    iteration + 1
                ))
            })?;
        last_height = height.value();
    }

    Ok(vec![
        format!("app_db_write_iterations={APP_DB_WRITE_ITERATIONS}"),
        format!("app_db_last_chain_tip_height={last_height}"),
    ])
}

fn execute_auth_restore_overlap_scenario(
    session: &PerfSession,
    scenario: &PerfScenarioDefinition,
    manifest: &PerfDatasetManifest,
    concurrency: u32,
    warmup_iterations: u32,
    measured_iterations: u32,
) -> Result<ScenarioExecutionResult, PerfError> {
    let request_specs = scenario_request_specs(session, scenario, manifest)?;
    let concurrency_usize = concurrency_as_usize(concurrency)?;
    let clients = build_clients(session.user_id, concurrency_usize)?;
    let workload_user_id = manifest.workload_user_id.ok_or_else(|| {
        PerfError::usage(format!(
            "{} requires workload_user_id in the dataset manifest",
            scenario.id
        ))
    })?;
    let workload_account_id = manifest.workload_account_id.ok_or_else(|| {
        PerfError::usage(format!(
            "{} requires workload_account_id in the dataset manifest",
            scenario.id
        ))
    })?;
    let workload_tracked_address = manifest.workload_tracked_address.clone().ok_or_else(|| {
        PerfError::usage(format!(
            "{} requires workload_tracked_address in the dataset manifest",
            scenario.id
        ))
    })?;
    let workload_profile = sync_workload_profile_for_scenario(scenario)?;
    let workload_handle = spawn_with_current_runtime_context(move || {
        run_sync_ledger_rebuild_workload(
            workload_user_id,
            workload_account_id,
            workload_tracked_address,
            PerfSyncWorkloadKind::AccountModel,
            workload_profile,
        )
    });

    for _ in 0..warmup_iterations {
        let _ = run_request_batch(&clients, &request_specs, &session.cookie_header)?;
    }

    let mut outcomes = Vec::new();
    for _ in 0..measured_iterations {
        outcomes.extend(run_request_batch(
            &clients,
            &request_specs,
            &session.cookie_header,
        )?);
    }

    let mut workload_failed = false;
    let workload_notes = match workload_handle.join() {
        Ok(Ok(notes)) => notes,
        Ok(Err(err)) => {
            workload_failed = true;
            vec![format!("workload_error={err}")]
        }
        Err(_) => {
            workload_failed = true;
            vec!["workload_error=perf auth-restore overlap thread panicked".to_string()]
        }
    };
    if workload_failed {
        outcomes.push(RequestOutcome {
            latency_ms: 0.0,
            success: false,
        });
    }

    let mut notes = vec![
        format!("primary_user_id={}", session.user_id),
        format!("workload_user_id={workload_user_id}"),
        format!("workload_account_id={workload_account_id}"),
    ];
    notes.extend(workload_notes);

    Ok(ScenarioExecutionResult { outcomes, notes })
}

fn execute_app_db_overlap_scenario(
    session: &PerfSession,
    scenario: &PerfScenarioDefinition,
    manifest: &PerfDatasetManifest,
    concurrency: u32,
    warmup_iterations: u32,
    measured_iterations: u32,
) -> Result<ScenarioExecutionResult, PerfError> {
    let request_specs = scenario_request_specs(session, scenario, manifest)?;
    let concurrency_usize = concurrency_as_usize(concurrency)?;
    let clients = build_clients(session.user_id, concurrency_usize)?;
    let workload_handle = spawn_with_current_runtime_context(run_app_db_write_workload);

    for _ in 0..warmup_iterations {
        let _ = run_request_batch(&clients, &request_specs, &session.cookie_header)?;
    }

    let mut outcomes = Vec::new();
    for _ in 0..measured_iterations {
        outcomes.extend(run_request_batch(
            &clients,
            &request_specs,
            &session.cookie_header,
        )?);
    }

    let mut workload_failed = false;
    let workload_notes = match workload_handle.join() {
        Ok(Ok(notes)) => notes,
        Ok(Err(err)) => {
            workload_failed = true;
            vec![format!("workload_error={err}")]
        }
        Err(_) => {
            workload_failed = true;
            vec!["workload_error=perf app-db workload thread panicked".to_string()]
        }
    };
    if workload_failed {
        outcomes.push(RequestOutcome {
            latency_ms: 0.0,
            success: false,
        });
    }

    let mut notes = vec![format!("primary_user_id={}", session.user_id)];
    notes.extend(workload_notes);

    Ok(ScenarioExecutionResult { outcomes, notes })
}

fn execute_export_overlap_scenario(
    session: &PerfSession,
    scenario: &PerfScenarioDefinition,
    manifest: &PerfDatasetManifest,
    concurrency: u32,
    warmup_iterations: u32,
    measured_iterations: u32,
) -> Result<ScenarioExecutionResult, PerfError> {
    let request_specs = scenario_request_specs(session, scenario, manifest)?;
    let concurrency_usize = concurrency_as_usize(concurrency)?;
    let clients = build_clients(session.user_id, concurrency_usize)?;
    let export_client = build_perf_client(session.user_id)?;
    let export_base_url = session.base_url.clone();
    let export_cookie = session.cookie_header.clone();
    let workload_handle = spawn_with_current_runtime_context(move || {
        run_export_workload(export_client, export_base_url, export_cookie)
    });

    for _ in 0..warmup_iterations {
        let _ = run_request_batch(&clients, &request_specs, &session.cookie_header)?;
    }

    let mut outcomes = Vec::new();
    for _ in 0..measured_iterations {
        outcomes.extend(run_request_batch(
            &clients,
            &request_specs,
            &session.cookie_header,
        )?);
    }

    let mut workload_failed = false;
    let workload_notes = match workload_handle.join() {
        Ok(Ok(notes)) => notes,
        Ok(Err(err)) => {
            workload_failed = true;
            vec![format!("workload_error={err}")]
        }
        Err(_) => {
            workload_failed = true;
            vec!["workload_error=perf export workload thread panicked".to_string()]
        }
    };
    if workload_failed {
        outcomes.push(RequestOutcome {
            latency_ms: 0.0,
            success: false,
        });
    }

    let notes = validate_export_overlap_scenario(session, manifest, &workload_notes)?;

    Ok(ScenarioExecutionResult { outcomes, notes })
}

fn execute_sync_overlap_scenario(
    session: &PerfSession,
    scenario: &PerfScenarioDefinition,
    manifest: &PerfDatasetManifest,
    concurrency: u32,
    warmup_iterations: u32,
    measured_iterations: u32,
) -> Result<ScenarioExecutionResult, PerfError> {
    let request_specs = scenario_request_specs(session, scenario, manifest)?;
    let concurrency_usize = concurrency_as_usize(concurrency)?;
    let clients = build_clients(session.user_id, concurrency_usize)?;
    let primary_account_id = manifest.primary_account_id.ok_or_else(|| {
        PerfError::usage(format!(
            "{} requires a dataset manifest with primary_account_id",
            scenario.id
        ))
    })?;
    let primary_tracked_address = manifest.primary_tracked_address.clone().ok_or_else(|| {
        PerfError::usage(format!(
            "{} requires a dataset manifest with primary_tracked_address",
            scenario.id
        ))
    })?;
    let workload_kind = sync_workload_kind_for_scenario(scenario)?;
    let workload_profile = sync_workload_profile_for_scenario(scenario)?;
    let user_id = session.user_id;
    let workload_handle = spawn_with_current_runtime_context(move || {
        run_sync_ledger_rebuild_workload(
            user_id,
            primary_account_id,
            primary_tracked_address,
            workload_kind,
            workload_profile,
        )
    });

    for _ in 0..warmup_iterations {
        let _ = run_request_batch(&clients, &request_specs, &session.cookie_header)?;
    }

    let mut outcomes = Vec::new();
    for _ in 0..measured_iterations {
        outcomes.extend(run_request_batch(
            &clients,
            &request_specs,
            &session.cookie_header,
        )?);
    }

    let mut workload_failed = false;
    let workload_notes = match workload_handle.join() {
        Ok(Ok(notes)) => notes,
        Ok(Err(err)) => {
            workload_failed = true;
            vec![format!("workload_error={err}")]
        }
        Err(_) => {
            workload_failed = true;
            vec!["workload_error=perf sync-write workload thread panicked".to_string()]
        }
    };
    if workload_failed {
        outcomes.push(RequestOutcome {
            latency_ms: 0.0,
            success: false,
        });
    }
    let notes = validate_sync_overlap_scenario(session, scenario, manifest, &workload_notes)?;

    Ok(ScenarioExecutionResult { outcomes, notes })
}

fn run_sync_ledger_rebuild_workload(
    user_id: UserId,
    account_id: DigitalAssetAccountId,
    primary_tracked_address: String,
    workload_kind: PerfSyncWorkloadKind,
    workload_profile: PerfSyncWorkloadProfile,
) -> Result<Vec<String>, PerfError> {
    let base_time = perf_seed_timestamp()?;
    let tracked_address = TrackedAddress::parse(&primary_tracked_address)
        .map_err(|err| PerfError::usage(format!("invalid perf tracked address: {err}")))?;
    let workload_label = match workload_kind {
        PerfSyncWorkloadKind::AccountModel => "account",
        PerfSyncWorkloadKind::Utxo => "utxo",
    };
    let account_records = matches!(workload_kind, PerfSyncWorkloadKind::AccountModel)
        .then(|| {
            build_large_account_transaction_records(
                &tracked_address,
                workload_profile.confirmed_count,
                workload_profile.pending_count,
            )
        })
        .transpose()?;
    let utxo_records = matches!(workload_kind, PerfSyncWorkloadKind::Utxo)
        .then(|| {
            build_large_utxo_transaction_records(
                &tracked_address,
                workload_profile.confirmed_count,
                workload_profile.pending_count,
            )
        })
        .transpose()?;
    for iteration in 0..workload_profile.rebuild_iterations {
        let observed_at = base_time + ChronoDuration::minutes(10_000 + i64::from(iteration));
        match workload_kind {
            PerfSyncWorkloadKind::AccountModel => {
                let records = account_records.as_ref().ok_or_else(|| {
                    PerfError::io("missing account-model perf sync workload records")
                })?;
                reconcile_account_transactions(
                    user_id,
                    SyncedAssetId::Ethereum,
                    Network::Mainnet,
                    records,
                    observed_at,
                )
                .map_err(|err| {
                    PerfError::io(format!(
                        "failed to reconcile perf account sync ledger for iteration {}: {err}",
                        iteration + 1
                    ))
                })?;
            }
            PerfSyncWorkloadKind::Utxo => {
                let records = utxo_records
                    .as_ref()
                    .ok_or_else(|| PerfError::io("missing UTXO perf sync workload records"))?;
                reconcile_address_transactions(
                    user_id,
                    SyncedAssetId::Bitcoin,
                    Network::Mainnet,
                    records,
                    observed_at,
                )
                .map_err(|err| {
                    PerfError::io(format!(
                        "failed to reconcile perf UTXO sync ledger for iteration {}: {err}",
                        iteration + 1
                    ))
                })?;
            }
        }
        rebuild_account_transaction_ledger(user_id, account_id, observed_at).map_err(|err| {
            PerfError::io(format!(
                "failed to rebuild perf sync ledger for iteration {}: {err}",
                iteration + 1
            ))
        })?;
    }

    Ok(vec![
        format!(
            "ledger_rebuild_iterations={}",
            workload_profile.rebuild_iterations
        ),
        format!("sync_workload_kind={workload_label}"),
        format!(
            "workload_confirmed_transactions={}",
            workload_profile.confirmed_count
        ),
        format!(
            "workload_pending_transactions={}",
            workload_profile.pending_count
        ),
    ])
}

fn scenario_validation_notes(
    session: &PerfSession,
    scenario: &PerfScenarioDefinition,
    manifest: &PerfDatasetManifest,
) -> Result<Vec<String>, PerfError> {
    match scenario.id {
        SCENARIO_WALLETS_EMPTY_READ => Ok(Vec::new()),
        SCENARIO_WALLETS_LARGE_READ => validate_large_wallets_dataset(session, manifest),
        SCENARIO_ACCOUNT_TRANSACTIONS_LARGE_READ => {
            validate_large_account_transactions_dataset(session, manifest, scenario.id)
        }
        SCENARIO_UTXO_TRANSACTIONS_LARGE_READ => {
            validate_large_account_transactions_dataset(session, manifest, scenario.id)
        }
        SCENARIO_SYNC_STATE_LARGE_READ => validate_large_sync_state_dataset(session, manifest),
        SCENARIO_AUTH_RESTORE_DURING_USER_DB_LOAD | SCENARIO_AUTH_RESTORE_DURING_APP_DB_WRITES => {
            Ok(Vec::new())
        }
        SCENARIO_READS_DURING_EXPORT => Ok(Vec::new()),
        SCENARIO_READS_DURING_SYNC
        | SCENARIO_READS_DURING_HEAVY_SYNC
        | SCENARIO_SETTINGS_WRITES_DURING_SYNC
        | SCENARIO_UTXO_READS_DURING_SYNC => Ok(Vec::new()),
        other => Err(PerfError::usage(format!(
            "unsupported scenario validation for '{other}'"
        ))),
    }
}

fn validate_sync_overlap_scenario(
    session: &PerfSession,
    scenario: &PerfScenarioDefinition,
    manifest: &PerfDatasetManifest,
    workload_notes: &[String],
) -> Result<Vec<String>, PerfError> {
    let account_id = manifest.primary_account_id.ok_or_else(|| {
        PerfError::usage(format!(
            "{} requires a dataset manifest with primary_account_id",
            scenario.id
        ))
    })?;
    let account_transactions = fetch_account_transactions(session, account_id)?;
    let wallet_counts = fetch_wallet_counts(session)?;

    if manifest.account_count != Some(wallet_counts.account_count) {
        return Err(PerfError::HttpClient(format!(
            "{} expected {} accounts but observed {}",
            scenario.id,
            manifest.account_count.unwrap_or_default(),
            wallet_counts.account_count
        )));
    }
    if manifest.address_count != Some(wallet_counts.account_count) {
        return Err(PerfError::HttpClient(format!(
            "{} expected {} addresses but observed {}",
            scenario.id,
            manifest.address_count.unwrap_or_default(),
            wallet_counts.account_count
        )));
    }
    let mut notes = vec![
        format!(
            "primary_account_confirmed_transactions={}",
            account_transactions.confirmed.total
        ),
        format!(
            "primary_account_pending_transactions={}",
            account_transactions.pending.total
        ),
        format!("wallet_count={}", wallet_counts.wallet_count),
        format!("account_count={}", wallet_counts.account_count),
    ];
    if scenario.id != SCENARIO_UTXO_READS_DURING_SYNC {
        let sync_snapshot = fetch_sync_state(session)?;
        if manifest.address_count != Some(sync_snapshot.addresses_total.value()) {
            return Err(PerfError::HttpClient(format!(
                "{} expected {} sync-state addresses but observed {}",
                scenario.id,
                manifest.address_count.unwrap_or_default(),
                sync_snapshot.addresses_total.value()
            )));
        }
        notes.push(format!(
            "sync_state_is_running={}",
            sync_snapshot.is_running
        ));
        notes.push(format!(
            "sync_addresses_total={}",
            sync_snapshot.addresses_total.value()
        ));
    }
    notes.extend(workload_notes.iter().cloned());

    if scenario.id == SCENARIO_SETTINGS_WRITES_DURING_SYNC {
        let settings = fetch_settings(session)?;
        let observed = settings.currency.as_ref().map(|c| c.code().to_string());
        if observed.as_deref() != Some(SETTINGS_SYNC_CURRENCY_CODE) {
            return Err(PerfError::HttpClient(format!(
                "settings-writes-during-sync expected saved currency {:?} but observed {:?}",
                SETTINGS_SYNC_CURRENCY_CODE, observed
            )));
        }
        notes.push(format!("saved_currency={}", SETTINGS_SYNC_CURRENCY_CODE));
    }

    Ok(notes)
}

fn validate_export_overlap_scenario(
    session: &PerfSession,
    manifest: &PerfDatasetManifest,
    workload_notes: &[String],
) -> Result<Vec<String>, PerfError> {
    let account_id = manifest.primary_account_id.ok_or_else(|| {
        PerfError::usage("reads-during-export requires a dataset manifest with primary_account_id")
    })?;
    let account_transactions = fetch_account_transactions(session, account_id)?;
    let wallet_counts = fetch_wallet_counts(session)?;
    let mut notes = vec![
        format!("wallet_count={}", wallet_counts.wallet_count),
        format!("account_count={}", wallet_counts.account_count),
        format!("confirmed_total={}", account_transactions.confirmed.total),
        format!("pending_total={}", account_transactions.pending.total),
    ];
    notes.extend(workload_notes.iter().cloned());
    Ok(notes)
}

fn validate_large_wallets_dataset(
    session: &PerfSession,
    manifest: &PerfDatasetManifest,
) -> Result<Vec<String>, PerfError> {
    let counts = fetch_wallet_counts(session)?;
    if manifest.account_count != Some(counts.account_count) {
        return Err(PerfError::HttpClient(format!(
            "wallets-large-read expected {} accounts but observed {}",
            manifest.account_count.unwrap_or_default(),
            counts.account_count
        )));
    }
    if manifest.address_count != Some(counts.account_count) {
        return Err(PerfError::HttpClient(format!(
            "wallets-large-read expected {} addresses but observed {}",
            manifest.address_count.unwrap_or_default(),
            counts.account_count
        )));
    }
    Ok(vec![
        format!("wallet_count={}", counts.wallet_count),
        format!("account_count={}", counts.account_count),
    ])
}

fn validate_large_account_transactions_dataset(
    session: &PerfSession,
    manifest: &PerfDatasetManifest,
    scenario_id: &str,
) -> Result<Vec<String>, PerfError> {
    let account_id = manifest.primary_account_id.ok_or_else(|| {
        PerfError::usage(format!(
            "{scenario_id} requires a dataset manifest with primary_account_id"
        ))
    })?;
    let account_transactions = fetch_account_transactions(session, account_id)?;
    let total_rows = account_transactions
        .confirmed
        .total
        .saturating_add(account_transactions.pending.total);
    let expected_minimum = manifest.rough_row_count_marker.unwrap_or(0);
    if u64::from(total_rows) < expected_minimum {
        return Err(PerfError::HttpClient(format!(
            "account-transactions-large-read expected at least {expected_minimum} rows but observed {total_rows}",
        )));
    }
    Ok(vec![
        format!("confirmed_total={}", account_transactions.confirmed.total),
        format!("pending_total={}", account_transactions.pending.total),
        format!(
            "confirmed_rows_returned={}",
            account_transactions.confirmed.rows.len()
        ),
        format!(
            "pending_rows_returned={}",
            account_transactions.pending.rows.len()
        ),
    ])
}

fn validate_large_sync_state_dataset(
    session: &PerfSession,
    manifest: &PerfDatasetManifest,
) -> Result<Vec<String>, PerfError> {
    let snapshot = fetch_sync_state(session)?;
    let addresses_total = snapshot.addresses_total.value();
    if manifest.address_count != Some(addresses_total) {
        return Err(PerfError::HttpClient(format!(
            "sync-state-large-read expected {} addresses but observed {addresses_total}",
            manifest.address_count.unwrap_or_default()
        )));
    }
    Ok(vec![
        format!("sync_state_is_running={}", snapshot.is_running),
        format!("sync_addresses_total={}", snapshot.addresses_total.value()),
        format!(
            "sync_addresses_synced={}",
            snapshot.addresses_synced.value()
        ),
        format!(
            "sync_addresses_failed={}",
            snapshot.addresses_failed.value()
        ),
    ])
}

fn scenario_request_specs(
    session: &PerfSession,
    scenario: &PerfScenarioDefinition,
    manifest: &PerfDatasetManifest,
) -> Result<Vec<PerfRequestSpec>, PerfError> {
    use super::http_client::{PerfRequestBody, PerfRequestMethod};

    match scenario.id {
        SCENARIO_WALLETS_EMPTY_READ | SCENARIO_WALLETS_LARGE_READ => Ok(vec![PerfRequestSpec {
            method: PerfRequestMethod::Get,
            url: endpoint_url(&session.base_url, "_app/user/wallets"),
            body: PerfRequestBody::Empty,
            context: "perf wallets GET",
        }]),
        SCENARIO_AUTH_RESTORE_DURING_USER_DB_LOAD | SCENARIO_AUTH_RESTORE_DURING_APP_DB_WRITES => {
            Ok(vec![PerfRequestSpec {
                method: PerfRequestMethod::Get,
                url: endpoint_url(&session.base_url, "_app/auth/me"),
                body: PerfRequestBody::Empty,
                context: "perf auth me GET",
            }])
        }
        SCENARIO_ACCOUNT_TRANSACTIONS_LARGE_READ | SCENARIO_UTXO_TRANSACTIONS_LARGE_READ => {
            let account_id = manifest.primary_account_id.ok_or_else(|| {
                PerfError::usage(format!(
                    "{} requires a dataset manifest with primary_account_id",
                    scenario.id
                ))
            })?;
            Ok(vec![PerfRequestSpec {
                method: PerfRequestMethod::Get,
                url: endpoint_url(
                    &session.base_url,
                    &format!("_app/user/account/{account_id}/transactions"),
                ),
                body: PerfRequestBody::Empty,
                context: "perf account transactions GET",
            }])
        }
        SCENARIO_READS_DURING_EXPORT => {
            let account_id = manifest.primary_account_id.ok_or_else(|| {
                PerfError::usage(
                    "reads-during-export requires a dataset manifest with primary_account_id",
                )
            })?;
            Ok(vec![
                PerfRequestSpec {
                    method: PerfRequestMethod::Get,
                    url: endpoint_url(&session.base_url, "_app/user/wallets"),
                    body: PerfRequestBody::Empty,
                    context: "perf wallets GET",
                },
                PerfRequestSpec {
                    method: PerfRequestMethod::Get,
                    url: endpoint_url(
                        &session.base_url,
                        &format!("_app/user/account/{account_id}/transactions"),
                    ),
                    body: PerfRequestBody::Empty,
                    context: "perf account transactions GET",
                },
            ])
        }
        SCENARIO_SYNC_STATE_LARGE_READ => Ok(vec![PerfRequestSpec {
            method: PerfRequestMethod::Get,
            url: endpoint_url(&session.base_url, "_app/user/transactions/sync/state"),
            body: PerfRequestBody::Empty,
            context: "perf sync state GET",
        }]),
        SCENARIO_READS_DURING_SYNC
        | SCENARIO_READS_DURING_HEAVY_SYNC
        | SCENARIO_SETTINGS_WRITES_DURING_SYNC
        | SCENARIO_UTXO_READS_DURING_SYNC => {
            let account_id = manifest.primary_account_id.ok_or_else(|| {
                PerfError::usage(format!(
                    "{} requires a dataset manifest with primary_account_id",
                    scenario.id
                ))
            })?;
            let mut requests = vec![
                PerfRequestSpec {
                    method: PerfRequestMethod::Get,
                    url: endpoint_url(&session.base_url, "_app/user/wallets"),
                    body: PerfRequestBody::Empty,
                    context: "perf wallets GET",
                },
                PerfRequestSpec {
                    method: PerfRequestMethod::Get,
                    url: endpoint_url(
                        &session.base_url,
                        &format!("_app/user/account/{account_id}/transactions"),
                    ),
                    body: PerfRequestBody::Empty,
                    context: "perf account transactions GET",
                },
            ];
            if scenario.id != SCENARIO_UTXO_READS_DURING_SYNC {
                requests.push(PerfRequestSpec {
                    method: PerfRequestMethod::Get,
                    url: endpoint_url(&session.base_url, "_app/user/transactions/sync/state"),
                    body: PerfRequestBody::Empty,
                    context: "perf sync state GET",
                });
            }
            if scenario.id == SCENARIO_SETTINGS_WRITES_DURING_SYNC {
                requests.insert(
                    1,
                    PerfRequestSpec {
                        method: PerfRequestMethod::Post,
                        url: endpoint_url(&session.base_url, "_app/user/settings/currency"),
                        body: PerfRequestBody::Json(json!({
                            "currency": SETTINGS_SYNC_CURRENCY_CODE,
                        })),
                        context: "perf settings currency POST",
                    },
                );
            }
            Ok(requests)
        }
        other => Err(PerfError::usage(format!(
            "unsupported scenario request specs for '{other}'"
        ))),
    }
}
