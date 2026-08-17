use super::{CHAIN_TIP_CACHE_TTL_MIN, UserTransactionMonitorError};
use crate::asset_capabilities::{
    network, synced_asset_instance, synced_asset_instance_asset_instance, synced_asset_instance_id,
};
use crate::db::load_chain_tip_state;
use crate::transactions::ChainTipHeight;
use crate::wallets::{Network, SyncedAssetId};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
pub(super) struct CachedChainTip {
    pub(super) height: ChainTipHeight,
    pub(super) fetched_at: Instant,
}

pub(super) fn chain_tip_cache_key(asset_id: SyncedAssetId, network: Network) -> String {
    format!("{}:{}", asset_id.as_str(), network.as_str())
}

#[derive(Default)]
pub(super) struct ChainTipCache {
    pub(super) tips: HashMap<String, CachedChainTip>,
}

impl ChainTipCache {
    pub(super) fn get_or_fetch<F>(
        &mut self,
        asset_id: SyncedAssetId,
        network: Network,
        now: Instant,
        now_utc: DateTime<Utc>,
        mut fetch_fn: F,
    ) -> Result<ChainTipHeight, UserTransactionMonitorError>
    where
        F: FnMut() -> Result<ChainTipHeight, UserTransactionMonitorError>,
    {
        let cache_key = chain_tip_cache_key(asset_id, network);
        if let Some(cached) = self.tips.get(&cache_key).copied() {
            let ttl = chain_tip_cache_ttl_for(asset_id);
            if asset_id == SyncedAssetId::Bitcoin || now.duration_since(cached.fetched_at) <= ttl {
                return Ok(cached.height);
            }
        }

        if should_reuse_persisted_tip(asset_id)
            && let Some(saved_tip) = load_chain_tip_state(asset_id, network)?
        {
            let ttl = chain_tip_cache_ttl_for(asset_id);
            let age = now_utc.signed_duration_since(saved_tip.updated_at);
            if let Ok(age_std) = age.to_std()
                && age_std <= ttl
            {
                self.tips.insert(
                    cache_key.clone(),
                    CachedChainTip {
                        height: saved_tip.chain_tip_height,
                        fetched_at: now,
                    },
                );
                return Ok(saved_tip.chain_tip_height);
            }
        }

        let fresh_tip = fetch_fn()?;
        self.tips.insert(
            cache_key,
            CachedChainTip {
                height: fresh_tip,
                fetched_at: now,
            },
        );
        Ok(fresh_tip)
    }
}

fn should_reuse_persisted_tip(asset_id: SyncedAssetId) -> bool {
    asset_id != SyncedAssetId::Bitcoin
}

pub(super) fn chain_tip_cache_ttl_for(asset_id: SyncedAssetId) -> Duration {
    let synced = synced_asset_instance(synced_asset_instance_id(asset_id));
    let instance = synced_asset_instance_asset_instance(synced);
    let net = network(instance.id.network_id);
    let avg_block_time = net
        .block_production
        .map(|bp| bp.average_block_time)
        .unwrap_or(Duration::ZERO);
    let half_block_time = avg_block_time.checked_div(2).unwrap_or(Duration::ZERO);
    std::cmp::max(half_block_time, CHAIN_TIP_CACHE_TTL_MIN)
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use crate::wallets::SyncedAssetId;
    use std::time::Duration;

    #[test]
    fn chain_tip_cache_ttl_uses_half_block_time_with_minimum_floor() {
        assert_eq!(
            chain_tip_cache_ttl_for(SyncedAssetId::Bitcoin),
            Duration::from_secs(300)
        );
        assert_eq!(
            chain_tip_cache_ttl_for(SyncedAssetId::Ethereum),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn bitcoin_chain_tip_uses_configured_provider_run_cache() {
        assert!(!should_reuse_persisted_tip(SyncedAssetId::Bitcoin));
        assert!(should_reuse_persisted_tip(SyncedAssetId::Ethereum));

        let mut cache = ChainTipCache::default();
        let now = Instant::now();
        let now_utc = Utc::now();
        let provider_tip = ChainTipHeight::try_new(900_001).expect("tip should parse");
        let mut provider_calls = 0_u32;
        let first = cache
            .get_or_fetch(
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                now,
                now_utc,
                || {
                    provider_calls = provider_calls.saturating_add(1);
                    Ok(provider_tip)
                },
            )
            .expect("configured provider tip should load");
        let second = cache
            .get_or_fetch(
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                now + chain_tip_cache_ttl_for(SyncedAssetId::Bitcoin) + Duration::from_secs(1),
                now_utc,
                || {
                    provider_calls = provider_calls.saturating_add(1);
                    Ok(provider_tip)
                },
            )
            .expect("run-local tip should be reused");

        assert_eq!(first, provider_tip);
        assert_eq!(second, provider_tip);
        assert_eq!(provider_calls, 1);
    }
}
