//! Synced asset registry.
//!
//! Keeps the code-defined BTC/ETH assets and instances available for synced
//! wallet flows. Manual assets use the unsynced catalog instead.

use super::{
    Asset, AssetId, AssetInstance, AssetInstanceId, BITCOIN_ASSET, BTC_BITCOIN_MAINNET_INSTANCE,
    ETH_ETHEREUM_MAINNET_INSTANCE, ETHEREUM_ASSET,
};
use once_cell::sync::OnceCell;
use std::collections::HashMap;

#[derive(Debug)]
pub(crate) struct AssetCatalogError(String);

impl std::fmt::Display for AssetCatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "asset catalog error: {}", self.0)
    }
}

#[derive(Debug)]
pub(crate) struct AssetRegistry {
    assets: HashMap<AssetId, Asset>,
    instances: HashMap<AssetInstanceId, AssetInstance>,
}

impl AssetRegistry {
    pub(crate) fn asset(&self, id: &AssetId) -> Option<&Asset> {
        self.assets.get(id)
    }

    pub(crate) fn instance(&self, id: &AssetInstanceId) -> Option<&AssetInstance> {
        self.instances.get(id)
    }
}

fn synced_registry() -> AssetRegistry {
    let mut assets: HashMap<AssetId, Asset> = HashMap::new();
    let mut instances: HashMap<AssetInstanceId, AssetInstance> = HashMap::new();
    merge_synced(&mut assets, &mut instances);
    AssetRegistry { assets, instances }
}

fn merge_synced(
    assets: &mut HashMap<AssetId, Asset>,
    instances: &mut HashMap<AssetInstanceId, AssetInstance>,
) {
    for a in [&BITCOIN_ASSET, &ETHEREUM_ASSET] {
        assets.entry(a.id.clone()).or_insert_with(|| a.clone());
    }
    for i in [
        &BTC_BITCOIN_MAINNET_INSTANCE,
        &ETH_ETHEREUM_MAINNET_INSTANCE,
    ] {
        instances.entry(i.id.clone()).or_insert_with(|| i.clone());
    }
}

static REGISTRY: OnceCell<AssetRegistry> = OnceCell::new();

pub(crate) fn load_registry() -> Result<&'static AssetRegistry, AssetCatalogError> {
    Ok(REGISTRY.get_or_init(synced_registry))
}

pub(super) fn registry() -> &'static AssetRegistry {
    REGISTRY.get_or_init(synced_registry)
}

pub(crate) fn asset(id: &AssetId) -> Option<&'static Asset> {
    registry().asset(id)
}

pub(crate) fn asset_instance(id: &AssetInstanceId) -> Option<&'static AssetInstance> {
    registry().instance(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synced_registry_contains_btc_and_eth_only() {
        let reg = load_registry().expect("synced registry loads");
        assert!(reg.asset(&AssetId::BITCOIN).is_some());
        assert!(reg.asset(&AssetId::ETHEREUM).is_some());
        assert!(
            reg.instance(
                &crate::asset_capabilities::SyncedAssetInstanceId::BtcBitcoinMainnet.as_instance(),
            )
            .is_some()
        );
        assert!(
            reg.instance(
                &crate::asset_capabilities::SyncedAssetInstanceId::EthEthereumMainnet.as_instance(),
            )
            .is_some()
        );
        assert_eq!(reg.assets.len(), 2);
        assert_eq!(reg.instances.len(), 2);
    }
}
