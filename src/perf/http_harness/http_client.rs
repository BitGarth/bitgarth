use super::PerfError;
use crate::auth::session::SESSION_COOKIE_NAME;
use crate::models::{AuthResponse, UserId, UserSettings};
use crate::traces::client::{IntegrationLabel, TracedBlockingClient};
use crate::transactions::AggregateSyncSnapshot;
use crate::wallets::{DigitalAssetAccountId, GetAccountTransactionsResponse};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::thread;

pub(super) struct PerfSession {
    pub(super) cookie_header: String,
    pub(super) user_id: UserId,
    pub(super) base_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct WalletReadCounts {
    pub(super) wallet_count: u32,
    pub(super) account_count: u32,
}

pub(super) fn build_perf_client(user_id: UserId) -> Result<TracedBlockingClient, PerfError> {
    TracedBlockingClient::builder(IntegrationLabel::new("perf-harness"), user_id)
        .build()
        .map_err(|err| PerfError::HttpClient(err.to_string()))
}

pub(super) fn endpoint_url(base_url: &str, path: &str) -> String {
    format!("{base_url}{}", path.trim_start_matches('/'))
}

pub(super) fn send_json_post(
    client: &TracedBlockingClient,
    url: String,
    cookie_header: Option<&str>,
    payload: &impl Serialize,
    context: &str,
) -> Result<crate::traces::client::TracedBlockingResponse, PerfError> {
    let body = serde_json::to_string(payload)
        .map_err(|err| PerfError::Json(format!("failed to serialize {context} body: {err}")))?;
    let mut request = client
        .post(url)
        .header("Content-Type", "application/json")
        .body(body);
    if let Some(cookie_header) = cookie_header {
        request = request.header("Cookie", cookie_header.to_string());
    }
    request
        .send()
        .map_err(|err| PerfError::HttpClient(format!("{context} failed: {err}")))
}

pub(super) fn send_authenticated_get(
    client: &TracedBlockingClient,
    url: String,
    cookie_header: &str,
    context: &str,
) -> Result<crate::traces::client::TracedBlockingResponse, PerfError> {
    client
        .get(url)
        .header("Cookie", cookie_header.to_string())
        .send()
        .map_err(|err| PerfError::HttpClient(format!("{context} failed: {err}")))
}

pub(super) fn send_authenticated_post_json(
    client: &TracedBlockingClient,
    url: String,
    cookie_header: &str,
    payload: &Value,
    context: &str,
) -> Result<crate::traces::client::TracedBlockingResponse, PerfError> {
    send_json_post(client, url, Some(cookie_header), payload, context)
}

pub(super) fn send_authenticated_post_empty(
    client: &TracedBlockingClient,
    url: String,
    cookie_header: &str,
    context: &str,
) -> Result<crate::traces::client::TracedBlockingResponse, PerfError> {
    client
        .post(url)
        .header("Cookie", cookie_header.to_string())
        .send()
        .map_err(|err| PerfError::HttpClient(format!("{context} failed: {err}")))
}

pub(super) fn parse_json_response<T: DeserializeOwned>(
    response: crate::traces::client::TracedBlockingResponse,
    context: &str,
) -> Result<T, PerfError> {
    let status_code = response.status();
    let response_url = response.url().to_string();
    let body = response
        .text()
        .map_err(|err| PerfError::Json(format!("{context} body was not valid UTF-8: {err}")))?;
    if !status_code.is_success() {
        return Err(PerfError::HttpClient(format!(
            "{context} returned {status_code} from {response_url}: {body}"
        )));
    }
    serde_json::from_str(&body)
        .map_err(|err| PerfError::Json(format!("failed to parse {context} body: {err}")))
}

pub(super) fn register_session(
    base_url: &str,
    username: &str,
    password: &str,
) -> Result<PerfSession, PerfError> {
    let client = build_perf_client(UserId::new())?;
    let response = send_json_post(
        &client,
        endpoint_url(base_url, "_app/auth/register"),
        None,
        &register_payload(username, password),
        "register perf user",
    )?;
    build_session_from_response(response, base_url)
}

fn register_payload(username: &str, password: &str) -> Value {
    serde_json::json!({
        "username": username,
        "password": password,
        "legal_acknowledgement": crate::legal::current_registration_acknowledgement(),
    })
}

pub(super) fn login_session(
    base_url: &str,
    username: &str,
    password: &str,
) -> Result<PerfSession, PerfError> {
    let client = build_perf_client(UserId::new())?;
    let response = send_json_post(
        &client,
        endpoint_url(base_url, "_app/auth/login"),
        None,
        &serde_json::json!({
            "username": username,
            "password": password,
        }),
        "login perf user",
    )?;
    build_session_from_response(response, base_url)
}

fn build_session_from_response(
    response: crate::traces::client::TracedBlockingResponse,
    base_url: &str,
) -> Result<PerfSession, PerfError> {
    let status_code = response.status();
    let headers = response.headers().clone();
    let body = response
        .text()
        .map_err(|err| PerfError::Json(format!("perf auth body was not valid UTF-8: {err}")))?;
    if !status_code.is_success() {
        return Err(PerfError::HttpClient(format!(
            "perf auth request returned {status_code}: {body}"
        )));
    }
    let auth: AuthResponse =
        serde_json::from_str(&body).map_err(|err| PerfError::Json(err.to_string()))?;
    let cookie_header = session_cookie_header(&headers)?;

    Ok(PerfSession {
        cookie_header,
        user_id: auth.user.user_id,
        base_url: base_url.to_string(),
    })
}

fn session_cookie_header(headers: &reqwest::header::HeaderMap) -> Result<String, PerfError> {
    let prefix = format!("{SESSION_COOKIE_NAME}=");
    for value in headers.get_all(reqwest::header::SET_COOKIE) {
        let header_value = value
            .to_str()
            .map_err(|err| PerfError::HttpClient(format!("invalid set-cookie header: {err}")))?;
        if let Some(cookie) = header_value
            .split(';')
            .next()
            .filter(|cookie| cookie.starts_with(&prefix))
        {
            return Ok(cookie.to_string());
        }
    }

    Err(PerfError::HttpClient(
        "perf auth response did not include a session cookie".to_string(),
    ))
}

pub(super) fn fetch_sync_state(session: &PerfSession) -> Result<AggregateSyncSnapshot, PerfError> {
    let client = build_perf_client(session.user_id)?;
    let response = send_authenticated_get(
        &client,
        endpoint_url(&session.base_url, "_app/user/transactions/sync/state"),
        &session.cookie_header,
        "fetch perf sync state",
    )?;
    parse_json_response(response, "fetch perf sync state")
}

pub(super) fn fetch_wallets(session: &PerfSession) -> Result<serde_json::Value, PerfError> {
    let client = build_perf_client(session.user_id)?;
    let response = send_authenticated_get(
        &client,
        endpoint_url(&session.base_url, "_app/user/wallets"),
        &session.cookie_header,
        "fetch perf wallets",
    )?;
    parse_json_response(response, "fetch perf wallets")
}

pub(super) fn fetch_settings(session: &PerfSession) -> Result<UserSettings, PerfError> {
    let client = build_perf_client(session.user_id)?;
    let response = send_authenticated_get(
        &client,
        endpoint_url(&session.base_url, "_app/user/settings"),
        &session.cookie_header,
        "fetch perf settings",
    )?;
    parse_json_response(response, "fetch perf settings")
}

pub(super) fn fetch_account_transactions(
    session: &PerfSession,
    account_id: DigitalAssetAccountId,
) -> Result<GetAccountTransactionsResponse, PerfError> {
    let client = build_perf_client(session.user_id)?;
    let response = send_authenticated_get(
        &client,
        endpoint_url(
            &session.base_url,
            &format!("_app/user/account/{account_id}/transactions"),
        ),
        &session.cookie_header,
        "fetch perf account transactions",
    )?;
    parse_json_response(response, "fetch perf account transactions")
}

pub(super) fn fetch_wallet_counts(session: &PerfSession) -> Result<WalletReadCounts, PerfError> {
    let wallets = fetch_wallets(session)?;
    let wallet_entries = wallets["wallets"].as_array().ok_or_else(|| {
        PerfError::Json("fetch perf wallets response did not contain a wallets array".to_string())
    })?;
    let wallet_count = u32::try_from(wallet_entries.len())
        .map_err(|_| PerfError::Json("wallet count exceeded u32".to_string()))?;
    let account_count = wallet_entries.iter().try_fold(0_u32, |total, wallet| {
        let accounts = wallet["accounts"].as_array().ok_or_else(|| {
            PerfError::Json("wallet entry did not contain an accounts array".to_string())
        })?;
        let count = u32::try_from(accounts.len())
            .map_err(|_| PerfError::Json("account count exceeded u32".to_string()))?;
        Ok::<u32, PerfError>(total.saturating_add(count))
    })?;
    Ok(WalletReadCounts {
        wallet_count,
        account_count,
    })
}

pub(super) fn build_clients(
    user_id: UserId,
    count: usize,
) -> Result<Vec<TracedBlockingClient>, PerfError> {
    (0..count).map(|_| build_perf_client(user_id)).collect()
}

pub(super) fn run_request_batch(
    clients: &[TracedBlockingClient],
    request_specs: &[PerfRequestSpec],
    cookie_header: &str,
) -> Result<Vec<super::measurement::RequestOutcome>, PerfError> {
    thread::scope(|scope| {
        let handles = clients
            .iter()
            .map(|client| {
                scope.spawn(move || run_client_request_flow(client, request_specs, cookie_header))
            })
            .collect::<Vec<_>>();

        handles
            .into_iter()
            .map(|handle| {
                handle.join().map_err(|_| {
                    PerfError::HttpClient("perf request thread panicked".to_string())
                })?
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|nested| nested.into_iter().flatten().collect())
    })
}

fn run_client_request_flow(
    client: &TracedBlockingClient,
    request_specs: &[PerfRequestSpec],
    cookie_header: &str,
) -> Result<Vec<super::measurement::RequestOutcome>, PerfError> {
    request_specs
        .iter()
        .map(|request_spec| perform_authenticated_request(client, request_spec, cookie_header))
        .collect()
}

fn perform_authenticated_request(
    client: &TracedBlockingClient,
    request_spec: &PerfRequestSpec,
    cookie_header: &str,
) -> Result<super::measurement::RequestOutcome, PerfError> {
    let started_at = std::time::Instant::now();
    let response = match (&request_spec.method, &request_spec.body) {
        (PerfRequestMethod::Get, PerfRequestBody::Empty) => send_authenticated_get(
            client,
            request_spec.url.clone(),
            cookie_header,
            request_spec.context,
        )?,
        (PerfRequestMethod::Post, PerfRequestBody::Json(body)) => send_authenticated_post_json(
            client,
            request_spec.url.clone(),
            cookie_header,
            body,
            request_spec.context,
        )?,
        (PerfRequestMethod::Post, PerfRequestBody::Empty) => send_authenticated_post_empty(
            client,
            request_spec.url.clone(),
            cookie_header,
            request_spec.context,
        )?,
        (PerfRequestMethod::Get, PerfRequestBody::Json(_)) => {
            return Err(PerfError::usage(
                "perf request spec cannot use a JSON body with GET",
            ));
        }
    };
    let latency_ms = started_at.elapsed().as_secs_f64() * 1_000.0;
    let status_code = response.status();
    let _body = response
        .text()
        .map_err(|err| PerfError::Json(format!("perf response body was not valid UTF-8: {err}")))?;
    Ok(super::measurement::RequestOutcome {
        latency_ms,
        success: status_code.is_success(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PerfRequestMethod {
    Get,
    Post,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum PerfRequestBody {
    Empty,
    Json(Value),
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct PerfRequestSpec {
    pub(super) method: PerfRequestMethod,
    pub(super) url: String,
    pub(super) body: PerfRequestBody,
    pub(super) context: &'static str,
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn register_payload_includes_current_legal_acknowledgement() {
        let payload = register_payload("perf-user", "perf-pass");

        assert_eq!(payload["username"], "perf-user");
        assert_eq!(payload["password"], "perf-pass");
        assert_eq!(
            payload["legal_acknowledgement"]["accepted_terms_version"],
            crate::legal::TERMS_VERSION
        );
        assert_eq!(
            payload["legal_acknowledgement"]["accepted_privacy_version"],
            crate::legal::PRIVACY_VERSION
        );
    }
}
