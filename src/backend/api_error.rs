use crate::models::FieldErrors;
use dioxus::fullstack::{AsStatusCode, StatusCode};
use dioxus::prelude::ServerFnError;
use serde::{Deserialize, Serialize};

pub(crate) const DEFAULT_INTERNAL_ERROR_MESSAGE: &str = "Internal server error";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ApiErrorCode {
    BadRequest,
    Unauthorized,
    Forbidden,
    Validation,
    Conflict,
    NotFound,
    TooManyRequests,
    Internal,
}

impl ApiErrorCode {
    pub(crate) fn status_code(self) -> StatusCode {
        match self {
            ApiErrorCode::BadRequest => StatusCode::BAD_REQUEST,
            ApiErrorCode::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiErrorCode::Forbidden => StatusCode::FORBIDDEN,
            ApiErrorCode::Validation => StatusCode::UNPROCESSABLE_ENTITY,
            ApiErrorCode::Conflict => StatusCode::CONFLICT,
            ApiErrorCode::NotFound => StatusCode::NOT_FOUND,
            ApiErrorCode::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
            ApiErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ApiErrorEnvelope {
    pub(crate) code: ApiErrorCode,
    pub(crate) message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) field_errors: Option<FieldErrors>,
}

impl ApiErrorEnvelope {
    pub(crate) fn is_unauthorized(&self) -> bool {
        self.code == ApiErrorCode::Unauthorized
    }

    pub(crate) fn is_validation(&self) -> bool {
        self.code == ApiErrorCode::Validation
    }

    pub(crate) fn is_conflict(&self) -> bool {
        self.code == ApiErrorCode::Conflict
    }

    pub(crate) fn is_not_found(&self) -> bool {
        self.code == ApiErrorCode::NotFound
    }

    pub(crate) fn is_internal(&self) -> bool {
        self.code == ApiErrorCode::Internal
    }

    pub(crate) fn first_field_error(&self, field: &str) -> Option<&String> {
        self.field_errors
            .as_ref()
            .and_then(|errors| errors.first(field))
    }

    pub(crate) fn field_errors(&self) -> Option<&FieldErrors> {
        self.field_errors.as_ref()
    }

    pub(crate) fn bad_request(message: impl Into<String>) -> Self {
        Self {
            code: ApiErrorCode::BadRequest,
            message: message.into(),
            field_errors: None,
        }
    }

    pub(crate) fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            code: ApiErrorCode::Unauthorized,
            message: message.into(),
            field_errors: None,
        }
    }

    pub(crate) fn forbidden(message: impl Into<String>) -> Self {
        Self {
            code: ApiErrorCode::Forbidden,
            message: message.into(),
            field_errors: None,
        }
    }

    #[cfg(feature = "server")]
    pub(crate) fn unauthorized_with_errors(
        message: impl Into<String>,
        field_errors: FieldErrors,
    ) -> Self {
        Self {
            code: ApiErrorCode::Unauthorized,
            message: message.into(),
            field_errors: Some(field_errors),
        }
    }

    pub(crate) fn validation(message: impl Into<String>, field_errors: FieldErrors) -> Self {
        Self {
            code: ApiErrorCode::Validation,
            message: message.into(),
            field_errors: Some(field_errors),
        }
    }

    pub(crate) fn conflict(message: impl Into<String>, field_errors: FieldErrors) -> Self {
        Self {
            code: ApiErrorCode::Conflict,
            message: message.into(),
            field_errors: Some(field_errors),
        }
    }

    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self {
            code: ApiErrorCode::NotFound,
            message: message.into(),
            field_errors: None,
        }
    }

    /// A transient, retryable failure (e.g. an upstream provider rate-limit).
    /// The message round-trips to the client, so phrase it for the user.
    pub(crate) fn too_many_requests(message: impl Into<String>) -> Self {
        Self {
            code: ApiErrorCode::TooManyRequests,
            message: message.into(),
            field_errors: None,
        }
    }

    pub(crate) fn internal() -> Self {
        Self {
            code: ApiErrorCode::Internal,
            message: DEFAULT_INTERNAL_ERROR_MESSAGE.to_string(),
            field_errors: None,
        }
    }
}

impl std::fmt::Display for ApiErrorEnvelope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ApiErrorEnvelope {}

impl AsStatusCode for ApiErrorEnvelope {
    fn as_status_code(&self) -> StatusCode {
        self.code.status_code()
    }
}

impl From<ServerFnError> for ApiErrorEnvelope {
    fn from(value: ServerFnError) -> Self {
        match value {
            ServerFnError::Args(message)
            | ServerFnError::MissingArg(message)
            | ServerFnError::Deserialization(message)
            | ServerFnError::Serialization(message) => Self::bad_request(message),
            ServerFnError::ServerError {
                message, code: 400, ..
            } => Self::bad_request(message),
            ServerFnError::ServerError {
                message, code: 401, ..
            } => Self::unauthorized(message),
            ServerFnError::ServerError {
                message, code: 403, ..
            } => Self::forbidden(message),
            ServerFnError::ServerError {
                message, code: 404, ..
            } => Self::not_found(message),
            ServerFnError::ServerError {
                message, code: 409, ..
            } => {
                let mut errors = FieldErrors::new();
                errors.add("conflict", message.clone());
                Self::conflict(message, errors)
            }
            ServerFnError::ServerError {
                message, code: 422, ..
            } => {
                let mut errors = FieldErrors::new();
                errors.add("request", message.clone());
                Self::validation(message, errors)
            }
            ServerFnError::ServerError {
                message, code: 429, ..
            } => Self::too_many_requests(message),
            _ => Self::internal(),
        }
    }
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    fn has_field_error(error: &ApiErrorEnvelope, field: &str) -> bool {
        error
            .field_errors
            .as_ref()
            .and_then(|fields| fields.get(field))
            .is_some()
    }

    #[test]
    fn api_error_code_maps_to_expected_statuses() {
        assert_eq!(
            ApiErrorCode::BadRequest.status_code(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            ApiErrorCode::Unauthorized.status_code(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(ApiErrorCode::Forbidden.status_code(), StatusCode::FORBIDDEN);
        assert_eq!(
            ApiErrorCode::Validation.status_code(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(ApiErrorCode::Conflict.status_code(), StatusCode::CONFLICT);
        assert_eq!(ApiErrorCode::NotFound.status_code(), StatusCode::NOT_FOUND);
        assert_eq!(
            ApiErrorCode::Internal.status_code(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn internal_error_uses_safe_default_message() {
        let error = ApiErrorEnvelope::internal();
        assert_eq!(error.code, ApiErrorCode::Internal);
        assert_eq!(error.message, DEFAULT_INTERNAL_ERROR_MESSAGE);
        assert_eq!(error.field_errors, None);
    }

    #[test]
    fn server_fn_bad_request_maps_to_bad_request_envelope() {
        let error = ApiErrorEnvelope::from(ServerFnError::Args("bad input".to_string()));
        assert_eq!(error.code, ApiErrorCode::BadRequest);
        assert_eq!(error.as_status_code(), StatusCode::BAD_REQUEST);
        assert_eq!(error.message, "bad input");
        assert_eq!(error.field_errors, None);
    }

    #[test]
    fn server_fn_conflict_maps_to_conflict_with_field_errors() {
        let error = ApiErrorEnvelope::from(ServerFnError::ServerError {
            message: "duplicate wallet label".to_string(),
            code: 409,
            details: None,
        });
        assert_eq!(error.code, ApiErrorCode::Conflict);
        assert_eq!(error.as_status_code(), StatusCode::CONFLICT);
        assert!(has_field_error(&error, "conflict"));
    }

    #[test]
    fn server_fn_forbidden_maps_to_forbidden_envelope() {
        let error = ApiErrorEnvelope::from(ServerFnError::ServerError {
            message: "sync control is disabled".to_string(),
            code: 403,
            details: None,
        });

        assert_eq!(error.code, ApiErrorCode::Forbidden);
        assert_eq!(error.as_status_code(), StatusCode::FORBIDDEN);
        assert_eq!(error.message, "sync control is disabled");
        assert_eq!(error.field_errors, None);
    }

    #[test]
    fn server_fn_too_many_requests_preserves_message() {
        let error = ApiErrorEnvelope::from(ServerFnError::ServerError {
            message: "CoinGecko is rate-limiting requests. Wait a moment and try again."
                .to_string(),
            code: 429,
            details: None,
        });
        assert_eq!(error.code, ApiErrorCode::TooManyRequests);
        assert_eq!(error.as_status_code(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            error.message,
            "CoinGecko is rate-limiting requests. Wait a moment and try again."
        );
        assert!(!error.is_internal());
    }

    #[test]
    fn unknown_server_fn_errors_map_to_safe_internal_message() {
        let error = ApiErrorEnvelope::from(ServerFnError::ServerError {
            message: "raw db failure details".to_string(),
            code: 500,
            details: None,
        });
        assert_eq!(error.code, ApiErrorCode::Internal);
        assert_eq!(error.as_status_code(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(error.message, DEFAULT_INTERNAL_ERROR_MESSAGE);
        assert_eq!(error.field_errors, None);
    }
}
