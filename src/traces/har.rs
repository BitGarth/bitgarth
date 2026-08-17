//! HAR 1.2 (HTTP Archive) format serde structs.
//!
//! These are custom structs conforming to the HAR 1.2 specification:
//! <https://github.com/ahmadnassri/har-spec/blob/master/versions/1.2.md>
//!
//! Only fields needed for HAR 1.2 compliance are included. Unknown numeric
//! fields use `-1` per the spec.

use serde::Serialize;

/// Top-level HAR document: `{ "log": { ... } }`
#[derive(Debug, Serialize)]
pub(crate) struct HarDocument {
    pub log: HarLog,
}

/// The `log` object containing metadata and entries.
#[derive(Debug, Serialize)]
pub(crate) struct HarLog {
    pub version: &'static str,
    pub creator: HarCreator,
    pub entries: Vec<HarEntry>,
}

/// Creator metadata for the HAR file.
#[derive(Debug, Serialize)]
pub(crate) struct HarCreator {
    pub name: &'static str,
    pub version: &'static str,
}

impl HarCreator {
    pub(crate) fn bitgarth() -> Self {
        Self {
            name: "BitGarth",
            version: env!("CARGO_PKG_VERSION"),
        }
    }
}

/// A single request/response pair.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HarEntry {
    pub started_date_time: String,
    pub time: i32,
    pub request: HarRequest,
    pub response: HarResponse,
    pub cache: HarCache,
    pub timings: HarTimings,
    #[serde(flatten)]
    pub extensions: HarEntryExtensions,
}

/// Non-standard BitGarth HAR extensions for trace diagnostics.
#[derive(Debug, Default, Serialize)]
pub(crate) struct HarEntryExtensions {
    #[serde(
        rename = "_bitgarthFailureStage",
        skip_serializing_if = "Option::is_none"
    )]
    pub failure_stage: Option<String>,
    #[serde(
        rename = "_bitgarthTransportErrorMessage",
        skip_serializing_if = "Option::is_none"
    )]
    pub transport_error_message: Option<String>,
    #[serde(
        rename = "_bitgarthTransportErrorKind",
        skip_serializing_if = "Option::is_none"
    )]
    pub transport_error_kind: Option<String>,
    #[serde(
        rename = "_bitgarthResponseBodyReadError",
        skip_serializing_if = "Option::is_none"
    )]
    pub response_body_read_error: Option<String>,
}

/// HTTP request data.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HarRequest {
    pub method: String,
    pub url: String,
    pub http_version: String,
    pub cookies: Vec<HarCookie>,
    pub headers: Vec<HarHeader>,
    pub query_string: Vec<HarQueryParam>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_data: Option<HarPostData>,
    pub headers_size: i32,
    pub body_size: i64,
}

/// HTTP response data.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HarResponse {
    pub status: u16,
    pub status_text: String,
    pub http_version: String,
    pub cookies: Vec<HarCookie>,
    pub headers: Vec<HarHeader>,
    pub content: HarContent,
    pub redirect_url: String,
    pub headers_size: i32,
    pub body_size: i64,
}

/// A single HTTP header.
#[derive(Debug, Serialize)]
pub(crate) struct HarHeader {
    pub name: String,
    pub value: String,
}

/// Cookie placeholder (empty, for HAR 1.2 compliance).
#[derive(Debug, Serialize)]
pub(crate) struct HarCookie {}

/// A single query string parameter.
#[derive(Debug, Serialize)]
pub(crate) struct HarQueryParam {
    pub name: String,
    pub value: String,
}

/// Request body (POST data).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HarPostData {
    pub mime_type: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoding: Option<String>,
}

/// Response body content.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HarContent {
    pub size: i64,
    pub mime_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoding: Option<String>,
}

/// Cache info (empty object for compliance).
#[derive(Debug, Serialize)]
pub(crate) struct HarCache {}

/// Timing info (all `-1` since we only record timestamps).
#[derive(Debug, Serialize)]
pub(crate) struct HarTimings {
    pub send: i32,
    pub wait: i32,
    pub receive: i32,
}

impl HarTimings {
    pub(crate) fn unknown() -> Self {
        Self {
            send: -1,
            wait: -1,
            receive: -1,
        }
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn test_har_creator_bitgarth() {
        let creator = HarCreator::bitgarth();
        assert_eq!(creator.name, "BitGarth");
        assert!(!creator.version.is_empty());
    }

    #[test]
    fn test_har_timings_unknown() {
        let timings = HarTimings::unknown();
        assert_eq!(timings.send, -1);
        assert_eq!(timings.wait, -1);
        assert_eq!(timings.receive, -1);
    }

    #[test]
    fn test_har_document_serializes_to_valid_json() {
        let doc = HarDocument {
            log: HarLog {
                version: "1.2",
                creator: HarCreator::bitgarth(),
                entries: vec![HarEntry {
                    started_date_time: "2026-02-05T22:22:52.758805+00:00".to_string(),
                    time: -1,
                    request: HarRequest {
                        method: "POST".to_string(),
                        url: "http://127.0.0.1:21325/acquire".to_string(),
                        http_version: "HTTP/1.1".to_string(),
                        cookies: vec![],
                        headers: vec![HarHeader {
                            name: "Content-Type".to_string(),
                            value: "application/json".to_string(),
                        }],
                        query_string: vec![],
                        post_data: Some(HarPostData {
                            mime_type: "application/json".to_string(),
                            text: r#"{"path":"1"}"#.to_string(),
                            encoding: None,
                        }),
                        headers_size: -1,
                        body_size: 12,
                    },
                    response: HarResponse {
                        status: 400,
                        status_text: "Bad Request".to_string(),
                        http_version: "HTTP/1.1".to_string(),
                        cookies: vec![],
                        headers: vec![],
                        content: HarContent {
                            size: 26,
                            mime_type: "application/json".to_string(),
                            text: Some(r#"{"error":"Invalid params"}"#.to_string()),
                            encoding: None,
                        },
                        redirect_url: String::new(),
                        headers_size: -1,
                        body_size: 26,
                    },
                    cache: HarCache {},
                    timings: HarTimings::unknown(),
                    extensions: HarEntryExtensions::default(),
                }],
            },
        };

        let json = serde_json::to_string_pretty(&doc).expect("serialization should succeed");

        // Verify it parses back as valid JSON
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("should be valid JSON");

        // Verify top-level structure
        assert_eq!(parsed["log"]["version"], "1.2");
        assert_eq!(parsed["log"]["creator"]["name"], "BitGarth");
        assert_eq!(parsed["log"]["entries"][0]["request"]["method"], "POST");
        assert_eq!(parsed["log"]["entries"][0]["response"]["status"], 400);
        assert_eq!(
            parsed["log"]["entries"][0]["response"]["statusText"],
            "Bad Request"
        );

        // Verify empty arrays serialize as []
        assert!(
            parsed["log"]["entries"][0]["request"]["cookies"]
                .as_array()
                .expect("should be array")
                .is_empty()
        );

        // Verify optional fields are skipped when None
        assert!(parsed["log"]["entries"][0]["response"]["content"]["encoding"].is_null());
    }

    #[test]
    fn test_post_data_without_encoding_skips_field() {
        let post_data = HarPostData {
            mime_type: "text/plain".to_string(),
            text: "hello".to_string(),
            encoding: None,
        };
        let json = serde_json::to_value(&post_data).expect("serialize");
        assert!(!json.as_object().expect("object").contains_key("encoding"));
    }

    #[test]
    fn test_post_data_with_encoding_includes_field() {
        let post_data = HarPostData {
            mime_type: "application/octet-stream".to_string(),
            text: "AQID".to_string(),
            encoding: Some("base64".to_string()),
        };
        let json = serde_json::to_value(&post_data).expect("serialize");
        assert_eq!(json["encoding"], "base64");
    }
}
