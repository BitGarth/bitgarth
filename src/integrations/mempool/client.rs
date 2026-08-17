use super::error::MempoolError;
use super::types::MempoolAddressTransaction;
use crate::traces::client::{TracedBlockingClient, TransportFailure, TransportFailureStage};
use crate::transactions::{ApiConfirmedBalance, TransactionCount, TxHash};
use reqwest::header::{HeaderMap, RETRY_AFTER};
use serde::Deserialize;
use serde_json::value::RawValue;
use std::collections::HashMap;
#[cfg(all(test, not(bitgarth_db_unit_only)))]
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;
use url::Url;

#[cfg(all(test, not(bitgarth_db_unit_only)))]
const RESPONSE_SNIPPET_MAX_BYTES: usize = 8 * 1024;

/// Client for the Mempool public API.
///
/// Accepts a pre-configured `TracedBlockingClient` — the caller is
/// responsible for setting timeouts and integration labeling.
pub(crate) struct MempoolClient {
    base_url: Url,
    client: TracedBlockingClient,
    total_api_call_counter: Option<Arc<AtomicU64>>,
    pagination_cache_hit_counter: Option<Arc<AtomicU64>>,
}

/// Mempool server implementations expose two different pagination styles:
///
/// - **Path-based** (public mempool.space / Esplora):
///   `GET /api/address/{addr}/txs/chain/{last_seen_txid}`
/// - **Query-param** (some self-hosted instances):
///   `GET /api/address/{addr}/txs?after_txid={last_seen_txid}`
///
/// We auto-detect on the first pagination attempt and reuse the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaginationStyle {
    /// `/api/address/{addr}/txs/chain/{txid}` (Esplora / public mempool.space)
    PathBased,
    /// `/api/address/{addr}/txs?after_txid={txid}` (some self-hosted instances)
    QueryParam,
}

#[derive(Debug, Default)]
struct PaginationStyleCache {
    by_host: HashMap<String, PaginationStyle>,
}

impl PaginationStyleCache {
    fn get(&self, host: &str) -> Option<PaginationStyle> {
        self.by_host.get(host).copied()
    }

    fn insert(&mut self, host: String, style: PaginationStyle) {
        self.by_host.insert(host, style);
    }
}

fn pagination_style_cache() -> &'static RwLock<PaginationStyleCache> {
    static PAGINATION_STYLE_CACHE: OnceLock<RwLock<PaginationStyleCache>> = OnceLock::new();
    PAGINATION_STYLE_CACHE.get_or_init(|| RwLock::new(PaginationStyleCache::default()))
}

fn pagination_host_key(base_url: &Url) -> String {
    base_url.origin().ascii_serialization()
}

#[derive(Debug)]
pub(crate) struct MempoolPageTransaction {
    pub(crate) txid: TxHash,
    pub(crate) payload_bytes: Vec<u8>,
}

#[derive(Debug)]
pub(crate) struct MempoolTransactionPage {
    pub(crate) request_url: Url,
    pub(crate) http_status_code: u16,
    pub(crate) transactions: Vec<MempoolPageTransaction>,
}

#[derive(Debug)]
struct SuccessfulHttpResponse {
    url: Url,
    status: u16,
    response_headers_json: Option<String>,
    body: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AddressStats {
    pub(crate) tx_count: TransactionCount,
    pub(crate) mempool_tx_count: TransactionCount,
    pub(crate) confirmed_balance: Option<ApiConfirmedBalance>,
}

#[derive(Debug, Deserialize)]
struct AddressStatsResponse {
    chain_stats: TxStats,
    mempool_stats: TxStats,
}

#[derive(Debug, Deserialize)]
struct TxStats {
    tx_count: u32,
    funded_txo_sum: Option<i64>,
    spent_txo_sum: Option<i64>,
}

impl MempoolClient {
    pub(crate) fn new(client: TracedBlockingClient, base_url: Url) -> Self {
        Self {
            base_url,
            client,
            total_api_call_counter: None,
            pagination_cache_hit_counter: None,
        }
    }

    pub(crate) fn with_total_api_call_counter(mut self, counter: Arc<AtomicU64>) -> Self {
        self.total_api_call_counter = Some(counter);
        self
    }

    pub(crate) fn with_pagination_cache_hit_counter(mut self, counter: Arc<AtomicU64>) -> Self {
        self.pagination_cache_hit_counter = Some(counter);
        self
    }

    fn cached_pagination_style(&self) -> Option<PaginationStyle> {
        let host_key = pagination_host_key(&self.base_url);
        match pagination_style_cache().read() {
            Ok(guard) => guard.get(&host_key),
            Err(poisoned) => poisoned.into_inner().get(&host_key),
        }
    }

    fn cache_pagination_style(&self, style: PaginationStyle) {
        let host_key = pagination_host_key(&self.base_url);
        match pagination_style_cache().write() {
            Ok(mut guard) => guard.insert(host_key, style),
            Err(poisoned) => poisoned.into_inner().insert(host_key, style),
        }
    }

    /// Fetch the current chain tip height as a raw string (e.g. `"885123"`).
    pub(crate) fn fetch_chain_tip_height(&self) -> Result<String, MempoolError> {
        let url = self.build_url("api/blocks/tip/height")?;
        self.perform_get_text(&url)
    }

    pub(crate) fn get_address_stats(&self, address: &str) -> Result<AddressStats, MempoolError> {
        let path = format!("api/address/{address}");
        let url = self.build_url(&path)?;
        let body = self.perform_get_text(&url)?;
        parse_address_stats_response(&url, &body)
    }

    pub(crate) fn fetch_first_page_raw(
        &self,
        address: &str,
    ) -> Result<MempoolTransactionPage, MempoolError> {
        let path = format!("api/address/{address}/txs");
        let url = self.build_url(&path)?;
        self.fetch_transaction_page(&url)
    }

    pub(crate) fn fetch_page_after_confirmed_raw(
        &self,
        address: &str,
        after_txid: &str,
    ) -> Result<MempoolTransactionPage, MempoolError> {
        let (_style, page) = self.detect_pagination_style_raw(address, after_txid)?;
        Ok(page)
    }

    fn detect_pagination_style_raw(
        &self,
        address: &str,
        after_txid: &str,
    ) -> Result<(PaginationStyle, MempoolTransactionPage), MempoolError> {
        if let Some(cached_style) = self.cached_pagination_style() {
            self.increment_pagination_cache_hits();
            match self.fetch_next_page_raw(address, after_txid, cached_style) {
                Ok(page) => return Ok((cached_style, page)),
                Err(MempoolError::UpstreamStatus { status: 404, .. })
                    if matches!(cached_style, PaginationStyle::PathBased) =>
                {
                    let page =
                        self.fetch_next_page_raw(address, after_txid, PaginationStyle::QueryParam)?;
                    self.cache_pagination_style(PaginationStyle::QueryParam);
                    return Ok((PaginationStyle::QueryParam, page));
                }
                Err(err) => return Err(err),
            }
        }

        match self.fetch_next_page_raw(address, after_txid, PaginationStyle::PathBased) {
            Ok(page) => {
                self.cache_pagination_style(PaginationStyle::PathBased);
                Ok((PaginationStyle::PathBased, page))
            }
            Err(MempoolError::UpstreamStatus { status: 404, .. }) => {
                let page =
                    self.fetch_next_page_raw(address, after_txid, PaginationStyle::QueryParam)?;
                self.cache_pagination_style(PaginationStyle::QueryParam);
                Ok((PaginationStyle::QueryParam, page))
            }
            Err(err) => Err(err),
        }
    }

    fn fetch_next_page_raw(
        &self,
        address: &str,
        after_txid: &str,
        style: PaginationStyle,
    ) -> Result<MempoolTransactionPage, MempoolError> {
        let url = match style {
            PaginationStyle::PathBased => {
                let path = format!("api/address/{address}/txs/chain/{after_txid}");
                self.build_url(&path)?
            }
            PaginationStyle::QueryParam => {
                let path = format!("api/address/{address}/txs");
                let mut url = self.build_url(&path)?;
                url.query_pairs_mut().append_pair("after_txid", after_txid);
                url
            }
        };
        self.fetch_transaction_page(&url)
    }

    fn build_url(&self, path: &str) -> Result<Url, MempoolError> {
        self.base_url
            .join(path)
            .map_err(|err| MempoolError::UrlJoin(err.to_string()))
    }

    fn perform_get_text(&self, url: &Url) -> Result<String, MempoolError> {
        let response = self.perform_get_bytes(url)?;
        let response_url = response.url.to_string();
        String::from_utf8(response.body).map_err(|err| MempoolError::Http {
            url: response_url.clone(),
            error: format!(
                "response body from {} was not valid UTF-8: {err}",
                response_url
            ),
        })
    }

    fn perform_get_bytes(&self, url: &Url) -> Result<SuccessfulHttpResponse, MempoolError> {
        self.increment_total_api_calls();
        let response = self.client.get(url.as_str()).send().map_err(|error| {
            let error = error.without_url();
            let failure =
                TransportFailure::from_reqwest_error(TransportFailureStage::SendFailed, &error);
            MempoolError::Http {
                url: url.to_string(),
                error: failure.persistence_message(),
            }
        })?;
        let response_url_raw = response.url().to_string();
        let response_url = Url::parse(&response_url_raw).map_err(|err| MempoolError::Http {
            url: response_url_raw.clone(),
            error: format!("invalid response URL {}: {err}", response_url_raw),
        })?;
        let status = response.status();
        let retry_after = parse_retry_after_header(response.headers());
        let response_headers_json = serialize_selected_response_headers_json(response.headers());
        let body = response.text().map_err(|error| {
            let failure = TransportFailure::new(
                TransportFailureStage::ResponseBodyReadFailed,
                error.to_string(),
                crate::traces::client::TransportErrorKind::Decode,
            );
            MempoolError::Http {
                url: response_url.to_string(),
                error: failure.persistence_message(),
            }
        })?;
        let body = body.into_bytes();

        if !status.is_success() {
            if status.as_u16() == 429 {
                return Err(MempoolError::RateLimited {
                    url: response_url.to_string(),
                    retry_after,
                    response_headers_json,
                    response_body: body,
                });
            }
            return Err(MempoolError::UpstreamStatus {
                url: response_url.to_string(),
                status: status.as_u16(),
                response_headers_json,
                response_body: body,
            });
        }

        Ok(SuccessfulHttpResponse {
            url: response_url,
            status: status.as_u16(),
            response_headers_json,
            body,
        })
    }

    fn fetch_transaction_page(&self, url: &Url) -> Result<MempoolTransactionPage, MempoolError> {
        let response = self.perform_get_bytes(url)?;
        parse_transaction_page(response)
    }

    fn increment_total_api_calls(&self) {
        if let Some(counter) = self.total_api_call_counter.as_ref() {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn increment_pagination_cache_hits(&self) {
        if let Some(counter) = self.pagination_cache_hit_counter.as_ref() {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Return the txid of the last confirmed transaction in the page, if any.
/// The first page mixes unconfirmed (mempool) and confirmed entries so we
/// must scan from the end.
#[cfg(all(test, not(bitgarth_db_unit_only)))]
fn last_confirmed_txid(page: &[MempoolAddressTransaction]) -> Option<String> {
    page.iter()
        .rev()
        .find(|tx| tx.status.confirmed)
        .map(|tx| tx.txid.clone())
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
fn confirmed_transaction_count(page: &[MempoolAddressTransaction]) -> usize {
    page.iter().filter(|tx| tx.status.confirmed).count()
}

fn parse_address_stats_response(url: &Url, body: &str) -> Result<AddressStats, MempoolError> {
    let parsed: AddressStatsResponse =
        serde_json::from_str(body).map_err(|err| MempoolError::Deserialize {
            url: url.to_string(),
            error: err.to_string(),
            http_status_code: None,
            response_headers_json: None,
            response_body: None,
        })?;
    let confirmed_balance = match (
        parsed.chain_stats.funded_txo_sum,
        parsed.chain_stats.spent_txo_sum,
    ) {
        (Some(funded), Some(spent)) if funded >= spent => {
            let balance = funded - spent;
            Some(
                ApiConfirmedBalance::from_smallest_unit_i64(balance).map_err(|err| {
                    MempoolError::Deserialize {
                        url: url.to_string(),
                        error: format!(
                            "invalid confirmed balance from funded_txo_sum - spent_txo_sum: {err}"
                        ),
                        http_status_code: None,
                        response_headers_json: None,
                        response_body: None,
                    }
                })?,
            )
        }
        (Some(_), Some(_)) => None,
        _ => None,
    };
    Ok(AddressStats {
        tx_count: TransactionCount::from_u32(parsed.chain_stats.tx_count),
        mempool_tx_count: TransactionCount::from_u32(parsed.mempool_stats.tx_count),
        confirmed_balance,
    })
}

fn parse_transaction_page(
    response: SuccessfulHttpResponse,
) -> Result<MempoolTransactionPage, MempoolError> {
    let raw_transactions: Vec<Box<RawValue>> =
        serde_json::from_slice(&response.body).map_err(|err| MempoolError::Deserialize {
            url: response.url.to_string(),
            error: err.to_string(),
            http_status_code: Some(response.status),
            response_headers_json: response.response_headers_json.clone(),
            response_body: Some(response.body.clone()),
        })?;
    let mut transactions = Vec::with_capacity(raw_transactions.len());
    for raw_value in raw_transactions {
        let raw_json = raw_value.get();
        let transaction: MempoolAddressTransaction =
            serde_json::from_str(raw_json).map_err(|err| MempoolError::Deserialize {
                url: response.url.to_string(),
                error: err.to_string(),
                http_status_code: Some(response.status),
                response_headers_json: response.response_headers_json.clone(),
                response_body: Some(response.body.clone()),
            })?;
        let txid = TxHash::parse(&transaction.txid).map_err(|err| MempoolError::Deserialize {
            url: response.url.to_string(),
            error: format!("invalid txid in transaction page: {err}"),
            http_status_code: Some(response.status),
            response_headers_json: response.response_headers_json.clone(),
            response_body: Some(response.body.clone()),
        })?;
        transactions.push(MempoolPageTransaction {
            txid,
            payload_bytes: raw_json.as_bytes().to_vec(),
        });
    }
    Ok(MempoolTransactionPage {
        request_url: response.url,
        http_status_code: response.status,
        transactions,
    })
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
fn should_early_exit_on_known_confirmed(
    page: &[MempoolAddressTransaction],
    known_confirmed_txids: &HashSet<String>,
) -> bool {
    if known_confirmed_txids.is_empty() {
        return false;
    }
    let mut saw_confirmed = false;
    for tx in page {
        if !tx.status.confirmed {
            continue;
        }
        saw_confirmed = true;
        if !known_confirmed_txids.contains(&tx.txid) {
            return false;
        }
    }
    saw_confirmed
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
fn should_early_exit_for_page(
    allow_known_confirmed_early_exit: bool,
    page: &[MempoolAddressTransaction],
    known_confirmed_txids: &HashSet<String>,
) -> bool {
    allow_known_confirmed_early_exit
        && should_early_exit_on_known_confirmed(page, known_confirmed_txids)
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
fn response_snippet(body: &str) -> String {
    const RESPONSE_SNIPPET_MAX_BYTES: usize = 8 * 1024;

    if body.len() <= RESPONSE_SNIPPET_MAX_BYTES {
        body.to_string()
    } else {
        let mut snippet = body[..RESPONSE_SNIPPET_MAX_BYTES].to_string();
        snippet.push_str("...");
        snippet
    }
}

fn serialize_selected_response_headers_json(headers: &HeaderMap) -> Option<String> {
    let retry_after = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    if retry_after.is_empty() {
        return None;
    }
    Some(serde_json::json!({ "retry-after": retry_after }).to_string())
}

fn parse_retry_after_header(headers: &HeaderMap) -> Option<Duration> {
    let raw = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    let seconds = raw.parse::<u64>().ok()?;
    Some(Duration::from_secs(seconds))
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::super::types::MempoolTransactionStatus;
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};
    use serde_json::Value;

    fn tx_json(txid: &str, confirmed: bool, fee: u64) -> String {
        let block_height = if confirmed {
            "123".to_string()
        } else {
            "null".to_string()
        };
        let block_time = if confirmed {
            "1700000000".to_string()
        } else {
            "null".to_string()
        };
        format!(
            concat!(
                "{{",
                "\"txid\":\"{txid}\",",
                "\"vin\":[],",
                "\"vout\":[],",
                "\"fee\":{fee},",
                "\"status\":{{",
                "\"confirmed\":{confirmed},",
                "\"block_height\":{block_height},",
                "\"block_hash\":null,",
                "\"block_time\":{block_time}",
                "}}",
                "}}"
            ),
            txid = txid,
            fee = fee,
            confirmed = confirmed,
            block_height = block_height,
            block_time = block_time,
        )
    }

    fn tx(txid: &str, confirmed: bool) -> MempoolAddressTransaction {
        MempoolAddressTransaction {
            txid: txid.to_string(),
            vin: Vec::new(),
            vout: Vec::new(),
            fee: None,
            status: MempoolTransactionStatus {
                confirmed,
                block_height: if confirmed { Some(100) } else { None },
                block_hash: None,
                block_time: None,
            },
        }
    }

    fn successful_http_response(url: Url, body: String) -> SuccessfulHttpResponse {
        SuccessfulHttpResponse {
            url,
            status: 200,
            response_headers_json: None,
            body: body.into_bytes(),
        }
    }

    #[test]
    fn response_snippet_short_body() {
        let body = "short";
        assert_eq!(response_snippet(body), "short");
    }

    #[test]
    fn response_snippet_truncates_long_body() {
        let body = "x".repeat(RESPONSE_SNIPPET_MAX_BYTES + 100);
        let snippet = response_snippet(&body);
        assert!(snippet.ends_with("..."));
        assert_eq!(snippet.len(), RESPONSE_SNIPPET_MAX_BYTES + 3);
    }

    #[test]
    fn last_confirmed_txid_returns_none_for_empty_page() {
        assert_eq!(last_confirmed_txid(&[]), None);
    }

    #[test]
    fn last_confirmed_txid_returns_none_when_all_unconfirmed() {
        let page = vec![tx("aaa", false), tx("bbb", false)];
        assert_eq!(last_confirmed_txid(&page), None);
    }

    #[test]
    fn last_confirmed_txid_finds_last_confirmed_in_mixed_page() {
        let page = vec![
            tx("unconf1", false),
            tx("conf1", true),
            tx("conf2", true),
            tx("unconf2", false),
        ];
        // The last confirmed scanning from the end is conf2.
        assert_eq!(last_confirmed_txid(&page), Some("conf2".to_string()));
    }

    #[test]
    fn last_confirmed_txid_returns_sole_confirmed() {
        let page = vec![tx("only", true)];
        assert_eq!(last_confirmed_txid(&page), Some("only".to_string()));
    }

    #[test]
    fn pagination_cache_returns_none_for_unknown_host() {
        let cache = PaginationStyleCache::default();
        assert_eq!(cache.get("https://mempool.example"), None);
    }

    #[test]
    fn pagination_cache_stores_and_reads_by_host() {
        let mut cache = PaginationStyleCache::default();
        cache.insert(
            "https://mempool.example".to_string(),
            PaginationStyle::QueryParam,
        );

        assert_eq!(
            cache.get("https://mempool.example"),
            Some(PaginationStyle::QueryParam)
        );
    }

    #[test]
    fn pagination_cache_keeps_hosts_independent() {
        let mut cache = PaginationStyleCache::default();
        cache.insert(
            "https://host-a.example".to_string(),
            PaginationStyle::PathBased,
        );
        cache.insert(
            "https://host-b.example".to_string(),
            PaginationStyle::QueryParam,
        );

        assert_eq!(
            cache.get("https://host-a.example"),
            Some(PaginationStyle::PathBased)
        );
        assert_eq!(
            cache.get("https://host-b.example"),
            Some(PaginationStyle::QueryParam)
        );
    }

    #[test]
    fn pagination_host_key_uses_origin() {
        let parsed = Url::parse("https://mempool.example:8443/api/address/foo/txs");
        assert!(parsed.is_ok());
        let Ok(url) = parsed else {
            return;
        };
        assert_eq!(
            pagination_host_key(&url),
            "https://mempool.example:8443".to_string()
        );
    }

    #[test]
    fn should_early_exit_on_known_confirmed_returns_true_when_all_confirmed_are_known() {
        let page = vec![
            tx("mempool-a", false),
            tx("conf-a", true),
            tx("conf-b", true),
        ];
        let known = HashSet::from(["conf-a".to_string(), "conf-b".to_string()]);
        assert!(should_early_exit_on_known_confirmed(&page, &known));
    }

    #[test]
    fn should_early_exit_on_known_confirmed_returns_false_when_one_confirmed_is_unknown() {
        let page = vec![tx("conf-a", true), tx("conf-b", true)];
        let known = HashSet::from(["conf-a".to_string()]);
        assert!(!should_early_exit_on_known_confirmed(&page, &known));
    }

    #[test]
    fn should_early_exit_on_known_confirmed_returns_false_for_mempool_only_page() {
        let page = vec![tx("mempool-a", false), tx("mempool-b", false)];
        let known = HashSet::from(["mempool-a".to_string()]);
        assert!(!should_early_exit_on_known_confirmed(&page, &known));
    }

    #[test]
    fn should_early_exit_on_known_confirmed_returns_false_when_known_set_is_empty() {
        let page = vec![tx("conf-a", true)];
        let known = HashSet::new();
        assert!(!should_early_exit_on_known_confirmed(&page, &known));
    }

    #[test]
    fn should_early_exit_for_page_returns_false_when_disabled() {
        let page = vec![tx("conf-a", true)];
        let known = HashSet::from(["conf-a".to_string()]);
        assert!(!should_early_exit_for_page(false, &page, &known));
        assert!(should_early_exit_for_page(true, &page, &known));
    }

    #[test]
    fn confirmed_transaction_count_ignores_unconfirmed_entries() {
        let page = vec![
            tx("mempool-a", false),
            tx("conf-a", true),
            tx("conf-b", true),
        ];
        assert_eq!(confirmed_transaction_count(&page), 2);
    }

    #[test]
    fn parse_address_stats_response_extracts_chain_and_mempool_counts() {
        let url =
            Url::parse("https://mempool.space/api/address/bc1qtest").expect("url should parse");
        let body = r#"{
            "chain_stats": { "tx_count": 150 },
            "mempool_stats": { "tx_count": 2 }
        }"#;

        let stats = parse_address_stats_response(&url, body).expect("stats should parse");
        assert_eq!(stats.tx_count, TransactionCount::from_u32(150));
        assert_eq!(stats.mempool_tx_count, TransactionCount::from_u32(2));
        assert!(stats.confirmed_balance.is_none());
    }

    #[test]
    fn parse_address_stats_response_computes_confirmed_balance_from_txo_sums() {
        let url =
            Url::parse("https://mempool.space/api/address/bc1qtest").expect("url should parse");
        let body = r#"{
            "chain_stats": { "tx_count": 10, "funded_txo_sum": 500000, "spent_txo_sum": 200000 },
            "mempool_stats": { "tx_count": 1 }
        }"#;

        let stats = parse_address_stats_response(&url, body).expect("stats should parse");
        assert_eq!(stats.tx_count, TransactionCount::from_u32(10));
        let balance = stats
            .confirmed_balance
            .expect("confirmed_balance should be present");
        assert_eq!(balance.amount().value(), 300_000_u128);
    }

    #[test]
    fn parse_address_stats_response_returns_none_when_balance_fields_missing() {
        let url =
            Url::parse("https://mempool.space/api/address/bc1qtest").expect("url should parse");
        let body = r#"{
            "chain_stats": { "tx_count": 5 },
            "mempool_stats": { "tx_count": 0, "funded_txo_sum": 100 }
        }"#;

        let stats = parse_address_stats_response(&url, body).expect("stats should parse");
        assert!(stats.confirmed_balance.is_none());
    }

    #[test]
    fn parse_address_stats_response_returns_none_for_negative_balance() {
        let url =
            Url::parse("https://mempool.space/api/address/bc1qtest").expect("url should parse");
        let body = r#"{
            "chain_stats": { "tx_count": 3, "funded_txo_sum": 100, "spent_txo_sum": 500 },
            "mempool_stats": { "tx_count": 0 }
        }"#;

        let stats = parse_address_stats_response(&url, body).expect("stats should parse");
        assert!(stats.confirmed_balance.is_none());
    }

    #[test]
    fn parse_retry_after_header_returns_duration_for_numeric_header() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("42"));

        let parsed = parse_retry_after_header(&headers);
        assert_eq!(parsed, Some(Duration::from_secs(42)));
    }

    #[test]
    fn parse_retry_after_header_returns_none_for_non_numeric_header() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("soon"));

        assert_eq!(parse_retry_after_header(&headers), None);
    }

    #[test]
    fn parse_transaction_page_preserves_object_bytes_and_semantic_reassembly() {
        let url =
            Url::parse("https://mempool.space/api/address/bc1qtest/txs").expect("url should parse");
        let tx1 = tx_json(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa01",
            false,
            111,
        );
        let tx2 = tx_json(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa02",
            true,
            222,
        );
        let body = format!("[\n  {},\n  {}\n]", tx1, tx2);

        let page = parse_transaction_page(successful_http_response(url, body.clone()))
            .expect("page should parse");

        assert_eq!(page.transactions.len(), 2);
        assert_eq!(page.transactions[0].payload_bytes, tx1.as_bytes());
        assert_eq!(page.transactions[1].payload_bytes, tx2.as_bytes());

        let reconstructed = format!(
            "[{}]",
            page.transactions
                .iter()
                .map(|transaction| {
                    String::from_utf8(transaction.payload_bytes.clone())
                        .expect("payload bytes should remain valid UTF-8 JSON")
                })
                .collect::<Vec<_>>()
                .join(",")
        );

        let original_value: Value =
            serde_json::from_str(&body).expect("original response should parse");
        let reconstructed_value: Value =
            serde_json::from_str(&reconstructed).expect("reconstructed response should parse");

        assert_eq!(reconstructed_value, original_value);
        assert_eq!(
            page.transactions
                .iter()
                .map(|transaction| transaction.txid.as_str().to_string())
                .collect::<Vec<_>>(),
            vec![
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa01".to_string(),
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa02".to_string(),
            ]
        );
    }

    #[test]
    fn parse_transaction_page_rejects_invalid_txid_as_deserialize_error() {
        let url =
            Url::parse("https://mempool.space/api/address/bc1qtest/txs").expect("url should parse");
        let body = format!("[{}]", tx_json("not-a-valid-txid", true, 111));

        let error = parse_transaction_page(successful_http_response(url.clone(), body))
            .expect_err("invalid txid should fail the whole page");

        match error {
            MempoolError::Deserialize {
                url: error_url,
                error,
                ..
            } => {
                assert_eq!(error_url, url.to_string());
                assert!(error.contains("invalid txid in transaction page"));
            }
            other => panic!("expected deserialize error, got {other:?}"),
        }
    }
}
