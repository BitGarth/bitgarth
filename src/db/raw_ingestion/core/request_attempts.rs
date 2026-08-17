use crate::db::error::DbError;
use crate::db::user_db::with_user_db_mut;
use crate::models::UserId;
use crate::wallets::DigitalAssetAddressId;
use chrono::{DateTime, Utc};
use rusqlite::params;

use super::ids::{RequestAttemptId, SyncRunId};
use super::shared::{
    CapturedResponseBody, EtherscanQueryJson, EtherscanRequestKind, MempoolPageKind,
    MempoolRequestKind, PageCursor, RequestAttemptOutcome, RequestMethod, RequestOutcomeKind,
    RequestUrl, ResponseHeadersJson,
};
use super::source_connections::IntegrationKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordRequestAttemptRequest {
    pub(crate) sync_run_id: SyncRunId,
    pub(crate) request_kind: MempoolRequestKind,
    pub(crate) request_url: RequestUrl,
    pub(crate) scope_address_id: DigitalAssetAddressId,
    pub(crate) page_cursor: Option<PageCursor>,
    pub(crate) page_kind: MempoolPageKind,
    pub(crate) attempted_at: DateTime<Utc>,
    pub(crate) outcome: RequestAttemptOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RecordedRequestAttempt {
    pub(crate) request_attempt_id: RequestAttemptId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordEtherscanRequestAttemptRequest {
    pub(crate) sync_run_id: SyncRunId,
    pub(crate) request_kind: EtherscanRequestKind,
    pub(crate) request_url: RequestUrl,
    pub(crate) scope_address_id: DigitalAssetAddressId,
    pub(crate) request_query_json: EtherscanQueryJson,
    pub(crate) attempted_at: DateTime<Utc>,
    pub(crate) outcome: RequestAttemptOutcome,
}

pub(crate) fn record_request_attempt(
    user_id: UserId,
    request: RecordRequestAttemptRequest,
) -> Result<RecordedRequestAttempt, DbError> {
    // Guardrail: reject success-path outcomes.
    // Only failure outcomes (TransportError, DeserializeError, non-2xx HttpResponse) are allowed.
    if let RequestAttemptOutcome::HttpResponse(response) = &request.outcome
        && response.http_status_code.is_success_category()
    {
        return Err(DbError::new(
            "request attempts only retain failure diagnostics; success-path HttpResponse not allowed",
        ));
    }
    let request_attempt_id = RequestAttemptId::new();
    let (
        outcome_kind,
        http_status_code,
        response_headers_json,
        response_body_truncated,
        response_body_was_truncated,
        transport_error_message,
    ) = match &request.outcome {
        RequestAttemptOutcome::HttpResponse(response) => (
            RequestOutcomeKind::HttpResponse.as_db_value(),
            Some(response.http_status_code.value()),
            response
                .response_headers_json
                .as_ref()
                .map(ResponseHeadersJson::as_str),
            response
                .response_body
                .as_ref()
                .map(CapturedResponseBody::as_slice),
            response
                .response_body
                .as_ref()
                .map(CapturedResponseBody::was_truncated)
                .unwrap_or(false),
            None,
        ),
        RequestAttemptOutcome::TransportError {
            transport_error_message,
        } => (
            RequestOutcomeKind::TransportError.as_db_value(),
            None,
            None,
            None,
            false,
            Some(transport_error_message.as_str()),
        ),
        RequestAttemptOutcome::DeserializeError(response) => (
            RequestOutcomeKind::DeserializeError.as_db_value(),
            Some(response.http_status_code.value()),
            response
                .response_headers_json
                .as_ref()
                .map(ResponseHeadersJson::as_str),
            response
                .response_body
                .as_ref()
                .map(CapturedResponseBody::as_slice),
            response
                .response_body
                .as_ref()
                .map(CapturedResponseBody::was_truncated)
                .unwrap_or(false),
            None,
        ),
    };
    with_user_db_mut(user_id, |conn| {
        conn.execute(
            "INSERT INTO request_attempts
             (id, sync_run_id, integration, request_kind, request_url, request_method, scope_address_id, page_cursor, page_kind, attempted_at, outcome_kind, http_status_code, response_headers_json, response_body_truncated, response_body_was_truncated, transport_error_message, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            params![
                request_attempt_id.to_string(),
                request.sync_run_id.to_string(),
                IntegrationKind::Mempool.as_db_value(),
                request.request_kind.as_db_value(),
                request.request_url.as_str(),
                RequestMethod::Get.as_db_value(),
                request.scope_address_id.to_string(),
                request.page_cursor.as_ref().map(PageCursor::as_str),
                request.page_kind.as_db_value(),
                request.attempted_at.to_rfc3339(),
                outcome_kind,
                http_status_code,
                response_headers_json,
                response_body_truncated,
                if response_body_was_truncated { 1_i64 } else { 0_i64 },
                transport_error_message,
                request.attempted_at.to_rfc3339(),
            ],
        )
        .map_err(|err| DbError::from_rusqlite_error("Failed to insert request attempt", err))?;
        Ok(RecordedRequestAttempt { request_attempt_id })
    })
}

pub(crate) fn record_etherscan_request_attempt(
    user_id: UserId,
    request: RecordEtherscanRequestAttemptRequest,
) -> Result<RecordedRequestAttempt, DbError> {
    // Guardrail: reject success-path outcomes.
    // Only failure outcomes (TransportError, DeserializeError, non-2xx HttpResponse) are allowed.
    if let RequestAttemptOutcome::HttpResponse(response) = &request.outcome
        && response.http_status_code.is_success_category()
    {
        return Err(DbError::new(
            "request attempts only retain failure diagnostics; success-path HttpResponse not allowed",
        ));
    }
    let request_attempt_id = RequestAttemptId::new();
    let (
        outcome_kind,
        http_status_code,
        response_headers_json,
        response_body_truncated,
        response_body_was_truncated,
        transport_error_message,
    ) = match &request.outcome {
        RequestAttemptOutcome::HttpResponse(response) => (
            RequestOutcomeKind::HttpResponse.as_db_value(),
            Some(response.http_status_code.value()),
            response
                .response_headers_json
                .as_ref()
                .map(ResponseHeadersJson::as_str),
            response
                .response_body
                .as_ref()
                .map(CapturedResponseBody::as_slice),
            response
                .response_body
                .as_ref()
                .map(CapturedResponseBody::was_truncated)
                .unwrap_or(false),
            None,
        ),
        RequestAttemptOutcome::TransportError {
            transport_error_message,
        } => (
            RequestOutcomeKind::TransportError.as_db_value(),
            None,
            None,
            None,
            false,
            Some(transport_error_message.as_str()),
        ),
        RequestAttemptOutcome::DeserializeError(response) => (
            RequestOutcomeKind::DeserializeError.as_db_value(),
            Some(response.http_status_code.value()),
            response
                .response_headers_json
                .as_ref()
                .map(ResponseHeadersJson::as_str),
            response
                .response_body
                .as_ref()
                .map(CapturedResponseBody::as_slice),
            response
                .response_body
                .as_ref()
                .map(CapturedResponseBody::was_truncated)
                .unwrap_or(false),
            None,
        ),
    };
    with_user_db_mut(user_id, |conn| {
        conn.execute(
            "INSERT INTO request_attempts
             (id, sync_run_id, integration, request_kind, request_url, request_method, scope_address_id, request_query_json, page_cursor, page_kind, attempted_at, outcome_kind, http_status_code, response_headers_json, response_body_truncated, response_body_was_truncated, transport_error_message, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![
                request_attempt_id.to_string(),
                request.sync_run_id.to_string(),
                IntegrationKind::Etherscan.as_db_value(),
                request.request_kind.as_db_value(),
                request.request_url.as_str(),
                RequestMethod::Get.as_db_value(),
                request.scope_address_id.to_string(),
                request.request_query_json.as_str(),
                Option::<String>::None,
                Option::<String>::None,
                request.attempted_at.to_rfc3339(),
                outcome_kind,
                http_status_code,
                response_headers_json,
                response_body_truncated,
                if response_body_was_truncated { 1_i64 } else { 0_i64 },
                transport_error_message,
                request.attempted_at.to_rfc3339(),
            ],
        )
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to insert etherscan request attempt", err)
        })?;
        Ok(RecordedRequestAttempt { request_attempt_id })
    })
}
