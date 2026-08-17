use std::fmt;
use std::time::Duration;

/// Errors from the Mempool API integration.
#[derive(Debug)]
pub(crate) enum MempoolError {
    /// HTTP transport error (network, timeout, body read failure).
    Http { url: String, error: String },
    /// Non-2xx upstream response.
    UpstreamStatus {
        url: String,
        status: u16,
        response_headers_json: Option<String>,
        response_body: Vec<u8>,
    },
    /// HTTP 429 response.
    RateLimited {
        url: String,
        retry_after: Option<Duration>,
        response_headers_json: Option<String>,
        response_body: Vec<u8>,
    },
    /// Response body could not be deserialized.
    Deserialize {
        url: String,
        error: String,
        http_status_code: Option<u16>,
        response_headers_json: Option<String>,
        response_body: Option<Vec<u8>>,
    },
    /// Base URL could not be joined with a path.
    UrlJoin(String),
}

impl fmt::Display for MempoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MempoolError::Http { error, .. } => write!(f, "Mempool HTTP error: {error}"),
            MempoolError::UpstreamStatus {
                url,
                status,
                response_body,
                ..
            } => write!(
                f,
                "Mempool returned status {status} for {url} (body: {})",
                response_body_snippet(response_body)
            ),
            MempoolError::RateLimited {
                url, retry_after, ..
            } => {
                if let Some(retry_after) = retry_after {
                    write!(
                        f,
                        "Mempool rate limited (HTTP 429) for {url} (retry_after={}s)",
                        retry_after.as_secs()
                    )
                } else {
                    write!(f, "Mempool rate limited (HTTP 429) for {url}")
                }
            }
            MempoolError::Deserialize { url, error, .. } => {
                write!(
                    f,
                    "Failed to deserialize Mempool response from {url}: {error}"
                )
            }
            MempoolError::UrlJoin(err) => write!(f, "Mempool URL join error: {err}"),
        }
    }
}

impl std::error::Error for MempoolError {}

fn response_body_snippet(response_body: &[u8]) -> String {
    const RESPONSE_SNIPPET_MAX_BYTES: usize = 8 * 1024;

    let body = String::from_utf8_lossy(response_body);
    if body.len() <= RESPONSE_SNIPPET_MAX_BYTES {
        body.to_string()
    } else {
        let mut snippet = body[..RESPONSE_SNIPPET_MAX_BYTES].to_string();
        snippet.push_str("...");
        snippet
    }
}
