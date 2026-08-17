use super::error::EtherscanError;
use super::types::{EtherscanInternalTx, EtherscanNormalTx};
use crate::amounts::UnsignedAmount;
use crate::traces::client::{TracedBlockingClient, TransportFailure, TransportFailureStage};
use crate::transactions::{ApiConfirmedBalance, TransactionCount, TxCountEstimate};
use serde::Deserialize;
use serde_json::value::RawValue;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use url::Url;

const RESPONSE_SNIPPET_MAX_BYTES: usize = 512;
const QUICK_ESTIMATE_OFFSET: u64 = 10_000;

/// Client for the Etherscan API v2.
///
/// Accepts a pre-configured `TracedBlockingClient` — the caller is
/// responsible for setting timeouts and integration labeling.
pub(crate) struct EtherscanClient {
    client: TracedBlockingClient,
    api_key: String,
    base_url: String,
    chain_id: u64,
    total_api_call_counter: Option<Arc<AtomicU64>>,
}

struct EtherscanHttpResponse {
    body: String,
    url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EtherscanRequestMetadata {
    pub(crate) request_url_without_api_key: String,
    pub(crate) request_query_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EtherscanFetchedItem<T> {
    pub(crate) parsed: T,
    pub(crate) raw_json_bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EtherscanFetchedPage<T> {
    pub(crate) request: EtherscanRequestMetadata,
    pub(crate) items: Vec<EtherscanFetchedItem<T>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EtherscanTransactionQuery {
    action: &'static str,
    address: String,
    start_block: u64,
    end_block: u64,
    sort: EtherscanSortOrder,
    page: u64,
    offset: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EtherscanSortOrder {
    Asc,
    Desc,
}

impl EtherscanSortOrder {
    const fn as_query_value(self) -> &'static str {
        match self {
            Self::Asc => "asc",
            Self::Desc => "desc",
        }
    }
}

impl EtherscanTransactionQuery {
    fn new(
        action: &'static str,
        address: &str,
        start_block: u64,
        end_block: u64,
        sort: EtherscanSortOrder,
        page: u64,
        offset: u64,
    ) -> Self {
        Self {
            action,
            address: address.to_string(),
            start_block,
            end_block,
            sort,
            page,
            offset,
        }
    }

    fn request_query_json(&self, chain_id: u64) -> String {
        serde_json::json!({
            "chainid": chain_id.to_string(),
            "module": "account",
            "action": self.action,
            "address": self.address,
            "startblock": self.start_block.to_string(),
            "endblock": self.end_block.to_string(),
            "sort": self.sort.as_query_value(),
            "page": self.page.to_string(),
            "offset": self.offset.to_string()
        })
        .to_string()
    }

    fn request_url_without_api_key(
        &self,
        base_url: &str,
        chain_id: u64,
    ) -> Result<String, EtherscanError> {
        let mut url = Url::parse(base_url).map_err(|err| EtherscanError::Http {
            url: base_url.to_string(),
            error: format!("invalid etherscan base url: {err}"),
        })?;
        {
            let mut query_pairs = url.query_pairs_mut();
            query_pairs.append_pair("chainid", &chain_id.to_string());
            query_pairs.append_pair("module", "account");
            query_pairs.append_pair("action", self.action);
            query_pairs.append_pair("address", &self.address);
            query_pairs.append_pair("startblock", &self.start_block.to_string());
            query_pairs.append_pair("endblock", &self.end_block.to_string());
            query_pairs.append_pair("sort", self.sort.as_query_value());
            query_pairs.append_pair("page", &self.page.to_string());
            query_pairs.append_pair("offset", &self.offset.to_string());
        }
        Ok(url.to_string())
    }

    fn request_url_with_api_key(
        &self,
        base_url: &str,
        chain_id: u64,
        api_key: &str,
    ) -> Result<String, EtherscanError> {
        let mut url = Url::parse(base_url).map_err(|err| EtherscanError::Http {
            url: base_url.to_string(),
            error: format!("invalid etherscan base url: {err}"),
        })?;
        {
            let mut query_pairs = url.query_pairs_mut();
            query_pairs.append_pair("chainid", &chain_id.to_string());
            query_pairs.append_pair("module", "account");
            query_pairs.append_pair("action", self.action);
            query_pairs.append_pair("address", &self.address);
            query_pairs.append_pair("startblock", &self.start_block.to_string());
            query_pairs.append_pair("endblock", &self.end_block.to_string());
            query_pairs.append_pair("sort", self.sort.as_query_value());
            query_pairs.append_pair("page", &self.page.to_string());
            query_pairs.append_pair("offset", &self.offset.to_string());
            query_pairs.append_pair("apikey", api_key);
        }
        Ok(url.to_string())
    }

    fn request_metadata(
        &self,
        base_url: &str,
        chain_id: u64,
    ) -> Result<EtherscanRequestMetadata, EtherscanError> {
        Ok(EtherscanRequestMetadata {
            request_url_without_api_key: self.request_url_without_api_key(base_url, chain_id)?,
            request_query_json: self.request_query_json(chain_id),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EtherscanBalanceQuery {
    address: String,
}

impl EtherscanBalanceQuery {
    fn new(address: &str) -> Self {
        Self {
            address: address.to_string(),
        }
    }

    fn request_query_json(&self, chain_id: u64) -> String {
        serde_json::json!({
            "chainid": chain_id.to_string(),
            "module": "account",
            "action": "balance",
            "address": self.address,
            "tag": "latest"
        })
        .to_string()
    }

    fn request_url_without_api_key(
        &self,
        base_url: &str,
        chain_id: u64,
    ) -> Result<String, EtherscanError> {
        let mut url = Url::parse(base_url).map_err(|err| EtherscanError::Http {
            url: base_url.to_string(),
            error: format!("invalid etherscan base url: {err}"),
        })?;
        {
            let mut query_pairs = url.query_pairs_mut();
            query_pairs.append_pair("chainid", &chain_id.to_string());
            query_pairs.append_pair("module", "account");
            query_pairs.append_pair("action", "balance");
            query_pairs.append_pair("address", &self.address);
            query_pairs.append_pair("tag", "latest");
        }
        Ok(url.to_string())
    }

    fn request_url_with_api_key(
        &self,
        base_url: &str,
        chain_id: u64,
        api_key: &str,
    ) -> Result<String, EtherscanError> {
        let mut url = Url::parse(base_url).map_err(|err| EtherscanError::Http {
            url: base_url.to_string(),
            error: format!("invalid etherscan base url: {err}"),
        })?;
        {
            let mut query_pairs = url.query_pairs_mut();
            query_pairs.append_pair("chainid", &chain_id.to_string());
            query_pairs.append_pair("module", "account");
            query_pairs.append_pair("action", "balance");
            query_pairs.append_pair("address", &self.address);
            query_pairs.append_pair("tag", "latest");
            query_pairs.append_pair("apikey", api_key);
        }
        Ok(url.to_string())
    }

    fn request_metadata(
        &self,
        base_url: &str,
        chain_id: u64,
    ) -> Result<EtherscanRequestMetadata, EtherscanError> {
        Ok(EtherscanRequestMetadata {
            request_url_without_api_key: self.request_url_without_api_key(base_url, chain_id)?,
            request_query_json: self.request_query_json(chain_id),
        })
    }
}

#[derive(Deserialize)]
struct RawEtherscanResponse {
    status: String,
    message: String,
    result: Box<RawValue>,
}

impl EtherscanClient {
    pub(crate) fn new(
        client: TracedBlockingClient,
        api_key: &str,
        base_url: &str,
        chain_id: u64,
    ) -> Self {
        Self {
            client,
            api_key: api_key.to_string(),
            base_url: base_url.to_string(),
            chain_id,
            total_api_call_counter: None,
        }
    }

    pub(crate) fn with_total_api_call_counter(mut self, counter: Arc<AtomicU64>) -> Self {
        self.total_api_call_counter = Some(counter);
        self
    }

    pub(crate) fn chain_id(&self) -> u64 {
        self.chain_id
    }

    /// Fetch the current block number.
    pub(crate) fn fetch_block_number(&self) -> Result<u64, EtherscanError> {
        let url = format!(
            "{}?chainid={}&module=proxy&action=eth_blockNumber&apikey={}",
            self.base_url, self.chain_id, self.api_key
        );

        #[derive(Deserialize)]
        struct BlockNumberResponse {
            #[serde(default)]
            status: Option<String>,
            #[serde(default)]
            message: Option<String>,
            result: String,
        }

        let http_response = self.perform_get(&url)?;

        let parsed: BlockNumberResponse =
            serde_json::from_str(&http_response.body).map_err(|e| EtherscanError::Deserialize {
                url: http_response.url.clone(),
                error: e.to_string(),
            })?;

        if parsed.status.as_deref() == Some("0") {
            return Err(EtherscanError::ApiError {
                status: parsed.status.unwrap_or_else(|| "0".to_string()),
                message: resolve_api_error_message(parsed.message.as_deref(), &parsed.result),
            });
        }

        parse_block_number_result(&parsed.result).map_err(|error| EtherscanError::Deserialize {
            url: http_response.url,
            error,
        })
    }

    /// Fetch the current native ETH balance for an address.
    pub(crate) fn fetch_native_balance(
        &self,
        address: &str,
    ) -> Result<ApiConfirmedBalance, EtherscanError> {
        let query = EtherscanBalanceQuery::new(address);
        let url = query.request_url_with_api_key(&self.base_url, self.chain_id, &self.api_key)?;
        let http_response = self.perform_get(&url)?;
        let response: RawEtherscanResponse =
            serde_json::from_str(&http_response.body).map_err(|e| EtherscanError::Deserialize {
                url: http_response.url.clone(),
                error: e.to_string(),
            })?;

        let result_message = extract_raw_result_message(&response.result)?;
        check_api_status(
            &response.status,
            &response.message,
            result_message.as_deref(),
        )?;
        let raw_balance = extract_raw_result_string(&response.result, &http_response.url)?;
        parse_native_balance_result(&raw_balance).map_err(|error| EtherscanError::Deserialize {
            url: http_response.url,
            error,
        })
    }

    pub(crate) fn native_balance_request_metadata(
        &self,
        address: &str,
    ) -> Result<EtherscanRequestMetadata, EtherscanError> {
        EtherscanBalanceQuery::new(address).request_metadata(&self.base_url, self.chain_id)
    }

    /// Fetch a page of normal transactions for an address.
    pub(crate) fn fetch_normal_transactions_page(
        &self,
        address: &str,
        start_block: u64,
        end_block: u64,
        page: u64,
        offset: u64,
    ) -> Result<EtherscanFetchedPage<EtherscanNormalTx>, EtherscanError> {
        self.fetch_transaction_page(EtherscanTransactionQuery::new(
            "txlist",
            address,
            start_block,
            end_block,
            EtherscanSortOrder::Desc,
            page,
            offset,
        ))
    }

    pub(crate) fn normal_transactions_request_metadata(
        &self,
        address: &str,
        start_block: u64,
        end_block: u64,
        page: u64,
        offset: u64,
    ) -> Result<EtherscanRequestMetadata, EtherscanError> {
        EtherscanTransactionQuery::new(
            "txlist",
            address,
            start_block,
            end_block,
            EtherscanSortOrder::Desc,
            page,
            offset,
        )
        .request_metadata(&self.base_url, self.chain_id)
    }

    /// Fetch a page of internal transactions for an address.
    pub(crate) fn fetch_internal_transactions_page(
        &self,
        address: &str,
        start_block: u64,
        end_block: u64,
        page: u64,
        offset: u64,
    ) -> Result<EtherscanFetchedPage<EtherscanInternalTx>, EtherscanError> {
        self.fetch_transaction_page(EtherscanTransactionQuery::new(
            "txlistinternal",
            address,
            start_block,
            end_block,
            EtherscanSortOrder::Desc,
            page,
            offset,
        ))
    }

    pub(crate) fn internal_transactions_request_metadata(
        &self,
        address: &str,
        start_block: u64,
        end_block: u64,
        page: u64,
        offset: u64,
    ) -> Result<EtherscanRequestMetadata, EtherscanError> {
        EtherscanTransactionQuery::new(
            "txlistinternal",
            address,
            start_block,
            end_block,
            EtherscanSortOrder::Desc,
            page,
            offset,
        )
        .request_metadata(&self.base_url, self.chain_id)
    }

    pub(crate) fn quick_estimate_tx_count(
        &self,
        address: &str,
    ) -> Result<TxCountEstimate, EtherscanError> {
        let txs: EtherscanFetchedPage<EtherscanNormalTx> =
            self.fetch_transaction_page(EtherscanTransactionQuery::new(
                "txlist",
                address,
                0,
                99_999_999,
                EtherscanSortOrder::Asc,
                1,
                QUICK_ESTIMATE_OFFSET,
            ))?;
        Ok(tx_count_estimate_from_page_len(txs.items.len()))
    }

    fn perform_get(&self, url: &str) -> Result<EtherscanHttpResponse, EtherscanError> {
        self.increment_total_api_calls();
        let response = self.client.get(url).send().map_err(|error| {
            let error = error.without_url();
            let failure =
                TransportFailure::from_reqwest_error(TransportFailureStage::SendFailed, &error);
            EtherscanError::Http {
                url: url.to_string(),
                error: failure.persistence_message(),
            }
        })?;
        let response_url = response.url().to_string();

        let status = response.status();
        let body = response.text().map_err(|error| {
            let failure = TransportFailure::new(
                TransportFailureStage::ResponseBodyReadFailed,
                error.to_string(),
                crate::traces::client::TransportErrorKind::Decode,
            );
            EtherscanError::Http {
                url: response_url.clone(),
                error: failure.persistence_message(),
            }
        })?;

        if !status.is_success() {
            let snippet = response_body_snippet(&body);
            return Err(EtherscanError::UpstreamStatus {
                url: response_url,
                status: status.as_u16(),
                body_snippet: snippet,
            });
        }

        Ok(EtherscanHttpResponse {
            body,
            url: response_url,
        })
    }

    fn increment_total_api_calls(&self) {
        if let Some(counter) = self.total_api_call_counter.as_ref() {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn fetch_transaction_page<T>(
        &self,
        query: EtherscanTransactionQuery,
    ) -> Result<EtherscanFetchedPage<T>, EtherscanError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let url = query.request_url_with_api_key(&self.base_url, self.chain_id, &self.api_key)?;
        let request = query.request_metadata(&self.base_url, self.chain_id)?;
        let http_response = self.perform_get(&url)?;
        let response: RawEtherscanResponse =
            serde_json::from_str(&http_response.body).map_err(|e| EtherscanError::Deserialize {
                url: http_response.url.clone(),
                error: e.to_string(),
            })?;

        let result_message = extract_raw_result_message(&response.result)?;
        check_api_status(
            &response.status,
            &response.message,
            result_message.as_deref(),
        )?;

        let items =
            extract_raw_result_items::<T>(&response.result, &response.message, &http_response.url)?;
        Ok(EtherscanFetchedPage { request, items })
    }
}

fn parse_block_number_result(raw_result: &str) -> Result<u64, String> {
    let trimmed = raw_result.trim();
    if let Some(hex_str) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        return u64::from_str_radix(hex_str, 16)
            .map_err(|error| format!("invalid block number hex: {error}"));
    }

    trimmed
        .parse::<u64>()
        .map_err(|error| format!("invalid block number value: {error}"))
}

fn parse_native_balance_result(raw_result: &str) -> Result<ApiConfirmedBalance, String> {
    let trimmed = raw_result.trim();
    if trimmed.is_empty() {
        return Err("empty balance result".to_string());
    }
    if !trimmed.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("invalid balance wei value: {trimmed}"));
    }
    let wei = trimmed
        .parse::<u128>()
        .map_err(|error| format!("invalid balance wei value: {error}"))?;
    Ok(ApiConfirmedBalance::from_amount(UnsignedAmount::from_u128(
        wei,
    )))
}

fn tx_count_estimate_from_page_len(len: usize) -> TxCountEstimate {
    let as_u32 = u32::try_from(len).unwrap_or(u32::MAX);
    let count = TransactionCount::from_u32(as_u32);
    if len < usize::try_from(QUICK_ESTIMATE_OFFSET).unwrap_or(usize::MAX) {
        TxCountEstimate::Exact(count)
    } else {
        TxCountEstimate::AtLeast(count)
    }
}

fn response_body_snippet(body: &str) -> String {
    let body_bytes = body.as_bytes();
    if body_bytes.len() <= RESPONSE_SNIPPET_MAX_BYTES {
        return body.to_string();
    }
    format!(
        "{}...",
        String::from_utf8_lossy(&body_bytes[..RESPONSE_SNIPPET_MAX_BYTES])
    )
}

fn check_api_status(
    status: &str,
    message: &str,
    result_message: Option<&str>,
) -> Result<(), EtherscanError> {
    let api_message = result_message.unwrap_or(message);
    if status == "0"
        && !is_no_transactions_message(message)
        && !is_no_transactions_message(api_message)
    {
        return Err(EtherscanError::ApiError {
            status: status.to_string(),
            message: api_message.to_string(),
        });
    }
    Ok(())
}

fn extract_raw_result_message(result: &RawValue) -> Result<Option<String>, EtherscanError> {
    let raw = result.get().trim();
    if raw == "null" || raw.starts_with('[') {
        return Ok(None);
    }
    serde_json::from_str::<String>(raw)
        .map(Some)
        .map_err(|error| EtherscanError::Deserialize {
            url: "etherscan-result".to_string(),
            error: error.to_string(),
        })
}

fn extract_raw_result_string(result: &RawValue, url: &str) -> Result<String, EtherscanError> {
    let raw = result.get().trim();
    serde_json::from_str::<String>(raw).map_err(|error| EtherscanError::Deserialize {
        url: url.to_string(),
        error: error.to_string(),
    })
}

fn extract_raw_result_items<T>(
    result: &RawValue,
    message: &str,
    url: &str,
) -> Result<Vec<EtherscanFetchedItem<T>>, EtherscanError>
where
    T: for<'de> Deserialize<'de>,
{
    let raw = result.get().trim();
    if raw == "null" {
        if is_no_transactions_message(message) {
            return Ok(Vec::new());
        }
        return Err(EtherscanError::ApiError {
            status: "0".to_string(),
            message: message.to_string(),
        });
    }

    if raw.starts_with('"') {
        let result_message =
            serde_json::from_str::<String>(raw).map_err(|error| EtherscanError::Deserialize {
                url: url.to_string(),
                error: error.to_string(),
            })?;
        if is_no_transactions_message(message) || is_no_transactions_message(&result_message) {
            return Ok(Vec::new());
        }
        return Err(EtherscanError::ApiError {
            status: "0".to_string(),
            message: result_message,
        });
    }

    let items = serde_json::from_str::<Vec<Box<RawValue>>>(raw).map_err(|error| {
        EtherscanError::Deserialize {
            url: url.to_string(),
            error: error.to_string(),
        }
    })?;
    items
        .into_iter()
        .map(|raw_item| {
            serde_json::from_str::<T>(raw_item.get())
                .map(|parsed| EtherscanFetchedItem {
                    parsed,
                    raw_json_bytes: raw_item.get().as_bytes().to_vec(),
                })
                .map_err(|error| EtherscanError::Deserialize {
                    url: url.to_string(),
                    error: error.to_string(),
                })
        })
        .collect()
}

fn resolve_api_error_message(message: Option<&str>, result: &str) -> String {
    let result_trimmed = result.trim();
    if !result_trimmed.is_empty() && !result_trimmed.eq_ignore_ascii_case("NOTOK") {
        return result_trimmed.to_string();
    }

    if let Some(message) = message {
        let message_trimmed = message.trim();
        if !message_trimmed.is_empty() {
            return message_trimmed.to_string();
        }
    }

    if !result_trimmed.is_empty() {
        return result_trimmed.to_string();
    }

    "Unknown Etherscan API error".to_string()
}

fn is_no_transactions_message(value: &str) -> bool {
    value
        .trim()
        .to_ascii_lowercase()
        .contains("no transactions found")
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn parse_normal_tx_response() {
        let json = r#"{
            "status": "1",
            "message": "OK",
            "result": [{
                "blockNumber": "1234567",
                "timeStamp": "1609459200",
                "hash": "0xabc123",
                "from": "0x1111111111111111111111111111111111111111",
                "to": "0x2222222222222222222222222222222222222222",
                "value": "1000000000000000000",
                "gas": "21000",
                "gasPrice": "20000000000",
                "gasUsed": "21000",
                "isError": "0",
                "txreceipt_status": "1",
                "nonce": "42"
            }]
        }"#;

        let response: RawEtherscanResponse = serde_json::from_str(json)
            .map_err(|e| format!("{e}"))
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(response.status, "1");
        let items = extract_raw_result_items::<EtherscanNormalTx>(
            &response.result,
            &response.message,
            "https://api.etherscan.io/v2/api",
        )
        .map_err(|e| format!("{e}"))
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].parsed.value, "1000000000000000000");
        assert_eq!(items[0].parsed.hash, "0xabc123");
        assert_eq!(items[0].parsed.nonce, "42");
        assert!(
            std::str::from_utf8(&items[0].raw_json_bytes)
                .expect("raw item utf8")
                .contains("\"hash\": \"0xabc123\"")
        );
    }

    #[test]
    fn parse_internal_tx_response() {
        let json = r#"{
            "status": "1",
            "message": "OK",
            "result": [{
                "blockNumber": "1234567",
                "timeStamp": "1609459200",
                "hash": "0xdef456",
                "from": "0x3333333333333333333333333333333333333333",
                "to": "0x4444444444444444444444444444444444444444",
                "value": "500000000000000000",
                "isError": "0",
                "type": "call"
            }]
        }"#;

        let response: RawEtherscanResponse = serde_json::from_str(json)
            .map_err(|e| format!("{e}"))
            .unwrap_or_else(|e| panic!("{e}"));
        let items = extract_raw_result_items::<EtherscanInternalTx>(
            &response.result,
            &response.message,
            "https://api.etherscan.io/v2/api",
        )
        .map_err(|e| format!("{e}"))
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].parsed.call_type, "call");
    }

    #[test]
    fn parse_empty_result() {
        let json = r#"{
            "status": "0",
            "message": "No transactions found",
            "result": []
        }"#;

        let response: RawEtherscanResponse = serde_json::from_str(json)
            .map_err(|e| format!("{e}"))
            .unwrap_or_else(|e| panic!("{e}"));
        let items = extract_raw_result_items::<EtherscanNormalTx>(
            &response.result,
            &response.message,
            "https://api.etherscan.io/v2/api",
        )
        .map_err(|e| format!("{e}"))
        .unwrap_or_else(|e| panic!("{e}"));
        assert!(items.is_empty());
        assert!(check_api_status(&response.status, &response.message, None).is_ok());
    }

    #[test]
    fn parse_string_no_transactions_result() {
        let json = r#"{
            "status": "0",
            "message": "No transactions found",
            "result": "No transactions found"
        }"#;

        let response: RawEtherscanResponse = serde_json::from_str(json)
            .map_err(|e| format!("{e}"))
            .unwrap_or_else(|e| panic!("{e}"));
        let items = extract_raw_result_items::<EtherscanNormalTx>(
            &response.result,
            &response.message,
            "https://api.etherscan.io/v2/api",
        )
        .map_err(|e| format!("{e}"))
        .unwrap_or_else(|e| panic!("{e}"));
        assert!(items.is_empty());
    }

    #[test]
    fn parse_null_result_pagination_overflow() {
        let json = r#"{
            "status": "0",
            "message": "Result window is too large, PageNo x Offset size must be less than or equal to 10000",
            "result": null
        }"#;

        let response: RawEtherscanResponse = serde_json::from_str(json)
            .map_err(|e| format!("{e}"))
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(response.status, "0");

        let result_message =
            extract_raw_result_message(&response.result).expect("result message extraction");
        let status_result = check_api_status(
            &response.status,
            &response.message,
            result_message.as_deref(),
        );
        assert!(status_result.is_err());

        let extract_result = extract_raw_result_items::<EtherscanNormalTx>(
            &response.result,
            &response.message,
            "https://api.etherscan.io/v2/api",
        );
        assert!(extract_result.is_err());
        let err = extract_result.unwrap_err();
        assert!(
            matches!(err, EtherscanError::ApiError { ref message, .. } if message.contains("Result window is too large")),
            "expected ApiError with pagination message, got: {err}"
        );
    }

    #[test]
    fn check_api_status_error() {
        let result = check_api_status("0", "NOTOK", Some("upstream message"));
        assert!(result.is_err());
    }

    #[test]
    fn resolve_api_error_message_prefers_result() {
        assert_eq!(
            resolve_api_error_message(
                Some("NOTOK"),
                "You are using a deprecated V1 endpoint, switch to Etherscan API V2"
            ),
            "You are using a deprecated V1 endpoint, switch to Etherscan API V2"
        );
    }

    #[test]
    fn resolve_api_error_message_falls_back_to_message_then_default() {
        assert_eq!(
            resolve_api_error_message(Some("NOTOK"), "NOTOK"),
            "NOTOK".to_string()
        );
        assert_eq!(
            resolve_api_error_message(Some("Invalid API Key"), ""),
            "Invalid API Key".to_string()
        );
        assert_eq!(
            resolve_api_error_message(None, ""),
            "Unknown Etherscan API error".to_string()
        );
    }

    #[test]
    fn parse_block_number_accepts_hex_and_decimal() {
        assert_eq!(
            parse_block_number_result("0x10")
                .map_err(|e| e.to_string())
                .unwrap_or_else(|e| panic!("{e}")),
            16_u64
        );
        assert_eq!(
            parse_block_number_result("16")
                .map_err(|e| e.to_string())
                .unwrap_or_else(|e| panic!("{e}")),
            16_u64
        );
    }

    #[test]
    fn parse_native_balance_result_accepts_decimal_wei() {
        let balance = parse_native_balance_result("12345678901234567890")
            .map_err(|e| e.to_string())
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            balance.amount(),
            UnsignedAmount::from_u128(12_345_678_901_234_567_890)
        );
    }

    #[test]
    fn parse_native_balance_result_rejects_non_decimal_values() {
        assert!(parse_native_balance_result("").is_err());
        assert!(parse_native_balance_result("-1").is_err());
        assert!(parse_native_balance_result("0x10").is_err());
        assert!(parse_native_balance_result("12.3").is_err());
    }

    #[test]
    fn native_balance_request_metadata_omits_api_key() {
        let query = EtherscanBalanceQuery::new("0x1111111111111111111111111111111111111111");
        let metadata = query
            .request_metadata("https://api.etherscan.io/v2/api", 1)
            .map_err(|e| format!("{e}"))
            .unwrap_or_else(|e| panic!("{e}"));
        let query_json: serde_json::Value = serde_json::from_str(&metadata.request_query_json)
            .expect("query metadata should parse as JSON");

        assert_eq!(query_json["module"], "account");
        assert_eq!(query_json["action"], "balance");
        assert_eq!(query_json["tag"], "latest");
        assert!(
            !metadata.request_url_without_api_key.contains("apikey"),
            "balance request metadata must not include API key"
        );
    }

    #[test]
    fn transaction_request_metadata_serializes_sort_order() {
        let asc = EtherscanTransactionQuery::new(
            "txlist",
            "0x1111111111111111111111111111111111111111",
            0,
            99,
            EtherscanSortOrder::Asc,
            1,
            1000,
        )
        .request_metadata("https://api.etherscan.io/v2/api", 1)
        .map_err(|e| format!("{e}"))
        .unwrap_or_else(|e| panic!("{e}"));
        let desc = EtherscanTransactionQuery::new(
            "txlist",
            "0x1111111111111111111111111111111111111111",
            0,
            99,
            EtherscanSortOrder::Desc,
            1,
            1000,
        )
        .request_metadata("https://api.etherscan.io/v2/api", 1)
        .map_err(|e| format!("{e}"))
        .unwrap_or_else(|e| panic!("{e}"));
        let asc_query_json: serde_json::Value =
            serde_json::from_str(&asc.request_query_json).expect("asc query JSON should parse");
        let desc_query_json: serde_json::Value =
            serde_json::from_str(&desc.request_query_json).expect("desc query JSON should parse");

        assert_eq!(asc_query_json["sort"], "asc");
        assert_eq!(desc_query_json["sort"], "desc");
        assert!(asc.request_url_without_api_key.contains("sort=asc"));
        assert!(desc.request_url_without_api_key.contains("sort=desc"));
    }

    #[test]
    fn tx_count_estimate_is_exact_when_page_is_below_window_limit() {
        let estimate = tx_count_estimate_from_page_len(9_999);
        assert_eq!(
            estimate,
            TxCountEstimate::Exact(TransactionCount::from_u32(9_999))
        );
    }

    #[test]
    fn tx_count_estimate_is_lower_bound_when_page_hits_window_limit() {
        let estimate = tx_count_estimate_from_page_len(10_000);
        assert_eq!(
            estimate,
            TxCountEstimate::AtLeast(TransactionCount::from_u32(10_000))
        );
    }

    #[test]
    fn response_snippet_short_body() {
        let body = "short";
        assert_eq!(response_body_snippet(body), "short");
    }

    #[test]
    fn response_snippet_truncates_long_body() {
        let body = "x".repeat(RESPONSE_SNIPPET_MAX_BYTES + 100);
        let snippet = response_body_snippet(&body);
        assert!(snippet.ends_with("..."));
        assert_eq!(snippet.len(), RESPONSE_SNIPPET_MAX_BYTES + 3);
    }
}
