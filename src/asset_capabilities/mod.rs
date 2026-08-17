#[cfg(any(feature = "server", test))]
use crate::amounts::AssetClass;
#[cfg(any(feature = "server", test))]
use crate::wallets::SyncedAssetId;

#[cfg(any(feature = "server", test))]
pub(crate) mod registry;
#[cfg(any(feature = "server", test))]
pub(crate) mod unsynced;
#[cfg(feature = "server")]
pub(crate) use registry::load_registry;
#[cfg(any(feature = "server", test))]
pub(crate) use registry::{asset, asset_instance};

#[cfg(feature = "server")]
pub(crate) fn load_unsynced_catalog()
-> Result<&'static unsynced::UnsyncedAssetCatalog, unsynced::UnsyncedCatalogError> {
    unsynced::load_catalog()
}

#[cfg(feature = "server")]
use crate::account_model::AccountModel;
#[cfg(any(feature = "server", test))]
use std::borrow::Cow;
#[cfg(any(feature = "server", test))]
use std::time::Duration;

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AssetIdError(String);

#[cfg(any(feature = "server", test))]
impl std::fmt::Display for AssetIdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid asset id: {}", self.0)
    }
}

#[cfg(any(feature = "server", test))]
fn validate_asset_id(value: &str) -> Result<(), AssetIdError> {
    const MAX_LEN: usize = 64;
    if value.is_empty() {
        return Err(AssetIdError("empty".to_string()));
    }
    if value.len() > MAX_LEN {
        return Err(AssetIdError(format!("longer than {MAX_LEN} chars")));
    }
    let valid = value
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid {
        return Err(AssetIdError(format!(
            "must be lowercase ascii letters, digits, or '-': {value}"
        )));
    }
    Ok(())
}

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct AssetId(Cow<'static, str>);

#[cfg(any(feature = "server", test))]
impl AssetId {
    pub(crate) const BITCOIN: Self = Self::borrowed_unchecked("bitcoin");
    pub(crate) const ETHEREUM: Self = Self::borrowed_unchecked("ethereum");

    pub(crate) const fn borrowed_unchecked(value: &'static str) -> Self {
        Self(Cow::Borrowed(value))
    }

    pub(crate) fn owned(value: String) -> Result<Self, AssetIdError> {
        validate_asset_id(&value)?;
        Ok(Self(Cow::Owned(value)))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(any(feature = "server", test))]
impl serde::Serialize for AssetId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

#[cfg(any(feature = "server", test))]
impl<'de> serde::Deserialize<'de> for AssetId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        AssetId::owned(raw).map_err(serde::de::Error::custom)
    }
}

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct UnitCode(Cow<'static, str>);

#[cfg(any(feature = "server", test))]
impl UnitCode {
    pub(crate) const fn borrowed_unchecked(value: &'static str) -> Self {
        Self(Cow::Borrowed(value))
    }

    pub(crate) fn owned(value: String) -> Self {
        Self(Cow::Owned(value))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PriceRefs {
    pub(crate) coingecko: Option<Cow<'static, str>>,
}

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AssetKind {
    NativeCoin,
}

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Asset {
    pub(crate) id: AssetId,
    pub(crate) canonical_name: Cow<'static, str>,
    pub(crate) default_unit_code: UnitCode,
    pub(crate) kind: AssetKind,
    pub(crate) price_refs: PriceRefs,
}

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(clippy::enum_variant_names)]
pub(crate) enum NetworkId {
    BitcoinMainnet,
    EthereumMainnet,
    CardanoMainnet,
    SolanaMainnet,
    BnbSmartChainMainnet,
    PolygonMainnet,
    AvalancheCChain,
    ArbitrumOne,
    OptimismMainnet,
    BaseMainnet,
    TronMainnet,
    RippleXrpMainnet,
    DogecoinMainnet,
    MoneroMainnet,
    TezosMainnet,
    ZcashMainnet,
}

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LedgerModel {
    Utxo,
    ExtendedUtxo,
    Account,
    SolanaAccount,
    XrpLedger,
}

#[cfg(feature = "server")]
impl LedgerModel {
    const fn account_model(self) -> AccountModel {
        match self {
            Self::Utxo | Self::ExtendedUtxo => AccountModel::Utxo,
            Self::Account | Self::SolanaAccount | Self::XrpLedger => AccountModel::Account,
        }
    }
}

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NetworkCapabilities {
    pub(crate) supports_nonce: bool,
    pub(crate) supports_internal_transfers: bool,
}

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlockProduction {
    pub(crate) average_block_time: Duration,
}

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NetworkSpec {
    pub(crate) id: NetworkId,
    pub(crate) display_name: &'static str,
    pub(crate) ledger_model: LedgerModel,
    pub(crate) capabilities: NetworkCapabilities,
    pub(crate) block_production: Option<BlockProduction>,
}

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum AssetNamespace {
    Native,
}

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct AssetInstanceId {
    pub(crate) asset_id: AssetId,
    pub(crate) network_id: NetworkId,
    pub(crate) namespace: AssetNamespace,
}

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AssetInstanceRole {
    Native,
}

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AssetInstance {
    pub(crate) id: AssetInstanceId,
    pub(crate) unit_code: UnitCode,
    pub(crate) symbol: Option<Cow<'static, str>>,
    pub(crate) decimal_precision: u8,
    pub(crate) asset_class: AssetClass,
    pub(crate) role: AssetInstanceRole,
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SyncProviderId {
    MempoolSpace,
    Etherscan,
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SyncProviderCapabilities {
    pub(crate) supports_transaction_sync: bool,
    pub(crate) supports_balance_only_sync: bool,
    pub(crate) supports_internal_transfers: bool,
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SyncProvider {
    pub(crate) id: SyncProviderId,
    pub(crate) display_name: &'static str,
    pub(crate) supported_networks: &'static [NetworkId],
    pub(crate) capabilities: SyncProviderCapabilities,
}

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SyncedAssetInstanceId {
    BtcBitcoinMainnet,
    EthEthereumMainnet,
}

#[cfg(any(feature = "server", test))]
impl SyncedAssetInstanceId {
    #[cfg(test)]
    pub(crate) fn from_instance(id: &AssetInstanceId) -> Option<Self> {
        if *id == BTC_BITCOIN_MAINNET_INSTANCE.id {
            Some(Self::BtcBitcoinMainnet)
        } else if *id == ETH_ETHEREUM_MAINNET_INSTANCE.id {
            Some(Self::EthEthereumMainnet)
        } else {
            None
        }
    }
}

#[cfg(test)]
impl SyncedAssetInstanceId {
    pub(crate) fn as_instance(self) -> AssetInstanceId {
        match self {
            Self::BtcBitcoinMainnet => BTC_BITCOIN_MAINNET_INSTANCE.id.clone(),
            Self::EthEthereumMainnet => ETH_ETHEREUM_MAINNET_INSTANCE.id.clone(),
        }
    }

    pub(crate) fn asset_id(self) -> AssetId {
        self.as_instance().asset_id
    }

    #[cfg(feature = "server")]
    pub(crate) fn network_id(self) -> NetworkId {
        self.as_instance().network_id
    }
}

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyncedAssetInstance {
    pub(crate) id: SyncedAssetInstanceId,
    pub(crate) asset_instance_id: AssetInstanceId,
    #[cfg(feature = "server")]
    pub(crate) default_sync_provider: SyncProviderId,
    #[cfg(feature = "server")]
    pub(crate) allowed_sync_providers: &'static [SyncProviderId],
}

#[cfg(any(feature = "server", test))]
static BITCOIN_ASSET: Asset = Asset {
    id: AssetId::BITCOIN,
    canonical_name: Cow::Borrowed("Bitcoin"),
    default_unit_code: UnitCode::borrowed_unchecked("BTC"),
    kind: AssetKind::NativeCoin,
    price_refs: PriceRefs {
        coingecko: Some(Cow::Borrowed("bitcoin")),
    },
};

#[cfg(any(feature = "server", test))]
static ETHEREUM_ASSET: Asset = Asset {
    id: AssetId::ETHEREUM,
    canonical_name: Cow::Borrowed("Ethereum"),
    default_unit_code: UnitCode::borrowed_unchecked("ETH"),
    kind: AssetKind::NativeCoin,
    price_refs: PriceRefs {
        coingecko: Some(Cow::Borrowed("ethereum")),
    },
};

#[cfg(any(feature = "server", test))]
const BITCOIN_MAINNET_NETWORK: NetworkSpec = NetworkSpec {
    id: NetworkId::BitcoinMainnet,
    display_name: "Bitcoin",
    ledger_model: LedgerModel::Utxo,
    capabilities: NetworkCapabilities {
        supports_nonce: false,
        supports_internal_transfers: false,
    },
    block_production: Some(BlockProduction {
        average_block_time: Duration::from_secs(600),
    }),
};

#[cfg(any(feature = "server", test))]
const ETHEREUM_MAINNET_NETWORK: NetworkSpec = NetworkSpec {
    id: NetworkId::EthereumMainnet,
    display_name: "Ethereum",
    ledger_model: LedgerModel::Account,
    capabilities: NetworkCapabilities {
        supports_nonce: true,
        supports_internal_transfers: true,
    },
    block_production: Some(BlockProduction {
        average_block_time: Duration::from_secs(12),
    }),
};

#[cfg(any(feature = "server", test))]
const CARDANO_MAINNET_NETWORK: NetworkSpec = NetworkSpec {
    id: NetworkId::CardanoMainnet,
    display_name: "Cardano",
    ledger_model: LedgerModel::ExtendedUtxo,
    capabilities: NetworkCapabilities {
        supports_nonce: false,
        supports_internal_transfers: false,
    },
    block_production: Some(BlockProduction {
        average_block_time: Duration::from_secs(20),
    }),
};

#[cfg(any(feature = "server", test))]
const SOLANA_MAINNET_NETWORK: NetworkSpec = NetworkSpec {
    id: NetworkId::SolanaMainnet,
    display_name: "Solana",
    ledger_model: LedgerModel::SolanaAccount,
    capabilities: NetworkCapabilities {
        supports_nonce: false,
        supports_internal_transfers: false,
    },
    block_production: Some(BlockProduction {
        average_block_time: Duration::from_millis(400),
    }),
};

#[cfg(any(feature = "server", test))]
const BNB_SMART_CHAIN_MAINNET_NETWORK: NetworkSpec = NetworkSpec {
    id: NetworkId::BnbSmartChainMainnet,
    display_name: "BNB Smart Chain",
    ledger_model: LedgerModel::Account,
    capabilities: NetworkCapabilities {
        supports_nonce: true,
        supports_internal_transfers: false,
    },
    block_production: Some(BlockProduction {
        average_block_time: Duration::from_secs(3),
    }),
};

#[cfg(any(feature = "server", test))]
const POLYGON_MAINNET_NETWORK: NetworkSpec = NetworkSpec {
    id: NetworkId::PolygonMainnet,
    display_name: "Polygon",
    ledger_model: LedgerModel::Account,
    capabilities: NetworkCapabilities {
        supports_nonce: true,
        supports_internal_transfers: false,
    },
    block_production: Some(BlockProduction {
        average_block_time: Duration::from_secs(2),
    }),
};

#[cfg(any(feature = "server", test))]
const AVALANCHE_C_CHAIN_NETWORK: NetworkSpec = NetworkSpec {
    id: NetworkId::AvalancheCChain,
    display_name: "Avalanche C-Chain",
    ledger_model: LedgerModel::Account,
    capabilities: NetworkCapabilities {
        supports_nonce: true,
        supports_internal_transfers: false,
    },
    block_production: Some(BlockProduction {
        average_block_time: Duration::from_secs(2),
    }),
};

#[cfg(any(feature = "server", test))]
const ARBITRUM_ONE_NETWORK: NetworkSpec = NetworkSpec {
    id: NetworkId::ArbitrumOne,
    display_name: "Arbitrum One",
    ledger_model: LedgerModel::Account,
    capabilities: NetworkCapabilities {
        supports_nonce: true,
        supports_internal_transfers: false,
    },
    block_production: Some(BlockProduction {
        average_block_time: Duration::from_millis(250),
    }),
};

#[cfg(any(feature = "server", test))]
const OPTIMISM_MAINNET_NETWORK: NetworkSpec = NetworkSpec {
    id: NetworkId::OptimismMainnet,
    display_name: "Optimism",
    ledger_model: LedgerModel::Account,
    capabilities: NetworkCapabilities {
        supports_nonce: true,
        supports_internal_transfers: false,
    },
    block_production: Some(BlockProduction {
        average_block_time: Duration::from_secs(2),
    }),
};

#[cfg(any(feature = "server", test))]
const BASE_MAINNET_NETWORK: NetworkSpec = NetworkSpec {
    id: NetworkId::BaseMainnet,
    display_name: "Base",
    ledger_model: LedgerModel::Account,
    capabilities: NetworkCapabilities {
        supports_nonce: true,
        supports_internal_transfers: false,
    },
    block_production: Some(BlockProduction {
        average_block_time: Duration::from_secs(2),
    }),
};

#[cfg(any(feature = "server", test))]
const TRON_MAINNET_NETWORK: NetworkSpec = NetworkSpec {
    id: NetworkId::TronMainnet,
    display_name: "Tron",
    ledger_model: LedgerModel::Account,
    capabilities: NetworkCapabilities {
        supports_nonce: true,
        supports_internal_transfers: false,
    },
    block_production: Some(BlockProduction {
        average_block_time: Duration::from_secs(3),
    }),
};

#[cfg(any(feature = "server", test))]
const RIPPLE_XRP_MAINNET_NETWORK: NetworkSpec = NetworkSpec {
    id: NetworkId::RippleXrpMainnet,
    display_name: "Ripple",
    ledger_model: LedgerModel::XrpLedger,
    capabilities: NetworkCapabilities {
        supports_nonce: false,
        supports_internal_transfers: false,
    },
    block_production: Some(BlockProduction {
        average_block_time: Duration::from_secs(4),
    }),
};

#[cfg(any(feature = "server", test))]
const DOGECOIN_MAINNET_NETWORK: NetworkSpec = NetworkSpec {
    id: NetworkId::DogecoinMainnet,
    display_name: "Dogecoin",
    ledger_model: LedgerModel::Utxo,
    capabilities: NetworkCapabilities {
        supports_nonce: false,
        supports_internal_transfers: false,
    },
    block_production: Some(BlockProduction {
        average_block_time: Duration::from_secs(60),
    }),
};

#[cfg(any(feature = "server", test))]
const MONERO_MAINNET_NETWORK: NetworkSpec = NetworkSpec {
    id: NetworkId::MoneroMainnet,
    display_name: "Monero",
    ledger_model: LedgerModel::Utxo,
    capabilities: NetworkCapabilities {
        supports_nonce: false,
        supports_internal_transfers: false,
    },
    block_production: Some(BlockProduction {
        average_block_time: Duration::from_secs(120),
    }),
};

#[cfg(any(feature = "server", test))]
const TEZOS_MAINNET_NETWORK: NetworkSpec = NetworkSpec {
    id: NetworkId::TezosMainnet,
    display_name: "Tezos",
    ledger_model: LedgerModel::Account,
    capabilities: NetworkCapabilities {
        supports_nonce: false,
        supports_internal_transfers: false,
    },
    block_production: Some(BlockProduction {
        average_block_time: Duration::from_secs(60),
    }),
};

#[cfg(any(feature = "server", test))]
const ZCASH_MAINNET_NETWORK: NetworkSpec = NetworkSpec {
    id: NetworkId::ZcashMainnet,
    display_name: "Zcash",
    ledger_model: LedgerModel::Utxo,
    capabilities: NetworkCapabilities {
        supports_nonce: false,
        supports_internal_transfers: false,
    },
    block_production: Some(BlockProduction {
        average_block_time: Duration::from_secs(75),
    }),
};

#[cfg(any(feature = "server", test))]
static BTC_BITCOIN_MAINNET_INSTANCE: AssetInstance = AssetInstance {
    id: AssetInstanceId {
        asset_id: AssetId::BITCOIN,
        network_id: NetworkId::BitcoinMainnet,
        namespace: AssetNamespace::Native,
    },
    unit_code: UnitCode::borrowed_unchecked("BTC"),
    symbol: Some(Cow::Borrowed("₿")),
    decimal_precision: 8,
    asset_class: AssetClass::Crypto,
    role: AssetInstanceRole::Native,
};

#[cfg(any(feature = "server", test))]
static ETH_ETHEREUM_MAINNET_INSTANCE: AssetInstance = AssetInstance {
    id: AssetInstanceId {
        asset_id: AssetId::ETHEREUM,
        network_id: NetworkId::EthereumMainnet,
        namespace: AssetNamespace::Native,
    },
    unit_code: UnitCode::borrowed_unchecked("ETH"),
    symbol: Some(Cow::Borrowed("Ξ")),
    decimal_precision: 18,
    asset_class: AssetClass::Crypto,
    role: AssetInstanceRole::Native,
};

#[cfg(feature = "server")]
const MEMPOOL_PROVIDER: SyncProvider = SyncProvider {
    id: SyncProviderId::MempoolSpace,
    display_name: "mempool.space",
    supported_networks: &[NetworkId::BitcoinMainnet],
    capabilities: SyncProviderCapabilities {
        supports_transaction_sync: true,
        supports_balance_only_sync: true,
        supports_internal_transfers: false,
    },
};

#[cfg(feature = "server")]
const ETHERSCAN_PROVIDER: SyncProvider = SyncProvider {
    id: SyncProviderId::Etherscan,
    display_name: "Etherscan",
    supported_networks: &[NetworkId::EthereumMainnet],
    capabilities: SyncProviderCapabilities {
        supports_transaction_sync: true,
        supports_balance_only_sync: true,
        supports_internal_transfers: true,
    },
};

#[cfg(feature = "server")]
const BITCOIN_SYNC_PROVIDERS: &[SyncProviderId] = &[SyncProviderId::MempoolSpace];
#[cfg(feature = "server")]
const ETHEREUM_SYNC_PROVIDERS: &[SyncProviderId] = &[SyncProviderId::Etherscan];

#[cfg(any(feature = "server", test))]
static BTC_BITCOIN_MAINNET_SYNCED: SyncedAssetInstance = SyncedAssetInstance {
    id: SyncedAssetInstanceId::BtcBitcoinMainnet,
    asset_instance_id: AssetInstanceId {
        asset_id: AssetId::BITCOIN,
        network_id: NetworkId::BitcoinMainnet,
        namespace: AssetNamespace::Native,
    },
    #[cfg(feature = "server")]
    default_sync_provider: SyncProviderId::MempoolSpace,
    #[cfg(feature = "server")]
    allowed_sync_providers: BITCOIN_SYNC_PROVIDERS,
};

#[cfg(any(feature = "server", test))]
static ETH_ETHEREUM_MAINNET_SYNCED: SyncedAssetInstance = SyncedAssetInstance {
    id: SyncedAssetInstanceId::EthEthereumMainnet,
    asset_instance_id: AssetInstanceId {
        asset_id: AssetId::ETHEREUM,
        network_id: NetworkId::EthereumMainnet,
        namespace: AssetNamespace::Native,
    },
    #[cfg(feature = "server")]
    default_sync_provider: SyncProviderId::Etherscan,
    #[cfg(feature = "server")]
    allowed_sync_providers: ETHEREUM_SYNC_PROVIDERS,
};

#[cfg(any(feature = "server", test))]
pub(crate) fn network(id: NetworkId) -> &'static NetworkSpec {
    let network = match id {
        NetworkId::BitcoinMainnet => &BITCOIN_MAINNET_NETWORK,
        NetworkId::EthereumMainnet => &ETHEREUM_MAINNET_NETWORK,
        NetworkId::CardanoMainnet => &CARDANO_MAINNET_NETWORK,
        NetworkId::SolanaMainnet => &SOLANA_MAINNET_NETWORK,
        NetworkId::BnbSmartChainMainnet => &BNB_SMART_CHAIN_MAINNET_NETWORK,
        NetworkId::PolygonMainnet => &POLYGON_MAINNET_NETWORK,
        NetworkId::AvalancheCChain => &AVALANCHE_C_CHAIN_NETWORK,
        NetworkId::ArbitrumOne => &ARBITRUM_ONE_NETWORK,
        NetworkId::OptimismMainnet => &OPTIMISM_MAINNET_NETWORK,
        NetworkId::BaseMainnet => &BASE_MAINNET_NETWORK,
        NetworkId::TronMainnet => &TRON_MAINNET_NETWORK,
        NetworkId::RippleXrpMainnet => &RIPPLE_XRP_MAINNET_NETWORK,
        NetworkId::DogecoinMainnet => &DOGECOIN_MAINNET_NETWORK,
        NetworkId::MoneroMainnet => &MONERO_MAINNET_NETWORK,
        NetworkId::TezosMainnet => &TEZOS_MAINNET_NETWORK,
        NetworkId::ZcashMainnet => &ZCASH_MAINNET_NETWORK,
    };
    let _ = (network.id, network.display_name);
    network
}

#[cfg(any(feature = "server", test))]
pub(crate) const fn network_slug(id: NetworkId) -> &'static str {
    match id {
        NetworkId::BitcoinMainnet => "bitcoin-mainnet",
        NetworkId::EthereumMainnet => "ethereum-mainnet",
        NetworkId::CardanoMainnet => "cardano-mainnet",
        NetworkId::SolanaMainnet => "solana-mainnet",
        NetworkId::BnbSmartChainMainnet => "bnb-smart-chain-mainnet",
        NetworkId::PolygonMainnet => "polygon-mainnet",
        NetworkId::AvalancheCChain => "avalanche-c-chain",
        NetworkId::ArbitrumOne => "arbitrum-one",
        NetworkId::OptimismMainnet => "optimism-mainnet",
        NetworkId::BaseMainnet => "base-mainnet",
        NetworkId::TronMainnet => "tron-mainnet",
        NetworkId::RippleXrpMainnet => "ripple-xrp-mainnet",
        NetworkId::DogecoinMainnet => "dogecoin-mainnet",
        NetworkId::MoneroMainnet => "monero-mainnet",
        NetworkId::TezosMainnet => "tezos-mainnet",
        NetworkId::ZcashMainnet => "zcash-mainnet",
    }
}

#[cfg(feature = "server")]
pub(crate) fn sync_provider(id: SyncProviderId) -> &'static SyncProvider {
    let provider = match id {
        SyncProviderId::MempoolSpace => &MEMPOOL_PROVIDER,
        SyncProviderId::Etherscan => &ETHERSCAN_PROVIDER,
    };
    let _ = (
        provider.display_name,
        provider.supported_networks,
        provider.capabilities.supports_transaction_sync,
    );
    provider
}

#[cfg(any(feature = "server", test))]
pub(crate) fn synced_asset_instance(id: SyncedAssetInstanceId) -> &'static SyncedAssetInstance {
    match id {
        SyncedAssetInstanceId::BtcBitcoinMainnet => &BTC_BITCOIN_MAINNET_SYNCED,
        SyncedAssetInstanceId::EthEthereumMainnet => &ETH_ETHEREUM_MAINNET_SYNCED,
    }
}

#[cfg(any(feature = "server", test))]
pub(crate) const fn synced_asset_instance_id(asset_id: SyncedAssetId) -> SyncedAssetInstanceId {
    match asset_id {
        SyncedAssetId::Bitcoin => SyncedAssetInstanceId::BtcBitcoinMainnet,
        SyncedAssetId::Ethereum => SyncedAssetInstanceId::EthEthereumMainnet,
    }
}

#[cfg(feature = "server")]
pub(crate) fn account_model_for(asset_id: SyncedAssetId) -> AccountModel {
    let synced = synced_asset_instance(synced_asset_instance_id(asset_id));
    let instance = synced_asset_instance_asset_instance(synced);
    network(instance.id.network_id).ledger_model.account_model()
}

#[cfg(feature = "server")]
pub(crate) fn default_sync_provider(asset_id: SyncedAssetId) -> SyncProviderId {
    synced_asset_instance(synced_asset_instance_id(asset_id)).default_sync_provider
}

#[cfg(any(feature = "server", test))]
pub(crate) fn synced_asset_instance_asset_instance(
    synced: &SyncedAssetInstance,
) -> &'static AssetInstance {
    if let Some(instance) = asset_instance(&synced.asset_instance_id) {
        return instance;
    }
    match synced.id {
        SyncedAssetInstanceId::BtcBitcoinMainnet => &BTC_BITCOIN_MAINNET_INSTANCE,
        SyncedAssetInstanceId::EthEthereumMainnet => &ETH_ETHEREUM_MAINNET_INSTANCE,
    }
}

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ManualAssetCatalogCandidateId {
    Synced(SyncedAssetInstanceId),
    Unsynced(unsynced::UnsyncedAssetInstanceId),
}

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManualAssetCatalogCandidate {
    pub(crate) id: ManualAssetCatalogCandidateId,
    pub(crate) search_rank: u32,
    pub(crate) asset_id: String,
    pub(crate) network_id: String,
    pub(crate) unit_code: String,
    pub(crate) asset_name: String,
    pub(crate) network_name: String,
    pub(crate) decimal_precision: u8,
    pub(crate) symbol: Option<String>,
    pub(crate) coingecko_id: String,
}

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ManualAssetSearchResult {
    BitGarthCatalog {
        asset_id: String,
        network_id: String,
        unit_code: String,
        asset_name: String,
        network_name: String,
        decimal_precision: u8,
        coingecko_id: String,
    },
    CoinGeckoCatalog {
        coingecko_id: String,
        symbol: String,
        name: String,
        platforms_json: Option<String>,
    },
}

#[cfg(test)]
pub(crate) fn network_display_name(id: NetworkId) -> &'static str {
    network(id).display_name
}

#[cfg(any(feature = "server", test))]
fn manual_catalog_match_quality(
    candidate: &ManualAssetCatalogCandidate,
    query: &str,
) -> Option<u8> {
    let unit_code = candidate.unit_code.to_ascii_lowercase();
    let canonical_name = candidate.asset_name.to_ascii_lowercase();
    let asset_id = candidate.asset_id.as_str();
    let network_name = candidate.network_name.to_ascii_lowercase();
    let network_id = candidate.network_id.as_str();

    if unit_code == query || asset_id == query {
        Some(0)
    } else if unit_code.starts_with(query)
        || canonical_name.starts_with(query)
        || asset_id.starts_with(query)
    {
        Some(1)
    } else if canonical_name.contains(query)
        || network_name.contains(query)
        || asset_id.contains(query)
        || network_id.contains(query)
    {
        Some(2)
    } else {
        None
    }
}

#[cfg(any(feature = "server", test))]
fn synced_manual_catalog_candidates() -> Vec<ManualAssetCatalogCandidate> {
    [
        (SyncedAssetInstanceId::BtcBitcoinMainnet, 0_u32),
        (SyncedAssetInstanceId::EthEthereumMainnet, 1_u32),
    ]
    .into_iter()
    .filter_map(|(id, search_rank)| {
        let synced = synced_asset_instance(id);
        let instance = synced_asset_instance_asset_instance(synced);
        let asset = asset(&instance.id.asset_id)?;
        let network = network(instance.id.network_id);
        let coingecko_id = asset.price_refs.coingecko.as_deref()?;
        Some(ManualAssetCatalogCandidate {
            id: ManualAssetCatalogCandidateId::Synced(id),
            search_rank,
            asset_id: asset.id.as_str().to_string(),
            network_id: network_slug(network.id).to_string(),
            unit_code: instance.unit_code.as_str().to_string(),
            asset_name: asset.canonical_name.to_string(),
            network_name: network.display_name.to_string(),
            decimal_precision: instance.decimal_precision,
            symbol: instance.symbol.as_deref().map(str::to_string),
            coingecko_id: coingecko_id.to_string(),
        })
    })
    .collect()
}

#[cfg(any(feature = "server", test))]
fn unsynced_manual_catalog_candidate(
    instance: &unsynced::UnsyncedAssetInstance,
) -> ManualAssetCatalogCandidate {
    ManualAssetCatalogCandidate {
        id: ManualAssetCatalogCandidateId::Unsynced(instance.id.clone()),
        search_rank: instance.market_cap_rank.saturating_add(2),
        asset_id: instance.id.asset_id.as_str().to_string(),
        network_id: instance.id.network_id.as_str().to_string(),
        unit_code: instance.unit_code.as_str().to_string(),
        asset_name: instance.canonical_name.clone(),
        network_name: instance.network_name.clone(),
        decimal_precision: instance.decimal_precision.as_u8(),
        symbol: instance.symbol.map(|symbol| symbol.to_string()),
        coingecko_id: instance.coingecko_id.as_str().to_string(),
    }
}

#[cfg(any(feature = "server", test))]
pub(crate) fn manual_catalog_candidates()
-> Result<Vec<ManualAssetCatalogCandidate>, unsynced::UnsyncedCatalogError> {
    let mut candidates = synced_manual_catalog_candidates();
    let catalog = unsynced::load_catalog()?;
    candidates.extend(
        catalog
            .manual_order
            .iter()
            .filter_map(|id| catalog.instance(id))
            .map(unsynced_manual_catalog_candidate),
    );
    Ok(candidates)
}

#[cfg(feature = "server")]
pub(crate) fn manual_catalog_candidate(
    id: &ManualAssetCatalogCandidateId,
) -> Result<Option<ManualAssetCatalogCandidate>, unsynced::UnsyncedCatalogError> {
    match id {
        ManualAssetCatalogCandidateId::Synced(synced_id) => {
            let id = ManualAssetCatalogCandidateId::Synced(*synced_id);
            Ok(synced_manual_catalog_candidates()
                .into_iter()
                .find(|candidate| candidate.id == id))
        }
        ManualAssetCatalogCandidateId::Unsynced(unsynced_id) => Ok(unsynced::load_catalog()?
            .instance(unsynced_id)
            .map(unsynced_manual_catalog_candidate)),
    }
}

#[cfg(any(feature = "server", test))]
pub(crate) fn search_manual_asset_instances(
    query: &str,
) -> Result<Vec<ManualAssetSearchResult>, unsynced::UnsyncedCatalogError> {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return Ok(Vec::new());
    }

    let mut matches = manual_catalog_candidates()?
        .into_iter()
        .filter_map(|candidate| {
            manual_catalog_match_quality(&candidate, &query).map(|quality| (quality, candidate))
        })
        .collect::<Vec<_>>();

    matches.sort_by(|(left_quality, left), (right_quality, right)| {
        left_quality
            .cmp(right_quality)
            .then_with(|| left.search_rank.cmp(&right.search_rank))
            .then_with(|| left.asset_id.cmp(&right.asset_id))
            .then_with(|| left.network_id.cmp(&right.network_id))
    });
    matches.truncate(25);

    Ok(matches
        .into_iter()
        .map(
            |(_quality, candidate)| ManualAssetSearchResult::BitGarthCatalog {
                asset_id: candidate.asset_id,
                network_id: candidate.network_id,
                unit_code: candidate.unit_code,
                asset_name: candidate.asset_name,
                network_name: candidate.network_name,
                decimal_precision: candidate.decimal_precision,
                coingecko_id: candidate.coingecko_id,
            },
        )
        .collect())
}

#[cfg(feature = "server")]
pub(crate) fn count_manual_asset_instance_matches(
    query: &str,
) -> Result<usize, unsynced::UnsyncedCatalogError> {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return Ok(0);
    }

    Ok(manual_catalog_candidates()?
        .iter()
        .filter(|candidate| manual_catalog_match_quality(candidate, &query).is_some())
        .count())
}

#[cfg(any(feature = "server", test))]
pub(crate) fn resolve_manual_coingecko_id(
    asset_id: &str,
    snapshot_coingecko_id: Option<&str>,
) -> Option<String> {
    if let Ok(catalog) = unsynced::load_catalog()
        && let Some(id) = catalog.coingecko_id_for_asset_id(asset_id)
    {
        return Some(id.to_string());
    }
    snapshot_coingecko_id.map(str::to_string)
}

#[cfg(feature = "server")]
pub(crate) fn manual_discovery_excluded_coingecko_ids()
-> Result<Vec<String>, unsynced::UnsyncedCatalogError> {
    Ok(manual_catalog_candidates()?
        .into_iter()
        .map(|candidate| candidate.coingecko_id)
        .collect())
}

#[cfg(feature = "server")]
pub(crate) fn coingecko_id_is_manual_discovery_excluded(
    coingecko_id: &str,
) -> Result<bool, unsynced::UnsyncedCatalogError> {
    Ok(manual_discovery_excluded_coingecko_ids()?
        .iter()
        .any(|id| id == coingecko_id))
}

#[cfg(feature = "server")]
pub(crate) fn manual_catalog_candidate_id_from_view(
    view: &crate::asset_views::ManualAssetInstanceIdView,
) -> Result<Option<ManualAssetCatalogCandidateId>, unsynced::UnsyncedCatalogError> {
    if unsynced::UnsyncedAssetId::parse(&view.asset_id).is_err() {
        return Ok(None);
    }
    if unsynced::UnsyncedNetworkId::parse(&view.network_id).is_err() {
        return Ok(None);
    }

    Ok(manual_catalog_candidates()?
        .into_iter()
        .find(|candidate| {
            candidate.asset_id == view.asset_id && candidate.network_id == view.network_id
        })
        .map(|candidate| candidate.id))
}

#[cfg(test)]
pub(crate) fn manual_migration_targets_for_unit_code(
    unit_code: &crate::wallets::ValidatedManualAssetUnitCode,
) -> Result<Vec<&'static unsynced::UnsyncedAssetInstance>, unsynced::UnsyncedCatalogError> {
    let catalog = unsynced::load_catalog()?;
    Ok(catalog
        .manual_order
        .iter()
        .filter_map(|id| catalog.instance(id))
        .filter(|instance| {
            instance
                .unit_code
                .as_str()
                .eq_ignore_ascii_case(unit_code.as_str())
        })
        .collect())
}

#[cfg(any(feature = "server", test))]
pub(crate) fn asset_id_for_synced_asset(asset_id: SyncedAssetId) -> AssetId {
    match asset_id {
        SyncedAssetId::Bitcoin => AssetId::BITCOIN,
        SyncedAssetId::Ethereum => AssetId::ETHEREUM,
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn synced_asset_instance_maps_to_concrete_instance() {
        assert_eq!(
            SyncedAssetInstanceId::BtcBitcoinMainnet.as_instance(),
            AssetInstanceId {
                asset_id: AssetId::BITCOIN,
                network_id: NetworkId::BitcoinMainnet,
                namespace: AssetNamespace::Native,
            }
        );
        assert_eq!(
            SyncedAssetInstanceId::EthEthereumMainnet.as_instance(),
            AssetInstanceId {
                asset_id: AssetId::ETHEREUM,
                network_id: NetworkId::EthereumMainnet,
                namespace: AssetNamespace::Native,
            }
        );
    }

    #[test]
    fn synced_asset_id_maps_to_synced_asset_instance() {
        assert_eq!(
            synced_asset_instance_id(SyncedAssetId::Bitcoin),
            SyncedAssetInstanceId::BtcBitcoinMainnet
        );
        assert_eq!(
            synced_asset_instance_id(SyncedAssetId::Ethereum),
            SyncedAssetInstanceId::EthEthereumMainnet
        );
    }

    #[test]
    fn price_refs_are_economic_asset_refs_not_wallet_refs() {
        let bitcoin = asset(&SyncedAssetInstanceId::BtcBitcoinMainnet.asset_id()).expect("bitcoin");
        let ethereum =
            asset(&SyncedAssetInstanceId::EthEthereumMainnet.asset_id()).expect("ethereum");

        assert_eq!(bitcoin.price_refs.coingecko.as_deref(), Some("bitcoin"));
        assert_eq!(ethereum.price_refs.coingecko.as_deref(), Some("ethereum"));
    }

    #[test]
    fn resolve_coingecko_id_prefers_catalog_over_snapshot() {
        let bitcoin = asset(&asset_id_for_synced_asset(SyncedAssetId::Bitcoin))
            .expect("bitcoin asset registered");
        assert_eq!(bitcoin.price_refs.coingecko.as_deref(), Some("bitcoin"));

        assert_eq!(
            resolve_manual_coingecko_id("ripple", Some("stale-snapshot")),
            Some("ripple".to_string()),
            "live catalog must override the db snapshot"
        );
        assert_eq!(
            resolve_manual_coingecko_id("totally-unknown-asset", Some("snap-id")),
            Some("snap-id".to_string())
        );
        assert_eq!(
            resolve_manual_coingecko_id("totally-unknown-asset", None),
            None
        );
    }

    #[test]
    #[cfg(feature = "server")]
    fn internal_transfer_support_requires_network_and_provider_support() {
        let bitcoin = synced_asset_instance(SyncedAssetInstanceId::BtcBitcoinMainnet);
        let bitcoin_network = network(bitcoin.asset_instance_id.network_id);
        let bitcoin_provider = sync_provider(bitcoin.default_sync_provider);
        assert!(!bitcoin_network.capabilities.supports_internal_transfers);
        assert!(!bitcoin_provider.capabilities.supports_internal_transfers);

        let ethereum = synced_asset_instance(SyncedAssetInstanceId::EthEthereumMainnet);
        let ethereum_network = network(ethereum.asset_instance_id.network_id);
        let ethereum_provider = sync_provider(ethereum.default_sync_provider);
        assert!(ethereum_network.capabilities.supports_internal_transfers);
        assert!(ethereum_provider.capabilities.supports_internal_transfers);
    }

    #[test]
    #[cfg(feature = "server")]
    fn synced_registry_entries_are_coherent() {
        for synced_id in [
            SyncedAssetInstanceId::BtcBitcoinMainnet,
            SyncedAssetInstanceId::EthEthereumMainnet,
        ] {
            let synced = synced_asset_instance(synced_id);
            let provider = sync_provider(synced.default_sync_provider);

            assert_eq!(synced.id, synced_id);
            assert_eq!(
                asset_instance(&synced.asset_instance_id).map(|instance| instance.id.clone()),
                Some(synced_id.as_instance())
            );
            let asset_id = synced_id.asset_id();
            assert_eq!(asset(&asset_id).map(|a| &a.id), Some(&asset_id));
            assert_eq!(network(synced_id.network_id()).id, synced_id.network_id());
            assert!(
                provider
                    .supported_networks
                    .contains(&synced_id.network_id())
            );
            assert!(synced.allowed_sync_providers.contains(&provider.id));
            assert!(provider.capabilities.supports_transaction_sync);
        }
    }

    #[test]
    fn synced_asset_instance_ids_recognize_concrete_instances() {
        assert_eq!(
            SyncedAssetInstanceId::from_instance(
                &SyncedAssetInstanceId::BtcBitcoinMainnet.as_instance()
            ),
            Some(SyncedAssetInstanceId::BtcBitcoinMainnet)
        );
        assert_eq!(
            SyncedAssetInstanceId::from_instance(
                &SyncedAssetInstanceId::EthEthereumMainnet.as_instance()
            ),
            Some(SyncedAssetInstanceId::EthEthereumMainnet)
        );
    }

    #[test]
    fn manual_search_uses_unsynced_catalog_and_data_defined_networks() {
        let rows = search_manual_asset_instances("algorand").expect("catalog search succeeds");
        assert!(rows.iter().any(|row| matches!(
            row,
            ManualAssetSearchResult::BitGarthCatalog {
                asset_id,
                network_id,
                ..
            } if asset_id == "algorand"
                && network_id == "algorand-mainnet"
        )));
        assert!(rows.iter().all(|row| {
            !matches!(
                row,
                ManualAssetSearchResult::BitGarthCatalog {
                    asset_id,
                    ..
                } if asset_id == "usd-coin"
            )
        }));
    }

    #[test]
    fn manual_search_includes_synced_assets_as_catalog_candidates() {
        let btc = search_manual_asset_instances("btc").expect("catalog search succeeds");
        assert!(btc.iter().any(|row| matches!(
            row,
            ManualAssetSearchResult::BitGarthCatalog {
                asset_id,
                network_id,
                unit_code,
                asset_name,
                network_name,
                decimal_precision,
                coingecko_id,
            } if asset_id == "bitcoin"
                && network_id == "bitcoin-mainnet"
                && unit_code == "BTC"
                && asset_name == "Bitcoin"
                && network_name == "Bitcoin"
                && *decimal_precision == 8
                && coingecko_id == "bitcoin"
        )));

        let eth = search_manual_asset_instances("eth").expect("catalog search succeeds");
        assert!(eth.iter().any(|row| matches!(
            row,
            ManualAssetSearchResult::BitGarthCatalog {
                asset_id,
                network_id,
                unit_code,
                asset_name,
                network_name,
                decimal_precision,
                coingecko_id,
            } if asset_id == "ethereum"
                && network_id == "ethereum-mainnet"
                && unit_code == "ETH"
                && asset_name == "Ethereum"
                && network_name == "Ethereum"
                && *decimal_precision == 18
                && coingecko_id == "ethereum"
        )));
    }

    #[test]
    fn manual_search_orders_by_match_quality_then_catalog_rank() {
        let rows = search_manual_asset_instances("ada").expect("catalog search succeeds");
        let first = rows.first().expect("ADA should match");
        assert!(matches!(
            first,
            ManualAssetSearchResult::BitGarthCatalog { unit_code, .. } if unit_code == "ADA"
        ));

        let rows = search_manual_asset_instances("mainnet").expect("catalog search succeeds");
        let first_unit_codes = rows
            .iter()
            .take(5)
            .map(|row| match row {
                ManualAssetSearchResult::BitGarthCatalog { unit_code, .. } => unit_code.as_str(),
                ManualAssetSearchResult::CoinGeckoCatalog { .. } => "coingecko",
            })
            .collect::<Vec<_>>();
        assert_eq!(first_unit_codes, ["BTC", "ETH", "XRP", "BNB", "SOL"]);
    }

    #[test]
    fn manual_search_result_can_represent_coingecko_catalog_rows() {
        let row = ManualAssetSearchResult::CoinGeckoCatalog {
            coingecko_id: "adappter-token".to_string(),
            symbol: "adp".to_string(),
            name: "Adappter Token".to_string(),
            platforms_json: Some(r#"{"ethereum":"0xabc"}"#.to_string()),
        };
        assert!(matches!(
            row,
            ManualAssetSearchResult::CoinGeckoCatalog { symbol, .. } if symbol == "adp"
        ));
    }

    #[test]
    #[cfg(feature = "server")]
    fn manual_catalog_candidate_id_from_view_resolves_synced_and_unsynced() {
        let btc = crate::asset_views::ManualAssetInstanceIdView {
            asset_id: "bitcoin".to_string(),
            network_id: "bitcoin-mainnet".to_string(),
        };
        assert_eq!(
            manual_catalog_candidate_id_from_view(&btc).expect("catalog loads"),
            Some(ManualAssetCatalogCandidateId::Synced(
                SyncedAssetInstanceId::BtcBitcoinMainnet
            ))
        );
        assert_eq!(
            manual_catalog_candidate(&ManualAssetCatalogCandidateId::Synced(
                SyncedAssetInstanceId::BtcBitcoinMainnet
            ))
            .expect("catalog loads")
            .map(|candidate| (candidate.asset_id, candidate.network_id)),
            Some(("bitcoin".to_string(), "bitcoin-mainnet".to_string()))
        );

        let ada = crate::asset_views::ManualAssetInstanceIdView {
            asset_id: "cardano".to_string(),
            network_id: "cardano-mainnet".to_string(),
        };
        assert!(matches!(
            manual_catalog_candidate_id_from_view(&ada).expect("catalog loads"),
            Some(ManualAssetCatalogCandidateId::Unsynced(id))
                if id.asset_id.as_str() == "cardano"
                    && id.network_id.as_str() == "cardano-mainnet"
        ));
    }

    #[test]
    #[cfg(feature = "server")]
    fn manual_catalog_candidate_id_from_view_distinguishes_unknown_from_catalog_error() {
        let invalid = crate::asset_views::ManualAssetInstanceIdView {
            asset_id: "BAD ID".to_string(),
            network_id: "cardano-mainnet".to_string(),
        };
        assert_eq!(
            manual_catalog_candidate_id_from_view(&invalid).expect("catalog not needed"),
            None
        );

        let unknown = crate::asset_views::ManualAssetInstanceIdView {
            asset_id: "definitely-not-real".to_string(),
            network_id: "definitely-not-real-mainnet".to_string(),
        };
        assert_eq!(
            manual_catalog_candidate_id_from_view(&unknown).expect("catalog loads"),
            None
        );
    }

    #[test]
    fn network_display_name_matches_network_spec() {
        for id in [
            NetworkId::BitcoinMainnet,
            NetworkId::EthereumMainnet,
            NetworkId::CardanoMainnet,
        ] {
            assert_eq!(network_display_name(id), network(id).display_name);
        }
    }

    #[test]
    fn manual_migration_targets_match_same_unit_in_catalog_order() {
        let code = crate::wallets::ValidatedManualAssetUnitCode::parse("USDC").expect("valid code");
        let targets = manual_migration_targets_for_unit_code(&code).expect("catalog loads");
        assert!(targets.len() >= 2);
        assert!(
            targets
                .iter()
                .all(|target| target.unit_code.as_str() == "USDC")
        );
    }

    #[test]
    fn manual_migration_targets_return_single_ada_and_exclude_synced_unknown() {
        let ada = crate::wallets::ValidatedManualAssetUnitCode::parse("ada").expect("valid code");
        let ada_targets = manual_migration_targets_for_unit_code(&ada).expect("catalog loads");
        assert_eq!(ada_targets.len(), 1);
        assert_eq!(ada_targets[0].id.asset_id.as_str(), "cardano");

        let btc = crate::wallets::ValidatedManualAssetUnitCode::parse("BTC").expect("valid code");
        assert!(
            manual_migration_targets_for_unit_code(&btc)
                .expect("catalog loads")
                .is_empty()
        );

        let unknown =
            crate::wallets::ValidatedManualAssetUnitCode::parse("ZZZZ").expect("valid code");
        assert!(
            manual_migration_targets_for_unit_code(&unknown)
                .expect("catalog loads")
                .is_empty()
        );
    }

    #[test]
    #[cfg(feature = "server")]
    fn new_ledger_models_project_to_account_storage() {
        assert_eq!(
            LedgerModel::SolanaAccount.account_model(),
            AccountModel::Account
        );
        assert_eq!(
            LedgerModel::XrpLedger.account_model(),
            AccountModel::Account
        );
        assert_eq!(LedgerModel::Utxo.account_model(), AccountModel::Utxo);
        assert_eq!(
            LedgerModel::ExtendedUtxo.account_model(),
            AccountModel::Utxo
        );
        assert_eq!(LedgerModel::Account.account_model(), AccountModel::Account);
    }

    #[test]
    #[cfg(feature = "server")]
    fn catalog_networks_resolve_and_round_trip() {
        for (id, slug, model) in [
            (
                NetworkId::SolanaMainnet,
                "solana-mainnet",
                LedgerModel::SolanaAccount,
            ),
            (
                NetworkId::BnbSmartChainMainnet,
                "bnb-smart-chain-mainnet",
                LedgerModel::Account,
            ),
            (
                NetworkId::PolygonMainnet,
                "polygon-mainnet",
                LedgerModel::Account,
            ),
            (
                NetworkId::AvalancheCChain,
                "avalanche-c-chain",
                LedgerModel::Account,
            ),
            (NetworkId::ArbitrumOne, "arbitrum-one", LedgerModel::Account),
            (
                NetworkId::OptimismMainnet,
                "optimism-mainnet",
                LedgerModel::Account,
            ),
            (NetworkId::BaseMainnet, "base-mainnet", LedgerModel::Account),
            (NetworkId::TronMainnet, "tron-mainnet", LedgerModel::Account),
            (
                NetworkId::RippleXrpMainnet,
                "ripple-xrp-mainnet",
                LedgerModel::XrpLedger,
            ),
            (
                NetworkId::DogecoinMainnet,
                "dogecoin-mainnet",
                LedgerModel::Utxo,
            ),
        ] {
            let spec = network(id);
            assert_eq!(spec.id, id);
            assert_eq!(spec.ledger_model, model);
            assert!(spec.block_production.is_some());
            let slug_from_view = network_slug(id);
            assert_eq!(slug_from_view, slug);
        }
    }

    #[cfg(feature = "server")]
    #[test]
    fn excluded_coingecko_ids_include_unsynced_catalog_entries() {
        let ids =
            super::manual_discovery_excluded_coingecko_ids().expect("exclusion ids should load");
        assert!(ids.iter().any(|id| id == "bitcoin"));
        assert!(ids.iter().any(|id| id == "ethereum"));
        // "cardano" is the unsynced-catalog (ADA) CoinGecko id used elsewhere in tests.
        assert!(ids.iter().any(|id| id == "cardano"));
        // The shared helper and the per-id predicate must agree.
        assert!(super::coingecko_id_is_manual_discovery_excluded("cardano").unwrap());
        assert!(
            !super::coingecko_id_is_manual_discovery_excluded("definitely-not-a-real-id").unwrap()
        );
        assert!(!ids.iter().any(|id| id == "definitely-not-a-real-id"));
    }
}
