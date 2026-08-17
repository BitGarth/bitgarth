mod client;
mod error;
mod types;

#[cfg(all(test, feature = "db-tests"))]
pub(crate) use client::EtherscanFetchedItem;
pub(crate) use client::{EtherscanClient, EtherscanFetchedPage, EtherscanRequestMetadata};
pub(crate) use error::EtherscanError;
pub(crate) use types::{EtherscanInternalTx, EtherscanNormalTx};

const ETHERSCAN_V2_BASE: &str = "https://api.etherscan.io/v2/api";

/// Networks supported by the Etherscan API.
///
/// The app layer maps its own `Network` type to this enum.
#[derive(Debug, Clone, Copy)]
pub(crate) enum EtherscanNetwork {
    EthereumMainnet,
    Sepolia,
}

impl EtherscanNetwork {
    pub(crate) fn base_url(self) -> &'static str {
        match self {
            Self::EthereumMainnet | Self::Sepolia => ETHERSCAN_V2_BASE,
        }
    }

    pub(crate) fn chain_id(self) -> u64 {
        match self {
            Self::EthereumMainnet => 1,
            Self::Sepolia => 11_155_111,
        }
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn etherscan_network_base_url() {
        assert_eq!(
            EtherscanNetwork::EthereumMainnet.base_url(),
            ETHERSCAN_V2_BASE
        );
        assert_eq!(EtherscanNetwork::Sepolia.base_url(), ETHERSCAN_V2_BASE);
    }

    #[test]
    fn etherscan_network_chain_id() {
        assert_eq!(EtherscanNetwork::EthereumMainnet.chain_id(), 1);
        assert_eq!(EtherscanNetwork::Sepolia.chain_id(), 11_155_111);
    }
}
