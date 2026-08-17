use super::http_client::register_session;
use super::scenarios::PerfScenarioDefinition;
use super::{PerfError, PerfRunId};
use crate::amounts::UnsignedAmount;
use crate::db::{
    AddEthAddressDbResult, AddressSyncSuccess, ProviderTransferKey, SyncAccountTransactionRecord,
    SyncAccountTransferRecord, SyncTransactionInputRecord, SyncTransactionOutputRecord,
    SyncTransactionRecord, add_bitcoin_address, add_ethereum_address,
    mark_address_sync_completed_failure, mark_address_sync_completed_success,
    rebuild_account_transaction_ledger, reconcile_account_transactions,
    reconcile_address_transactions,
};
use crate::ethereum::{EthAddress, RawEthAddress, TransferKind};
use crate::models::UserId;
use crate::transactions::{
    ChainTipHeight, ChainTransactionStatus, SyncErrorMessage, TrackedAddress, TransactionSyncRunId,
    TxHash,
};
use crate::wallets::{
    BtcAddress, DigitalAssetAccountId, DigitalAssetAddressId, Label, Network, RawBtcAddress,
    SyncedAssetId, WALLET_LABEL_MAX_LENGTH,
};
use chrono::{Duration as ChronoDuration, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{read_to_string, write};

const DATASET_MANIFEST_FILENAME: &str = "perf-dataset.json";
pub(super) const DEFAULT_PASSWORD: &str = "SecurePass123";

pub(super) const SYNC_LEDGER_REBUILD_ITERATIONS: u32 = 12;
pub(super) const LARGE_WALLETS_WALLET_COUNT: u32 = 24;
pub(super) const LARGE_WALLETS_ACCOUNTS_PER_WALLET: u32 = 3;
pub(super) const LARGE_ACCOUNT_TRANSACTIONS_CONFIRMED_COUNT: u32 = 180;
pub(super) const LARGE_ACCOUNT_TRANSACTIONS_PENDING_COUNT: u32 = 24;
const LARGE_ACCOUNT_TRANSACTIONS_PRIMARY_ADDRESS_INDEX: u64 = 4_096;
const LARGE_SYNC_STATE_START_ADDRESS_INDEX: u64 = 2_000_000;
const LARGE_SYNC_STATE_WALLET_COUNT: u32 = 20;
const LARGE_SYNC_STATE_ACCOUNTS_PER_WALLET: u32 = 4;
const LARGE_SYNC_STATE_FAILURE_MODULO: u32 = 5;
pub(super) const HEAVY_SYNC_OVERLAP_WALLET_COUNT: u32 = 48;
pub(super) const HEAVY_SYNC_OVERLAP_ACCOUNTS_PER_WALLET: u32 = 4;
pub(super) const HEAVY_SYNC_OVERLAP_CONFIRMED_COUNT: u32 = 720;
pub(super) const HEAVY_SYNC_OVERLAP_PENDING_COUNT: u32 = 96;
const HEAVY_SYNC_OVERLAP_START_ADDRESS_INDEX: u64 = 3_000_000;
pub(super) const HEAVY_SYNC_LEDGER_REBUILD_ITERATIONS: u32 = 24;
pub(super) const EXPORT_WORKLOAD_ITERATIONS: u32 = 4;
pub(super) const APP_DB_WRITE_ITERATIONS: u32 = 512;
pub(super) const SETTINGS_SYNC_CURRENCY_CODE: &str = "EUR";
pub(super) const PERF_OWNED_BTC_ADDRESS: &str = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PerfSyncWorkloadKind {
    AccountModel,
    Utxo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PerfSyncWorkloadProfile {
    pub(super) confirmed_count: u32,
    pub(super) pending_count: u32,
    pub(super) rebuild_iterations: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct PerfDatasetManifest {
    pub(super) dataset_id: String,
    pub(super) dataset_shape: String,
    pub(super) username: String,
    pub(super) password: String,
    pub(super) user_id: UserId,
    pub(super) primary_account_id: Option<DigitalAssetAccountId>,
    pub(super) primary_tracked_address: Option<String>,
    pub(super) workload_user_id: Option<UserId>,
    pub(super) workload_account_id: Option<DigitalAssetAccountId>,
    pub(super) workload_tracked_address: Option<String>,
    pub(super) rough_row_count_marker: Option<u64>,
    pub(super) account_count: Option<u32>,
    pub(super) address_count: Option<u32>,
    pub(super) sync_active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct WalletDatasetCounts {
    pub(super) account_count: u32,
    pub(super) address_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AccountTransactionDatasetCounts {
    pub(super) account_id: DigitalAssetAccountId,
    pub(super) address_id: DigitalAssetAddressId,
    pub(super) tracked_address: TrackedAddress,
    pub(super) confirmed_count: u32,
    pub(super) pending_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SyncOverlapDatasetCounts {
    pub(super) account_id: DigitalAssetAccountId,
    pub(super) tracked_address: TrackedAddress,
    pub(super) account_count: u32,
    pub(super) address_count: u32,
    pub(super) confirmed_count: u32,
    pub(super) pending_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SyncStateDatasetCounts {
    pub(super) account_count: u32,
    pub(super) address_count: u32,
}

pub(super) fn create_dataset_manifest(
    request: &super::PerfRunRequest,
    scenario: &PerfScenarioDefinition,
    base_url: &str,
) -> Result<PerfDatasetManifest, PerfError> {
    let dataset_id = if request.dataset_id.as_str() == crate::perf::PLACEHOLDER_DATASET_ID {
        scenario.default_dataset_id.to_string()
    } else {
        request.dataset_id.as_str().to_string()
    };

    match dataset_id.as_str() {
        "tiny-empty" => create_tiny_empty_dataset_manifest(&dataset_id, scenario, base_url),
        super::scenarios::DATASET_WALLETS_MANY_ACCOUNTS => {
            create_large_wallets_dataset_manifest(&dataset_id, scenario, base_url)
        }
        super::scenarios::DATASET_ACCOUNT_TRANSACTIONS_HEAVY => {
            create_large_account_transactions_dataset_manifest(&dataset_id, scenario, base_url)
        }
        super::scenarios::DATASET_UTXO_TRANSACTIONS_HEAVY => {
            create_large_utxo_transactions_dataset_manifest(&dataset_id, scenario, base_url)
        }
        super::scenarios::DATASET_SYNC_STATE_MANY_ADDRESSES => {
            create_large_sync_state_dataset_manifest(&dataset_id, scenario, base_url)
        }
        super::scenarios::DATASET_AUTH_RESTORE_CROSS_USER => {
            create_auth_restore_cross_user_dataset_manifest(&dataset_id, scenario, base_url)
        }
        super::scenarios::DATASET_SYNC_MOCK_HEAVY => {
            create_sync_mock_heavy_dataset_manifest(&dataset_id, scenario, base_url)
        }
        super::scenarios::DATASET_SYNC_OVERLAP_HEAVY => {
            create_sync_overlap_heavy_dataset_manifest(&dataset_id, scenario, base_url)
        }
        other => Err(PerfError::usage(format!(
            "unsupported generated dataset '{other}' for this phase"
        ))),
    }
}

fn create_tiny_empty_dataset_manifest(
    dataset_id: &str,
    scenario: &PerfScenarioDefinition,
    base_url: &str,
) -> Result<PerfDatasetManifest, PerfError> {
    let username = format!("perf_{}", PerfRunId::new().as_str().to_ascii_lowercase());
    let session = register_session(base_url, &username, DEFAULT_PASSWORD)?;
    let manifest = PerfDatasetManifest {
        dataset_id: dataset_id.to_string(),
        dataset_shape: scenario.default_dataset_shape.to_string(),
        username,
        password: DEFAULT_PASSWORD.to_string(),
        user_id: session.user_id,
        primary_account_id: None,
        primary_tracked_address: None,
        workload_user_id: None,
        workload_account_id: None,
        workload_tracked_address: None,
        rough_row_count_marker: Some(0),
        account_count: Some(0),
        address_count: Some(0),
        sync_active: false,
    };
    write_dataset_manifest(&manifest)?;
    Ok(manifest)
}

fn create_sync_mock_heavy_dataset_manifest(
    dataset_id: &str,
    scenario: &PerfScenarioDefinition,
    base_url: &str,
) -> Result<PerfDatasetManifest, PerfError> {
    let username = format!("perf_{}", PerfRunId::new().as_str().to_ascii_lowercase());
    let session = register_session(base_url, &username, DEFAULT_PASSWORD)?;
    let primary_account = seed_large_account_transactions_dataset(
        session.user_id,
        LARGE_ACCOUNT_TRANSACTIONS_CONFIRMED_COUNT,
        LARGE_ACCOUNT_TRANSACTIONS_PENDING_COUNT,
    )?;
    let manifest = PerfDatasetManifest {
        dataset_id: dataset_id.to_string(),
        dataset_shape: scenario.default_dataset_shape.to_string(),
        username,
        password: DEFAULT_PASSWORD.to_string(),
        user_id: session.user_id,
        primary_account_id: Some(primary_account.account_id),
        primary_tracked_address: Some(primary_account.tracked_address.as_str().to_string()),
        workload_user_id: None,
        workload_account_id: None,
        workload_tracked_address: None,
        rough_row_count_marker: Some(u64::from(
            primary_account
                .confirmed_count
                .saturating_add(primary_account.pending_count),
        )),
        account_count: Some(1),
        address_count: Some(1),
        sync_active: false,
    };
    write_dataset_manifest(&manifest)?;
    Ok(manifest)
}

fn create_sync_overlap_heavy_dataset_manifest(
    dataset_id: &str,
    scenario: &PerfScenarioDefinition,
    base_url: &str,
) -> Result<PerfDatasetManifest, PerfError> {
    let username = format!("perf_{}", PerfRunId::new().as_str().to_ascii_lowercase());
    let session = register_session(base_url, &username, DEFAULT_PASSWORD)?;
    let counts = seed_heavy_sync_overlap_dataset(session.user_id)?;
    let manifest = PerfDatasetManifest {
        dataset_id: dataset_id.to_string(),
        dataset_shape: scenario.default_dataset_shape.to_string(),
        username,
        password: DEFAULT_PASSWORD.to_string(),
        user_id: session.user_id,
        primary_account_id: Some(counts.account_id),
        primary_tracked_address: Some(counts.tracked_address.as_str().to_string()),
        workload_user_id: None,
        workload_account_id: None,
        workload_tracked_address: None,
        rough_row_count_marker: Some(
            u64::from(counts.account_count)
                + u64::from(counts.confirmed_count)
                + u64::from(counts.pending_count),
        ),
        account_count: Some(counts.account_count),
        address_count: Some(counts.address_count),
        sync_active: false,
    };
    write_dataset_manifest(&manifest)?;
    Ok(manifest)
}

fn create_large_wallets_dataset_manifest(
    dataset_id: &str,
    scenario: &PerfScenarioDefinition,
    base_url: &str,
) -> Result<PerfDatasetManifest, PerfError> {
    let username = format!("perf_{}", PerfRunId::new().as_str().to_ascii_lowercase());
    let session = register_session(base_url, &username, DEFAULT_PASSWORD)?;
    let counts = seed_large_wallets_dataset(
        session.user_id,
        LARGE_WALLETS_WALLET_COUNT,
        LARGE_WALLETS_ACCOUNTS_PER_WALLET,
    )?;
    let manifest = PerfDatasetManifest {
        dataset_id: dataset_id.to_string(),
        dataset_shape: scenario.default_dataset_shape.to_string(),
        username,
        password: DEFAULT_PASSWORD.to_string(),
        user_id: session.user_id,
        primary_account_id: None,
        primary_tracked_address: None,
        workload_user_id: None,
        workload_account_id: None,
        workload_tracked_address: None,
        rough_row_count_marker: Some(u64::from(counts.account_count)),
        account_count: Some(counts.account_count),
        address_count: Some(counts.address_count),
        sync_active: false,
    };
    write_dataset_manifest(&manifest)?;
    Ok(manifest)
}

fn create_large_account_transactions_dataset_manifest(
    dataset_id: &str,
    scenario: &PerfScenarioDefinition,
    base_url: &str,
) -> Result<PerfDatasetManifest, PerfError> {
    let username = format!("perf_{}", PerfRunId::new().as_str().to_ascii_lowercase());
    let session = register_session(base_url, &username, DEFAULT_PASSWORD)?;
    let counts = seed_large_account_transactions_dataset(
        session.user_id,
        LARGE_ACCOUNT_TRANSACTIONS_CONFIRMED_COUNT,
        LARGE_ACCOUNT_TRANSACTIONS_PENDING_COUNT,
    )?;
    let manifest = PerfDatasetManifest {
        dataset_id: dataset_id.to_string(),
        dataset_shape: scenario.default_dataset_shape.to_string(),
        username,
        password: DEFAULT_PASSWORD.to_string(),
        user_id: session.user_id,
        primary_account_id: Some(counts.account_id),
        primary_tracked_address: Some(counts.tracked_address.as_str().to_string()),
        workload_user_id: None,
        workload_account_id: None,
        workload_tracked_address: None,
        rough_row_count_marker: Some(u64::from(
            counts.confirmed_count.saturating_add(counts.pending_count),
        )),
        account_count: Some(1),
        address_count: Some(1),
        sync_active: false,
    };
    write_dataset_manifest(&manifest)?;
    Ok(manifest)
}

fn create_large_utxo_transactions_dataset_manifest(
    dataset_id: &str,
    scenario: &PerfScenarioDefinition,
    base_url: &str,
) -> Result<PerfDatasetManifest, PerfError> {
    let username = format!("perf_{}", PerfRunId::new().as_str().to_ascii_lowercase());
    let session = register_session(base_url, &username, DEFAULT_PASSWORD)?;
    let counts = seed_large_utxo_transactions_dataset(
        session.user_id,
        LARGE_ACCOUNT_TRANSACTIONS_CONFIRMED_COUNT,
        LARGE_ACCOUNT_TRANSACTIONS_PENDING_COUNT,
    )?;
    let manifest = PerfDatasetManifest {
        dataset_id: dataset_id.to_string(),
        dataset_shape: scenario.default_dataset_shape.to_string(),
        username,
        password: DEFAULT_PASSWORD.to_string(),
        user_id: session.user_id,
        primary_account_id: Some(counts.account_id),
        primary_tracked_address: Some(counts.tracked_address.as_str().to_string()),
        workload_user_id: None,
        workload_account_id: None,
        workload_tracked_address: None,
        rough_row_count_marker: Some(u64::from(
            counts.confirmed_count.saturating_add(counts.pending_count),
        )),
        account_count: Some(1),
        address_count: Some(1),
        sync_active: false,
    };
    write_dataset_manifest(&manifest)?;
    Ok(manifest)
}

fn create_large_sync_state_dataset_manifest(
    dataset_id: &str,
    scenario: &PerfScenarioDefinition,
    base_url: &str,
) -> Result<PerfDatasetManifest, PerfError> {
    let username = format!("perf_{}", PerfRunId::new().as_str().to_ascii_lowercase());
    let session = register_session(base_url, &username, DEFAULT_PASSWORD)?;
    let counts = seed_large_sync_state_dataset(
        session.user_id,
        LARGE_SYNC_STATE_WALLET_COUNT,
        LARGE_SYNC_STATE_ACCOUNTS_PER_WALLET,
    )?;
    let manifest = PerfDatasetManifest {
        dataset_id: dataset_id.to_string(),
        dataset_shape: scenario.default_dataset_shape.to_string(),
        username,
        password: DEFAULT_PASSWORD.to_string(),
        user_id: session.user_id,
        primary_account_id: None,
        primary_tracked_address: None,
        workload_user_id: None,
        workload_account_id: None,
        workload_tracked_address: None,
        rough_row_count_marker: Some(u64::from(counts.address_count)),
        account_count: Some(counts.account_count),
        address_count: Some(counts.address_count),
        sync_active: false,
    };
    write_dataset_manifest(&manifest)?;
    Ok(manifest)
}

fn create_auth_restore_cross_user_dataset_manifest(
    dataset_id: &str,
    scenario: &PerfScenarioDefinition,
    base_url: &str,
) -> Result<PerfDatasetManifest, PerfError> {
    let primary_username = format!("perf_{}", PerfRunId::new().as_str().to_ascii_lowercase());
    let primary_session = register_session(base_url, &primary_username, DEFAULT_PASSWORD)?;

    let workload_username = format!("perf_{}", PerfRunId::new().as_str().to_ascii_lowercase());
    let workload_session = register_session(base_url, &workload_username, DEFAULT_PASSWORD)?;
    let workload_counts = seed_large_account_transactions_dataset(
        workload_session.user_id,
        LARGE_ACCOUNT_TRANSACTIONS_CONFIRMED_COUNT,
        LARGE_ACCOUNT_TRANSACTIONS_PENDING_COUNT,
    )?;

    let manifest = PerfDatasetManifest {
        dataset_id: dataset_id.to_string(),
        dataset_shape: scenario.default_dataset_shape.to_string(),
        username: primary_username,
        password: DEFAULT_PASSWORD.to_string(),
        user_id: primary_session.user_id,
        primary_account_id: None,
        primary_tracked_address: None,
        workload_user_id: Some(workload_session.user_id),
        workload_account_id: Some(workload_counts.account_id),
        workload_tracked_address: Some(workload_counts.tracked_address.as_str().to_string()),
        rough_row_count_marker: Some(u64::from(
            workload_counts
                .confirmed_count
                .saturating_add(workload_counts.pending_count),
        )),
        account_count: Some(1),
        address_count: Some(1),
        sync_active: false,
    };
    write_dataset_manifest(&manifest)?;
    Ok(manifest)
}

fn seed_large_wallets_dataset(
    user_id: UserId,
    wallet_count: u32,
    accounts_per_wallet: u32,
) -> Result<WalletDatasetCounts, PerfError> {
    let seeded_accounts = seed_wallet_accounts(user_id, wallet_count, accounts_per_wallet)?;
    let account_count = u32::try_from(seeded_accounts.len())
        .map_err(|_| PerfError::io("perf seeded account count exceeded u32"))?;
    Ok(WalletDatasetCounts {
        account_count,
        address_count: account_count,
    })
}

pub(super) fn seed_large_account_transactions_dataset(
    user_id: UserId,
    confirmed_count: u32,
    pending_count: u32,
) -> Result<AccountTransactionDatasetCounts, PerfError> {
    let observed_at = perf_seed_timestamp()?;
    let wallet_label = parse_wallet_label("Perf Account Tx Wallet")?;
    let owned_address_value =
        generated_eth_address_value(LARGE_ACCOUNT_TRANSACTIONS_PRIMARY_ADDRESS_INDEX);
    let owned_address = parse_generated_eth_address(owned_address_value.clone())?;
    let tracked_address = parse_generated_tracked_address(owned_address_value.clone())?;
    let seeded_account = add_ethereum_address(
        user_id,
        &owned_address,
        Network::Mainnet,
        None,
        Some(&wallet_label),
        observed_at,
    )
    .map_err(|err| PerfError::io(format!("failed to seed perf transaction account: {err}")))?;
    let records =
        build_large_account_transaction_records(&tracked_address, confirmed_count, pending_count)?;
    reconcile_account_transactions(
        user_id,
        SyncedAssetId::Ethereum,
        Network::Mainnet,
        &records,
        observed_at,
    )
    .map_err(|err| PerfError::io(format!("failed to seed perf account transactions: {err}")))?;
    rebuild_account_transaction_ledger(user_id, seeded_account.account_id, observed_at).map_err(
        |err| PerfError::io(format!("failed to rebuild perf transaction ledger: {err}")),
    )?;
    Ok(AccountTransactionDatasetCounts {
        account_id: seeded_account.account_id,
        address_id: seeded_account.address_id,
        tracked_address,
        confirmed_count,
        pending_count,
    })
}

pub(super) fn seed_large_utxo_transactions_dataset(
    user_id: UserId,
    confirmed_count: u32,
    pending_count: u32,
) -> Result<AccountTransactionDatasetCounts, PerfError> {
    let observed_at = perf_seed_timestamp()?;
    let wallet_label = parse_wallet_label("Perf Bitcoin Tx Wallet")?;
    let owned_address = parse_perf_btc_address(PERF_OWNED_BTC_ADDRESS, Network::Mainnet)?;
    let tracked_address = parse_generated_tracked_address(owned_address.canonical().to_string())?;
    let seeded_account = add_bitcoin_address(
        user_id,
        &owned_address,
        Network::Mainnet,
        None,
        Some(&wallet_label),
        observed_at,
    )
    .map_err(|err| PerfError::io(format!("failed to seed perf bitcoin account: {err}")))?;
    let records =
        build_large_utxo_transaction_records(&tracked_address, confirmed_count, pending_count)?;
    reconcile_address_transactions(
        user_id,
        SyncedAssetId::Bitcoin,
        Network::Mainnet,
        &records,
        observed_at,
    )
    .map_err(|err| PerfError::io(format!("failed to seed perf UTXO transactions: {err}")))?;
    rebuild_account_transaction_ledger(user_id, seeded_account.account_id, observed_at)
        .map_err(|err| PerfError::io(format!("failed to rebuild perf UTXO ledger: {err}")))?;
    Ok(AccountTransactionDatasetCounts {
        account_id: seeded_account.account_id,
        address_id: seeded_account.address_id,
        tracked_address,
        confirmed_count,
        pending_count,
    })
}

fn seed_large_sync_state_dataset(
    user_id: UserId,
    wallet_count: u32,
    accounts_per_wallet: u32,
) -> Result<SyncStateDatasetCounts, PerfError> {
    let seeded_accounts = seed_wallet_accounts_with_seed_range(
        user_id,
        wallet_count,
        accounts_per_wallet,
        "Perf Sync State Wallet",
        LARGE_SYNC_STATE_START_ADDRESS_INDEX,
    )?;
    let address_ids = seeded_accounts
        .iter()
        .map(|account| account.address_id)
        .collect::<Vec<_>>();
    seed_mixed_sync_state_for_address_ids(user_id, &address_ids)?;

    let account_count = u32::try_from(seeded_accounts.len())
        .map_err(|_| PerfError::io("perf seeded sync account count exceeded u32"))?;
    Ok(SyncStateDatasetCounts {
        account_count,
        address_count: account_count,
    })
}

pub(super) fn seed_heavy_sync_overlap_dataset(
    user_id: UserId,
) -> Result<SyncOverlapDatasetCounts, PerfError> {
    let primary_account = seed_large_account_transactions_dataset(
        user_id,
        HEAVY_SYNC_OVERLAP_CONFIRMED_COUNT,
        HEAVY_SYNC_OVERLAP_PENDING_COUNT,
    )?;
    let seeded_accounts = seed_wallet_accounts_with_seed_range(
        user_id,
        HEAVY_SYNC_OVERLAP_WALLET_COUNT,
        HEAVY_SYNC_OVERLAP_ACCOUNTS_PER_WALLET,
        "Perf Heavy Sync Wallet",
        HEAVY_SYNC_OVERLAP_START_ADDRESS_INDEX,
    )?;
    let mut address_ids = Vec::with_capacity(seeded_accounts.len().saturating_add(1));
    address_ids.push(primary_account.address_id);
    address_ids.extend(seeded_accounts.iter().map(|account| account.address_id));
    seed_mixed_sync_state_for_address_ids(user_id, &address_ids)?;

    let extra_account_count = u32::try_from(seeded_accounts.len())
        .map_err(|_| PerfError::io("heavy sync overlap account count exceeded u32"))?;
    let account_count = extra_account_count.saturating_add(1);
    Ok(SyncOverlapDatasetCounts {
        account_id: primary_account.account_id,
        tracked_address: primary_account.tracked_address,
        account_count,
        address_count: account_count,
        confirmed_count: HEAVY_SYNC_OVERLAP_CONFIRMED_COUNT,
        pending_count: HEAVY_SYNC_OVERLAP_PENDING_COUNT,
    })
}

fn seed_mixed_sync_state_for_address_ids(
    user_id: UserId,
    address_ids: &[DigitalAssetAddressId],
) -> Result<(), PerfError> {
    let observed_at = perf_seed_timestamp()?;
    let last_tip_height = ChainTipHeight::try_new(950_000)
        .map_err(|err| PerfError::io(format!("failed to build perf chain tip height: {err}")))?;
    let failure_modulo = usize::try_from(LARGE_SYNC_STATE_FAILURE_MODULO)
        .map_err(|_| PerfError::io("perf failure modulo exceeded usize"))?;
    for (index, address_id) in address_ids.iter().enumerate() {
        let run_id = TransactionSyncRunId::new();
        let minute_offset = i64::try_from(index)
            .map_err(|_| PerfError::io("perf sync-state index exceeded i64"))?;
        let started_at = observed_at + ChronoDuration::minutes(minute_offset);
        let completed_at = started_at + ChronoDuration::seconds(30);
        if (index + 1) % failure_modulo == 0 {
            let error =
                SyncErrorMessage::sanitize(format!("Perf sync failure for address {address_id}",));
            mark_address_sync_completed_failure(
                user_id,
                *address_id,
                run_id,
                started_at,
                completed_at,
                &error,
                true,
            )
            .map_err(|err| PerfError::io(format!("failed to seed perf sync failure: {err}")))?;
        } else {
            let success = AddressSyncSuccess {
                address_id: *address_id,
                run_id,
                started_at,
                completed_at,
                last_tip_height,
                new_tx_count: crate::transactions::TransactionCount::from_u32(
                    (index % 3) as u32 + 1,
                ),
                updated_tx_count: crate::transactions::TransactionCount::from_u32(
                    (index % 2) as u32,
                ),
                api_confirmed_balance: None,
            };
            mark_address_sync_completed_success(user_id, &success)
                .map_err(|err| PerfError::io(format!("failed to seed perf sync success: {err}")))?;
        }
    }
    Ok(())
}

fn seed_wallet_accounts(
    user_id: UserId,
    wallet_count: u32,
    accounts_per_wallet: u32,
) -> Result<Vec<AddEthAddressDbResult>, PerfError> {
    seed_wallet_accounts_with_seed_range(
        user_id,
        wallet_count,
        accounts_per_wallet,
        "Perf Wallet",
        1,
    )
}

fn seed_wallet_accounts_with_seed_range(
    user_id: UserId,
    wallet_count: u32,
    accounts_per_wallet: u32,
    wallet_label_prefix: &str,
    starting_address_index: u64,
) -> Result<Vec<AddEthAddressDbResult>, PerfError> {
    if wallet_count == 0 {
        return Ok(Vec::new());
    }
    if accounts_per_wallet == 0 {
        return Err(PerfError::usage(
            "perf dataset accounts_per_wallet must be greater than zero",
        ));
    }

    let observed_at = perf_seed_timestamp()?;
    let mut next_address_index = starting_address_index;
    let mut seeded_accounts =
        Vec::with_capacity((wallet_count.saturating_mul(accounts_per_wallet)) as usize);
    for wallet_index in 0..wallet_count {
        let wallet_label =
            parse_wallet_label(&format!("{wallet_label_prefix} {:02}", wallet_index + 1))?;
        let first_address =
            parse_generated_eth_address(generated_eth_address_value(next_address_index))?;
        next_address_index = next_address_index.saturating_add(1);
        let first_account = add_ethereum_address(
            user_id,
            &first_address,
            Network::Mainnet,
            None,
            Some(&wallet_label),
            observed_at,
        )
        .map_err(|err| PerfError::io(format!("failed to seed perf wallet account: {err}")))?;
        let wallet_id = first_account.wallet_id;
        seeded_accounts.push(first_account);

        for _ in 1..accounts_per_wallet {
            let address =
                parse_generated_eth_address(generated_eth_address_value(next_address_index))?;
            next_address_index = next_address_index.saturating_add(1);
            let account = add_ethereum_address(
                user_id,
                &address,
                Network::Mainnet,
                Some(&wallet_id),
                None,
                observed_at,
            )
            .map_err(|err| {
                PerfError::io(format!("failed to seed perf wallet sibling account: {err}"))
            })?;
            seeded_accounts.push(account);
        }
    }
    Ok(seeded_accounts)
}

pub(super) fn perf_seed_timestamp() -> Result<chrono::DateTime<Utc>, PerfError> {
    Utc.with_ymd_and_hms(2026, 3, 1, 12, 0, 0)
        .single()
        .ok_or_else(|| PerfError::io("invalid static perf seed timestamp"))
}

fn parse_wallet_label(value: &str) -> Result<Label, PerfError> {
    Label::parse_with_limit(value, WALLET_LABEL_MAX_LENGTH)
        .map_err(|err| PerfError::usage(format!("invalid perf wallet label: {err}")))
}

pub(super) fn generated_eth_address_value(index: u64) -> String {
    format!("0x{:040x}", index)
}

pub(super) fn generated_tx_hash_value(index: u64) -> String {
    format!("{index:064x}")
}

pub(super) fn parse_generated_eth_address(value: String) -> Result<EthAddress, PerfError> {
    let raw = RawEthAddress::new(value);
    EthAddress::parse(&raw)
        .map_err(|err| PerfError::usage(format!("invalid generated perf eth address: {err}")))
}

pub(super) fn parse_perf_btc_address(
    value: &str,
    network: Network,
) -> Result<BtcAddress, PerfError> {
    let raw = RawBtcAddress::new(value.to_string());
    BtcAddress::parse(&raw, network)
        .map_err(|err| PerfError::usage(format!("invalid generated perf btc address: {err}")))
}

pub(super) fn parse_generated_tracked_address(value: String) -> Result<TrackedAddress, PerfError> {
    TrackedAddress::parse(&value)
        .map_err(|err| PerfError::usage(format!("invalid generated perf tracked address: {err}")))
}

pub(super) fn parse_generated_tx_hash(index: u64) -> Result<TxHash, PerfError> {
    TxHash::parse(&generated_tx_hash_value(index))
        .map_err(|err| PerfError::usage(format!("invalid generated perf tx hash: {err}")))
}

pub(super) fn unsigned_amount_from_i64(
    value: i64,
    context: &str,
) -> Result<UnsignedAmount, PerfError> {
    UnsignedAmount::try_from_i64(value)
        .map_err(|err| PerfError::usage(format!("invalid perf {context} amount: {err}")))
}

pub(super) fn build_large_account_transaction_records(
    owned_address: &TrackedAddress,
    confirmed_count: u32,
    pending_count: u32,
) -> Result<Vec<SyncAccountTransactionRecord>, PerfError> {
    let observed_at = perf_seed_timestamp()?;
    let mut records = Vec::with_capacity((confirmed_count.saturating_add(pending_count)) as usize);

    for index in 0..confirmed_count {
        let tx_hash = parse_generated_tx_hash(100_000_u64 + u64::from(index))?;
        let external_address = parse_generated_tracked_address(generated_eth_address_value(
            200_000_u64 + u64::from(index),
        ))?;
        let block_height = 950_000_i64 + i64::from(index);
        let minutes_ago = i64::from(confirmed_count.saturating_sub(index));
        records.push(SyncAccountTransactionRecord {
            tx_hash,
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(block_height),
            block_hash: Some(generated_tx_hash_value(300_000_u64 + u64::from(index))),
            block_time: Some(observed_at - ChronoDuration::minutes(minutes_ago)),
            fee_amount: Some(unsigned_amount_from_i64(
                21_000_i64 + i64::from(index % 17),
                "fee",
            )?),
            nonce: Some(i64::from(index)),
            transfers: vec![SyncAccountTransferRecord {
                provider_transfer_key: ProviderTransferKey::normal(),
                transfer_index: 0_i64,
                transfer_kind: TransferKind::Normal,
                from_address: Some(external_address),
                to_address: Some(owned_address.clone()),
                value_amount: unsigned_amount_from_i64(
                    1_000_000_000_000_000_i64 + i64::from(index) * 1_000_i64,
                    "confirmed transfer",
                )?,
            }],
        });
    }

    for index in 0..pending_count {
        let tx_hash = parse_generated_tx_hash(400_000_u64 + u64::from(index))?;
        let external_address = parse_generated_tracked_address(generated_eth_address_value(
            500_000_u64 + u64::from(index),
        ))?;
        records.push(SyncAccountTransactionRecord {
            tx_hash,
            status: ChainTransactionStatus::Pending,
            block_height: None,
            block_hash: None,
            block_time: None,
            fee_amount: Some(unsigned_amount_from_i64(
                21_000_i64 + i64::from(index % 11),
                "pending fee",
            )?),
            nonce: Some(i64::from(confirmed_count) + i64::from(index)),
            transfers: vec![SyncAccountTransferRecord {
                provider_transfer_key: ProviderTransferKey::normal(),
                transfer_index: 0_i64,
                transfer_kind: TransferKind::Normal,
                from_address: Some(external_address),
                to_address: Some(owned_address.clone()),
                value_amount: unsigned_amount_from_i64(
                    500_000_000_000_000_i64 + i64::from(index) * 500_i64,
                    "pending transfer",
                )?,
            }],
        });
    }

    Ok(records)
}

pub(super) fn build_large_utxo_transaction_records(
    owned_address: &TrackedAddress,
    confirmed_count: u32,
    pending_count: u32,
) -> Result<Vec<SyncTransactionRecord>, PerfError> {
    let observed_at = perf_seed_timestamp()?;
    let mut records = Vec::with_capacity((confirmed_count.saturating_add(pending_count)) as usize);

    for index in 0..confirmed_count {
        let tx_hash = parse_generated_tx_hash(600_000_u64 + u64::from(index))?;
        let value_amount = 100_000_i64 + i64::from(index % 29);
        let script_pubkey_hex = format!("0014{:08x}", 700_000_u64 + u64::from(index));
        let minutes_ago = i64::from(confirmed_count.saturating_sub(index));
        records.push(SyncTransactionRecord {
            tx_hash,
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(970_000_i64 + i64::from(index)),
            block_hash: Some(generated_tx_hash_value(710_000_u64 + u64::from(index))),
            block_time: Some(observed_at - ChronoDuration::minutes(minutes_ago)),
            fee_amount: Some(150_i64 + i64::from(index % 7)),
            inputs: Vec::new(),
            outputs: vec![SyncTransactionOutputRecord {
                output_index: 0_i64,
                raw_address: Some(owned_address.clone()),
                script_pubkey_hex,
                value_amount,
            }],
        });
    }

    for index in 0..pending_count {
        let tx_hash = parse_generated_tx_hash(800_000_u64 + u64::from(index))?;
        let spent_tx_hash = parse_generated_tx_hash(600_000_u64 + u64::from(index))?;
        let value_amount = 100_000_i64 + i64::from(index % 29);
        records.push(SyncTransactionRecord {
            tx_hash,
            status: ChainTransactionStatus::Pending,
            block_height: None,
            block_hash: None,
            block_time: None,
            fee_amount: Some(225_i64 + i64::from(index % 11)),
            inputs: vec![SyncTransactionInputRecord {
                input_index: 0_i64,
                prev_tx_hash: spent_tx_hash,
                prev_output_index: 0_i64,
                prev_address: Some(owned_address.clone()),
                value_amount: Some(value_amount),
            }],
            outputs: Vec::new(),
        });
    }

    Ok(records)
}

fn write_dataset_manifest(manifest: &PerfDatasetManifest) -> Result<(), PerfError> {
    let project_dir =
        crate::project_paths::get_project_dir().map_err(|err| PerfError::io(err.to_string()))?;
    let path = project_dir.join(DATASET_MANIFEST_FILENAME);
    let body = serde_json::to_string_pretty(manifest)
        .map_err(|err| PerfError::Json(format!("failed to serialize dataset manifest: {err}")))?;
    write(&path, body).map_err(|err| {
        PerfError::io(format!(
            "failed to write dataset manifest {}: {err}",
            path.display()
        ))
    })
}

pub(super) fn read_dataset_manifest(
    project_dir: &std::path::Path,
) -> Result<PerfDatasetManifest, PerfError> {
    let path = project_dir.join(DATASET_MANIFEST_FILENAME);
    let body = read_to_string(&path).map_err(|err| {
        PerfError::io(format!(
            "failed to read dataset manifest {}: {err}",
            path.display()
        ))
    })?;
    serde_json::from_str(&body)
        .map_err(|err| PerfError::Json(format!("failed to parse dataset manifest: {err}")))
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn generated_eth_address_values_parse_and_differ() {
        let first = parse_generated_eth_address(generated_eth_address_value(1))
            .expect("first generated address should parse");
        let second = parse_generated_eth_address(generated_eth_address_value(2))
            .expect("second generated address should parse");

        assert_ne!(first, second);
        assert_eq!(
            first.normalized(),
            "0x0000000000000000000000000000000000000001"
        );
    }

    #[test]
    fn build_large_account_transaction_records_preserves_requested_counts() {
        let owned_address = parse_generated_tracked_address(generated_eth_address_value(1))
            .expect("owned address should parse");
        let records = build_large_account_transaction_records(&owned_address, 3, 2)
            .expect("records should build");

        assert_eq!(records.len(), 5);
        assert_eq!(
            records
                .iter()
                .filter(|record| record.status == ChainTransactionStatus::Confirmed)
                .count(),
            3
        );
        assert_eq!(
            records
                .iter()
                .filter(|record| record.status == ChainTransactionStatus::Pending)
                .count(),
            2
        );
        assert!(records.iter().all(|record| {
            record.transfers.len() == 1
                && record.transfers[0].to_address.as_ref() == Some(&owned_address)
        }));
    }

    #[test]
    fn build_large_utxo_transaction_records_preserves_requested_counts() {
        let owned_address = parse_generated_tracked_address(PERF_OWNED_BTC_ADDRESS.to_string())
            .expect("owned address should parse");
        let records = build_large_utxo_transaction_records(&owned_address, 3, 2)
            .expect("records should build");

        assert_eq!(records.len(), 5);
        assert_eq!(
            records
                .iter()
                .filter(|record| record.status == ChainTransactionStatus::Confirmed)
                .count(),
            3
        );
        assert_eq!(
            records
                .iter()
                .filter(|record| record.status == ChainTransactionStatus::Pending)
                .count(),
            2
        );
        assert_eq!(records[0].outputs.len(), 1);
        assert_eq!(
            records[0].outputs[0].raw_address.as_ref(),
            Some(&owned_address)
        );
        assert_eq!(records[3].inputs.len(), 1);
        assert_eq!(
            records[3].inputs[0].prev_address.as_ref(),
            Some(&owned_address)
        );
    }
}
