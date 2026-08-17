use crate::integrations::mempool::MempoolClient;
use crate::models::{
    EtherscanBaseUrl, MempoolBaseUrl, MempoolBaseUrlSource, RawEtherscanApiKey,
    resolve_effective_etherscan_base_url, resolve_effective_mempool_base_url,
};
use crate::traces::client::{IntegrationLabel, TracedBlockingClient};
use crate::transactions::ChainTipHeight;
use std::sync::Arc;
use std::time::Duration;
use url::Url;

use super::MEMPOOL_REQUEST_TIMEOUT_SECONDS;
use super::context::{LABEL_MEMPOOL, SyncHttpCounters};
use super::error::UserTransactionMonitorError;

pub(super) fn resolve_mempool_base_url_from_settings(
    settings: &crate::models::UserSettings,
) -> Result<(MempoolBaseUrl, MempoolBaseUrlSource), UserTransactionMonitorError> {
    resolve_effective_mempool_base_url(settings.mempool_base_url.as_ref()).map_err(|err| {
        if settings.mempool_base_url.is_some() {
            UserTransactionMonitorError::InvalidConfiguredBaseUrl(err.to_string())
        } else {
            UserTransactionMonitorError::InvalidDefaultBaseUrl(err.to_string())
        }
    })
}

/// Construct a [`MempoolClient`] from app-layer configuration.
pub(super) fn build_mempool_client(
    user_id: crate::models::UserId,
    base_url: &MempoolBaseUrl,
    base_url_source: MempoolBaseUrlSource,
    http_counters: &SyncHttpCounters,
) -> Result<MempoolClient, UserTransactionMonitorError> {
    let parsed_base_url = Url::parse(base_url.as_str()).map_err(|err| match base_url_source {
        MempoolBaseUrlSource::UserOverride => {
            UserTransactionMonitorError::InvalidConfiguredBaseUrl(err.to_string())
        }
        MempoolBaseUrlSource::DefaultPublic => {
            UserTransactionMonitorError::InvalidDefaultBaseUrl(err.to_string())
        }
    })?;
    let client = TracedBlockingClient::builder(IntegrationLabel::new(LABEL_MEMPOOL), user_id)
        .configure(|builder| builder.timeout(Duration::from_secs(MEMPOOL_REQUEST_TIMEOUT_SECONDS)))
        .redact_headers(&["authorization"])
        .build()
        .map_err(|err| {
            UserTransactionMonitorError::Http(format!("failed to build mempool HTTP client: {err}"))
        })?;
    Ok(MempoolClient::new(client, parsed_base_url)
        .with_total_api_call_counter(Arc::clone(&http_counters.total_api_calls))
        .with_pagination_cache_hit_counter(Arc::clone(&http_counters.pagination_cache_hits)))
}

/// Bridge: call the integration's `fetch_chain_tip_height` and parse into app type.
pub(super) fn fetch_mempool_chain_tip(
    client: &MempoolClient,
) -> Result<ChainTipHeight, UserTransactionMonitorError> {
    let body = client
        .fetch_chain_tip_height()
        .map_err(UserTransactionMonitorError::from)?;
    let parsed = body.trim().parse::<i64>().map_err(|err| {
        UserTransactionMonitorError::Parse(format!("tip height parse error: {err}"))
    })?;
    ChainTipHeight::try_new(parsed)
        .map_err(|err| UserTransactionMonitorError::Parse(format!("tip height invalid: {err}")))
}

pub(super) fn resolve_etherscan_api_key_from_settings(
    settings: &crate::models::UserSettings,
) -> Option<RawEtherscanApiKey> {
    settings
        .etherscan_api_key
        .as_ref()
        .filter(|value| !value.as_str().trim().is_empty())
        .cloned()
}

pub(super) fn resolve_etherscan_base_url_from_settings(
    settings: &crate::models::UserSettings,
) -> Result<Option<EtherscanBaseUrl>, UserTransactionMonitorError> {
    match settings.etherscan_base_url.as_ref() {
        None => Ok(None),
        Some(raw) => {
            let (url, _source) =
                resolve_effective_etherscan_base_url(Some(raw)).map_err(|err| {
                    UserTransactionMonitorError::InvalidConfiguredBaseUrl(err.to_string())
                })?;
            Ok(Some(url))
        }
    }
}
