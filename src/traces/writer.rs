//! Trace file writer — pure functions for path/filename generation and
//! the side-effecting `write_trace` function for filesystem I/O.

use super::har::{
    HarCache, HarContent, HarCreator, HarDocument, HarEntry, HarEntryExtensions, HarHeader, HarLog,
    HarPostData, HarRequest, HarResponse, HarTimings,
};
use chrono::{DateTime, Datelike, Timelike, Utc};
use std::path::{Path, PathBuf};

/// All data needed to write a trace (passed from middleware to writer).
pub(crate) struct TraceData {
    pub label: String,
    pub user_traces_dir: PathBuf,
    pub req_timestamp: DateTime<Utc>,
    pub req_method: String,
    pub req_url: String,
    pub req_headers: Vec<(String, String)>,
    pub req_body: Option<Vec<u8>>,
    pub rsp_timestamp: DateTime<Utc>,
    pub rsp_status: u16,
    pub rsp_status_text: String,
    pub rsp_headers: Vec<(String, String)>,
    pub rsp_body: Option<Vec<u8>>,
    pub failure: Option<TraceFailureMetadata>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TraceFailureStage {
    SendFailed,
    ResponseBodyReadFailed,
}

impl TraceFailureStage {
    fn as_har_value(self) -> &'static str {
        match self {
            Self::SendFailed => "send_failed",
            Self::ResponseBodyReadFailed => "response_body_read_failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TraceFailureMetadata {
    pub stage: TraceFailureStage,
    pub message: String,
    pub kind: Option<String>,
}

// ============ Constants ============

/// Header names to redact (compared case-insensitively).
const REDACTED_HEADERS: &[&str] = &[
    "authorization",
    "cookie",
    "set-cookie",
    "x-api-key",
    "proxy-authorization",
];

const REDACTED_VALUE: &str = "***REDACTED***";

// ============ Pure Functions ============

/// Format a timestamp as a filesystem-safe request ID with microsecond precision.
///
/// Example: `"20260205T222252.758805Z"`
pub(crate) fn format_request_id(timestamp: &DateTime<Utc>) -> String {
    timestamp.format("%Y%m%dT%H%M%S%.6fZ").to_string()
}

/// Build the trace directory path.
///
/// `{user_traces_dir}/{year}/{month}/{day}/{hour}/`
pub(crate) fn build_trace_dir(user_traces_dir: &Path, timestamp: &DateTime<Utc>) -> PathBuf {
    user_traces_dir
        .join(format!("{:04}", timestamp.year()))
        .join(format!("{:02}", timestamp.month()))
        .join(format!("{:02}", timestamp.day()))
        .join(format!("{:02}", timestamp.hour()))
}

/// Sanitize a URL path for use in filenames.
///
/// Strips leading slash, replaces `/` with `-`, removes non-filesystem-safe characters.
/// Returns `"root"` for empty or root paths.
pub(crate) fn sanitize_url_path(url_path: &str) -> String {
    let trimmed = url_path.strip_prefix('/').unwrap_or(url_path);
    if trimmed.is_empty() {
        return "root".to_string();
    }
    trimmed
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Build the filename prefix: `{requestId}-{label}-{METHOD}-{path}-{outcome}`
pub(crate) fn build_filename_prefix(
    request_id: &str,
    label: &str,
    method: &str,
    url_path: &str,
    outcome: &str,
) -> String {
    let sanitized_path = sanitize_url_path(url_path);
    format!(
        "{}-{}-{}-{}-{}",
        request_id,
        label,
        method,
        sanitized_path,
        sanitize_filename_component(outcome)
    )
}

fn sanitize_filename_component(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();

    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

fn filename_outcome_suffix(rsp_status: u16, failure: Option<&TraceFailureMetadata>) -> String {
    if rsp_status != 0 {
        return rsp_status.to_string();
    }

    failure
        .and_then(|metadata| metadata.kind.as_deref())
        .filter(|kind| !kind.is_empty())
        .map(sanitize_filename_component)
        .unwrap_or_else(|| "unknown".to_string())
}

/// Redact sensitive headers, returning a new vec with values replaced.
pub(crate) fn redact_headers(headers: &[(String, String)]) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| {
            let should_redact = REDACTED_HEADERS
                .iter()
                .any(|h| h.eq_ignore_ascii_case(name));
            if should_redact {
                (name.clone(), REDACTED_VALUE.to_string())
            } else {
                (name.clone(), value.clone())
            }
        })
        .collect()
}

/// Extract the path component from a full URL string.
pub(crate) fn extract_url_path(url: &str) -> String {
    url::Url::parse(url)
        .map(|u| u.path().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Find a header value by name (case-insensitive).
pub(crate) fn find_header_value<'a>(
    headers: &'a [(String, String)],
    name: &str,
) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// Build a complete HAR document from trace data components.
///
/// Headers are redacted in the HAR output. Body text is included as-is
/// (or base64-encoded for non-UTF-8 content).
pub(crate) struct HarDocumentInput<'a> {
    pub req_timestamp: &'a DateTime<Utc>,
    pub req_method: &'a str,
    pub req_url: &'a str,
    pub req_headers: &'a [(String, String)],
    pub req_body: Option<&'a [u8]>,
    pub rsp_timestamp: &'a DateTime<Utc>,
    pub rsp_status: u16,
    pub rsp_status_text: &'a str,
    pub rsp_headers: &'a [(String, String)],
    pub rsp_body: Option<&'a [u8]>,
    pub failure: Option<&'a TraceFailureMetadata>,
}

pub(crate) fn build_har_document(input: HarDocumentInput<'_>) -> HarDocument {
    let elapsed_ms = (*input.rsp_timestamp - *input.req_timestamp).num_milliseconds() as i32;

    let redacted_req_headers = redact_headers(input.req_headers);
    let redacted_rsp_headers = redact_headers(input.rsp_headers);

    let req_content_type = find_header_value(input.req_headers, "content-type");
    let rsp_content_type = find_header_value(input.rsp_headers, "content-type");

    // Build request post_data
    let post_data = input.req_body.and_then(|body| {
        if body.is_empty() {
            return None;
        }
        let mime_type = req_content_type
            .unwrap_or("application/octet-stream")
            .to_string();
        let (text, encoding) = body_to_text_and_encoding(body);
        Some(HarPostData {
            mime_type,
            text,
            encoding,
        })
    });

    // Build response content
    let (rsp_text, rsp_encoding) = match input.rsp_body {
        Some(body) if !body.is_empty() => {
            let (text, encoding) = body_to_text_and_encoding(body);
            (Some(text), encoding)
        }
        _ => (None, None),
    };

    let rsp_body_size = input.rsp_body.map(|b| b.len() as i64).unwrap_or(0);
    let req_body_size = input.req_body.map(|b| b.len() as i64).unwrap_or(0);
    let extensions = match input.failure {
        Some(failure) => HarEntryExtensions {
            failure_stage: Some(failure.stage.as_har_value().to_string()),
            transport_error_message: match failure.stage {
                TraceFailureStage::SendFailed => Some(failure.message.clone()),
                TraceFailureStage::ResponseBodyReadFailed => None,
            },
            transport_error_kind: failure.kind.clone(),
            response_body_read_error: match failure.stage {
                TraceFailureStage::SendFailed => None,
                TraceFailureStage::ResponseBodyReadFailed => Some(failure.message.clone()),
            },
        },
        None => HarEntryExtensions::default(),
    };

    // Parse query string from URL
    let query_string = url::Url::parse(input.req_url)
        .map(|u| {
            u.query_pairs()
                .map(|(k, v)| super::har::HarQueryParam {
                    name: k.into_owned(),
                    value: v.into_owned(),
                })
                .collect()
        })
        .unwrap_or_default();

    HarDocument {
        log: HarLog {
            version: "1.2",
            creator: HarCreator::bitgarth(),
            entries: vec![HarEntry {
                started_date_time: input
                    .req_timestamp
                    .to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
                time: elapsed_ms,
                request: HarRequest {
                    method: input.req_method.to_string(),
                    url: input.req_url.to_string(),
                    http_version: "HTTP/1.1".to_string(),
                    cookies: vec![],
                    headers: redacted_req_headers
                        .into_iter()
                        .map(|(name, value)| HarHeader { name, value })
                        .collect(),
                    query_string,
                    post_data,
                    headers_size: -1,
                    body_size: req_body_size,
                },
                response: HarResponse {
                    status: input.rsp_status,
                    status_text: input.rsp_status_text.to_string(),
                    http_version: "HTTP/1.1".to_string(),
                    cookies: vec![],
                    headers: redacted_rsp_headers
                        .into_iter()
                        .map(|(name, value)| HarHeader { name, value })
                        .collect(),
                    content: HarContent {
                        size: rsp_body_size,
                        mime_type: rsp_content_type
                            .unwrap_or("application/octet-stream")
                            .to_string(),
                        text: rsp_text,
                        encoding: rsp_encoding,
                    },
                    redirect_url: String::new(),
                    headers_size: -1,
                    body_size: rsp_body_size,
                },
                cache: HarCache {},
                timings: HarTimings::unknown(),
                extensions,
            }],
        },
    }
}

/// Convert a body byte slice to a (text, optional encoding) pair.
///
/// If the body is valid UTF-8, returns it as-is with no encoding.
/// Otherwise, returns base64 with `encoding: "base64"`.
fn body_to_text_and_encoding(body: &[u8]) -> (String, Option<String>) {
    match std::str::from_utf8(body) {
        Ok(text) => (text.to_string(), None),
        Err(_) => {
            use base64::Engine;
            let encoded = base64::engine::general_purpose::STANDARD.encode(body);
            (encoded, Some("base64".to_string()))
        }
    }
}

// ============ Side-Effecting Function ============

/// Write one HAR trace file to the filesystem.
///
/// Returns `Err` on I/O failure. Callers should catch and log, never propagate
/// to the HTTP request flow.
pub(crate) fn write_trace(data: TraceData) -> Result<(), std::io::Error> {
    let request_id = format_request_id(&data.req_timestamp);
    let url_path = extract_url_path(&data.req_url);
    let trace_dir = build_trace_dir(&data.user_traces_dir, &data.req_timestamp);
    let outcome = filename_outcome_suffix(data.rsp_status, data.failure.as_ref());
    let prefix = build_filename_prefix(
        &request_id,
        &data.label,
        &data.req_method,
        &url_path,
        &outcome,
    );

    std::fs::create_dir_all(&trace_dir)?;

    // Write HAR file
    let har = build_har_document(HarDocumentInput {
        req_timestamp: &data.req_timestamp,
        req_method: &data.req_method,
        req_url: &data.req_url,
        req_headers: &data.req_headers,
        req_body: data.req_body.as_deref(),
        rsp_timestamp: &data.rsp_timestamp,
        rsp_status: data.rsp_status,
        rsp_status_text: &data.rsp_status_text,
        rsp_headers: &data.rsp_headers,
        rsp_body: data.rsp_body.as_deref(),
        failure: data.failure.as_ref(),
    });
    let har_json = serde_json::to_string_pretty(&har).map_err(std::io::Error::other)?;
    std::fs::write(trace_dir.join(format!("{prefix}.har")), har_json)?;

    Ok(())
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use crate::models::UserId;
    use chrono::TimeZone;

    #[test]
    fn test_format_request_id() {
        let ts = Utc
            .with_ymd_and_hms(2026, 2, 5, 22, 22, 52)
            .unwrap()
            .with_nanosecond(758_805_000)
            .expect("valid nanoseconds");
        assert_eq!(format_request_id(&ts), "20260205T222252.758805Z");
    }

    #[test]
    fn test_format_request_id_zero_microseconds() {
        let ts = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(format_request_id(&ts), "20260101T000000.000000Z");
    }

    #[test]
    fn test_build_trace_dir() {
        let ts = Utc.with_ymd_and_hms(2026, 2, 5, 22, 0, 0).unwrap();
        let user_id = UserId::new();
        let base_dir = PathBuf::from(format!("/data/users/{}/traces", user_id));
        let dir = build_trace_dir(&base_dir, &ts);
        let expected = base_dir.join("2026").join("02").join("05").join("22");
        assert_eq!(dir, expected);
    }

    #[test]
    fn test_build_trace_dir_single_digit_month() {
        let ts = Utc.with_ymd_and_hms(2026, 1, 3, 9, 0, 0).unwrap();
        let user_id = UserId::new();
        let base_dir = PathBuf::from(format!("/data/users/{}/traces", user_id));
        let dir = build_trace_dir(&base_dir, &ts);
        // Month, day, and hour should be zero-padded
        let expected = base_dir.join("2026").join("01").join("03").join("09");
        assert_eq!(dir, expected);
    }

    #[test]
    fn test_sanitize_url_path_simple() {
        assert_eq!(sanitize_url_path("/acquire"), "acquire");
    }

    #[test]
    fn test_sanitize_url_path_multi_segment() {
        assert_eq!(sanitize_url_path("/call/123456"), "call-123456");
    }

    #[test]
    fn test_sanitize_url_path_root() {
        assert_eq!(sanitize_url_path("/"), "root");
    }

    #[test]
    fn test_sanitize_url_path_empty() {
        assert_eq!(sanitize_url_path(""), "root");
    }

    #[test]
    fn test_sanitize_url_path_many_segments() {
        assert_eq!(sanitize_url_path("/acquire/1/null"), "acquire-1-null");
    }

    #[test]
    fn test_build_filename_prefix() {
        let prefix = build_filename_prefix(
            "20260205T222252.758805Z",
            "trezor-bridge",
            "POST",
            "/acquire",
            "200",
        );
        assert_eq!(
            prefix,
            "20260205T222252.758805Z-trezor-bridge-POST-acquire-200"
        );
    }

    #[test]
    fn test_filename_outcome_suffix_prefers_status_code() {
        assert_eq!(filename_outcome_suffix(200, None), "200");
        assert_eq!(
            filename_outcome_suffix(
                200,
                Some(&TraceFailureMetadata {
                    stage: TraceFailureStage::ResponseBodyReadFailed,
                    message: "decoder error".to_string(),
                    kind: Some("body".to_string()),
                })
            ),
            "200"
        );
    }

    #[test]
    fn test_filename_outcome_suffix_uses_failure_kind_without_status() {
        assert_eq!(
            filename_outcome_suffix(
                0,
                Some(&TraceFailureMetadata {
                    stage: TraceFailureStage::SendFailed,
                    message: "connection timed out".to_string(),
                    kind: Some("timeout".to_string()),
                })
            ),
            "timeout"
        );
        assert_eq!(
            filename_outcome_suffix(
                0,
                Some(&TraceFailureMetadata {
                    stage: TraceFailureStage::SendFailed,
                    message: "mystery failure".to_string(),
                    kind: None,
                })
            ),
            "unknown"
        );
    }

    #[test]
    fn test_redact_headers_sensitive() {
        let headers = vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            (
                "Authorization".to_string(),
                "Bearer secret-token".to_string(),
            ),
            ("cookie".to_string(), "session=abc123".to_string()),
            ("X-Custom".to_string(), "value".to_string()),
            ("SET-COOKIE".to_string(), "new=cookie".to_string()),
            ("X-Api-Key".to_string(), "my-key".to_string()),
            ("Proxy-Authorization".to_string(), "Basic cred".to_string()),
        ];
        let redacted = redact_headers(&headers);
        assert_eq!(redacted[0].1, "application/json");
        assert_eq!(redacted[1].1, REDACTED_VALUE);
        assert_eq!(redacted[2].1, REDACTED_VALUE);
        assert_eq!(redacted[3].1, "value");
        assert_eq!(redacted[4].1, REDACTED_VALUE);
        assert_eq!(redacted[5].1, REDACTED_VALUE);
        assert_eq!(redacted[6].1, REDACTED_VALUE);
    }

    #[test]
    fn test_redact_headers_preserves_names() {
        let headers = vec![("Authorization".to_string(), "secret".to_string())];
        let redacted = redact_headers(&headers);
        assert_eq!(redacted[0].0, "Authorization");
    }

    #[test]
    fn test_extract_url_path() {
        assert_eq!(
            extract_url_path("http://127.0.0.1:21325/acquire"),
            "/acquire"
        );
        assert_eq!(
            extract_url_path("http://127.0.0.1:21325/call/abc"),
            "/call/abc"
        );
    }

    #[test]
    fn test_extract_url_path_invalid() {
        assert_eq!(extract_url_path("not-a-url"), "unknown");
    }

    #[test]
    fn test_find_header_value_case_insensitive() {
        let headers = vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            ("X-Custom".to_string(), "value".to_string()),
        ];
        assert_eq!(
            find_header_value(&headers, "content-type"),
            Some("application/json")
        );
        assert_eq!(
            find_header_value(&headers, "Content-Type"),
            Some("application/json")
        );
        assert_eq!(
            find_header_value(&headers, "CONTENT-TYPE"),
            Some("application/json")
        );
    }

    #[test]
    fn test_find_header_value_missing() {
        let headers = vec![("X-Custom".to_string(), "value".to_string())];
        assert_eq!(find_header_value(&headers, "Authorization"), None);
    }

    #[test]
    fn test_body_to_text_and_encoding_utf8() {
        let (text, encoding) = body_to_text_and_encoding(b"hello world");
        assert_eq!(text, "hello world");
        assert!(encoding.is_none());
    }

    #[test]
    fn test_body_to_text_and_encoding_binary() {
        let binary = &[0xFF, 0xFE, 0x00, 0x01];
        let (text, encoding) = body_to_text_and_encoding(binary);
        assert_eq!(encoding, Some("base64".to_string()));
        // Verify it's valid base64
        use base64::Engine;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&text)
            .expect("valid base64");
        assert_eq!(decoded, binary);
    }

    #[test]
    fn test_build_har_document_redacts_headers() {
        let req_headers = vec![
            ("Authorization".to_string(), "Bearer secret".to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
        ];
        let rsp_headers = vec![("Set-Cookie".to_string(), "session=abc".to_string())];
        let ts = Utc.with_ymd_and_hms(2026, 2, 5, 22, 0, 0).unwrap();

        let har = build_har_document(HarDocumentInput {
            req_timestamp: &ts,
            req_method: "POST",
            req_url: "http://localhost/test",
            req_headers: &req_headers,
            req_body: None,
            rsp_timestamp: &ts,
            rsp_status: 200,
            rsp_status_text: "OK",
            rsp_headers: &rsp_headers,
            rsp_body: None,
            failure: None,
        });

        let entry = &har.log.entries[0];
        // Authorization should be redacted
        let auth_header = entry
            .request
            .headers
            .iter()
            .find(|h| h.name == "Authorization")
            .expect("header should exist");
        assert_eq!(auth_header.value, REDACTED_VALUE);

        // Content-Type should NOT be redacted
        let ct_header = entry
            .request
            .headers
            .iter()
            .find(|h| h.name == "Content-Type")
            .expect("header should exist");
        assert_eq!(ct_header.value, "application/json");

        // Set-Cookie should be redacted
        let sc_header = entry
            .response
            .headers
            .iter()
            .find(|h| h.name == "Set-Cookie")
            .expect("header should exist");
        assert_eq!(sc_header.value, REDACTED_VALUE);
    }

    #[test]
    fn test_build_har_document_records_send_failure_metadata() {
        let ts = Utc.with_ymd_and_hms(2026, 2, 5, 22, 0, 0).unwrap();

        let har = build_har_document(HarDocumentInput {
            req_timestamp: &ts,
            req_method: "GET",
            req_url: "http://localhost/test?apikey=***REDACTED***",
            req_headers: &[("Authorization".to_string(), "Bearer secret".to_string())],
            req_body: None,
            rsp_timestamp: &ts,
            rsp_status: 0,
            rsp_status_text: "No Response",
            rsp_headers: &[],
            rsp_body: None,
            failure: Some(&TraceFailureMetadata {
                stage: TraceFailureStage::SendFailed,
                message: "send_failed: connection refused".to_string(),
                kind: Some("connect".to_string()),
            }),
        });

        let entry = &har.log.entries[0];
        assert_eq!(entry.response.status, 0);
        assert_eq!(entry.response.status_text, "No Response");
        assert_eq!(
            entry.extensions.failure_stage.as_deref(),
            Some("send_failed")
        );
        assert_eq!(
            entry.extensions.transport_error_message.as_deref(),
            Some("send_failed: connection refused")
        );
        assert_eq!(
            entry.extensions.transport_error_kind.as_deref(),
            Some("connect")
        );
        assert_eq!(entry.extensions.response_body_read_error, None);
    }

    #[test]
    fn test_write_trace_creates_files() {
        let tmp_dir =
            std::env::temp_dir().join(format!("bitgarth_trace_test_{}", ulid::Ulid::new()));
        let user_id = UserId::new();
        let label = "test-service".to_string();
        let ts = Utc.with_ymd_and_hms(2026, 3, 15, 14, 30, 0).unwrap();
        let user_traces_dir = tmp_dir.join(format!("users/{}/traces", user_id));

        let data = TraceData {
            label,
            user_traces_dir: user_traces_dir.clone(),
            req_timestamp: ts,
            req_method: "POST".to_string(),
            req_url: "http://127.0.0.1:21325/acquire".to_string(),
            req_headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            req_body: Some(br#"{"path":"1"}"#.to_vec()),
            rsp_timestamp: ts,
            rsp_status: 400,
            rsp_status_text: "Bad Request".to_string(),
            rsp_headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            rsp_body: Some(br#"{"error":"Invalid params"}"#.to_vec()),
            failure: None,
        };

        write_trace(data).expect("write_trace should succeed");

        let trace_dir = user_traces_dir.join("2026/03/15/14");
        assert!(trace_dir.exists(), "trace directory should exist");

        // Check that exactly 1 HAR file was created
        let files: Vec<_> = std::fs::read_dir(&trace_dir)
            .expect("read dir")
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(files.len(), 1, "should have only the HAR file");

        // Find the .har file
        let har_file = files
            .iter()
            .find(|f| f.path().extension().is_some_and(|e| e == "har"))
            .expect(".har file should exist");
        assert_eq!(
            har_file.file_name().to_str(),
            Some("20260315T143000.000000Z-test-service-POST-acquire-400.har")
        );
        let har_content = std::fs::read_to_string(har_file.path()).expect("read har");
        let har_json: serde_json::Value = serde_json::from_str(&har_content).expect("valid JSON");
        assert_eq!(har_json["log"]["version"], "1.2");
        assert_eq!(har_json["log"]["entries"][0]["response"]["status"], 400);
        assert_eq!(
            har_json["log"]["entries"][0]["request"]["postData"]["text"],
            "{\"path\":\"1\"}"
        );
        assert_eq!(
            har_json["log"]["entries"][0]["response"]["content"]["text"],
            "{\"error\":\"Invalid params\"}"
        );

        // Cleanup
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_write_trace_no_body_files_when_empty() {
        let tmp_dir =
            std::env::temp_dir().join(format!("bitgarth_trace_test_empty_{}", ulid::Ulid::new()));
        let user_id = UserId::new();
        let label = "test-service".to_string();
        let ts = Utc.with_ymd_and_hms(2026, 3, 15, 14, 30, 0).unwrap();
        let user_traces_dir = tmp_dir.join(format!("users/{}/traces", user_id));

        let data = TraceData {
            label,
            user_traces_dir: user_traces_dir.clone(),
            req_timestamp: ts,
            req_method: "POST".to_string(),
            req_url: "http://127.0.0.1:21325/enumerate".to_string(),
            req_headers: vec![],
            req_body: None,
            rsp_timestamp: ts,
            rsp_status: 200,
            rsp_status_text: "OK".to_string(),
            rsp_headers: vec![],
            rsp_body: None,
            failure: None,
        };

        write_trace(data).expect("write_trace should succeed");

        let trace_dir = user_traces_dir.join("2026/03/15/14");

        // Should only have the .har file
        let files: Vec<_> = std::fs::read_dir(&trace_dir)
            .expect("read dir")
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(files.len(), 1, "should have only the .har file");
        assert!(files[0].path().extension().is_some_and(|e| e == "har"));
        assert_eq!(
            files[0].file_name().to_str(),
            Some("20260315T143000.000000Z-test-service-POST-enumerate-200.har")
        );

        // Cleanup
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_write_trace_records_response_body_read_failure_in_har() {
        let tmp_dir =
            std::env::temp_dir().join(format!("bitgarth_trace_test_rsp_err_{}", ulid::Ulid::new()));
        let user_id = UserId::new();
        let label = "test-service".to_string();
        let ts = Utc.with_ymd_and_hms(2026, 3, 15, 14, 30, 0).unwrap();
        let user_traces_dir = tmp_dir.join(format!("users/{}/traces", user_id));

        let data = TraceData {
            label,
            user_traces_dir: user_traces_dir.clone(),
            req_timestamp: ts,
            req_method: "GET".to_string(),
            req_url: "http://127.0.0.1:21325/status".to_string(),
            req_headers: vec![],
            req_body: None,
            rsp_timestamp: ts,
            rsp_status: 200,
            rsp_status_text: "OK".to_string(),
            rsp_headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            rsp_body: None,
            failure: Some(TraceFailureMetadata {
                stage: TraceFailureStage::ResponseBodyReadFailed,
                message: "decoder error".to_string(),
                kind: Some("body".to_string()),
            }),
        };

        write_trace(data).expect("write_trace should succeed");

        let trace_dir = user_traces_dir.join("2026/03/15/14");
        let files: Vec<_> = std::fs::read_dir(&trace_dir)
            .expect("read dir")
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(files.len(), 1, "should have only the HAR file");

        let har_file = files
            .iter()
            .find(|f| f.path().extension().is_some_and(|e| e == "har"))
            .expect(".har file should exist");
        let har_content = std::fs::read_to_string(har_file.path()).expect("read har");
        let har_json: serde_json::Value = serde_json::from_str(&har_content).expect("valid JSON");
        assert_eq!(
            har_json["log"]["entries"][0]["_bitgarthFailureStage"],
            "response_body_read_failed"
        );
        assert_eq!(
            har_json["log"]["entries"][0]["_bitgarthResponseBodyReadError"],
            "decoder error"
        );
        assert_eq!(
            har_json["log"]["entries"][0]["_bitgarthTransportErrorKind"],
            "body"
        );
        assert_eq!(
            har_file.file_name().to_str(),
            Some("20260315T143000.000000Z-test-service-GET-status-200.har")
        );

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_write_trace_uses_failure_kind_when_status_is_unavailable() {
        let tmp_dir = std::env::temp_dir().join(format!(
            "bitgarth_trace_test_send_err_{}",
            ulid::Ulid::new()
        ));
        let user_id = UserId::new();
        let label = "test-service".to_string();
        let ts = Utc.with_ymd_and_hms(2026, 3, 15, 14, 30, 0).unwrap();
        let user_traces_dir = tmp_dir.join(format!("users/{}/traces", user_id));

        let data = TraceData {
            label,
            user_traces_dir: user_traces_dir.clone(),
            req_timestamp: ts,
            req_method: "GET".to_string(),
            req_url: "http://127.0.0.1:21325/status".to_string(),
            req_headers: vec![],
            req_body: None,
            rsp_timestamp: ts,
            rsp_status: 0,
            rsp_status_text: "No Response".to_string(),
            rsp_headers: vec![],
            rsp_body: None,
            failure: Some(TraceFailureMetadata {
                stage: TraceFailureStage::SendFailed,
                message: "connection timed out".to_string(),
                kind: Some("timeout".to_string()),
            }),
        };

        write_trace(data).expect("write_trace should succeed");

        let trace_dir = user_traces_dir.join("2026/03/15/14");
        let files: Vec<_> = std::fs::read_dir(&trace_dir)
            .expect("read dir")
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(files.len(), 1, "should have only the HAR file");
        assert_eq!(
            files[0].file_name().to_str(),
            Some("20260315T143000.000000Z-test-service-GET-status-timeout.har")
        );

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }
}
