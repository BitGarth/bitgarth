use crate::db::error::DbError;
use crate::db::user_db::with_user_db_mut;
use crate::models::UserId;
use crate::transactions::TxHash;
use chrono::{DateTime, Utc};
use rusqlite::params;
use serde_json::Value;
use ulid::Ulid;

use super::ids::{
    RawEtherscanInternalTransactionVersionId, RawEtherscanNormalTransactionVersionId,
    RawMempoolTransactionVersionId, SyncRunId,
};
use super::shared::EtherscanTraceId;
use super::source_connections::IntegrationKind;

pub(crate) enum RawObjectKind {
    Mempool,
    EtherscanNormal,
    EtherscanInternal,
}

impl RawObjectKind {
    fn as_db_value(&self) -> &'static str {
        match self {
            Self::Mempool => "mempool_transaction",
            Self::EtherscanNormal => "etherscan_normal_transaction",
            Self::EtherscanInternal => "etherscan_internal_transaction",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RawObjectKeyJson(String);

impl RawObjectKeyJson {
    fn parse(raw: String) -> Result<Self, DbError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(DbError::new("raw object key json cannot be empty"));
        }
        let parsed: Value = serde_json::from_str(trimmed).map_err(|err| {
            DbError::new(format!("raw object key json must be valid JSON: {err}"))
        })?;
        if !parsed.is_object() {
            return Err(DbError::new("raw object key json must be a JSON object"));
        }
        Ok(Self(trimmed.to_string()))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RawObjectKey {
    Mempool {
        txid: TxHash,
    },
    EtherscanNormal {
        tx_hash: TxHash,
    },
    EtherscanInternal {
        tx_hash: TxHash,
        trace_id: EtherscanTraceId,
    },
}

impl RawObjectKey {
    fn kind(&self) -> RawObjectKind {
        match self {
            Self::Mempool { .. } => RawObjectKind::Mempool,
            Self::EtherscanNormal { .. } => RawObjectKind::EtherscanNormal,
            Self::EtherscanInternal { .. } => RawObjectKind::EtherscanInternal,
        }
    }

    fn to_json(&self) -> Result<RawObjectKeyJson, DbError> {
        match self {
            Self::Mempool { txid } => {
                RawObjectKeyJson::parse(format!(r#"{{"txid":"{}"}}"#, txid.as_str()))
            }
            Self::EtherscanNormal { tx_hash } => {
                RawObjectKeyJson::parse(format!(r#"{{"tx_hash":"{}"}}"#, tx_hash.as_str()))
            }
            Self::EtherscanInternal { tx_hash, trace_id } => RawObjectKeyJson::parse(format!(
                r#"{{"tx_hash":"{}","trace_id":"{}"}}"#,
                tx_hash.as_str(),
                trace_id.as_str()
            )),
        }
    }

    pub(crate) fn to_json_string(&self) -> Result<String, DbError> {
        Ok(self.to_json()?.as_str().to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RawParserKind {
    Mempool,
    EtherscanNormal,
    EtherscanInternal,
}

impl RawParserKind {
    fn as_db_value(self) -> &'static str {
        match self {
            Self::Mempool => "mempool_transaction_to_sync_record",
            Self::EtherscanNormal => "etherscan_normal_transaction_to_sync_record",
            Self::EtherscanInternal => "etherscan_internal_transaction_to_sync_record",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RawParseAttemptStatus {
    Failure,
}

impl RawParseAttemptStatus {
    fn as_db_value(self) -> &'static str {
        match self {
            Self::Failure => "failure",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParserVersion(String);

impl ParserVersion {
    pub(crate) fn parse(raw: &str) -> Result<Self, DbError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(DbError::new("parser version cannot be empty"));
        }
        Ok(Self(trimmed.to_string()))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParseFailureMessage(String);

impl ParseFailureMessage {
    pub(crate) fn parse(raw: String) -> Result<Self, DbError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(DbError::new("parse failure message cannot be empty"));
        }
        Ok(Self(trimmed.to_string()))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

pub(crate) struct RecordRawParseAttemptRequest {
    pub(crate) sync_run_id: SyncRunId,
    pub(crate) integration: IntegrationKind,
    pub(crate) raw_object_key: RawObjectKey,
    pub(crate) raw_version_id: RawVersionId,
    pub(crate) parser_kind: RawParserKind,
    pub(crate) parser_version: ParserVersion,
    pub(crate) status: RawParseAttemptStatus,
    pub(crate) error_message: Option<ParseFailureMessage>,
    pub(crate) attempted_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RawVersionId {
    Mempool(RawMempoolTransactionVersionId),
    EtherscanNormal(RawEtherscanNormalTransactionVersionId),
    EtherscanInternal(RawEtherscanInternalTransactionVersionId),
}

impl RawVersionId {
    fn as_string(self) -> String {
        match self {
            Self::Mempool(id) => id.to_string(),
            Self::EtherscanNormal(id) => id.to_string(),
            Self::EtherscanInternal(id) => id.to_string(),
        }
    }
}
pub(crate) fn record_raw_parse_attempt(
    user_id: UserId,
    request: RecordRawParseAttemptRequest,
) -> Result<(), DbError> {
    if !matches!(request.status, RawParseAttemptStatus::Failure) {
        return Err(DbError::new(
            "raw parse attempts only retain failure diagnostics",
        ));
    }
    if request.error_message.is_none() {
        return Err(DbError::new(
            "failed raw parse attempt must include an error message",
        ));
    }

    let parse_attempt_id = Ulid::new().to_string();
    let raw_object_key_json = request.raw_object_key.to_json()?;
    with_user_db_mut(user_id, |conn| {
        conn.execute(
                "INSERT INTO raw_parse_attempts
             (id, sync_run_id, integration, raw_object_kind, raw_object_key_json, raw_version_id, parser_kind, parser_version, status, error_message, attempted_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                parse_attempt_id,
                request.sync_run_id.to_string(),
                request.integration.as_db_value(),
                request.raw_object_key.kind().as_db_value(),
                raw_object_key_json.as_str(),
                request.raw_version_id.as_string(),
                request.parser_kind.as_db_value(),
                request.parser_version.as_str(),
                request.status.as_db_value(),
                request.error_message.as_ref().map(ParseFailureMessage::as_str),
                request.attempted_at.to_rfc3339(),
                request.attempted_at.to_rfc3339(),
            ],
        )
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to insert raw parse attempt", err)
        })?;
        Ok(())
    })
}
