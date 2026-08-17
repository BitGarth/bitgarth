//! App-layer sync integration modules.
//!
//! These modules sit above the transport-layer `src/integrations/*` clients.
//! They own provider-specific sync concerns such as pagination,
//! raw-ingestion coordination, and normalization from transport-owned types
//! into sync-domain records.

use crate::asset_capabilities::{SyncProviderId, default_sync_provider};
use crate::db::SyncAddress;
use crate::db::raw_ingestion::OpaqueJsonText;
use crate::tasks::jobs::sync::{
    IntegrationIterationContext, IntegrationSyncPlan, RunContext, SyncClients, SyncIterationResult,
    UserTransactionMonitorError,
};
use crate::transactions::{AddressBackfillState, TxCountEstimate};

pub(crate) mod etherscan;
pub(crate) mod mempool;

pub(crate) trait AddressSyncIntegration {
    fn sync_plan(
        &self,
        _address: &SyncAddress,
        _allow_known_confirmed_early_exit: bool,
    ) -> Result<IntegrationSyncPlan, UserTransactionMonitorError> {
        Ok(IntegrationSyncPlan {
            is_backfill_active: false,
        })
    }

    fn estimate_first_sync_tx_count(
        &self,
        _context: IntegrationEstimateContext<'_>,
    ) -> Result<Option<TxCountEstimate>, UserTransactionMonitorError> {
        Ok(None)
    }

    fn unfinished_backfill_state(&self, _address: &SyncAddress) -> Option<AddressBackfillState> {
        None
    }

    fn sync_one_iteration(
        &mut self,
        context: IntegrationIterationContext<'_>,
    ) -> Result<SyncIterationResult, UserTransactionMonitorError>;

    fn current_run_summary_json(
        &self,
    ) -> Result<Option<OpaqueJsonText>, UserTransactionMonitorError> {
        Ok(None)
    }

    fn reset_iteration_state(&mut self) {}
}

#[derive(Clone, Copy)]
pub(crate) struct IntegrationEstimateContext<'a> {
    pub(crate) run: RunContext<'a>,
    pub(crate) address: &'a SyncAddress,
    pub(crate) clients: SyncClients<'a>,
}

pub(crate) fn provider_for_address(address: &SyncAddress) -> SyncProviderId {
    default_sync_provider(address.asset_id)
}

pub(crate) fn estimate_first_sync_tx_count(
    context: IntegrationEstimateContext<'_>,
) -> Result<Option<TxCountEstimate>, UserTransactionMonitorError> {
    match provider_for_address(context.address) {
        SyncProviderId::MempoolSpace => {
            mempool::MempoolAddressSyncIntegration::new().estimate_first_sync_tx_count(context)
        }
        SyncProviderId::Etherscan => {
            etherscan::EtherscanAddressSyncIntegration::new().estimate_first_sync_tx_count(context)
        }
    }
}

pub(crate) fn unfinished_backfill_state(address: &SyncAddress) -> Option<AddressBackfillState> {
    match provider_for_address(address) {
        SyncProviderId::MempoolSpace => {
            mempool::MempoolAddressSyncIntegration::new().unfinished_backfill_state(address)
        }
        SyncProviderId::Etherscan => {
            etherscan::EtherscanAddressSyncIntegration::new().unfinished_backfill_state(address)
        }
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use crate::transactions::{AddressBackfillCursor, EthereumBlockNumber, MempoolCursorTxid};
    use crate::wallets::{DigitalAssetAddressId, Network, SyncedAssetId};

    fn make_sync_address(asset_id: SyncedAssetId) -> SyncAddress {
        let address = match asset_id {
            SyncedAssetId::Bitcoin => {
                "bc1qtestaddress000000000000000000000000000000000000000000000"
            }
            SyncedAssetId::Ethereum => "0x1111111111111111111111111111111111111111",
        };
        SyncAddress {
            address_id: DigitalAssetAddressId::new(),
            address: crate::transactions::TrackedAddress::parse(address).expect("valid address"),
            asset_id,
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

    #[test]
    fn provider_for_address_maps_supported_asset_families() {
        let bitcoin = make_sync_address(SyncedAssetId::Bitcoin);
        let ethereum = make_sync_address(SyncedAssetId::Ethereum);

        assert_eq!(provider_for_address(&bitcoin), SyncProviderId::MempoolSpace);
        assert_eq!(provider_for_address(&ethereum), SyncProviderId::Etherscan);
    }

    #[test]
    fn unfinished_backfill_state_returns_provider_neutral_mempool_metadata() {
        let mut address = make_sync_address(SyncedAssetId::Bitcoin);
        let cursor_txid = MempoolCursorTxid::parse(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .expect("cursor should parse");
        address.mempool_backfill_cursor_txid = Some(cursor_txid.clone());

        let state = super::unfinished_backfill_state(&address)
            .expect("mempool cursor should expose unfinished backfill state");

        assert_eq!(state.cursor, AddressBackfillCursor::Mempool { cursor_txid });
        assert_eq!(state.expected_tx_count, None);
    }

    #[test]
    fn unfinished_backfill_state_returns_provider_neutral_etherscan_metadata() {
        let mut address = make_sync_address(SyncedAssetId::Ethereum);
        let end_block = EthereumBlockNumber::try_new(456_789).expect("block should be valid");
        address.etherscan_backfill_end_block = Some(end_block);

        let state = super::unfinished_backfill_state(&address)
            .expect("etherscan cursor should expose unfinished backfill state");

        assert_eq!(state.cursor, AddressBackfillCursor::Etherscan { end_block });
        assert_eq!(state.expected_tx_count, None);
    }
}
