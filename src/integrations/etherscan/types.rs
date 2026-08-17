use serde::Deserialize;

/// Normal transaction from `txlist` endpoint.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EtherscanNormalTx {
    pub hash: String,
    pub block_number: String,
    pub time_stamp: String,
    pub from: String,
    pub to: String,
    pub value: String,
    pub gas_price: String,
    pub gas_used: String,
    #[serde(default)]
    pub is_error: String,
    #[serde(default)]
    pub txreceipt_status: String,
    #[serde(default)]
    pub nonce: String,
}

/// Internal transaction from `txlistinternal` endpoint.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EtherscanInternalTx {
    pub hash: String,
    pub block_number: String,
    pub time_stamp: String,
    pub from: String,
    pub to: String,
    pub value: String,
    #[serde(default)]
    pub is_error: String,
    #[serde(default, rename = "type")]
    pub call_type: String,
    #[serde(default, rename = "traceId")]
    pub trace_id: String,
}
