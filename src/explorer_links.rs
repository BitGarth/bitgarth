use crate::models::{
    EtherscanBaseUrlError, MempoolBaseUrlError, resolve_effective_etherscan_base_url,
    resolve_effective_mempool_base_url,
};
use crate::settings::SettingsState;
use crate::wallets::{Network, SyncedAssetId};
use std::fmt;

#[derive(Debug)]
pub(crate) enum ExplorerLinkError {
    Mempool(MempoolBaseUrlError),
    Etherscan(EtherscanBaseUrlError),
    UnsupportedNetwork {
        asset: SyncedAssetId,
        network: Network,
    },
}

impl fmt::Display for ExplorerLinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mempool(err) => write!(f, "{err}"),
            Self::Etherscan(err) => write!(f, "{err}"),
            Self::UnsupportedNetwork { asset, network } => write!(
                f,
                "{} explorer links are unavailable for {}",
                asset.as_str(),
                network.as_str()
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DigitalAssetAddressRef<'a> {
    Bitcoin { network: Network, address: &'a str },
    Ethereum { network: Network, address: &'a str },
}

impl<'a> DigitalAssetAddressRef<'a> {
    pub(crate) fn from_asset(asset: SyncedAssetId, network: Network, address: &'a str) -> Self {
        match asset {
            SyncedAssetId::Bitcoin => Self::Bitcoin { network, address },
            SyncedAssetId::Ethereum => Self::Ethereum { network, address },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DigitalAssetTransactionRef<'a> {
    Bitcoin { network: Network, tx_hash: &'a str },
    Ethereum { network: Network, tx_hash: &'a str },
}

impl<'a> DigitalAssetTransactionRef<'a> {
    pub(crate) fn from_asset(asset: SyncedAssetId, network: Network, tx_hash: &'a str) -> Self {
        match asset {
            SyncedAssetId::Bitcoin => Self::Bitcoin { network, tx_hash },
            SyncedAssetId::Ethereum => Self::Ethereum { network, tx_hash },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExplorerTarget<'a> {
    Address(DigitalAssetAddressRef<'a>),
    Transaction(DigitalAssetTransactionRef<'a>),
}

pub(crate) fn explorer_url(
    settings_state: &SettingsState,
    target: ExplorerTarget<'_>,
) -> Result<String, ExplorerLinkError> {
    match target {
        ExplorerTarget::Address(target) => address_explorer_url(settings_state, target),
        ExplorerTarget::Transaction(target) => tx_explorer_url(settings_state, target),
    }
}

pub(crate) fn address_explorer_url(
    settings_state: &SettingsState,
    target: DigitalAssetAddressRef<'_>,
) -> Result<String, ExplorerLinkError> {
    match target {
        DigitalAssetAddressRef::Bitcoin { network, address } => {
            ensure_supported_network(SyncedAssetId::Bitcoin, network)?;
            let configured_override = (settings_state.mempool_base_url)();
            let (base_url, _) = resolve_effective_mempool_base_url(configured_override.as_ref())
                .map_err(ExplorerLinkError::Mempool)?;
            base_url
                .address_url(address)
                .map_err(ExplorerLinkError::Mempool)
        }
        DigitalAssetAddressRef::Ethereum { network, address } => {
            ensure_supported_network(SyncedAssetId::Ethereum, network)?;
            let configured_override = (settings_state.etherscan_base_url)();
            let (api_url, _) = resolve_effective_etherscan_base_url(configured_override.as_ref())
                .map_err(ExplorerLinkError::Etherscan)?;
            api_url
                .address_url(address)
                .map_err(ExplorerLinkError::Etherscan)
        }
    }
}

pub(crate) fn tx_explorer_url(
    settings_state: &SettingsState,
    target: DigitalAssetTransactionRef<'_>,
) -> Result<String, ExplorerLinkError> {
    match target {
        DigitalAssetTransactionRef::Bitcoin { network, tx_hash } => {
            ensure_supported_network(SyncedAssetId::Bitcoin, network)?;
            let configured_override = (settings_state.mempool_base_url)();
            let (base_url, _) = resolve_effective_mempool_base_url(configured_override.as_ref())
                .map_err(ExplorerLinkError::Mempool)?;
            base_url
                .transaction_url(tx_hash)
                .map_err(ExplorerLinkError::Mempool)
        }
        DigitalAssetTransactionRef::Ethereum { network, tx_hash } => {
            ensure_supported_network(SyncedAssetId::Ethereum, network)?;
            let configured_override = (settings_state.etherscan_base_url)();
            let (api_url, _) = resolve_effective_etherscan_base_url(configured_override.as_ref())
                .map_err(ExplorerLinkError::Etherscan)?;
            api_url
                .transaction_url(tx_hash)
                .map_err(ExplorerLinkError::Etherscan)
        }
    }
}

fn ensure_supported_network(
    asset: SyncedAssetId,
    network: Network,
) -> Result<(), ExplorerLinkError> {
    match network {
        Network::Mainnet => Ok(()),
        Network::Testnet | Network::Signet | Network::Regtest => {
            Err(ExplorerLinkError::UnsupportedNetwork { asset, network })
        }
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use crate::models::{EtherscanBaseUrl, MempoolBaseUrl};
    use crate::wallets::{Network, SyncedAssetId};

    #[test]
    fn address_ref_from_asset_preserves_network_and_value() {
        let target = super::DigitalAssetAddressRef::from_asset(
            SyncedAssetId::Bitcoin,
            Network::Testnet,
            "addr",
        );

        assert_eq!(
            target,
            super::DigitalAssetAddressRef::Bitcoin {
                network: Network::Testnet,
                address: "addr",
            }
        );
    }

    #[test]
    fn transaction_ref_from_asset_preserves_network_and_value() {
        let target = super::DigitalAssetTransactionRef::from_asset(
            SyncedAssetId::Ethereum,
            Network::Mainnet,
            "0xhash",
        );

        assert_eq!(
            target,
            super::DigitalAssetTransactionRef::Ethereum {
                network: Network::Mainnet,
                tx_hash: "0xhash",
            }
        );
    }

    #[test]
    fn non_mainnet_targets_are_unavailable() {
        let err = super::ensure_supported_network(SyncedAssetId::Bitcoin, Network::Signet)
            .expect_err("signet should not guess a mainnet explorer");

        assert!(matches!(
            err,
            super::ExplorerLinkError::UnsupportedNetwork {
                asset: SyncedAssetId::Bitcoin,
                network: Network::Signet,
            }
        ));
    }

    // ============ Bitcoin Address URL Tests ============

    #[test]
    fn bitcoin_address_url_default_mempool_base() {
        let base = MempoolBaseUrl::default_public();
        let url = base.address_url("bc1qtest123").expect("should build URL");
        assert_eq!(url, "https://mempool.space/address/bc1qtest123");
    }

    #[test]
    fn bitcoin_address_url_custom_mempool_base() {
        let base =
            MempoolBaseUrl::parse("https://my-mempool.example.com").expect("should parse custom");
        let url = base.address_url("bc1qtest123").expect("should build URL");
        assert_eq!(url, "https://my-mempool.example.com/address/bc1qtest123");
    }

    #[test]
    fn bitcoin_address_url_custom_mempool_with_trailing_slash() {
        let base = MempoolBaseUrl::parse("https://my-mempool.example.com/").expect("should parse");
        let url = base.address_url("bc1qtest123").expect("should build URL");
        assert_eq!(url, "https://my-mempool.example.com/address/bc1qtest123");
    }

    #[test]
    fn bitcoin_address_url_invalid_mempool_returns_error() {
        let err = MempoolBaseUrl::parse("not a url").expect_err("should fail");
        assert!(!err.to_string().is_empty());
    }

    // ============ Bitcoin Transaction URL Tests ============

    #[test]
    fn bitcoin_tx_url_default_mempool_base() {
        let base = MempoolBaseUrl::default_public();
        let url = base
            .transaction_url("abc123def456")
            .expect("should build URL");
        assert_eq!(url, "https://mempool.space/tx/abc123def456");
    }

    #[test]
    fn bitcoin_tx_url_custom_mempool_base() {
        let base =
            MempoolBaseUrl::parse("https://my-mempool.example.com").expect("should parse custom");
        let url = base
            .transaction_url("abc123def456")
            .expect("should build URL");
        assert_eq!(url, "https://my-mempool.example.com/tx/abc123def456");
    }

    // ============ Ethereum Address URL Tests ============

    #[test]
    fn ethereum_address_url_from_default_api() {
        let api = EtherscanBaseUrl::default_public();
        let url = api
            .address_url("0xABC123")
            .expect("should derive and build URL");
        assert_eq!(url, "https://etherscan.io/address/0xABC123");
    }

    #[test]
    fn ethereum_address_url_from_custom_api_with_port() {
        let api =
            EtherscanBaseUrl::parse("http://localhost:9000/api/").expect("should parse custom");
        let url = api
            .address_url("0xABC123")
            .expect("should derive and build URL");
        assert_eq!(url, "http://localhost:9000/address/0xABC123");
    }

    #[test]
    fn ethereum_address_url_from_custom_host_without_api_prefix() {
        let api = EtherscanBaseUrl::parse("https://etherscan.example.internal/api/")
            .expect("should parse");
        let url = api
            .address_url("0xABC123")
            .expect("should derive and build URL");
        assert_eq!(url, "https://etherscan.example.internal/address/0xABC123");
    }

    // ============ Ethereum Transaction URL Tests ============

    #[test]
    fn ethereum_tx_url_from_default_api() {
        let api = EtherscanBaseUrl::default_public();
        let url = api
            .transaction_url("0xdeadbeef")
            .expect("should derive and build URL");
        assert_eq!(url, "https://etherscan.io/tx/0xdeadbeef");
    }

    #[test]
    fn ethereum_tx_url_from_custom_api_with_port() {
        let api =
            EtherscanBaseUrl::parse("http://localhost:9000/api/").expect("should parse custom");
        let url = api
            .transaction_url("0xdeadbeef")
            .expect("should derive and build URL");
        assert_eq!(url, "http://localhost:9000/tx/0xdeadbeef");
    }

    // ============ Etherscan Web Explorer Root Derivation Tests ============

    #[test]
    fn derive_web_explorer_root_strips_api_prefix_and_path() {
        let api =
            EtherscanBaseUrl::parse("https://api.etherscan.io/v2/api/").expect("should parse");
        let root = api.derive_web_explorer_root().expect("should derive");
        assert_eq!(root.as_str(), "https://etherscan.io/");
    }

    #[test]
    fn derive_web_explorer_root_from_api_without_v2_path() {
        let api = EtherscanBaseUrl::parse("https://api.etherscan.io/api/").expect("should parse");
        let root = api.derive_web_explorer_root().expect("should derive");
        assert_eq!(root.as_str(), "https://etherscan.io/");
    }

    #[test]
    fn derive_web_explorer_root_localhost_with_port() {
        let api = EtherscanBaseUrl::parse("http://localhost:9000/api/").expect("should parse");
        let root = api.derive_web_explorer_root().expect("should derive");
        assert_eq!(root.as_str(), "http://localhost:9000/");
    }

    #[test]
    fn derive_web_explorer_root_custom_host_without_api_prefix() {
        let api = EtherscanBaseUrl::parse("https://etherscan.example.internal/api/")
            .expect("should parse");
        let root = api.derive_web_explorer_root().expect("should derive");
        assert_eq!(root.as_str(), "https://etherscan.example.internal/");
    }

    #[test]
    fn derive_web_explorer_root_preserves_trailing_slash() {
        let api = EtherscanBaseUrl::parse("https://api.etherscan.io/v2/api").expect("should parse");
        let root = api.derive_web_explorer_root().expect("should derive");
        assert!(root.as_str().ends_with('/'));
    }

    // ============ Invalid Override Error Tests ============

    #[test]
    fn invalid_mempool_url_returns_error_not_fallback() {
        let result = MempoolBaseUrl::parse("not a url");
        assert!(result.is_err());
    }

    #[test]
    fn invalid_etherscan_url_returns_error_not_fallback() {
        let result = EtherscanBaseUrl::parse("not a url");
        assert!(result.is_err());
    }

    // ============ Trailing Slash Consistency ============

    #[test]
    fn mempool_url_trailing_slash_consistency() {
        let without_slash = MempoolBaseUrl::parse("https://mempool.space").expect("should parse");
        let with_slash = MempoolBaseUrl::parse("https://mempool.space/").expect("should parse");
        assert_eq!(
            without_slash.address_url("test").unwrap(),
            with_slash.address_url("test").unwrap()
        );
        assert_eq!(
            without_slash.transaction_url("test").unwrap(),
            with_slash.transaction_url("test").unwrap()
        );
    }

    #[test]
    fn etherscan_url_trailing_slash_consistency() {
        let without_slash =
            EtherscanBaseUrl::parse("https://api.etherscan.io/v2/api").expect("should parse");
        let with_slash =
            EtherscanBaseUrl::parse("https://api.etherscan.io/v2/api/").expect("should parse");
        assert_eq!(
            without_slash.address_url("0xABC").unwrap(),
            with_slash.address_url("0xABC").unwrap()
        );
        assert_eq!(
            without_slash.transaction_url("0xABC").unwrap(),
            with_slash.transaction_url("0xABC").unwrap()
        );
    }
}
