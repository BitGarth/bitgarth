use super::context::SyncIterationResult;
use crate::db::CoverageInvalidationTargets;
use crate::db::DbError;
use crate::integrations::etherscan::EtherscanError;
use crate::integrations::mempool::MempoolError;
use crate::wallets::Network;

#[derive(Debug)]
pub(crate) enum UserTransactionMonitorError {
    Db(DbError),
    InvalidConfiguredBaseUrl(String),
    InvalidDefaultBaseUrl(String),
    MissingEtherscanApiKey,
    UnsupportedEthereumNetwork(Network),
    Http(String),
    UpstreamStatus {
        url: String,
        status: u16,
        body_snippet: String,
    },
    RateLimited {
        integration: String,
        retry_after: Option<std::time::Duration>,
        message: String,
    },
    Etherscan(String),
    Parse(String),
    Deserialize {
        url: String,
        error: String,
    },
    CoverageInvalidation {
        error: Box<UserTransactionMonitorError>,
        targets: Box<CoverageInvalidationTargets>,
    },
}

impl std::fmt::Display for UserTransactionMonitorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UserTransactionMonitorError::Db(err) => write!(f, "{err}"),
            UserTransactionMonitorError::InvalidConfiguredBaseUrl(err) => {
                write!(f, "Configured sync provider base URL is invalid: {err}")
            }
            UserTransactionMonitorError::InvalidDefaultBaseUrl(err) => {
                write!(f, "Default sync provider base URL is invalid: {err}")
            }
            UserTransactionMonitorError::MissingEtherscanApiKey => {
                write!(
                    f,
                    "{}",
                    crate::transactions::MISSING_ETHERSCAN_API_KEY_ERROR
                )
            }
            UserTransactionMonitorError::UnsupportedEthereumNetwork(network) => {
                write!(
                    f,
                    "Ethereum sync does not support network: {}",
                    network.as_str()
                )
            }
            UserTransactionMonitorError::Http(err) => {
                write!(f, "Sync HTTP request failed: {err}")
            }
            UserTransactionMonitorError::UpstreamStatus {
                url,
                status,
                body_snippet,
            } => {
                write!(
                    f,
                    "Sync provider returned status {} for {} (body snippet: {})",
                    status, url, body_snippet
                )
            }
            UserTransactionMonitorError::RateLimited {
                integration,
                retry_after,
                message,
            } => {
                let _ = retry_after;
                write!(
                    f,
                    "Rate limit reached for integration {}: {}",
                    integration, message
                )
            }
            UserTransactionMonitorError::Etherscan(err) => {
                write!(f, "{err}")
            }
            UserTransactionMonitorError::Parse(err) => {
                write!(f, "Failed to parse sync payload: {err}")
            }
            UserTransactionMonitorError::Deserialize { url, error } => {
                write!(
                    f,
                    "Failed to deserialize sync provider response from {url}: {error}"
                )
            }
            UserTransactionMonitorError::CoverageInvalidation { error, .. } => {
                write!(f, "{error}")
            }
        }
    }
}

impl std::error::Error for UserTransactionMonitorError {}

pub(crate) fn preserve_iteration_error(
    error: impl Into<UserTransactionMonitorError>,
    iteration: &SyncIterationResult,
) -> UserTransactionMonitorError {
    error
        .into()
        .with_coverage_invalidation(iteration.coverage_invalidation.clone())
}

impl UserTransactionMonitorError {
    pub(crate) fn counts_as_address_failure(&self) -> bool {
        match self {
            Self::Db(_)
            | Self::InvalidConfiguredBaseUrl(_)
            | Self::InvalidDefaultBaseUrl(_)
            | Self::MissingEtherscanApiKey
            | Self::UnsupportedEthereumNetwork(_)
            | Self::RateLimited { .. } => false,
            Self::Http(_)
            | Self::UpstreamStatus { .. }
            | Self::Etherscan(_)
            | Self::Parse(_)
            | Self::Deserialize { .. } => true,
            Self::CoverageInvalidation { error, .. } => error.counts_as_address_failure(),
        }
    }

    pub(crate) fn with_coverage_invalidation(self, targets: CoverageInvalidationTargets) -> Self {
        if targets.address_ids.is_empty() && targets.account_ids.is_empty() {
            return self;
        }
        match self {
            Self::CoverageInvalidation {
                error,
                targets: mut existing,
            } => {
                existing.union_with(targets);
                Self::CoverageInvalidation {
                    error,
                    targets: existing,
                }
            }
            error => Self::CoverageInvalidation {
                error: Box::new(error),
                targets: Box::new(targets),
            },
        }
    }

    pub(crate) fn coverage_invalidation(&self) -> Option<&CoverageInvalidationTargets> {
        match self {
            Self::CoverageInvalidation { targets, .. } => Some(targets),
            _ => None,
        }
    }
}

impl From<DbError> for UserTransactionMonitorError {
    fn from(value: DbError) -> Self {
        UserTransactionMonitorError::Db(value)
    }
}

impl From<EtherscanError> for UserTransactionMonitorError {
    fn from(value: EtherscanError) -> Self {
        if value.is_rate_limited() {
            return UserTransactionMonitorError::RateLimited {
                integration: "etherscan".to_string(),
                message: value.to_string(),
                retry_after: None,
            };
        }

        UserTransactionMonitorError::Etherscan(value.to_string())
    }
}

impl From<MempoolError> for UserTransactionMonitorError {
    fn from(value: MempoolError) -> Self {
        match value {
            MempoolError::RateLimited {
                url,
                retry_after,
                response_headers_json,
                response_body,
            } => UserTransactionMonitorError::RateLimited {
                integration: "mempool".to_string(),
                message: MempoolError::RateLimited {
                    url,
                    retry_after,
                    response_headers_json,
                    response_body,
                }
                .to_string(),
                retry_after,
            },
            MempoolError::UpstreamStatus {
                url,
                status,
                response_body,
                ..
            } => UserTransactionMonitorError::UpstreamStatus {
                url,
                status,
                body_snippet: mempool_response_body_snippet(&response_body),
            },
            MempoolError::Deserialize { url, error, .. } => {
                UserTransactionMonitorError::Deserialize { url, error }
            }
            other => UserTransactionMonitorError::Http(other.to_string()),
        }
    }
}

fn mempool_response_body_snippet(response_body: &[u8]) -> String {
    const MEMPOOL_RESPONSE_SNIPPET_MAX_BYTES: usize = 8 * 1024;

    let body = String::from_utf8_lossy(response_body);
    if body.len() <= MEMPOOL_RESPONSE_SNIPPET_MAX_BYTES {
        body.to_string()
    } else {
        let mut snippet = body[..MEMPOOL_RESPONSE_SNIPPET_MAX_BYTES].to_string();
        snippet.push_str("...");
        snippet
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn user_transaction_monitor_http_error_display_is_provider_neutral() {
        let error =
            UserTransactionMonitorError::Http("Etherscan HTTP error for https://api.etherscan.io/v2/api?apikey=***REDACTED***: timeout".to_string());
        assert_eq!(
            error.to_string(),
            "Sync HTTP request failed: Etherscan HTTP error for https://api.etherscan.io/v2/api?apikey=***REDACTED***: timeout"
        );
    }

    #[test]
    fn missing_etherscan_api_key_display_matches_shared_constant() {
        assert_eq!(
            UserTransactionMonitorError::MissingEtherscanApiKey.to_string(),
            crate::transactions::MISSING_ETHERSCAN_API_KEY_ERROR
        );
    }
}
