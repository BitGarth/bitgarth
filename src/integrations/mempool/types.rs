use serde::Deserialize;

/// Transaction confirmation status from the Mempool API.
#[derive(Debug, Deserialize)]
pub(crate) struct MempoolTransactionStatus {
    pub(crate) confirmed: bool,
    #[serde(default)]
    pub(crate) block_height: Option<i64>,
    #[serde(default)]
    pub(crate) block_hash: Option<String>,
    #[serde(default)]
    pub(crate) block_time: Option<i64>,
}

/// Previous output referenced by an input.
#[derive(Debug, Deserialize)]
pub(crate) struct MempoolPrevOut {
    #[serde(default)]
    pub(crate) scriptpubkey_address: Option<String>,
    pub(crate) value: i64,
}

/// Transaction input.
#[derive(Debug, Deserialize)]
pub(crate) struct MempoolInput {
    #[serde(default)]
    pub(crate) txid: Option<String>,
    #[serde(default)]
    pub(crate) vout: Option<i64>,
    #[serde(default)]
    pub(crate) prevout: Option<MempoolPrevOut>,
}

/// Transaction output.
#[derive(Debug, Deserialize)]
pub(crate) struct MempoolOutput {
    pub(crate) scriptpubkey: String,
    #[serde(default)]
    pub(crate) scriptpubkey_address: Option<String>,
    pub(crate) value: i64,
}

/// Full address transaction from the Mempool `/api/address/{address}/txs` endpoint.
#[derive(Debug, Deserialize)]
pub(crate) struct MempoolAddressTransaction {
    pub(crate) txid: String,
    #[serde(default)]
    pub(crate) vin: Vec<MempoolInput>,
    #[serde(default)]
    pub(crate) vout: Vec<MempoolOutput>,
    #[serde(default)]
    pub(crate) fee: Option<i64>,
    pub(crate) status: MempoolTransactionStatus,
}
