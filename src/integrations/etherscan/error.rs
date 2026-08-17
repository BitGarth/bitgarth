use std::fmt;

const REDACTED_QUERY_VALUE: &str = "***REDACTED***";
const SENSITIVE_QUERY_PARAMS: &[&str] =
    &["apikey", "api_key", "key", "token", "secret", "password"];

/// Errors from the Etherscan API integration.
#[derive(Debug)]
pub(crate) enum EtherscanError {
    /// HTTP transport error (network, timeout, body read failure).
    Http { url: String, error: String },
    /// Non-2xx upstream response.
    UpstreamStatus {
        url: String,
        status: u16,
        body_snippet: String,
    },
    /// Response body could not be deserialized.
    Deserialize { url: String, error: String },
    /// Etherscan API returned status "0" with an error message.
    ApiError { status: String, message: String },
}

impl fmt::Display for EtherscanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EtherscanError::Http { url, error } => {
                write!(
                    f,
                    "Etherscan HTTP error for {}: {error}",
                    sanitize_url_for_logs(url)
                )
            }
            EtherscanError::UpstreamStatus {
                url,
                status,
                body_snippet,
            } => {
                write!(
                    f,
                    "Etherscan returned status {status} for {} (body: {body_snippet})",
                    sanitize_url_for_logs(url)
                )
            }
            EtherscanError::Deserialize { url, error } => {
                write!(
                    f,
                    "Failed to deserialize Etherscan response from {}: {error}",
                    sanitize_url_for_logs(url)
                )
            }
            EtherscanError::ApiError { status, message } => {
                write!(f, "Etherscan API error (status={status}): {message}")
            }
        }
    }
}

impl std::error::Error for EtherscanError {}

impl EtherscanError {
    pub(crate) fn is_rate_limited(&self) -> bool {
        match self {
            EtherscanError::UpstreamStatus { status, .. } => *status == 429,
            EtherscanError::ApiError { status, message } => {
                is_rate_limited_text(status) || is_rate_limited_text(message)
            }
            _ => false,
        }
    }
}

fn is_rate_limited_text(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase();
    normalized.contains("rate limit")
        || normalized.contains("max rate")
        || normalized.contains("too many request")
        || normalized.contains("throttl")
}

fn sanitize_url_for_logs(url: &str) -> String {
    let Some((prefix, query, fragment)) = split_url_query_and_fragment(url) else {
        return url.to_string();
    };

    let redacted_query = query
        .split('&')
        .map(redact_query_segment)
        .collect::<Vec<_>>()
        .join("&");

    match fragment {
        Some(fragment) => format!("{prefix}?{redacted_query}#{fragment}"),
        None => format!("{prefix}?{redacted_query}"),
    }
}

fn split_url_query_and_fragment(url: &str) -> Option<(&str, &str, Option<&str>)> {
    let (prefix, suffix) = url.split_once('?')?;
    let (query, fragment) = match suffix.split_once('#') {
        Some((query, fragment)) => (query, Some(fragment)),
        None => (suffix, None),
    };
    Some((prefix, query, fragment))
}

fn redact_query_segment(segment: &str) -> String {
    if segment.is_empty() {
        return String::new();
    }

    let name = match segment.split_once('=') {
        Some((name, _value)) => name,
        None => segment,
    };

    if !is_sensitive_query_param(name) {
        return segment.to_string();
    }

    format!("{name}={REDACTED_QUERY_VALUE}")
}

fn is_sensitive_query_param(name: &str) -> bool {
    SENSITIVE_QUERY_PARAMS
        .iter()
        .any(|candidate| name.eq_ignore_ascii_case(candidate))
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn display_http_error() {
        let err = EtherscanError::Http {
            url: "https://api.etherscan.io/v2/api?apikey=secret".to_string(),
            error: "connection refused".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Etherscan HTTP error for https://api.etherscan.io/v2/api?apikey=***REDACTED***: connection refused"
        );
    }

    #[test]
    fn display_deserialize_error_redacts_sensitive_query_params() {
        let err = EtherscanError::Deserialize {
            url: "https://api.etherscan.io/v2/api?module=account&ApiKey=secret&token=abc"
                .to_string(),
            error: "invalid type".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Failed to deserialize Etherscan response from https://api.etherscan.io/v2/api?module=account&ApiKey=***REDACTED***&token=***REDACTED***: invalid type"
        );
    }

    #[test]
    fn display_upstream_status_redacts_sensitive_query_params() {
        let err = EtherscanError::UpstreamStatus {
            url: "https://api.etherscan.io/v2/api?apikey=secret&action=txlist".to_string(),
            status: 500,
            body_snippet: "oops".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Etherscan returned status 500 for https://api.etherscan.io/v2/api?apikey=***REDACTED***&action=txlist (body: oops)"
        );
    }

    #[test]
    fn sanitize_url_for_logs_preserves_fragment_and_flag_params() {
        assert_eq!(
            sanitize_url_for_logs(
                "https://api.etherscan.io/v2/api?foo=bar&apikey=secret&debug#frag"
            ),
            "https://api.etherscan.io/v2/api?foo=bar&apikey=***REDACTED***&debug#frag"
        );
    }

    #[test]
    fn is_rate_limited_upstream_429() {
        let err = EtherscanError::UpstreamStatus {
            url: "https://api.etherscan.io/v2/api".to_string(),
            status: 429,
            body_snippet: "".to_string(),
        };
        assert!(err.is_rate_limited());
    }

    #[test]
    fn is_rate_limited_api_message() {
        let err = EtherscanError::ApiError {
            status: "0".to_string(),
            message: "Max rate limit reached".to_string(),
        };
        assert!(err.is_rate_limited());
    }

    #[test]
    fn is_not_rate_limited_normal_error() {
        let err = EtherscanError::Http {
            url: "https://api.etherscan.io/v2/api".to_string(),
            error: "timeout".to_string(),
        };
        assert!(!err.is_rate_limited());
    }
}
