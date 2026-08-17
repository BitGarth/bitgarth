//! Traced HTTP clients for integrations.
//!
//! Server code should use traced clients for outgoing HTTP requests.
//! When `BGTRACES=fs` is set, requests/responses are captured in HAR 1.2
//! trace files. When tracing is disabled, calls delegate directly to reqwest.

use super::writer::{
    self, TraceData, TraceFailureMetadata, TraceFailureStage as WriterTraceFailureStage,
};
use crate::models::UserId;
use crate::project_paths::get_user_traces_dir;
use chrono::Utc;
use dioxus::logger::tracing;
use serde_json::Value;
use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;

const REDACTED_VALUE: &str = "***REDACTED***";

const DEFAULT_REDACTED_QUERY_PARAMS: &[&str] =
    &["apikey", "api_key", "key", "token", "secret", "password"];

const DEFAULT_REDACTED_HEADERS: &[&str] = &[
    "authorization",
    "cookie",
    "set-cookie",
    "x-api-key",
    "x-cg-demo-api-key",
    "x-cg-pro-api-key",
];

/// Label identifying an HTTP integration for tracing purposes.
///
/// This is an open type - new integrations supply any string label
/// without modifying this module.
#[derive(Debug, Clone)]
pub(crate) struct IntegrationLabel(Cow<'static, str>);

impl IntegrationLabel {
    pub(crate) fn new(label: impl Into<Cow<'static, str>>) -> Self {
        Self(label.into())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for IntegrationLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Errors from constructing a traced client.
#[derive(Debug)]
pub(crate) enum TracedClientError {
    ClientBuild(String),
}

impl fmt::Display for TracedClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClientBuild(msg) => write!(f, "failed to build HTTP client: {msg}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransportFailureStage {
    SendFailed,
    ResponseBodyReadFailed,
}

impl TransportFailureStage {
    fn as_str(self) -> &'static str {
        match self {
            Self::SendFailed => "send_failed",
            Self::ResponseBodyReadFailed => "response_body_read_failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransportErrorKind {
    Timeout,
    Connect,
    Request,
    Body,
    Decode,
    Tls,
    Unknown,
}

impl TransportErrorKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Connect => "connect",
            Self::Request => "request",
            Self::Body => "body",
            Self::Decode => "decode",
            Self::Tls => "tls",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReqwestErrorFlags {
    timeout: bool,
    connect: bool,
    request: bool,
    body: bool,
    decode: bool,
}

fn classify_transport_error(flags: ReqwestErrorFlags, message: &str) -> TransportErrorKind {
    if flags.timeout {
        return TransportErrorKind::Timeout;
    }
    if flags.connect {
        return TransportErrorKind::Connect;
    }
    if flags.body {
        return TransportErrorKind::Body;
    }
    if flags.decode {
        return TransportErrorKind::Decode;
    }

    let message = message.to_ascii_lowercase();
    if message.contains("tls")
        || message.contains("ssl")
        || message.contains("certificate")
        || message.contains("handshake")
    {
        return TransportErrorKind::Tls;
    }

    if flags.request {
        TransportErrorKind::Request
    } else {
        TransportErrorKind::Unknown
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransportFailure {
    stage: TransportFailureStage,
    message: String,
    kind: TransportErrorKind,
}

impl TransportFailure {
    pub(crate) fn new(
        stage: TransportFailureStage,
        message: impl Into<String>,
        kind: TransportErrorKind,
    ) -> Self {
        Self {
            stage,
            message: message.into(),
            kind,
        }
    }

    pub(crate) fn from_reqwest_error(stage: TransportFailureStage, error: &reqwest::Error) -> Self {
        let message = error.to_string();
        let kind = classify_transport_error(
            ReqwestErrorFlags {
                timeout: error.is_timeout(),
                connect: error.is_connect(),
                request: error.is_request(),
                body: error.is_body(),
                decode: error.is_decode(),
            },
            &message,
        );

        Self::new(stage, message, kind)
    }

    pub(crate) fn persistence_message(&self) -> String {
        format!("{}: {}", self.stage.as_str(), self.message)
    }

    fn to_trace_failure_metadata(&self) -> TraceFailureMetadata {
        TraceFailureMetadata {
            stage: match self.stage {
                TransportFailureStage::SendFailed => WriterTraceFailureStage::SendFailed,
                TransportFailureStage::ResponseBodyReadFailed => {
                    WriterTraceFailureStage::ResponseBodyReadFailed
                }
            },
            message: self.message.clone(),
            kind: Some(self.kind.as_str().to_string()),
        }
    }
}

// ============ Shared redaction ============

fn normalize_redaction_name(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

fn push_redaction_name(seen: &mut HashSet<String>, output: &mut Vec<String>, name: &str) {
    let normalized = normalize_redaction_name(name);
    if normalized.is_empty() {
        return;
    }

    if seen.insert(normalized.clone()) {
        output.push(normalized);
    }
}

fn split_url_query_and_fragment(url: &str) -> Option<(&str, &str, Option<&str>)> {
    let (prefix, query_and_fragment) = url.split_once('?')?;
    let (query, fragment) = match query_and_fragment.split_once('#') {
        Some((query, fragment)) => (query, Some(fragment)),
        None => (query_and_fragment, None),
    };

    Some((prefix, query, fragment))
}

/// Built-in query parameter names that are always redacted (case-insensitive).
fn default_redacted_query_params() -> &'static [&'static str] {
    DEFAULT_REDACTED_QUERY_PARAMS
}

/// Built-in header names that are always redacted (case-insensitive).
fn default_redacted_headers() -> &'static [&'static str] {
    DEFAULT_REDACTED_HEADERS
}

/// Merge built-in and custom redaction names using case-insensitive deduplication.
fn merge_redaction_names(defaults: &[&str], custom: &[String]) -> Vec<String> {
    let mut merged = Vec::new();
    let mut seen = HashSet::new();

    for name in defaults {
        push_redaction_name(&mut seen, &mut merged, name);
    }

    for name in custom {
        push_redaction_name(&mut seen, &mut merged, name);
    }

    merged
}

fn redaction_name_set(names: &[String]) -> HashSet<String> {
    names
        .iter()
        .map(|name| normalize_redaction_name(name))
        .collect()
}

/// Redact matching query parameter values in a URL string.
///
/// Matching is case-insensitive. If the URL has no query string, the input URL
/// is returned unchanged.
fn redact_url(url: &str, params_to_redact: &[String]) -> String {
    let names = redaction_name_set(params_to_redact);
    if names.is_empty() {
        return url.to_string();
    }

    let Some((prefix, query, fragment)) = split_url_query_and_fragment(url) else {
        return url.to_string();
    };

    let redacted_query = query
        .split('&')
        .map(|segment| {
            if segment.is_empty() {
                return String::new();
            }

            match segment.split_once('=') {
                Some((name, value)) => {
                    if names.contains(&name.to_ascii_lowercase()) {
                        format!("{name}={REDACTED_VALUE}")
                    } else {
                        format!("{name}={value}")
                    }
                }
                None => {
                    if names.contains(&segment.to_ascii_lowercase()) {
                        format!("{segment}={REDACTED_VALUE}")
                    } else {
                        segment.to_string()
                    }
                }
            }
        })
        .collect::<Vec<_>>()
        .join("&");

    match fragment {
        Some(fragment) => format!("{prefix}?{redacted_query}#{fragment}"),
        None => format!("{prefix}?{redacted_query}"),
    }
}

/// Redact matching header values.
///
/// Matching is case-insensitive and preserves the original header casing.
fn redact_headers(
    headers: &[(String, String)],
    headers_to_redact: &[String],
) -> Vec<(String, String)> {
    let names = redaction_name_set(headers_to_redact);

    headers
        .iter()
        .map(|(name, value)| {
            if names.contains(&name.to_ascii_lowercase()) {
                (name.clone(), REDACTED_VALUE.to_string())
            } else {
                (name.clone(), value.clone())
            }
        })
        .collect()
}

// ============ Shared trace recording ============

#[allow(clippy::too_many_arguments)]
fn record_trace(
    user_id: UserId,
    label: &IntegrationLabel,
    req_method: String,
    req_url: String,
    req_headers: Vec<(String, String)>,
    req_body: Option<Vec<u8>>,
    rsp_status: u16,
    rsp_status_text: String,
    rsp_headers: Vec<(String, String)>,
    rsp_body: Option<Vec<u8>>,
    failure: Option<TraceFailureMetadata>,
    req_timestamp: chrono::DateTime<Utc>,
    rsp_timestamp: chrono::DateTime<Utc>,
) {
    let user_traces_dir = match get_user_traces_dir(user_id) {
        Ok(dir) => dir,
        Err(err) => {
            tracing::warn!("Failed to resolve user traces dir: {err}");
            return;
        }
    };

    let trace_data = TraceData {
        label: label.as_str().to_string(),
        user_traces_dir,
        req_timestamp,
        req_method,
        req_url,
        req_headers,
        req_body,
        rsp_timestamp,
        rsp_status,
        rsp_status_text,
        rsp_headers,
        rsp_body,
        failure,
    };

    if let Err(err) = writer::write_trace(trace_data) {
        tracing::warn!("Failed to write HTTP trace: {err}");
    }
}

#[derive(Clone)]
struct RequestTraceSnapshot {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
    timestamp: chrono::DateTime<Utc>,
}

#[derive(Clone)]
struct ResponseTraceSnapshot {
    status: reqwest::StatusCode,
    status_text: String,
    url: String,
    headers: Vec<(String, String)>,
    timestamp: chrono::DateTime<Utc>,
}

fn record_trace_snapshot(
    user_id: UserId,
    label: &IntegrationLabel,
    request: &RequestTraceSnapshot,
    response: &ResponseTraceSnapshot,
    rsp_body: Option<Vec<u8>>,
    failure: Option<TraceFailureMetadata>,
) {
    record_trace(
        user_id,
        label,
        request.method.clone(),
        request.url.clone(),
        request.headers.clone(),
        request.body.clone(),
        response.status.as_u16(),
        response.status_text.clone(),
        response.headers.clone(),
        rsp_body,
        failure,
        request.timestamp,
        response.timestamp,
    );
}

fn collect_headers(headers: &reqwest::header::HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| {
            (
                name.to_string(),
                value.to_str().unwrap_or("[non-utf8]").to_string(),
            )
        })
        .collect()
}

fn status_text(status: reqwest::StatusCode) -> String {
    status.canonical_reason().unwrap_or("Unknown").to_string()
}

fn redact_json_value(value: &mut Value, fields_to_redact: &HashSet<String>) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if fields_to_redact.contains(&key.to_ascii_lowercase()) {
                    *value = Value::String(REDACTED_VALUE.to_string());
                } else {
                    redact_json_value(value, fields_to_redact);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                redact_json_value(value, fields_to_redact);
            }
        }
        _ => {}
    }
}

fn redact_json_body(bytes: &[u8], fields_to_redact: &[String]) -> Vec<u8> {
    let fields = redaction_name_set(fields_to_redact);
    if fields.is_empty() {
        return bytes.to_vec();
    }

    let Ok(mut value) = serde_json::from_slice::<Value>(bytes) else {
        return bytes.to_vec();
    };

    redact_json_value(&mut value, &fields);
    serde_json::to_vec(&value).unwrap_or_else(|_| bytes.to_vec())
}

fn redacted_body_option(bytes: &[u8], fields_to_redact: &[String]) -> Option<Vec<u8>> {
    if bytes.is_empty() {
        None
    } else {
        Some(redact_json_body(bytes, fields_to_redact))
    }
}

fn send_failed_trace_failure(error: &reqwest::Error) -> TraceFailureMetadata {
    TransportFailure::from_reqwest_error(TransportFailureStage::SendFailed, error)
        .to_trace_failure_metadata()
}

fn response_body_read_failed_trace_failure(error: &reqwest::Error) -> TraceFailureMetadata {
    TransportFailure::from_reqwest_error(TransportFailureStage::ResponseBodyReadFailed, error)
        .to_trace_failure_metadata()
}

fn record_send_failure_trace(
    user_id: UserId,
    label: &IntegrationLabel,
    request: &RequestTraceSnapshot,
    failure: TraceFailureMetadata,
) {
    record_trace(
        user_id,
        label,
        request.method.clone(),
        request.url.clone(),
        request.headers.clone(),
        request.body.clone(),
        0,
        "No Response".to_string(),
        Vec::new(),
        None,
        Some(failure),
        request.timestamp,
        Utc::now(),
    );
}

// ============ TracedAsyncClient ============

/// Async HTTP client with automatic tracing when `BGTRACES=fs` is set.
///
/// Used by integrations that run in async contexts.
pub(crate) struct TracedAsyncClient {
    client: reqwest::Client,
    user_id: UserId,
    label: IntegrationLabel,
    tracing_enabled: bool,
    redacted_query_params: Vec<String>,
    redacted_headers: Vec<String>,
    redacted_json_body_fields: Vec<String>,
}

/// Builder for [`TracedAsyncClient`].
pub(crate) struct TracedAsyncClientBuilder {
    label: IntegrationLabel,
    user_id: UserId,
    client_builder: reqwest::ClientBuilder,
    custom_redacted_query_params: Vec<String>,
    custom_redacted_headers: Vec<String>,
    custom_redacted_json_body_fields: Vec<String>,
}

impl TracedAsyncClient {
    pub(crate) fn builder(label: IntegrationLabel, user_id: UserId) -> TracedAsyncClientBuilder {
        TracedAsyncClientBuilder {
            label,
            user_id,
            client_builder: reqwest::Client::builder(),
            custom_redacted_query_params: Vec::new(),
            custom_redacted_headers: Vec::new(),
            custom_redacted_json_body_fields: Vec::new(),
        }
    }

    /// Start building a POST request.
    pub(crate) fn post(&self, url: impl Into<String>) -> TracedAsyncRequestBuilder<'_> {
        let url = url.into();
        TracedAsyncRequestBuilder {
            client: self,
            method: reqwest::Method::POST,
            url,
            headers: Vec::new(),
            body: None,
        }
    }

    /// Start building a GET request.
    pub(crate) fn get(&self, url: impl Into<String>) -> TracedAsyncRequestBuilder<'_> {
        let url = url.into();
        TracedAsyncRequestBuilder {
            client: self,
            method: reqwest::Method::GET,
            url,
            headers: Vec::new(),
            body: None,
        }
    }
}

impl TracedAsyncClientBuilder {
    /// Customize the underlying `reqwest::ClientBuilder`.
    pub(crate) fn configure(
        mut self,
        configure: impl FnOnce(reqwest::ClientBuilder) -> reqwest::ClientBuilder,
    ) -> Self {
        self.client_builder = configure(self.client_builder);
        self
    }

    /// Add JSON body field names to redact from traced request and response bodies.
    pub(crate) fn redact_json_body_fields(mut self, names: &[&str]) -> Self {
        self.custom_redacted_json_body_fields
            .extend(names.iter().map(|name| (*name).to_string()));
        self
    }

    pub(crate) fn build(self) -> Result<TracedAsyncClient, TracedClientError> {
        let client = self
            .client_builder
            .user_agent(crate::user_agent::user_agent())
            .build()
            .map_err(|err| TracedClientError::ClientBuild(err.to_string()))?;

        let redacted_query_params = merge_redaction_names(
            default_redacted_query_params(),
            &self.custom_redacted_query_params,
        );
        let redacted_headers =
            merge_redaction_names(default_redacted_headers(), &self.custom_redacted_headers);
        let redacted_json_body_fields =
            merge_redaction_names(&[], &self.custom_redacted_json_body_fields);

        Ok(TracedAsyncClient {
            client,
            user_id: self.user_id,
            label: self.label,
            tracing_enabled: super::is_tracing_enabled(),
            redacted_query_params,
            redacted_headers,
            redacted_json_body_fields,
        })
    }
}

/// In-progress async request that will be traced on send.
pub(crate) struct TracedAsyncRequestBuilder<'a> {
    client: &'a TracedAsyncClient,
    method: reqwest::Method,
    url: String,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
}

impl<'a> TracedAsyncRequestBuilder<'a> {
    /// Add a header to the request.
    pub(crate) fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Set the request body.
    pub(crate) fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// Send the request and return the response.
    pub(crate) async fn send(self) -> Result<TracedAsyncResponse, reqwest::Error> {
        let mut request_builder = self.client.client.request(self.method.clone(), &self.url);

        for (name, value) in &self.headers {
            request_builder = request_builder.header(name, value);
        }

        if let Some(ref body) = self.body {
            request_builder = request_builder.body(body.clone());
        }

        if !self.client.tracing_enabled {
            let response = request_builder.send().await?;
            return TracedAsyncResponse::from_response(
                response,
                &self.client.redacted_query_params,
            )
            .await;
        }

        // Tracing enabled: capture metadata around the request
        let req_timestamp = Utc::now();
        let request_snapshot = RequestTraceSnapshot {
            method: self.method.to_string(),
            url: redact_url(&self.url, &self.client.redacted_query_params),
            headers: redact_headers(&self.headers, &self.client.redacted_headers),
            body: self
                .body
                .as_deref()
                .map(|body| redact_json_body(body, &self.client.redacted_json_body_fields)),
            timestamp: req_timestamp,
        };

        let response = match request_builder.send().await {
            Ok(response) => response,
            Err(error) => {
                let error = error.without_url();
                let failure = send_failed_trace_failure(&error);
                record_send_failure_trace(
                    self.client.user_id,
                    &self.client.label,
                    &request_snapshot,
                    failure,
                );
                return Err(error);
            }
        };

        let response_snapshot = ResponseTraceSnapshot {
            status: response.status(),
            status_text: status_text(response.status()),
            url: redact_url(response.url().as_str(), &self.client.redacted_query_params),
            headers: redact_headers(
                &collect_headers(response.headers()),
                &self.client.redacted_headers,
            ),
            timestamp: Utc::now(),
        };

        let rsp_body_bytes = match response.bytes().await {
            Ok(bytes) => bytes.to_vec(),
            Err(error) => {
                let error = error.without_url();
                let failure = response_body_read_failed_trace_failure(&error);
                record_trace_snapshot(
                    self.client.user_id,
                    &self.client.label,
                    &request_snapshot,
                    &response_snapshot,
                    None,
                    Some(failure),
                );
                return Err(error);
            }
        };

        record_trace_snapshot(
            self.client.user_id,
            &self.client.label,
            &request_snapshot,
            &response_snapshot,
            redacted_body_option(&rsp_body_bytes, &self.client.redacted_json_body_fields),
            None,
        );

        Ok(TracedAsyncResponse {
            status: response_snapshot.status,
            body: rsp_body_bytes,
            #[cfg(any(feature = "desktop", test))]
            url: response_snapshot.url,
        })
    }
}

/// Response from a traced async request.
///
/// The body has been fully read (required for tracing), so it is available
/// as bytes without further I/O.
pub(crate) struct TracedAsyncResponse {
    status: reqwest::StatusCode,
    body: Vec<u8>,
    #[cfg(any(feature = "desktop", test))]
    url: String,
}

impl TracedAsyncResponse {
    async fn from_response(
        response: reqwest::Response,
        _redacted_query_params: &[String],
    ) -> Result<Self, reqwest::Error> {
        let status = response.status();
        #[cfg(any(feature = "desktop", test))]
        let url = redact_url(response.url().as_str(), _redacted_query_params);
        let body = response.bytes().await?.to_vec();
        #[cfg(any(feature = "desktop", test))]
        {
            Ok(Self { status, body, url })
        }
        #[cfg(not(any(feature = "desktop", test)))]
        {
            Ok(Self { status, body })
        }
    }

    pub(crate) fn status(&self) -> reqwest::StatusCode {
        self.status
    }

    #[cfg(any(feature = "desktop", test))]
    pub(crate) fn url(&self) -> &str {
        &self.url
    }

    /// Consume the response and return the body as text.
    pub(crate) fn text(self) -> Result<String, std::string::FromUtf8Error> {
        String::from_utf8(self.body)
    }
}

// ============ TracedBlockingClient ============

/// Blocking HTTP client with automatic tracing when `BGTRACES=fs` is set.
pub(crate) struct TracedBlockingClient {
    client: reqwest::blocking::Client,
    user_id: UserId,
    label: IntegrationLabel,
    tracing_enabled: bool,
    redacted_query_params: Vec<String>,
    redacted_headers: Vec<String>,
    redacted_json_body_fields: Vec<String>,
}

/// Builder for [`TracedBlockingClient`].
pub(crate) struct TracedBlockingClientBuilder {
    label: IntegrationLabel,
    user_id: UserId,
    client_builder: reqwest::blocking::ClientBuilder,
    custom_redacted_query_params: Vec<String>,
    custom_redacted_headers: Vec<String>,
    custom_redacted_json_body_fields: Vec<String>,
}

impl TracedBlockingClient {
    pub(crate) fn builder(label: IntegrationLabel, user_id: UserId) -> TracedBlockingClientBuilder {
        TracedBlockingClientBuilder {
            label,
            user_id,
            client_builder: reqwest::blocking::Client::builder(),
            custom_redacted_query_params: Vec::new(),
            custom_redacted_headers: Vec::new(),
            custom_redacted_json_body_fields: Vec::new(),
        }
    }

    /// Start building a GET request.
    pub(crate) fn get(&self, url: impl Into<String>) -> TracedBlockingRequestBuilder<'_> {
        let url = url.into();
        TracedBlockingRequestBuilder {
            client: self,
            method: reqwest::Method::GET,
            url,
            headers: Vec::new(),
            body: None,
        }
    }

    /// Start building a POST request.
    #[cfg(feature = "dev-config")]
    pub(crate) fn post(&self, url: impl Into<String>) -> TracedBlockingRequestBuilder<'_> {
        let url = url.into();
        TracedBlockingRequestBuilder {
            client: self,
            method: reqwest::Method::POST,
            url,
            headers: Vec::new(),
            body: None,
        }
    }
}

impl TracedBlockingClientBuilder {
    /// Customize the underlying `reqwest::blocking::ClientBuilder`.
    pub(crate) fn configure(
        mut self,
        configure: impl FnOnce(reqwest::blocking::ClientBuilder) -> reqwest::blocking::ClientBuilder,
    ) -> Self {
        self.client_builder = configure(self.client_builder);
        self
    }

    /// Add query parameter names to redact in addition to built-in defaults.
    pub(crate) fn redact_query_params(mut self, names: &[&str]) -> Self {
        self.custom_redacted_query_params
            .extend(names.iter().map(|name| (*name).to_string()));
        self
    }

    /// Add header names to redact in addition to built-in defaults.
    pub(crate) fn redact_headers(mut self, names: &[&str]) -> Self {
        self.custom_redacted_headers
            .extend(names.iter().map(|name| (*name).to_string()));
        self
    }

    pub(crate) fn build(self) -> Result<TracedBlockingClient, TracedClientError> {
        let client = self
            .client_builder
            .user_agent(crate::user_agent::user_agent())
            .build()
            .map_err(|err| TracedClientError::ClientBuild(err.to_string()))?;

        let redacted_query_params = merge_redaction_names(
            default_redacted_query_params(),
            &self.custom_redacted_query_params,
        );
        let redacted_headers =
            merge_redaction_names(default_redacted_headers(), &self.custom_redacted_headers);
        let redacted_json_body_fields =
            merge_redaction_names(&[], &self.custom_redacted_json_body_fields);

        Ok(TracedBlockingClient {
            client,
            user_id: self.user_id,
            label: self.label,
            tracing_enabled: super::is_tracing_enabled(),
            redacted_query_params,
            redacted_headers,
            redacted_json_body_fields,
        })
    }

    #[cfg(all(test, feature = "db-tests"))]
    pub(crate) fn build_for_tests_with_tracing(
        self,
        tracing_enabled: bool,
    ) -> Result<TracedBlockingClient, TracedClientError> {
        let client = self
            .client_builder
            .user_agent(crate::user_agent::user_agent())
            .build()
            .map_err(|err| TracedClientError::ClientBuild(err.to_string()))?;

        let redacted_query_params = merge_redaction_names(
            default_redacted_query_params(),
            &self.custom_redacted_query_params,
        );
        let redacted_headers =
            merge_redaction_names(default_redacted_headers(), &self.custom_redacted_headers);
        let redacted_json_body_fields =
            merge_redaction_names(&[], &self.custom_redacted_json_body_fields);

        Ok(TracedBlockingClient {
            client,
            user_id: self.user_id,
            label: self.label,
            tracing_enabled,
            redacted_query_params,
            redacted_headers,
            redacted_json_body_fields,
        })
    }
}

/// In-progress blocking request that will be traced on send.
pub(crate) struct TracedBlockingRequestBuilder<'a> {
    client: &'a TracedBlockingClient,
    method: reqwest::Method,
    url: String,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
}

impl<'a> TracedBlockingRequestBuilder<'a> {
    /// Add a header to the request.
    pub(crate) fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Set the request body.
    #[cfg(feature = "dev-config")]
    pub(crate) fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// Send the request and return the response.
    pub(crate) fn send(self) -> Result<TracedBlockingResponse, reqwest::Error> {
        let mut request_builder = self.client.client.request(self.method.clone(), &self.url);

        for (name, value) in &self.headers {
            request_builder = request_builder.header(name, value);
        }

        if let Some(ref body) = self.body {
            request_builder = request_builder.body(body.clone());
        }

        if !self.client.tracing_enabled {
            let response = request_builder.send()?;
            return TracedBlockingResponse::from_response(
                response,
                &self.client.redacted_query_params,
            );
        }

        // Tracing enabled: capture metadata around the request
        let req_timestamp = Utc::now();
        let request_snapshot = RequestTraceSnapshot {
            method: self.method.to_string(),
            url: redact_url(&self.url, &self.client.redacted_query_params),
            headers: redact_headers(&self.headers, &self.client.redacted_headers),
            body: self
                .body
                .as_deref()
                .map(|body| redact_json_body(body, &self.client.redacted_json_body_fields)),
            timestamp: req_timestamp,
        };

        let response = match request_builder.send() {
            Ok(response) => response,
            Err(error) => {
                let error = error.without_url();
                let failure = send_failed_trace_failure(&error);
                record_send_failure_trace(
                    self.client.user_id,
                    &self.client.label,
                    &request_snapshot,
                    failure,
                );
                return Err(error);
            }
        };

        let response_headers = response.headers().clone();
        let response_snapshot = ResponseTraceSnapshot {
            status: response.status(),
            status_text: status_text(response.status()),
            url: redact_url(response.url().as_str(), &self.client.redacted_query_params),
            headers: redact_headers(
                &collect_headers(&response_headers),
                &self.client.redacted_headers,
            ),
            timestamp: Utc::now(),
        };

        let rsp_body_bytes = match response.bytes() {
            Ok(bytes) => bytes.to_vec(),
            Err(error) => {
                let error = error.without_url();
                let failure = response_body_read_failed_trace_failure(&error);
                record_trace_snapshot(
                    self.client.user_id,
                    &self.client.label,
                    &request_snapshot,
                    &response_snapshot,
                    None,
                    Some(failure),
                );
                return Err(error);
            }
        };

        record_trace_snapshot(
            self.client.user_id,
            &self.client.label,
            &request_snapshot,
            &response_snapshot,
            redacted_body_option(&rsp_body_bytes, &self.client.redacted_json_body_fields),
            None,
        );

        Ok(TracedBlockingResponse {
            status: response_snapshot.status,
            body: rsp_body_bytes,
            url: response_snapshot.url,
            headers: response_headers,
        })
    }
}

/// Response from a traced blocking request.
///
/// The body has been fully read (required for tracing), so it is available
/// as bytes without further I/O.
pub(crate) struct TracedBlockingResponse {
    status: reqwest::StatusCode,
    body: Vec<u8>,
    url: String,
    headers: reqwest::header::HeaderMap,
}

impl TracedBlockingResponse {
    fn from_response(
        response: reqwest::blocking::Response,
        redacted_query_params: &[String],
    ) -> Result<Self, reqwest::Error> {
        let status = response.status();
        let url = redact_url(response.url().as_str(), redacted_query_params);
        let headers = response.headers().clone();
        let body = response.bytes()?.to_vec();

        Ok(Self {
            status,
            body,
            url,
            headers,
        })
    }

    pub(crate) fn status(&self) -> reqwest::StatusCode {
        self.status
    }

    pub(crate) fn url(&self) -> &str {
        &self.url
    }

    pub(crate) fn headers(&self) -> &reqwest::header::HeaderMap {
        &self.headers
    }

    /// Consume the response and return the body as text.
    pub(crate) fn text(self) -> Result<String, std::string::FromUtf8Error> {
        String::from_utf8(self.body)
    }

    /// Consume the response and return the raw body bytes.
    #[cfg(feature = "dev-config")]
    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.body
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use crate::project_paths::push_project_dir_override;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{LazyLock, Mutex};
    use std::time::Duration;

    static TRACE_TEST_MUTEX: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    fn create_test_project_dir(prefix: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("{prefix}_{}", ulid::Ulid::new()));
        fs::create_dir_all(&path).expect("test project dir should be created");
        path
    }

    fn failed_loopback_url(path: &str) -> String {
        format!("http://127.0.0.1:1{path}")
    }

    fn collect_files(root: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let entries = match fs::read_dir(root) {
            Ok(entries) => entries,
            Err(_) => return files,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(collect_files(&path));
            } else {
                files.push(path);
            }
        }
        files
    }

    fn read_single_trace_file(project_dir: &Path, user_id: UserId) -> serde_json::Value {
        let trace_root = project_dir
            .join("users")
            .join(user_id.to_string())
            .join("traces");
        let files = collect_files(&trace_root);
        assert_eq!(
            files.len(),
            1,
            "trace output should contain only one HAR file"
        );
        assert!(
            files[0].extension().is_some_and(|ext| ext == "har"),
            "trace output should be HAR-only"
        );
        let har_content = fs::read_to_string(&files[0]).expect("trace HAR should be readable");
        serde_json::from_str(&har_content).expect("trace HAR should be valid JSON")
    }

    #[test]
    fn integration_label_as_str() {
        let label = IntegrationLabel::new("trezor-bridge");
        assert_eq!(label.as_str(), "trezor-bridge");
    }

    #[test]
    fn integration_label_display() {
        let label = IntegrationLabel::new("mempool");
        assert_eq!(format!("{label}"), "mempool");
    }

    #[test]
    fn traced_client_error_display() {
        let err = TracedClientError::ClientBuild("timeout".to_string());
        assert_eq!(err.to_string(), "failed to build HTTP client: timeout");
    }

    #[test]
    fn classify_transport_error_maps_representative_shapes() {
        assert_eq!(
            classify_transport_error(
                ReqwestErrorFlags {
                    timeout: true,
                    connect: true,
                    request: true,
                    body: false,
                    decode: false,
                },
                "operation timed out",
            ),
            TransportErrorKind::Timeout
        );
        assert_eq!(
            classify_transport_error(
                ReqwestErrorFlags {
                    timeout: false,
                    connect: true,
                    request: true,
                    body: false,
                    decode: false,
                },
                "connection refused",
            ),
            TransportErrorKind::Connect
        );
        assert_eq!(
            classify_transport_error(
                ReqwestErrorFlags {
                    timeout: false,
                    connect: false,
                    request: false,
                    body: true,
                    decode: false,
                },
                "body stream terminated early",
            ),
            TransportErrorKind::Body
        );
        assert_eq!(
            classify_transport_error(
                ReqwestErrorFlags {
                    timeout: false,
                    connect: false,
                    request: false,
                    body: false,
                    decode: true,
                },
                "decode error",
            ),
            TransportErrorKind::Decode
        );
        assert_eq!(
            classify_transport_error(
                ReqwestErrorFlags {
                    timeout: false,
                    connect: false,
                    request: true,
                    body: false,
                    decode: false,
                },
                "tls handshake failure",
            ),
            TransportErrorKind::Tls
        );
        assert_eq!(
            classify_transport_error(
                ReqwestErrorFlags {
                    timeout: false,
                    connect: false,
                    request: true,
                    body: false,
                    decode: false,
                },
                "builder error",
            ),
            TransportErrorKind::Request
        );
        assert_eq!(
            classify_transport_error(
                ReqwestErrorFlags {
                    timeout: false,
                    connect: false,
                    request: false,
                    body: false,
                    decode: false,
                },
                "something odd happened",
            ),
            TransportErrorKind::Unknown
        );
    }

    #[test]
    fn transport_failure_persistence_message_prefixes_failure_stage() {
        let failure = TransportFailure {
            stage: TransportFailureStage::SendFailed,
            message: "connection refused".to_string(),
            kind: TransportErrorKind::Connect,
        };

        assert_eq!(
            failure.persistence_message(),
            "send_failed: connection refused"
        );
        assert_eq!(
            failure.to_trace_failure_metadata().kind.as_deref(),
            Some("connect")
        );
    }

    #[test]
    fn status_text_known_status() {
        assert_eq!(status_text(reqwest::StatusCode::OK), "OK");
        assert_eq!(status_text(reqwest::StatusCode::NOT_FOUND), "Not Found");
    }

    #[test]
    fn merge_redaction_names_combines_defaults_and_custom_without_duplicates() {
        let custom = vec!["X-Api-Key".to_string(), "custom-header".to_string()];
        let merged = merge_redaction_names(default_redacted_headers(), &custom);

        assert!(merged.contains(&"authorization".to_string()));
        assert!(merged.contains(&"x-api-key".to_string()));
        assert!(merged.contains(&"custom-header".to_string()));

        let x_api_key_count = merged
            .iter()
            .filter(|name| name.as_str() == "x-api-key")
            .count();
        assert_eq!(x_api_key_count, 1);
    }

    #[test]
    fn redact_url_redacts_matching_params_case_insensitively() {
        let params = vec!["apikey".to_string(), "token".to_string()];
        let url = "https://example.com/api?ApiKey=one&foo=bar&token=two&apikey=three";

        let redacted = redact_url(url, &params);

        assert_eq!(
            redacted,
            "https://example.com/api?ApiKey=***REDACTED***&foo=bar&token=***REDACTED***&apikey=***REDACTED***"
        );
    }

    #[test]
    fn redact_url_leaves_non_matching_params() {
        let params = vec!["apikey".to_string()];
        let url = "https://example.com/api?foo=bar&baz=qux";

        let redacted = redact_url(url, &params);

        assert_eq!(redacted, "https://example.com/api?foo=bar&baz=qux");
    }

    #[test]
    fn redact_url_handles_fragment_and_flag_params() {
        let params = vec!["token".to_string(), "debug".to_string()];
        let url = "https://example.com/api?foo=bar&token=secret&debug#section";

        let redacted = redact_url(url, &params);

        assert_eq!(
            redacted,
            "https://example.com/api?foo=bar&token=***REDACTED***&debug=***REDACTED***#section"
        );
    }

    #[test]
    fn redact_headers_redacts_matching_headers_case_insensitively() {
        let headers = vec![
            ("Authorization".to_string(), "Bearer abc".to_string()),
            ("x-custom".to_string(), "value".to_string()),
            ("X-Api-Key".to_string(), "key123".to_string()),
        ];
        let names = vec!["authorization".to_string(), "x-api-key".to_string()];

        let redacted = redact_headers(&headers, &names);

        assert_eq!(redacted[0].1, REDACTED_VALUE);
        assert_eq!(redacted[1].1, "value");
        assert_eq!(redacted[2].1, REDACTED_VALUE);
    }

    #[test]
    fn redact_json_body_redacts_nested_fields_case_insensitively() {
        let body = br#"{
            "order_secret": "order-secret",
            "nested": {
                "Management_Secret": "management-secret",
                "safe": "value"
            },
            "tokens": [
                {"premium_access_token": "token"}
            ]
        }"#;
        let redacted = redact_json_body(
            body,
            &[
                "order_secret".to_string(),
                "management_secret".to_string(),
                "premium_access_token".to_string(),
            ],
        );
        let value: Value = serde_json::from_slice(&redacted).expect("redacted body should be JSON");

        assert_eq!(value["order_secret"], REDACTED_VALUE);
        assert_eq!(value["nested"]["Management_Secret"], REDACTED_VALUE);
        assert_eq!(value["nested"]["safe"], "value");
        assert_eq!(value["tokens"][0]["premium_access_token"], REDACTED_VALUE);
    }

    #[test]
    fn redact_json_body_redacts_entitlement_token_fields() {
        let body = br#"{
            "entitlement_token": "signed.jwt.payload",
            "premium_access_token": "also-sensitive",
            "token_id": "01K...",
            "active_token": {
                "token_id": "01K...",
                "tier": "premium"
            },
            "orders": []
        }"#;
        let redacted = redact_json_body(
            body,
            &[
                "entitlement_token".to_string(),
                "premium_access_token".to_string(),
            ],
        );
        let value: Value = serde_json::from_slice(&redacted).expect("redacted body should be JSON");

        assert_eq!(value["entitlement_token"], REDACTED_VALUE);
        assert_eq!(value["premium_access_token"], REDACTED_VALUE);
        assert_eq!(value["token_id"], "01K...");
        assert_eq!(value["active_token"]["tier"], "premium");
    }

    #[test]
    fn defaults_cannot_be_removed_when_customizing_redaction() {
        let custom = vec!["x-extra-secret".to_string()];
        let merged = merge_redaction_names(default_redacted_headers(), &custom);

        assert!(merged.contains(&"authorization".to_string()));
        assert!(merged.contains(&"x-extra-secret".to_string()));
    }

    #[test]
    fn traced_blocking_builder_collects_custom_redaction_names() {
        let user_id = UserId::new();
        let builder = TracedBlockingClient::builder(IntegrationLabel::new("etherscan"), user_id)
            .configure(|builder| builder.timeout(std::time::Duration::from_secs(1)))
            .redact_query_params(&["apikey"])
            .redact_headers(&["x-session-token"]);

        assert_eq!(builder.custom_redacted_query_params, vec!["apikey"]);
        assert_eq!(builder.custom_redacted_headers, vec!["x-session-token"]);
    }

    #[test]
    fn traced_blocking_response_status_and_url() {
        let response = TracedBlockingResponse {
            status: reqwest::StatusCode::ACCEPTED,
            body: b"ok".to_vec(),
            url: "https://example.com/api?apikey=***REDACTED***".to_string(),
            headers: reqwest::header::HeaderMap::new(),
        };

        assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
        assert_eq!(
            response.url(),
            "https://example.com/api?apikey=***REDACTED***"
        );
    }

    #[test]
    fn traced_blocking_response_text_consumes_body() {
        let response = TracedBlockingResponse {
            status: reqwest::StatusCode::OK,
            body: b"hello".to_vec(),
            url: "https://example.com".to_string(),
            headers: reqwest::header::HeaderMap::new(),
        };

        let text = response
            .text()
            .unwrap_or_else(|err| panic!("failed to decode response body: {err}"));
        assert_eq!(text, "hello");
    }

    #[test]
    fn default_header_redaction_includes_coingecko_keys() {
        let headers = merge_redaction_names(default_redacted_headers(), &[]);

        assert!(
            headers.iter().any(|name| name == "x-cg-pro-api-key"),
            "CoinGecko Pro keys must be redacted by default"
        );
        assert!(
            headers.iter().any(|name| name == "x-cg-demo-api-key"),
            "CoinGecko demo keys must be redacted by default"
        );
    }

    #[test]
    fn traced_blocking_send_failure_writes_single_har_trace() {
        let _serial = TRACE_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let project_dir = create_test_project_dir("bitgarth_blocking_send_failed");
        let user_id = UserId::new();
        let client = TracedBlockingClient {
            client: reqwest::blocking::Client::builder()
                .timeout(Duration::from_millis(200))
                .build()
                .expect("blocking client should build"),
            user_id,
            label: IntegrationLabel::new("etherscan"),
            tracing_enabled: true,
            redacted_query_params: vec!["apikey".to_string()],
            redacted_headers: vec!["authorization".to_string()],
            redacted_json_body_fields: Vec::new(),
        };

        {
            let _project_dir_guard =
                push_project_dir_override(project_dir.clone()).expect("project dir override");
            let error = match client
                .get(failed_loopback_url("/api?apikey=secret"))
                .header("Authorization", "Bearer secret")
                .send()
            {
                Ok(_) => panic!("send should fail before a response exists"),
                Err(error) => error,
            };
            assert!(
                !error.to_string().is_empty(),
                "transport error should be preserved"
            );
        }

        let har = read_single_trace_file(&project_dir, user_id);
        assert_eq!(har["log"]["entries"][0]["response"]["status"], 0);
        assert_eq!(
            har["log"]["entries"][0]["response"]["statusText"],
            "No Response"
        );
        assert_eq!(
            har["log"]["entries"][0]["_bitgarthFailureStage"],
            "send_failed"
        );
        assert!(
            har["log"]["entries"][0]["_bitgarthTransportErrorMessage"]
                .as_str()
                .is_some_and(|message| !message.is_empty())
        );
        let request_url = har["log"]["entries"][0]["request"]["url"]
            .as_str()
            .expect("request url should be present");
        assert!(
            request_url.contains("apikey=***REDACTED***"),
            "request url should redact API keys"
        );
        assert!(
            !request_url.contains("apikey=secret"),
            "request url should not keep raw API keys"
        );
        assert_eq!(
            har["log"]["entries"][0]["request"]["headers"][0]["value"],
            REDACTED_VALUE
        );

        let _ = fs::remove_dir_all(project_dir);
    }

    #[test]
    fn traced_async_send_failure_writes_single_har_trace() {
        let _serial = TRACE_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let project_dir = create_test_project_dir("bitgarth_async_send_failed");
        let user_id = UserId::new();
        let client = TracedAsyncClient {
            client: reqwest::Client::builder()
                .timeout(Duration::from_millis(200))
                .build()
                .expect("async client should build"),
            user_id,
            label: IntegrationLabel::new("bitgarth-central"),
            tracing_enabled: true,
            redacted_query_params: vec!["apikey".to_string()],
            redacted_headers: vec!["authorization".to_string()],
            redacted_json_body_fields: Vec::new(),
        };

        {
            let _project_dir_guard =
                push_project_dir_override(project_dir.clone()).expect("project dir override");
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime should build");
            let error = runtime.block_on(async {
                client
                    .get(failed_loopback_url("/session?apikey=secret"))
                    .header("Authorization", "Bearer secret")
                    .send()
                    .await
            });
            let error = match error {
                Ok(_) => panic!("send should fail before a response exists"),
                Err(error) => error,
            };
            assert!(
                !error.to_string().is_empty(),
                "transport error should be preserved"
            );
        }

        let har = read_single_trace_file(&project_dir, user_id);
        assert_eq!(har["log"]["entries"][0]["response"]["status"], 0);
        assert_eq!(
            har["log"]["entries"][0]["_bitgarthFailureStage"],
            "send_failed"
        );
        assert!(
            har["log"]["entries"][0]["_bitgarthTransportErrorMessage"]
                .as_str()
                .is_some_and(|message| !message.is_empty())
        );
        assert_eq!(
            har["log"]["entries"][0]["request"]["headers"][0]["value"],
            REDACTED_VALUE
        );

        let _ = fs::remove_dir_all(project_dir);
    }

    #[test]
    fn traced_async_response_url_accessor() {
        let response = TracedAsyncResponse {
            status: reqwest::StatusCode::OK,
            body: b"{}".to_vec(),
            url: "https://example.com/api?token=***REDACTED***".to_string(),
        };

        assert_eq!(
            response.url(),
            "https://example.com/api?token=***REDACTED***"
        );
    }
}
