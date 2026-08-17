//! Manual asset discovery helpers backed by public CoinGecko metadata.
//! Server-only.

use crate::integrations::coingecko::client::CoingeckoClient;
use crate::integrations::coingecko::{
    CoinGeckoCredentialMode, CoingeckoCoinDetail, CoingeckoListCoin,
};
use crate::models::{ApiKeyProvider, SimpleApiKey, UserId};
use crate::traces::client::{IntegrationLabel, TracedBlockingClient};
use crate::wallets::{
    ManualAssetDiscoveryDetailRequest, ManualAssetDiscoveryDetailResponse,
    ManualAssetDiscoveryPlatformRow,
};
use chrono::{DateTime, Duration, Utc};
use dioxus::logger::tracing;
use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};
use std::time::Duration as StdDuration;

pub(crate) const COINGECKO_CATALOG_REFRESH_TTL: Duration = Duration::days(7);

#[cfg(feature = "server")]
const _: () = {
    // Task 4 consumes these; keep strict server builds warning-clean between commits.
    let _ = catalog_total;
    let _ = match_total;
    let _ = open_prices_conn_or_warn;
    let _ = COINGECKO_DETAIL_CACHE_TTL;
    let _ = COINGECKO_DETAIL_RATE_LIMIT_FALLBACK;
    let _ = &KEYLESS_DETAIL_LOOKUP_CACHE;
    let _ = detail_lookup_uses_keyless_cache;
    let _ = keyless_detail_cached_response;
    let _ = record_keyless_detail_success;
    let _ = record_keyless_detail_rate_limit;
    let _ = retry_after_duration;
};

const COINGECKO_CATALOG_REQUEST_TIMEOUT: StdDuration = StdDuration::from_secs(10);
const COINGECKO_DETAIL_REQUEST_TIMEOUT: StdDuration = StdDuration::from_secs(5);
const COINGECKO_DETAIL_CACHE_TTL: Duration = Duration::minutes(30);
const COINGECKO_DETAIL_RATE_LIMIT_FALLBACK: Duration = Duration::seconds(30);
const COINGECKO_DETAIL_RATE_LIMIT_MAX_SECONDS: i64 = 5 * 60;
const COINGECKO_DETAIL_CACHE_MAX_ENTRIES: usize = 128;
const COINGECKO_DETAIL_RATE_LIMIT_MAX_ENTRIES: usize = 128;
const MANUAL_ASSET_SEARCH_LIMIT: usize = 25;

#[derive(Debug)]
pub(crate) enum ManualAssetDiscoveryDetailError {
    InvalidCoingeckoId(String),
    RemoteLookupNotAllowed,
    Database(crate::db::DbError),
    /// CoinGecko rate-limited the lookup (HTTP 429). Transient and retryable —
    /// distinct from `Provider` so the handler can surface a clear, non-500
    /// message instead of a generic internal error.
    RateLimited {
        retry_after: Option<String>,
    },
    Provider(String),
}

impl std::fmt::Display for ManualAssetDiscoveryDetailError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidCoingeckoId(message) => write!(f, "invalid CoinGecko id: {message}"),
            Self::RemoteLookupNotAllowed => write!(f, "remote CoinGecko lookup is not allowed"),
            Self::Database(err) => write!(f, "{err}"),
            Self::RateLimited { .. } => write!(f, "CoinGecko rate-limited the lookup"),
            Self::Provider(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ManualAssetDiscoveryDetailError {}

pub(crate) async fn load_manual_asset_discovery_detail(
    user_id: UserId,
    request: ManualAssetDiscoveryDetailRequest,
) -> Result<ManualAssetDiscoveryDetailResponse, ManualAssetDiscoveryDetailError> {
    let coingecko_id =
        crate::asset_capabilities::unsynced::CoingeckoAssetId::parse(&request.coingecko_id)
            .map_err(|err| ManualAssetDiscoveryDetailError::InvalidCoingeckoId(err.to_string()))?;

    let price_fetching_enabled = crate::db::get_price_fetching_enabled(user_id)
        .map_err(ManualAssetDiscoveryDetailError::Database)?;
    if !price_fetching_enabled && !request.allow_remote_lookup {
        return Err(ManualAssetDiscoveryDetailError::RemoteLookupNotAllowed);
    }

    let credential_mode = credential_mode_from_api_key_load(crate::db::load_api_key(
        user_id,
        ApiKeyProvider::CoinGecko,
    ))
    .map_err(ManualAssetDiscoveryDetailError::Database)?;
    let coingecko_id_string = coingecko_id.as_str().to_string();
    let use_keyless_cache = detail_lookup_uses_keyless_cache(&credential_mode);

    if use_keyless_cache {
        match KEYLESS_DETAIL_LOOKUP_CACHE.lock() {
            Ok(mut cache) => {
                if let Some(response) =
                    keyless_detail_cached_response(&mut cache, &coingecko_id_string, Utc::now())?
                {
                    return Ok(response);
                }
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "manual asset discovery: keyless CoinGecko detail cache unavailable"
                );
            }
        }
    }

    let fetch_id = coingecko_id_string.clone();
    let detail_result = tokio::task::spawn_blocking(move || {
        fetch_coingecko_detail_blocking(user_id, credential_mode, &fetch_id)
    })
    .await
    .map_err(|err| ManualAssetDiscoveryDetailError::Provider(err.to_string()))
    .and_then(|result| result);

    let response_result = detail_result.map(coingecko_detail_to_response);

    if use_keyless_cache {
        match KEYLESS_DETAIL_LOOKUP_CACHE.lock() {
            Ok(mut cache) => match &response_result {
                Ok(response) => record_keyless_detail_success(
                    &mut cache,
                    &coingecko_id_string,
                    Utc::now(),
                    response.clone(),
                ),
                Err(ManualAssetDiscoveryDetailError::RateLimited { retry_after }) => {
                    record_keyless_detail_rate_limit(
                        &mut cache,
                        &coingecko_id_string,
                        Utc::now(),
                        retry_after.as_deref(),
                    );
                }
                Err(_) => {}
            },
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "manual asset discovery: keyless CoinGecko detail cache unavailable"
                );
            }
        }
    }

    response_result
}

pub(crate) fn search_manual_asset_candidates(
    query: &str,
) -> Result<Vec<crate::asset_capabilities::ManualAssetSearchResult>, crate::db::DbError> {
    let mut results = crate::asset_capabilities::search_manual_asset_instances(query)
        .map_err(|err| crate::db::DbError::new(format!("manual asset catalog search: {err}")))?;
    let mut returned_coingecko_ids = results
        .iter()
        .filter_map(|row| match row {
            crate::asset_capabilities::ManualAssetSearchResult::BitGarthCatalog {
                coingecko_id,
                ..
            } => Some(coingecko_id.clone()),
            crate::asset_capabilities::ManualAssetSearchResult::CoinGeckoCatalog { .. } => None,
        })
        .collect::<HashSet<_>>();

    if results.len() >= MANUAL_ASSET_SEARCH_LIMIT {
        results.truncate(MANUAL_ASSET_SEARCH_LIMIT);
        return Ok(results);
    }

    let conn = match crate::db::initialize_prices_db() {
        Ok(conn) => conn,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "manual asset discovery: failed to open prices db for catalog search"
            );
            return Ok(results);
        }
    };
    let remaining = MANUAL_ASSET_SEARCH_LIMIT - results.len();
    let coingecko_rows = match crate::db::search_coingecko_asset_catalog(&conn, query, remaining) {
        Ok(rows) => rows,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "manual asset discovery: failed to search CoinGecko catalog"
            );
            return Ok(results);
        }
    };

    for row in coingecko_rows {
        if crate::asset_capabilities::coingecko_id_is_manual_discovery_excluded(
            &row.provider_asset_id,
        )
        .map_err(|err| crate::db::DbError::new(format!("manual asset catalog exclusion: {err}")))?
        {
            continue;
        }
        if !returned_coingecko_ids.insert(row.provider_asset_id.clone()) {
            continue;
        }
        results.push(
            crate::asset_capabilities::ManualAssetSearchResult::CoinGeckoCatalog {
                coingecko_id: row.provider_asset_id,
                symbol: row.symbol,
                name: row.name,
                platforms_json: row.platforms_json,
            },
        );
        if results.len() >= MANUAL_ASSET_SEARCH_LIMIT {
            break;
        }
    }

    Ok(results)
}

/// Deduped, local-only count of searchable manual assets.
///
/// `query = None` returns the grand total (BitGarth catalog candidates + active
/// CoinGecko rows not already represented). `query = Some(q)` returns the true
/// number of matches across both pools, ignoring the 25-row display cap.
///
/// `prices_conn = None` reflects a prices-db open failure: the CoinGecko pool
/// contributes 0 and the BitGarth catalog count is returned alone.
fn manual_asset_count(
    query: Option<&str>,
    prices_conn: Option<&rusqlite::Connection>,
) -> Result<usize, crate::db::DbError> {
    let bitgarth =
        match query {
            None => crate::asset_capabilities::manual_catalog_candidates()
                .map_err(|err| {
                    crate::db::DbError::new(format!("manual asset BitGarth catalog: {err}"))
                })?
                .len(),
            Some(q) => crate::asset_capabilities::count_manual_asset_instance_matches(q).map_err(
                |err| crate::db::DbError::new(format!("manual asset BitGarth search: {err}")),
            )?,
        };

    let coingecko = match prices_conn {
        Some(conn) => {
            let excluded = crate::asset_capabilities::manual_discovery_excluded_coingecko_ids()
                .map_err(|err| {
                    crate::db::DbError::new(format!("manual asset exclusion ids: {err}"))
                })?;
            let total_active = crate::db::count_active_coingecko_catalog(conn, query)?;
            let overlap = crate::db::count_active_coingecko_in_set(conn, &excluded, query)?;
            total_active.saturating_sub(overlap)
        }
        None => 0,
    };

    Ok(bitgarth + coingecko)
}

/// Placeholder grand total plus whether the local CoinGecko catalog is empty.
/// Local-only; never triggers a remote catalog refresh. `coingecko_catalog_empty`
/// is false when the prices db could not be opened (emptiness is unknown).
pub(crate) fn catalog_total() -> Result<(usize, bool), crate::db::DbError> {
    let conn = open_prices_conn_or_warn();
    let total = manual_asset_count(None, conn.as_ref())?;
    let coingecko_catalog_empty = match conn.as_ref() {
        Some(conn) => crate::db::count_active_coingecko_catalog(conn, None)? == 0,
        None => false,
    };
    Ok((total, coingecko_catalog_empty))
}

/// True total of matches for `query` across both pools (beyond the 25-row cap).
pub(crate) fn match_total(query: &str) -> Result<usize, crate::db::DbError> {
    let conn = open_prices_conn_or_warn();
    manual_asset_count(Some(query), conn.as_ref())
}

fn open_prices_conn_or_warn() -> Option<rusqlite::Connection> {
    match crate::db::initialize_prices_db() {
        Ok(conn) => Some(conn),
        Err(err) => {
            tracing::warn!(
                error = %err,
                "manual asset count: failed to open prices db; using BitGarth catalog count only"
            );
            None
        }
    }
}

pub(crate) async fn refresh_coingecko_catalog_for_manual_asset_search(
    user_id: UserId,
    allow_remote_refresh: bool,
) {
    let conn = match crate::db::initialize_prices_db() {
        Ok(conn) => conn,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "manual asset discovery: failed to open prices db; skipping CoinGecko catalog refresh"
            );
            return;
        }
    };
    let now = Utc::now();
    let latest_retrieved_at = match crate::db::latest_coingecko_catalog_retrieved_at(&conn) {
        Ok(value) => value,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "manual asset discovery: failed to inspect CoinGecko catalog freshness"
            );
            None
        }
    };
    if coingecko_catalog_is_fresh(latest_retrieved_at, now) {
        return;
    }

    let price_fetching_enabled = match crate::db::get_price_fetching_enabled(user_id) {
        Ok(value) => value,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "manual asset discovery: failed to load price-fetching setting; skipping CoinGecko catalog refresh"
            );
            false
        }
    };
    if !should_attempt_remote_catalog_refresh(price_fetching_enabled, allow_remote_refresh) {
        return;
    }

    let credential_mode = match credential_mode_from_api_key_load(crate::db::load_api_key(
        user_id,
        ApiKeyProvider::CoinGecko,
    )) {
        Ok(mode) => mode,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "manual asset discovery: failed to load CoinGecko API key; skipping provider request"
            );
            return;
        }
    };

    let fetched = tokio::task::spawn_blocking(move || {
        fetch_coingecko_catalog_blocking(user_id, credential_mode)
    })
    .await
    .ok()
    .flatten();
    let Some(coins) = fetched else {
        tracing::warn!(
            "manual asset discovery: CoinGecko catalog refresh failed; using local catalog data"
        );
        return;
    };

    let retrieved_at = Utc::now();
    let rows = coins
        .into_iter()
        .map(|coin| coingecko_list_coin_to_catalog_upsert(coin, retrieved_at))
        .collect::<Vec<_>>();
    if let Err(err) =
        crate::db::replace_or_upsert_coingecko_catalog_rows(&conn, &rows, retrieved_at)
    {
        tracing::warn!(
            error = %err,
            "manual asset discovery: failed to persist CoinGecko catalog rows"
        );
    }
}

fn should_attempt_remote_catalog_refresh(
    price_fetching_enabled: bool,
    allow_remote_refresh: bool,
) -> bool {
    price_fetching_enabled || allow_remote_refresh
}

fn coingecko_catalog_is_fresh(
    latest_retrieved_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> bool {
    latest_retrieved_at
        .map(|retrieved_at| retrieved_at >= now - COINGECKO_CATALOG_REFRESH_TTL)
        .unwrap_or(false)
}

fn credential_mode_from_api_key(api_key: Option<SimpleApiKey>) -> CoinGeckoCredentialMode {
    match api_key {
        Some(api_key) => CoinGeckoCredentialMode::Pro { api_key },
        None => CoinGeckoCredentialMode::PublicKeyless,
    }
}

fn credential_mode_from_api_key_load(
    api_key: Result<Option<SimpleApiKey>, crate::db::DbError>,
) -> Result<CoinGeckoCredentialMode, crate::db::DbError> {
    api_key.map(credential_mode_from_api_key)
}

static KEYLESS_DETAIL_LOOKUP_CACHE: LazyLock<Mutex<KeylessDetailLookupCache>> =
    LazyLock::new(|| Mutex::new(KeylessDetailLookupCache::default()));

#[derive(Default)]
struct KeylessDetailLookupCache {
    details: HashMap<String, KeylessDetailCacheEntry>,
    rate_limits: HashMap<String, DateTime<Utc>>,
}

#[derive(Clone)]
struct KeylessDetailCacheEntry {
    response: ManualAssetDiscoveryDetailResponse,
    cached_at: DateTime<Utc>,
}

fn detail_lookup_uses_keyless_cache(credential_mode: &CoinGeckoCredentialMode) -> bool {
    matches!(credential_mode, CoinGeckoCredentialMode::PublicKeyless)
}

fn keyless_detail_cached_response(
    cache: &mut KeylessDetailLookupCache,
    coingecko_id: &str,
    now: DateTime<Utc>,
) -> Result<Option<ManualAssetDiscoveryDetailResponse>, ManualAssetDiscoveryDetailError> {
    if let Some(retry_until) = cache.rate_limits.get(coingecko_id).copied() {
        if retry_until > now {
            return Err(ManualAssetDiscoveryDetailError::RateLimited { retry_after: None });
        }
        cache.rate_limits.remove(coingecko_id);
    }

    if let Some(entry) = cache.details.get(coingecko_id) {
        if entry.cached_at + COINGECKO_DETAIL_CACHE_TTL > now {
            return Ok(Some(entry.response.clone()));
        }
        cache.details.remove(coingecko_id);
    }

    Ok(None)
}

fn record_keyless_detail_success(
    cache: &mut KeylessDetailLookupCache,
    coingecko_id: &str,
    now: DateTime<Utc>,
    response: ManualAssetDiscoveryDetailResponse,
) {
    cache.rate_limits.remove(coingecko_id);
    cache.details.insert(
        coingecko_id.to_string(),
        KeylessDetailCacheEntry {
            response,
            cached_at: now,
        },
    );
    prune_keyless_detail_cache(cache, now);
}

fn record_keyless_detail_rate_limit(
    cache: &mut KeylessDetailLookupCache,
    coingecko_id: &str,
    now: DateTime<Utc>,
    retry_after: Option<&str>,
) {
    cache.details.remove(coingecko_id);
    cache.rate_limits.insert(
        coingecko_id.to_string(),
        now + retry_after_duration(retry_after),
    );
    prune_keyless_detail_cache(cache, now);
}

fn retry_after_duration(retry_after: Option<&str>) -> Duration {
    retry_after
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .filter(|seconds| *seconds > 0)
        .map(|seconds| seconds.min(COINGECKO_DETAIL_RATE_LIMIT_MAX_SECONDS))
        .map(Duration::seconds)
        .unwrap_or(COINGECKO_DETAIL_RATE_LIMIT_FALLBACK)
}

fn prune_keyless_detail_cache(cache: &mut KeylessDetailLookupCache, now: DateTime<Utc>) {
    cache
        .details
        .retain(|_, entry| entry.cached_at + COINGECKO_DETAIL_CACHE_TTL > now);
    cache
        .rate_limits
        .retain(|_, retry_until| *retry_until > now);

    while cache.details.len() > COINGECKO_DETAIL_CACHE_MAX_ENTRIES {
        let Some(oldest_key) = cache
            .details
            .iter()
            .min_by_key(|(_, entry)| entry.cached_at)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        cache.details.remove(&oldest_key);
    }

    while cache.rate_limits.len() > COINGECKO_DETAIL_RATE_LIMIT_MAX_ENTRIES {
        let Some(oldest_key) = cache
            .rate_limits
            .iter()
            .min_by_key(|(_, retry_until)| *retry_until)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        cache.rate_limits.remove(&oldest_key);
    }
}

fn fetch_coingecko_catalog_blocking(
    user_id: UserId,
    credential_mode: CoinGeckoCredentialMode,
) -> Option<Vec<CoingeckoListCoin>> {
    let traced = TracedBlockingClient::builder(
        IntegrationLabel::new("coingecko-manual-asset-catalog"),
        user_id,
    )
    .configure(|builder| builder.timeout(COINGECKO_CATALOG_REQUEST_TIMEOUT))
    .redact_headers(&["x-cg-pro-api-key"])
    .build()
    .ok()?;
    let client = CoingeckoClient::from_credential_mode(traced, credential_mode).ok()?;

    client.coins_list(true).ok()
}

fn fetch_coingecko_detail_blocking(
    user_id: UserId,
    credential_mode: CoinGeckoCredentialMode,
    coingecko_id: &str,
) -> Result<CoingeckoCoinDetail, ManualAssetDiscoveryDetailError> {
    let traced =
        TracedBlockingClient::builder(IntegrationLabel::new("coingecko-coin-detail"), user_id)
            .configure(|builder| builder.timeout(COINGECKO_DETAIL_REQUEST_TIMEOUT))
            .redact_headers(&["x-cg-pro-api-key"])
            .build()
            .map_err(|err| ManualAssetDiscoveryDetailError::Provider(err.to_string()))?;
    let client = CoingeckoClient::from_credential_mode(traced, credential_mode)
        .map_err(|err| ManualAssetDiscoveryDetailError::Provider(err.to_string()))?;

    client
        .coin_detail(coingecko_id)
        .map_err(classify_detail_error)
}

/// Map a CoinGecko client error to the discovery error. A 429 becomes the
/// dedicated `RateLimited` variant (transient, retryable); everything else is a
/// generic `Provider` failure (logged server-side, surfaced as internal).
fn classify_detail_error(
    err: crate::integrations::coingecko::client::CoingeckoError,
) -> ManualAssetDiscoveryDetailError {
    match err {
        crate::integrations::coingecko::client::CoingeckoError::Api {
            status_code: 429,
            retry_after,
            ..
        } => ManualAssetDiscoveryDetailError::RateLimited { retry_after },
        crate::integrations::coingecko::client::CoingeckoError::UnexpectedResponse {
            status_code: 429,
            headers,
            ..
        } => ManualAssetDiscoveryDetailError::RateLimited {
            retry_after: retry_after_header(&headers),
        },
        other if other.is_rate_limited() => {
            ManualAssetDiscoveryDetailError::RateLimited { retry_after: None }
        }
        other => ManualAssetDiscoveryDetailError::Provider(format!(
            "CoinGecko detail lookup failed: {other}"
        )),
    }
}

fn retry_after_header(headers: &[(String, String)]) -> Option<String> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
        .map(|(_, value)| value.clone())
}

fn coingecko_list_coin_to_catalog_upsert(
    coin: CoingeckoListCoin,
    retrieved_at: DateTime<Utc>,
) -> crate::db::CoinGeckoCatalogUpsert {
    let platforms_json = if coin.platforms.is_empty() {
        None
    } else {
        serde_json::to_string(&coin.platforms).ok()
    };

    crate::db::CoinGeckoCatalogUpsert {
        provider_asset_id: coin.id,
        normalized_symbol: coin.symbol.to_ascii_lowercase(),
        symbol: coin.symbol,
        name: coin.name,
        platforms_json,
        status: "active".to_string(),
        retrieved_at,
    }
}

fn coingecko_detail_to_response(detail: CoingeckoCoinDetail) -> ManualAssetDiscoveryDetailResponse {
    let suggested_unit_code = crate::wallets::ValidatedManualAssetUnitCode::parse(&detail.symbol)
        .ok()
        .map(|unit_code| unit_code.as_str().to_string());
    let mut platforms = detail
        .detail_platforms
        .into_iter()
        .filter(|(provider_platform_id, _)| !provider_platform_id.trim().is_empty())
        .map(|(provider_platform_id, platform)| {
            let contract_address = non_empty_trimmed(platform.contract_address);
            let suggested_decimal_precision = platform
                .decimal_place
                .filter(|value| *value <= crate::wallets::ManualAssetDisplayScale::MAX);
            ManualAssetDiscoveryPlatformRow {
                network_id: network_id_from_coingecko_platform(&provider_platform_id),
                network_name: network_name_from_coingecko_platform(&provider_platform_id),
                provider_platform_id,
                contract_address,
                suggested_decimal_precision,
            }
        })
        .collect::<Vec<_>>();
    platforms.sort_by(|left, right| {
        left.provider_platform_id
            .cmp(&right.provider_platform_id)
            .then_with(|| left.contract_address.cmp(&right.contract_address))
    });

    ManualAssetDiscoveryDetailResponse {
        coingecko_id: detail.id,
        name: detail.name,
        symbol: detail.symbol,
        suggested_unit_code,
        default_decimal_precision: 6,
        platforms,
    }
}

fn non_empty_trimmed(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn network_id_from_coingecko_platform(provider_platform_id: &str) -> String {
    let mut network_id = String::new();
    let mut previous_was_separator = false;
    for character in provider_platform_id.trim().chars() {
        if character.is_ascii_alphanumeric() {
            network_id.push(character.to_ascii_lowercase());
            previous_was_separator = false;
        } else if !previous_was_separator && !network_id.is_empty() {
            network_id.push('-');
            previous_was_separator = true;
        }
    }
    while network_id.ends_with('-') {
        network_id.pop();
    }
    if network_id.is_empty() {
        "coingecko-platform".to_string()
    } else {
        network_id
    }
}

fn network_name_from_coingecko_platform(provider_platform_id: &str) -> String {
    provider_platform_id
        .split(['-', '_', ' '])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => {
                    let mut word = first.to_uppercase().collect::<String>();
                    word.push_str(chars.as_str());
                    word
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn utc(raw: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(raw)
            .expect("test timestamp should parse")
            .with_timezone(&Utc)
    }

    #[test]
    fn coingecko_rate_limit_classifies_as_rate_limited() {
        let err = crate::integrations::coingecko::client::CoingeckoError::Api {
            status_code: 429,
            error_code: 429,
            error_message: "rate limit".to_string(),
            retry_after: Some("19".to_string()),
        };
        assert!(matches!(
            classify_detail_error(err),
            ManualAssetDiscoveryDetailError::RateLimited {
                retry_after: Some(value)
            } if value == "19"
        ));
    }

    #[test]
    fn coingecko_non_rate_limit_classifies_as_provider() {
        let err =
            crate::integrations::coingecko::client::CoingeckoError::Decode("boom".to_string());
        assert!(matches!(
            classify_detail_error(err),
            ManualAssetDiscoveryDetailError::Provider(_)
        ));
    }

    #[test]
    fn coingecko_unexpected_rate_limit_preserves_retry_after_header() {
        let err = crate::integrations::coingecko::client::CoingeckoError::UnexpectedResponse {
            status_code: 429,
            headers: vec![("Retry-After".to_string(), "42".to_string())],
            body: "rate limited".to_string(),
        };
        assert!(matches!(
            classify_detail_error(err),
            ManualAssetDiscoveryDetailError::RateLimited {
                retry_after: Some(value)
            } if value == "42"
        ));
    }

    fn sample_detail_response(coingecko_id: &str) -> ManualAssetDiscoveryDetailResponse {
        ManualAssetDiscoveryDetailResponse {
            coingecko_id: coingecko_id.to_string(),
            name: "Good Games Guild".to_string(),
            symbol: "ggg".to_string(),
            suggested_unit_code: Some("GGG".to_string()),
            default_decimal_precision: 6,
            platforms: vec![ManualAssetDiscoveryPlatformRow {
                provider_platform_id: "ethereum".to_string(),
                contract_address: Some("0xabc".to_string()),
                suggested_decimal_precision: Some(18),
                network_id: "ethereum".to_string(),
                network_name: "Ethereum".to_string(),
            }],
        }
    }

    #[test]
    fn keyless_detail_cache_returns_fresh_success() {
        let mut cache = KeylessDetailLookupCache::default();
        let now = utc("2026-06-19T16:05:00Z");
        let response = sample_detail_response("good-games-guild");

        record_keyless_detail_success(&mut cache, "good-games-guild", now, response.clone());

        assert_eq!(
            keyless_detail_cached_response(
                &mut cache,
                "good-games-guild",
                now + Duration::minutes(1)
            )
            .expect("cache check should succeed"),
            Some(response)
        );
    }

    #[test]
    fn keyless_detail_cache_expires_success() {
        let mut cache = KeylessDetailLookupCache::default();
        let now = utc("2026-06-19T16:05:00Z");

        record_keyless_detail_success(
            &mut cache,
            "good-games-guild",
            now,
            sample_detail_response("good-games-guild"),
        );

        assert_eq!(
            keyless_detail_cached_response(
                &mut cache,
                "good-games-guild",
                now + COINGECKO_DETAIL_CACHE_TTL + Duration::seconds(1),
            )
            .expect("expired cache check should succeed"),
            None
        );
    }

    #[test]
    fn keyless_detail_rate_limit_suppresses_until_retry_after() {
        let mut cache = KeylessDetailLookupCache::default();
        let now = utc("2026-06-19T16:05:00Z");

        record_keyless_detail_rate_limit(&mut cache, "good-games-guild", now, Some("19"));

        assert!(matches!(
            keyless_detail_cached_response(
                &mut cache,
                "good-games-guild",
                now + Duration::seconds(18)
            ),
            Err(ManualAssetDiscoveryDetailError::RateLimited { .. })
        ));
        assert_eq!(
            keyless_detail_cached_response(
                &mut cache,
                "good-games-guild",
                now + Duration::seconds(20)
            )
            .expect("expired backoff should be removed"),
            None
        );
    }

    #[test]
    fn keyless_detail_rate_limit_uses_fallback_for_invalid_retry_after() {
        let mut cache = KeylessDetailLookupCache::default();
        let now = utc("2026-06-19T16:05:00Z");

        record_keyless_detail_rate_limit(&mut cache, "good-games-guild", now, Some("not-seconds"));

        assert!(matches!(
            keyless_detail_cached_response(
                &mut cache,
                "good-games-guild",
                now + COINGECKO_DETAIL_RATE_LIMIT_FALLBACK - Duration::seconds(1),
            ),
            Err(ManualAssetDiscoveryDetailError::RateLimited { .. })
        ));
        assert_eq!(
            keyless_detail_cached_response(
                &mut cache,
                "good-games-guild",
                now + COINGECKO_DETAIL_RATE_LIMIT_FALLBACK + Duration::seconds(1),
            )
            .expect("fallback backoff should expire"),
            None
        );
    }

    #[test]
    fn keyless_detail_rate_limit_clamps_oversized_retry_after() {
        let mut cache = KeylessDetailLookupCache::default();
        let now = utc("2026-06-19T16:05:00Z");

        record_keyless_detail_rate_limit(&mut cache, "good-games-guild", now, Some("999999999999"));

        assert!(matches!(
            keyless_detail_cached_response(
                &mut cache,
                "good-games-guild",
                now + Duration::seconds(COINGECKO_DETAIL_RATE_LIMIT_MAX_SECONDS - 1),
            ),
            Err(ManualAssetDiscoveryDetailError::RateLimited { .. })
        ));
        assert_eq!(
            keyless_detail_cached_response(
                &mut cache,
                "good-games-guild",
                now + Duration::seconds(COINGECKO_DETAIL_RATE_LIMIT_MAX_SECONDS + 1),
            )
            .expect("oversized retry-after should clamp and expire"),
            None
        );
    }

    #[test]
    fn pro_detail_lookup_is_not_server_cached() {
        let pro_key = SimpleApiKey::new("coingecko-pro-key".to_string()).expect("key");
        assert!(detail_lookup_uses_keyless_cache(
            &CoinGeckoCredentialMode::PublicKeyless
        ));
        assert!(!detail_lookup_uses_keyless_cache(
            &CoinGeckoCredentialMode::Pro { api_key: pro_key }
        ));
    }

    #[test]
    fn poisoned_or_unavailable_process_cache_can_be_bypassed_by_fetch_result_helpers() {
        let mut cache = KeylessDetailLookupCache::default();
        let now = utc("2026-06-19T16:05:00Z");

        assert_eq!(
            keyless_detail_cached_response(&mut cache, "good-games-guild", now)
                .expect("empty cache should not block lookup"),
            None
        );

        record_keyless_detail_success(
            &mut cache,
            "good-games-guild",
            now,
            sample_detail_response("good-games-guild"),
        );
        assert!(
            keyless_detail_cached_response(&mut cache, "other-asset", now)
                .expect("different asset should miss")
                .is_none()
        );
    }

    #[test]
    fn keyless_detail_cache_prunes_expired_and_caps_success_entries() {
        let mut cache = KeylessDetailLookupCache::default();
        let now = utc("2026-06-19T16:05:00Z");

        record_keyless_detail_success(
            &mut cache,
            "expired-asset",
            now - COINGECKO_DETAIL_CACHE_TTL - Duration::seconds(1),
            sample_detail_response("expired-asset"),
        );
        for index in 0..=COINGECKO_DETAIL_CACHE_MAX_ENTRIES {
            let coingecko_id = format!("asset-{index}");
            record_keyless_detail_success(
                &mut cache,
                &coingecko_id,
                now + Duration::seconds(index as i64),
                sample_detail_response(&coingecko_id),
            );
        }

        assert!(cache.details.len() <= COINGECKO_DETAIL_CACHE_MAX_ENTRIES);
        assert!(!cache.details.contains_key("expired-asset"));
        assert!(!cache.details.contains_key("asset-0"));
        assert!(cache.details.contains_key("asset-128"));
    }

    #[test]
    fn keyless_detail_cache_prunes_expired_and_caps_rate_limit_entries() {
        let mut cache = KeylessDetailLookupCache::default();
        let now = utc("2026-06-19T16:05:00Z");

        cache
            .rate_limits
            .insert("expired-asset".to_string(), now - Duration::seconds(1));
        for index in 0..=COINGECKO_DETAIL_RATE_LIMIT_MAX_ENTRIES {
            cache.rate_limits.insert(
                format!("asset-{index}"),
                now + Duration::seconds(index as i64 + 1),
            );
        }

        prune_keyless_detail_cache(&mut cache, now);

        assert!(cache.rate_limits.len() <= COINGECKO_DETAIL_RATE_LIMIT_MAX_ENTRIES);
        assert!(!cache.rate_limits.contains_key("expired-asset"));
        assert!(!cache.rate_limits.contains_key("asset-0"));
        assert!(cache.rate_limits.contains_key("asset-128"));
    }

    #[test]
    fn refresh_gate_requires_setting_or_one_time_consent() {
        assert!(!should_attempt_remote_catalog_refresh(false, false));
        assert!(should_attempt_remote_catalog_refresh(true, false));
        assert!(should_attempt_remote_catalog_refresh(false, true));
    }

    #[test]
    fn fresh_catalog_skips_refresh() {
        let now = utc("2026-06-08T12:00:00Z");

        assert!(coingecko_catalog_is_fresh(
            Some(now - Duration::days(7) + Duration::seconds(1)),
            now
        ));
        assert!(!coingecko_catalog_is_fresh(
            Some(now - Duration::days(7) - Duration::seconds(1)),
            now
        ));
        assert!(!coingecko_catalog_is_fresh(None, now));
    }

    #[test]
    fn pro_key_load_failure_skips_credential_mode() {
        let err = crate::db::DbError::new("boom");
        assert!(credential_mode_from_api_key_load(Err(err)).is_err());
    }

    #[test]
    fn list_coin_maps_to_public_catalog_row() {
        let mut platforms = HashMap::new();
        platforms.insert("ethereum".to_string(), "0xabc".to_string());
        let retrieved_at = utc("2026-06-08T12:00:00Z");
        let row = coingecko_list_coin_to_catalog_upsert(
            CoingeckoListCoin {
                id: "usd-coin".to_string(),
                symbol: "USDC".to_string(),
                name: "USD Coin".to_string(),
                platforms,
            },
            retrieved_at,
        );

        assert_eq!(row.provider_asset_id, "usd-coin");
        assert_eq!(row.symbol, "USDC");
        assert_eq!(row.normalized_symbol, "usdc");
        assert_eq!(row.name, "USD Coin");
        assert_eq!(
            row.platforms_json.as_deref(),
            Some(r#"{"ethereum":"0xabc"}"#)
        );
        assert_eq!(row.status, "active");
        assert_eq!(row.retrieved_at, retrieved_at);
    }

    #[test]
    fn manual_asset_count_degrades_to_bitgarth_catalog_when_prices_conn_missing() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        // No prices connection => CoinGecko contributes 0; total equals the combined
        // BitGarth manual catalog, including synced BTC/ETH candidates.
        let total = super::manual_asset_count(None, None).expect("count should compute");
        let bitgarth_catalog = crate::asset_capabilities::manual_catalog_candidates()
            .expect("manual catalog candidates")
            .len();
        assert_eq!(total, bitgarth_catalog);
    }

    #[test]
    fn manual_asset_count_query_uses_uncapped_bitgarth_matches() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let total = super::manual_asset_count(Some("mainnet"), None).expect("count should compute");
        let capped = crate::asset_capabilities::search_manual_asset_instances("mainnet")
            .expect("manual search")
            .len();
        let uncapped = crate::asset_capabilities::count_manual_asset_instance_matches("mainnet")
            .expect("manual match count");

        assert_eq!(total, uncapped);
        assert_eq!(capped, uncapped.min(25));
    }

    #[test]
    fn manual_asset_count_dedupes_coingecko_against_unsynced_catalog() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let conn = crate::db::initialize_prices_db().expect("prices db should initialize");
        let retrieved_at = chrono::DateTime::parse_from_rfc3339("2026-06-08T12:00:00Z")
            .expect("timestamp")
            .with_timezone(&chrono::Utc);
        // "cardano" duplicates an unsynced-catalog entry => must NOT increase the total.
        // "adappter-token" is a genuine CoinGecko-only row => +1.
        // Name uses "Discovery Token ADP" so it does NOT match the "ada" prefix query.
        crate::db::replace_or_upsert_coingecko_catalog_rows(
            &conn,
            &[
                crate::db::CoinGeckoCatalogUpsert {
                    provider_asset_id: "cardano".to_string(),
                    symbol: "ada".to_string(),
                    normalized_symbol: "ada".to_string(),
                    name: "Cardano duplicate".to_string(),
                    platforms_json: None,
                    status: "active".to_string(),
                    retrieved_at,
                },
                crate::db::CoinGeckoCatalogUpsert {
                    provider_asset_id: "adappter-token".to_string(),
                    symbol: "adp".to_string(),
                    normalized_symbol: "adp".to_string(),
                    name: "Discovery Token ADP".to_string(),
                    platforms_json: None,
                    status: "active".to_string(),
                    retrieved_at,
                },
            ],
            retrieved_at,
        )
        .expect("seed catalog rows");

        let bitgarth_catalog = crate::asset_capabilities::manual_catalog_candidates()
            .expect("manual catalog candidates")
            .len();
        let total = super::manual_asset_count(None, Some(&conn)).expect("count should compute");
        assert_eq!(
            total,
            bitgarth_catalog + 1,
            "only the genuine CoinGecko-only row adds to the total"
        );

        // Match total for "ada" includes unsynced ADA but excludes the duplicate "cardano" row.
        let ada_matches = super::manual_asset_count(Some("ada"), Some(&conn)).expect("match count");
        let bitgarth_ada = crate::asset_capabilities::count_manual_asset_instance_matches("ada")
            .expect("BitGarth match count");
        assert_eq!(ada_matches, bitgarth_ada);
    }

    #[test]
    fn detail_response_filters_unusable_precision_and_invalid_unit_code() {
        let mut detail_platforms = HashMap::new();
        detail_platforms.insert(
            "polygon-pos".to_string(),
            crate::integrations::coingecko::CoingeckoPlatformDetail {
                contract_address: " 0xabc ".to_string(),
                decimal_place: Some(6),
            },
        );
        detail_platforms.insert(
            "ethereum".to_string(),
            crate::integrations::coingecko::CoingeckoPlatformDetail {
                contract_address: "".to_string(),
                decimal_place: Some(19),
            },
        );
        detail_platforms.insert(
            "".to_string(),
            crate::integrations::coingecko::CoingeckoPlatformDetail {
                contract_address: "ignored".to_string(),
                decimal_place: Some(8),
            },
        );

        let response = coingecko_detail_to_response(CoingeckoCoinDetail {
            id: "bad-unit-fixture".to_string(),
            symbol: "bad-unit".to_string(),
            name: "Bad Unit Fixture".to_string(),
            web_slug: "bad-unit-fixture".to_string(),
            market_cap_rank: Some(1),
            detail_platforms,
        });

        assert_eq!(response.suggested_unit_code, None);
        assert_eq!(response.default_decimal_precision, 6);
        assert_eq!(response.platforms.len(), 2);
        let ethereum = response
            .platforms
            .iter()
            .find(|row| row.provider_platform_id == "ethereum")
            .expect("ethereum platform");
        assert_eq!(ethereum.suggested_decimal_precision, None);
        let polygon = response
            .platforms
            .iter()
            .find(|row| row.provider_platform_id == "polygon-pos")
            .expect("polygon platform");
        assert_eq!(polygon.contract_address.as_deref(), Some("0xabc"));
        assert_eq!(polygon.suggested_decimal_precision, Some(6));
        assert_eq!(polygon.network_id, "polygon-pos");
        assert_eq!(polygon.network_name, "Polygon Pos");
    }
}
