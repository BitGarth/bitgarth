use crate::db::error::DbError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::convert::TryFrom;
use url::Url;

pub(crate) enum SyncRunScopeKind {
    Address,
}

impl SyncRunScopeKind {
    pub(crate) fn as_db_value(&self) -> &'static str {
        match self {
            Self::Address => "address",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyncRunTriggerKind {
    Scheduled,
    Manual,
    Backfill,
}

impl SyncRunTriggerKind {
    pub(crate) fn as_db_value(&self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::Manual => "manual",
            Self::Backfill => "backfill",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyncRunStatus {
    Started,
    CompletedSuccess,
    CompletedFailure,
}

impl SyncRunStatus {
    pub(crate) fn as_db_value(&self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::CompletedSuccess => "completed_success",
            Self::CompletedFailure => "completed_failure",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MempoolRequestKind {
    AddressTransactionsFirstPage,
    AddressTransactionsAfterConfirmed,
}

impl MempoolRequestKind {
    pub(crate) fn as_db_value(&self) -> &'static str {
        match self {
            Self::AddressTransactionsFirstPage => "mempool_address_transactions_first_page",
            Self::AddressTransactionsAfterConfirmed => {
                "mempool_address_transactions_after_confirmed"
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum MempoolPageKind {
    FirstPage,
    PaginatedAfterConfirmed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EtherscanRequestKind {
    NativeBalance,
    NormalTransactionsPage,
    InternalTransactionsPage,
}

impl EtherscanRequestKind {
    pub(crate) fn as_db_value(&self) -> &'static str {
        match self {
            Self::NativeBalance => "etherscan_native_balance",
            Self::NormalTransactionsPage => "etherscan_normal_transactions_page",
            Self::InternalTransactionsPage => "etherscan_internal_transactions_page",
        }
    }
}

impl MempoolPageKind {
    pub(crate) fn as_db_value(&self) -> &'static str {
        match self {
            Self::FirstPage => "first_page",
            Self::PaginatedAfterConfirmed => "paginated_after_confirmed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestMethod {
    Get,
}

impl RequestMethod {
    pub(crate) fn as_db_value(&self) -> &'static str {
        match self {
            Self::Get => "GET",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestOutcomeKind {
    HttpResponse,
    TransportError,
    DeserializeError,
}

impl RequestOutcomeKind {
    pub(crate) fn as_db_value(&self) -> &'static str {
        match self {
            Self::HttpResponse => "http_response",
            Self::TransportError => "transport_error",
            Self::DeserializeError => "deserialize_error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpaqueJsonText(String);

impl OpaqueJsonText {
    pub(crate) fn parse(raw: String) -> Result<Self, DbError> {
        if raw.trim().is_empty() {
            return Err(DbError::new("summary_json cannot be empty"));
        }
        Ok(Self(raw))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RequestUrl(Url);

impl RequestUrl {
    pub(crate) fn parse(raw: &str) -> Result<Self, DbError> {
        Url::parse(raw)
            .map(Self)
            .map_err(|err| DbError::new(format!("Invalid request URL: {err}")))
    }

    pub(crate) fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EtherscanQueryJson(String);

impl EtherscanQueryJson {
    pub(crate) fn parse(raw: String) -> Result<Self, DbError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(DbError::new("etherscan request query json cannot be empty"));
        }
        let parsed: Value = serde_json::from_str(trimmed).map_err(|err| {
            DbError::new(format!(
                "etherscan request query json must be valid JSON: {err}"
            ))
        })?;
        let object = parsed
            .as_object()
            .ok_or_else(|| DbError::new("etherscan request query json must be a JSON object"))?;
        if object.is_empty() {
            return Err(DbError::new(
                "etherscan request query json must not be an empty object",
            ));
        }
        if object.contains_key("apikey") {
            return Err(DbError::new(
                "etherscan request query json must not contain apikey",
            ));
        }
        Ok(Self(trimmed.to_string()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EtherscanChainId(i64);

impl EtherscanChainId {
    pub(crate) fn try_new(value: u64) -> Result<Self, DbError> {
        let parsed = i64::try_from(value)
            .map_err(|_| DbError::new(format!("etherscan chain id exceeds i64 range: {value}")))?;
        if parsed <= 0 {
            return Err(DbError::new(format!(
                "etherscan chain id must be positive, got {value}"
            )));
        }
        Ok(Self(parsed))
    }

    pub(crate) fn value(self) -> i64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EtherscanTraceId(String);

impl EtherscanTraceId {
    pub(crate) fn parse(raw: &str) -> Result<Self, DbError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(DbError::new("etherscan trace id cannot be empty"));
        }
        Ok(Self(trimmed.to_string()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PageCursor(String);

impl PageCursor {
    pub(crate) fn parse(raw: &str) -> Result<Self, DbError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(DbError::new("page cursor cannot be empty"));
        }
        Ok(Self(trimmed.to_string()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HttpStatusCode(u16);

impl HttpStatusCode {
    pub(crate) fn try_new(value: u16) -> Result<Self, DbError> {
        if !(100..=599).contains(&value) {
            return Err(DbError::new(format!(
                "HTTP status code must be between 100 and 599, got {value}"
            )));
        }
        Ok(Self(value))
    }

    pub(crate) fn is_success_category(self) -> bool {
        (200..=299).contains(&self.0)
    }

    pub(crate) fn value(self) -> i64 {
        i64::from(self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResponseHeadersJson(String);

impl ResponseHeadersJson {
    pub(crate) fn parse(raw: String) -> Result<Self, DbError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(DbError::new("response headers json cannot be empty"));
        }
        let parsed: Value = serde_json::from_str(trimmed).map_err(|err| {
            DbError::new(format!("response headers json must be valid JSON: {err}"))
        })?;
        if !parsed.is_object() {
            return Err(DbError::new("response headers json must be a JSON object"));
        }
        Ok(Self(trimmed.to_string()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransportErrorMessage(String);

impl TransportErrorMessage {
    pub(crate) fn parse(raw: String) -> Result<Self, DbError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(DbError::new("transport error message cannot be empty"));
        }
        Ok(Self(trimmed.to_string()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CapturedResponseBody {
    bytes: Vec<u8>,
    was_truncated: bool,
}

impl CapturedResponseBody {
    pub(crate) fn truncate(bytes: Vec<u8>, max_bytes: usize) -> Option<Self> {
        if bytes.is_empty() {
            return None;
        }
        let was_truncated = bytes.len() > max_bytes;
        let bytes = if was_truncated {
            bytes[..max_bytes].to_vec()
        } else {
            bytes
        };
        Some(Self {
            bytes,
            was_truncated,
        })
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn was_truncated(&self) -> bool {
        self.was_truncated
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RequestAttemptHttpResponse {
    pub(crate) http_status_code: HttpStatusCode,
    pub(crate) response_headers_json: Option<ResponseHeadersJson>,
    pub(crate) response_body: Option<CapturedResponseBody>,
}

impl RequestAttemptHttpResponse {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RequestAttemptOutcome {
    HttpResponse(RequestAttemptHttpResponse),
    TransportError {
        transport_error_message: TransportErrorMessage,
    },
    DeserializeError(RequestAttemptHttpResponse),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RawVersionWriteOutcome {
    InsertedNewHead,
    ReusedCurrentHead,
}
