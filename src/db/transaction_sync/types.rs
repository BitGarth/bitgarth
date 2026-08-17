use crate::amounts::UnsignedAmount;
use crate::db::raw_ingestion::SyncRunId;
use crate::ethereum::TransferKind;
use crate::transactions::{
    AggregateSyncResult, ApiConfirmedBalance, ChainTipHeight, ChainTransactionStatus,
    ConsecutiveFailureCount, EthereumBlockNumber, MempoolCursorTxid, SyncErrorMessage,
    SyncIntegrationId, TrackedAddress, TransactionCount, TransactionSyncResult,
    TransactionSyncRunId, TxHash,
};
use crate::wallets::{
    AddressScheme, DigitalAssetAccountId, DigitalAssetAddressId, Network, SyncedAssetId,
};
use chrono::{DateTime, Utc};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MempoolHistoryProof {
    pub(crate) confirmed_tx_count: TransactionCount,
    pub(crate) complete_height: ChainTipHeight,
}

#[derive(Debug, Clone)]
pub(crate) struct SyncAddress {
    pub(crate) address_id: DigitalAssetAddressId,
    pub(crate) address: TrackedAddress,
    pub(crate) asset_id: SyncedAssetId,
    pub(crate) network: Network,
    pub(crate) account_id: Option<DigitalAssetAccountId>,
    pub(crate) derivation_change: Option<u32>,
    pub(crate) derivation_index: Option<u32>,
    pub(crate) address_scheme: Option<AddressScheme>,
    pub(crate) last_completed_at: Option<DateTime<Utc>>,
    pub(crate) last_result: Option<TransactionSyncResult>,
    pub(crate) last_tip_height: Option<ChainTipHeight>,
    pub(crate) mempool_backfill_cursor_txid: Option<MempoolCursorTxid>,
    pub(crate) mempool_expected_tx_count: Option<TransactionCount>,
    pub(crate) mempool_history_proof: Option<MempoolHistoryProof>,
    pub(crate) mempool_history_scan_start_run_id: Option<SyncRunId>,
    pub(crate) etherscan_backfill_end_block: Option<EthereumBlockNumber>,
    pub(crate) etherscan_history_checkpoint_verified: bool,
    pub(crate) has_api_confirmed_balance: bool,
    pub(crate) consecutive_failure_count: ConsecutiveFailureCount,
}

#[derive(Debug, Clone)]
pub(crate) struct AccountSyncStateRow {
    pub(crate) account_id: DigitalAssetAccountId,
    pub(crate) last_scanned_time: Option<DateTime<Utc>>,
    pub(crate) gap_limit: u32,
    pub(crate) last_derived_external_index: Option<u32>,
    pub(crate) last_derived_internal_index: Option<u32>,
    pub(crate) mempool_history_next_address_id: Option<DigitalAssetAddressId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HdMempoolHistoryFrontierUpdate {
    pub(crate) account_id: DigitalAssetAccountId,
    pub(crate) next_address_id: Option<DigitalAssetAddressId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MempoolHistoryPageWorkUpdate {
    pub(crate) address_id: DigitalAssetAddressId,
    pub(crate) next_cursor: Option<MempoolCursorTxid>,
    pub(crate) hd_frontier: Option<HdMempoolHistoryFrontierUpdate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AccountIntegrationSyncStateRow {
    pub(crate) account_id: DigitalAssetAccountId,
    pub(crate) integration_id: SyncIntegrationId,
    pub(crate) last_started_at: Option<DateTime<Utc>>,
    pub(crate) last_completed_at: Option<DateTime<Utc>>,
    pub(crate) last_result: Option<AggregateSyncResult>,
    pub(crate) last_error: Option<SyncErrorMessage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AccountIntegrationSyncStart {
    pub(crate) was_interrupted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HdAccountChainFrontierPhase {
    ExistingAddresses,
    DerivedAddresses,
    ActiveRescan,
}

impl HdAccountChainFrontierPhase {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::ExistingAddresses => "existing_addresses",
            Self::DerivedAddresses => "derived_addresses",
            Self::ActiveRescan => "active_rescan",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HdAccountChainSyncState {
    ExistingAddresses {
        next_index_to_scan: u32,
        consecutive_unused: u32,
    },
    DerivedAddresses {
        next_index_to_scan: u32,
        consecutive_unused: u32,
    },
    ActiveRescan {
        next_index_to_scan: u32,
        consecutive_unused: u32,
        active_rescan_from_index: u32,
    },
}

impl HdAccountChainSyncState {
    pub(crate) fn frontier_phase(&self) -> HdAccountChainFrontierPhase {
        match self {
            Self::ExistingAddresses { .. } => HdAccountChainFrontierPhase::ExistingAddresses,
            Self::DerivedAddresses { .. } => HdAccountChainFrontierPhase::DerivedAddresses,
            Self::ActiveRescan { .. } => HdAccountChainFrontierPhase::ActiveRescan,
        }
    }

    pub(crate) fn next_index_to_scan(&self) -> u32 {
        match self {
            Self::ExistingAddresses {
                next_index_to_scan, ..
            }
            | Self::DerivedAddresses {
                next_index_to_scan, ..
            }
            | Self::ActiveRescan {
                next_index_to_scan, ..
            } => *next_index_to_scan,
        }
    }

    pub(crate) fn consecutive_unused(&self) -> u32 {
        match self {
            Self::ExistingAddresses {
                consecutive_unused, ..
            }
            | Self::DerivedAddresses {
                consecutive_unused, ..
            }
            | Self::ActiveRescan {
                consecutive_unused, ..
            } => *consecutive_unused,
        }
    }

    pub(crate) fn active_rescan_from_index(&self) -> Option<u32> {
        match self {
            Self::ExistingAddresses { .. } | Self::DerivedAddresses { .. } => None,
            Self::ActiveRescan {
                active_rescan_from_index,
                ..
            } => Some(*active_rescan_from_index),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HdAccountChainSyncStateRow {
    pub(crate) account_id: DigitalAssetAccountId,
    pub(crate) derivation_change: u32,
    pub(crate) frontier_state: HdAccountChainSyncState,
    pub(crate) updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub(crate) struct AccountSyncBundle {
    pub(crate) account_id: DigitalAssetAccountId,
    pub(crate) asset_id: SyncedAssetId,
    pub(crate) network: Network,
    pub(crate) hd_key_extended_pubkey: String,
    pub(crate) address_scheme: AddressScheme,
    pub(crate) sync_state: Option<AccountSyncStateRow>,
    pub(crate) external_addresses: Vec<SyncAddress>,
    pub(crate) internal_addresses: Vec<SyncAddress>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyncTransactionInputRecord {
    pub(crate) input_index: i64,
    pub(crate) prev_tx_hash: TxHash,
    pub(crate) prev_output_index: i64,
    pub(crate) prev_address: Option<TrackedAddress>,
    pub(crate) value_amount: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyncTransactionOutputRecord {
    pub(crate) output_index: i64,
    pub(crate) raw_address: Option<TrackedAddress>,
    pub(crate) script_pubkey_hex: String,
    pub(crate) value_amount: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyncTransactionRecord {
    pub(crate) tx_hash: TxHash,
    pub(crate) status: ChainTransactionStatus,
    pub(crate) block_height: Option<i64>,
    pub(crate) block_hash: Option<String>,
    pub(crate) block_time: Option<DateTime<Utc>>,
    pub(crate) fee_amount: Option<i64>,
    pub(crate) inputs: Vec<SyncTransactionInputRecord>,
    pub(crate) outputs: Vec<SyncTransactionOutputRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ProviderTransferKey(String);

impl ProviderTransferKey {
    pub(crate) fn normal() -> Self {
        Self("normal".to_string())
    }

    pub(crate) fn from_internal_trace_id(trace_id: &str) -> Option<Self> {
        let trace_id = trace_id.trim();
        (!trace_id.is_empty()).then(|| Self(format!("internal:{trace_id}")))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn is_normal(&self) -> bool {
        self.0 == "normal"
    }

    pub(crate) fn internal_trace_id(&self) -> Option<&str> {
        self.0.strip_prefix("internal:")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyncAccountTransferRecord {
    pub(crate) provider_transfer_key: ProviderTransferKey,
    pub(crate) transfer_index: i64,
    pub(crate) transfer_kind: TransferKind,
    pub(crate) from_address: Option<TrackedAddress>,
    pub(crate) to_address: Option<TrackedAddress>,
    pub(crate) value_amount: UnsignedAmount,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyncAccountTransactionRecord {
    pub(crate) tx_hash: TxHash,
    pub(crate) status: ChainTransactionStatus,
    pub(crate) block_height: Option<i64>,
    pub(crate) block_hash: Option<String>,
    pub(crate) block_time: Option<DateTime<Utc>>,
    pub(crate) fee_amount: Option<UnsignedAmount>,
    pub(crate) nonce: Option<i64>,
    pub(crate) transfers: Vec<SyncAccountTransferRecord>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CoverageInvalidationTargets {
    pub(crate) address_ids: HashSet<DigitalAssetAddressId>,
    pub(crate) account_ids: HashSet<DigitalAssetAccountId>,
}

impl CoverageInvalidationTargets {
    pub(crate) fn union_with(&mut self, other: Self) {
        self.address_ids.extend(other.address_ids);
        self.account_ids.extend(other.account_ids);
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TransactionSyncReconcileSummary {
    pub(crate) new_tx_count: TransactionCount,
    pub(crate) updated_tx_count: TransactionCount,
    pub(crate) coverage_invalidation: CoverageInvalidationTargets,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AddressSyncSuccess {
    pub(crate) address_id: DigitalAssetAddressId,
    pub(crate) run_id: TransactionSyncRunId,
    pub(crate) started_at: DateTime<Utc>,
    pub(crate) completed_at: DateTime<Utc>,
    pub(crate) last_tip_height: ChainTipHeight,
    pub(crate) new_tx_count: TransactionCount,
    pub(crate) updated_tx_count: TransactionCount,
    pub(crate) api_confirmed_balance: Option<ApiConfirmedBalance>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AddressApiConfirmedBalanceRow {
    pub(crate) address_id: DigitalAssetAddressId,
    pub(crate) last_completed_at: Option<DateTime<Utc>>,
    pub(crate) api_confirmed_balance: Option<ApiConfirmedBalance>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChainTipStateRow {
    pub(crate) chain_tip_height: ChainTipHeight,
    pub(crate) updated_at: DateTime<Utc>,
}
