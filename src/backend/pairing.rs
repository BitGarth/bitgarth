use super::ApiErrorEnvelope;
#[cfg(feature = "server")]
use super::ProxyHeaderTrust;
#[cfg(feature = "server")]
use crate::client_capabilities::{CapabilityId, ClientCapabilityRecord, ClientKeyVerifier};
#[cfg(feature = "server")]
use crate::db::encryption::ClientKeyWrapper;
#[cfg(feature = "server")]
use crate::models::FieldErrors;
#[cfg(feature = "server")]
use crate::pairing::{
    ApprovedPairingBinding, ApprovedPairingClaim, ClaimedPairing, MAX_START_BODY_BYTES,
    PairingClaimError, PairingStartError, PairingStartRequest, PairingStartResponse, PairingStore,
    PairingTransitionError, format_code, format_expiry,
};
#[cfg(feature = "server")]
use axum::Json;
#[cfg(feature = "server")]
use axum::body::to_bytes;
#[cfg(feature = "server")]
use axum::extract::{ConnectInfo, Extension, Path, Request};
#[cfg(feature = "server")]
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
#[cfg(feature = "server")]
use axum::response::{IntoResponse, Response};
#[cfg(feature = "server")]
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
#[cfg(feature = "server")]
use chrono::Utc;
#[cfg(feature = "server")]
use dioxus::fullstack::FullstackContext;
use dioxus::prelude::*;
#[cfg(feature = "server")]
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
#[cfg(feature = "server")]
use std::net::{IpAddr, SocketAddr};
#[cfg(feature = "server")]
use std::str::FromStr;
#[cfg(feature = "server")]
use std::sync::Arc;
#[cfg(feature = "server")]
use url::Url;
#[cfg(feature = "server")]
use zeroize::Zeroizing;

#[cfg(feature = "server")]
pub(crate) async fn start_pairing(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Extension(store): Extension<Arc<PairingStore>>,
    Extension(proxy_trust): Extension<ProxyHeaderTrust>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let body = match to_bytes(body, MAX_START_BODY_BYTES).await {
        Ok(body) => body,
        Err(_) => return api_error(ApiErrorEnvelope::bad_request("Invalid JSON request body")),
    };
    let start = match serde_json::from_slice::<PairingStartRequest>(&body) {
        Ok(start) => start,
        Err(_) => return api_error(ApiErrorEnvelope::bad_request("Invalid JSON request body")),
    };
    let start = match start.validate() {
        Ok(start) => start,
        Err(errors) => {
            return api_error(ApiErrorEnvelope::validation("Validation error", errors));
        }
    };

    let source = match trusted_source(peer.ip(), &parts.headers, proxy_trust) {
        Ok(source) => source,
        Err(error) => return api_error(error),
    };
    let origin = match request_origin(&parts.uri, &parts.headers, proxy_trust) {
        Ok(origin) => origin,
        Err(error) => return api_error(error),
    };

    let generated = std::iter::repeat_with(|| {
        let mut id = [0_u8; 32];
        let mut code = [0_u8; 8];
        OsRng.fill_bytes(&mut id);
        OsRng.fill_bytes(&mut code);
        (id, code)
    });
    let started = match store.start(Utc::now(), source, start, generated, |verifier| {
        crate::db::find_capability_identity_by_verifier(verifier).map(|row| row.is_some())
    }) {
        Ok(started) => started,
        Err(PairingStartError::RateLimited {
            retry_after_seconds,
        })
        | Err(PairingStartError::CapacityFull {
            retry_after_seconds,
        }) => {
            return api_error_with_retry_after(
                ApiErrorEnvelope::too_many_requests("Pairing start limit reached"),
                retry_after_seconds,
            );
        }
        Err(PairingStartError::VerifierConflict) => {
            let mut errors = FieldErrors::new();
            errors.add(
                "key_verifier",
                "A pairing or client already uses this key verifier".to_owned(),
            );
            return api_error(ApiErrorEnvelope::conflict(
                "Key verifier already exists",
                errors,
            ));
        }
        Err(PairingStartError::Database | PairingStartError::GenerationExhausted) => {
            return api_error(ApiErrorEnvelope::internal());
        }
    };

    let display_code = format_code(&started.code);
    let mut approval_url = origin;
    approval_url.set_path("/pair");
    approval_url
        .query_pairs_mut()
        .append_pair("code", &display_code);
    Json(PairingStartResponse {
        pairing_id: started.capability_id.to_string(),
        code: display_code,
        approval_url: approval_url.to_string(),
        expires_at: format_expiry(started.expires_at),
    })
    .into_response()
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum PairingClaimResponse {
    Pending,
    Active {
        remote_user_id: String,
        permissions: Vec<String>,
    },
}

#[cfg(feature = "server")]
pub(crate) async fn claim_pairing(
    Path(pairing_id): Path<String>,
    Extension(store): Extension<Arc<PairingStore>>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let raw_key = match parse_client_key(&parts.headers) {
        Ok(raw_key) => raw_key,
        Err(()) => return api_error(ApiErrorEnvelope::unauthorized("Invalid Client Key")),
    };
    match to_bytes(body, 1).await {
        Ok(body) if body.is_empty() => {}
        Ok(_) | Err(_) => {
            return api_error(ApiErrorEnvelope::bad_request(
                "Pairing claim body must be empty",
            ));
        }
    }
    let capability_id = match CapabilityId::from_str(&pairing_id) {
        Ok(capability_id) => capability_id,
        Err(_) => return api_error(ApiErrorEnvelope::not_found("Pairing not found")),
    };
    let now = Utc::now();
    let verifier = ClientKeyVerifier::from_raw_key(&raw_key);
    let claimed = store.claim(now, capability_id, verifier, |claim| {
        activate_pairing_claim(&raw_key, claim, now)
    });
    let claimed = match claimed {
        Ok(claimed) => match durable_claim(capability_id, verifier, now) {
            Ok(DurableClaim::Active(durable)) if durable == claimed => durable,
            Ok(DurableClaim::Active(_) | DurableClaim::Unauthorized | DurableClaim::Missing) => {
                return api_error(ApiErrorEnvelope::unauthorized("Invalid Client Key"));
            }
            Err(()) => return api_error(ApiErrorEnvelope::internal()),
        },
        Err(PairingClaimError::NotFound) => match durable_claim(capability_id, verifier, now) {
            Ok(DurableClaim::Active(claimed)) => claimed,
            Ok(DurableClaim::Unauthorized) => {
                return api_error(ApiErrorEnvelope::unauthorized("Invalid Client Key"));
            }
            Ok(DurableClaim::Missing) => {
                return api_error(ApiErrorEnvelope::not_found("Pairing not found"));
            }
            Err(()) => return api_error(ApiErrorEnvelope::internal()),
        },
        Err(PairingClaimError::Unauthorized) => {
            return api_error(ApiErrorEnvelope::unauthorized("Invalid Client Key"));
        }
        Err(PairingClaimError::Pending) => {
            let mut response =
                (StatusCode::ACCEPTED, Json(PairingClaimResponse::Pending)).into_response();
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("5"));
            return response;
        }
        Err(PairingClaimError::Denied) => {
            return api_error(ApiErrorEnvelope::forbidden("Pairing was denied"));
        }
        Err(PairingClaimError::Activation) => {
            return api_error(ApiErrorEnvelope::internal());
        }
    };
    Json(claim_response(claimed)).into_response()
}

#[cfg(feature = "server")]
pub(super) fn parse_client_key(headers: &HeaderMap) -> Result<Zeroizing<[u8; 32]>, ()> {
    let values = headers.get_all(header::AUTHORIZATION);
    let mut values = values.iter();
    let value = values.next().ok_or(())?;
    if values.next().is_some() {
        return Err(());
    }
    let value = value.to_str().map_err(|_| ())?;
    let (scheme, token) = value.split_once(' ').ok_or(())?;
    if !scheme.eq_ignore_ascii_case("Bearer")
        || token.len() != 43
        || token.chars().any(char::is_whitespace)
    {
        return Err(());
    }

    let mut raw_key = Zeroizing::new([0_u8; 32]);
    let decoded = URL_SAFE_NO_PAD
        .decode_slice(token, raw_key.as_mut_slice())
        .map_err(|_| ())?;
    if decoded != raw_key.len() {
        return Err(());
    }
    let canonical = Zeroizing::new(URL_SAFE_NO_PAD.encode(raw_key.as_slice()));
    if canonical.as_str() != token {
        return Err(());
    }
    Ok(raw_key)
}

#[cfg(feature = "server")]
enum DurableClaim {
    Missing,
    Unauthorized,
    Active(ClaimedPairing),
}

#[cfg(feature = "server")]
fn durable_claim(
    capability_id: CapabilityId,
    verifier: ClientKeyVerifier,
    now: chrono::DateTime<Utc>,
) -> Result<DurableClaim, ()> {
    let Some(record) = crate::db::load_client_capability(capability_id).map_err(|_| ())? else {
        return Ok(DurableClaim::Missing);
    };
    if record.key_verifier != verifier {
        return Ok(DurableClaim::Unauthorized);
    }
    if record.revoked_at.is_some()
        || record
            .expires_at
            .is_some_and(|expires_at| expires_at <= now)
    {
        return Ok(DurableClaim::Unauthorized);
    }
    let envelope = crate::db::encryption::read_envelope(record.user_id).map_err(|_| ())?;
    let compatible = match (
        envelope,
        record.wrapped_dek.as_ref(),
        record.wrap_nonce.as_ref(),
    ) {
        (crate::db::encryption::DbEnvelope::Encrypted { .. }, Some(_), Some(_)) => true,
        #[cfg(feature = "dev-config")]
        (crate::db::encryption::DbEnvelope::UnencryptedDev, None, None) => true,
        _ => false,
    };
    if !compatible {
        return Ok(DurableClaim::Unauthorized);
    }
    Ok(DurableClaim::Active(ClaimedPairing {
        user_id: record.user_id,
        permission: record.permission,
    }))
}

#[cfg(feature = "server")]
fn activate_pairing_claim(
    raw_key: &[u8; 32],
    claim: ApprovedPairingClaim<'_>,
    now: chrono::DateTime<Utc>,
) -> Result<ClaimedPairing, ()> {
    activate_pairing_claim_with_check(raw_key, claim, now, |_| Ok(()))
}

#[cfg(feature = "server")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum ActivationPoint {
    AfterNameWrite,
    AfterAppCommit,
}

#[cfg(feature = "server")]
fn activate_pairing_claim_with_check<F>(
    raw_key: &[u8; 32],
    claim: ApprovedPairingClaim<'_>,
    now: chrono::DateTime<Utc>,
    mut check: F,
) -> Result<ClaimedPairing, ()>
where
    F: FnMut(ActivationPoint) -> Result<(), ()>,
{
    let verifier = ClientKeyVerifier::from_raw_key(raw_key);
    match durable_claim(claim.capability_id, verifier, now)? {
        DurableClaim::Active(claimed)
            if claimed.user_id == claim.user_id && claimed.permission == claim.permission =>
        {
            return Ok(claimed);
        }
        DurableClaim::Active(_) | DurableClaim::Unauthorized => return Err(()),
        DurableClaim::Missing => {}
    }

    crate::db::insert_paired_client_name(claim.user_id, claim.capability_id, claim.client_name)
        .map_err(|_| ())?;
    if check(ActivationPoint::AfterNameWrite).is_err() {
        let _ = crate::db::delete_paired_client_name(claim.user_id, claim.capability_id);
        return Err(());
    }
    let wrapper = match claim.dek {
        Some(dek) => match ClientKeyWrapper::wrap(
            dek,
            raw_key,
            claim.user_id,
            claim.capability_id,
            claim.permission,
        ) {
            Ok(wrapper) => Some(wrapper),
            Err(_) => {
                let _ = crate::db::delete_paired_client_name(claim.user_id, claim.capability_id);
                return Err(());
            }
        },
        #[cfg(feature = "dev-config")]
        None => None,
        #[cfg(not(feature = "dev-config"))]
        None => {
            let _ = crate::db::delete_paired_client_name(claim.user_id, claim.capability_id);
            return Err(());
        }
    };
    let record = ClientCapabilityRecord {
        capability_id: claim.capability_id,
        user_id: claim.user_id,
        key_verifier: verifier,
        wrapped_dek: wrapper.as_ref().map(|value| value.ciphertext.clone()),
        wrap_nonce: wrapper.map(|value| value.nonce),
        permission: claim.permission,
        created_at: now,
        expires_at: claim.active_expires_at,
        last_used_at: None,
        revoked_at: None,
    };
    if crate::db::insert_active_client_capability(&record).is_err() {
        if let Ok(DurableClaim::Active(claimed)) = durable_claim(claim.capability_id, verifier, now)
            && claimed.user_id == claim.user_id
            && claimed.permission == claim.permission
        {
            return Ok(claimed);
        }
        let _ = crate::db::delete_paired_client_name(claim.user_id, claim.capability_id);
        return Err(());
    }
    check(ActivationPoint::AfterAppCommit)?;
    Ok(ClaimedPairing {
        user_id: claim.user_id,
        permission: claim.permission,
    })
}

#[cfg(feature = "server")]
fn claim_response(claimed: ClaimedPairing) -> PairingClaimResponse {
    PairingClaimResponse::Active {
        remote_user_id: claimed.user_id.to_string(),
        permissions: vec![claimed.permission.as_str().to_owned()],
    }
}

#[cfg(feature = "server")]
pub(crate) async fn unimplemented_public_endpoint() -> Response {
    api_error(ApiErrorEnvelope::not_found("Not found"))
}

#[cfg(all(feature = "server", feature = "desktop", not(test)))]
const _: () = {
    let _ = start_pairing;
    let _ = claim_pairing;
    let _ = unimplemented_public_endpoint;
    let _ = PairingStore::new;
};

#[cfg(feature = "server")]
fn trusted_source(
    peer: IpAddr,
    headers: &HeaderMap,
    proxy_trust: ProxyHeaderTrust,
) -> Result<IpAddr, ApiErrorEnvelope> {
    if !proxy_trust.allows_forwarded_for() {
        return Ok(peer);
    }

    let values = headers.get_all("x-forwarded-for");
    let mut values = values.iter();
    let Some(value) = values.next() else {
        return Err(ApiErrorEnvelope::bad_request(
            "A single valid X-Forwarded-For address is required",
        ));
    };
    if values.next().is_some() {
        return Err(ApiErrorEnvelope::bad_request(
            "A single valid X-Forwarded-For address is required",
        ));
    }
    let value = value.to_str().map_err(|_| {
        ApiErrorEnvelope::bad_request("A single valid X-Forwarded-For address is required")
    })?;
    if value.contains(',') || value.trim() != value {
        return Err(ApiErrorEnvelope::bad_request(
            "A single valid X-Forwarded-For address is required",
        ));
    }
    value.parse::<IpAddr>().map_err(|_| {
        ApiErrorEnvelope::bad_request("A single valid X-Forwarded-For address is required")
    })
}

#[cfg(feature = "server")]
pub(super) fn request_origin(
    uri: &Uri,
    headers: &HeaderMap,
    proxy_trust: ProxyHeaderTrust,
) -> Result<Url, ApiErrorEnvelope> {
    let scheme = uri.scheme_str().map(str::to_owned).or_else(|| {
        proxy_trust.allows_forwarded_proto().then(|| {
            headers
                .get("x-forwarded-proto")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        })?
    });
    let scheme = scheme.as_deref().unwrap_or("http");
    if !matches!(scheme, "http" | "https") {
        return Err(ApiErrorEnvelope::bad_request("Invalid request origin"));
    }
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| ApiErrorEnvelope::bad_request("Invalid request origin"))?;
    let origin = Url::parse(&format!("{scheme}://{host}"))
        .map_err(|_| ApiErrorEnvelope::bad_request("Invalid request origin"))?;
    if !origin.username().is_empty()
        || origin.password().is_some()
        || origin.host().is_none()
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
    {
        return Err(ApiErrorEnvelope::bad_request("Invalid request origin"));
    }
    Ok(origin)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PairingReviewDetails {
    pub(crate) pairing_id: String,
    pub(crate) code: String,
    pub(crate) client_name: String,
    pub(crate) permissions: Vec<String>,
    pub(crate) expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum PairingReviewResponse {
    LoginRequired { code: String },
    Ready { pairing: PairingReviewDetails },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApprovePairingRequest {
    pub(crate) pairing_id: String,
    pub(crate) code: String,
    pub(crate) permissions: Vec<String>,
    pub(crate) code_matches: bool,
    pub(crate) expires_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DenyPairingRequest {
    pub(crate) pairing_id: String,
    pub(crate) code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PairingActionResponse {
    pub(crate) status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PairedClientView {
    pub(crate) capability_id: String,
    pub(crate) name: String,
    pub(crate) permission: String,
    pub(crate) created_at: String,
    pub(crate) expires_at: Option<String>,
    pub(crate) last_used_at: Option<String>,
    pub(crate) revoked_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevokePairedClientRequest {
    pub(crate) capability_id: String,
}

#[cfg(feature = "server")]
fn pairing_store() -> Result<Arc<PairingStore>, ApiErrorEnvelope> {
    FullstackContext::current()
        .and_then(|context| context.extension::<Arc<PairingStore>>())
        .ok_or_else(ApiErrorEnvelope::internal)
}

#[cfg(feature = "server")]
fn current_request_parts() -> Result<(HeaderMap, Uri, ProxyHeaderTrust), ApiErrorEnvelope> {
    let context = FullstackContext::current().ok_or_else(ApiErrorEnvelope::internal)?;
    let proxy_trust = context.extension::<ProxyHeaderTrust>().unwrap_or_default();
    let parts = context.parts_mut();
    Ok((parts.headers.clone(), parts.uri.clone(), proxy_trust))
}

#[cfg(feature = "server")]
fn validate_csrf() -> Result<(), ApiErrorEnvelope> {
    let (headers, uri, proxy_trust) = current_request_parts()?;
    validate_csrf_headers(&headers, &uri, proxy_trust)
}

#[cfg(feature = "server")]
fn validate_csrf_headers(
    headers: &HeaderMap,
    uri: &Uri,
    proxy_trust: ProxyHeaderTrust,
) -> Result<(), ApiErrorEnvelope> {
    let request_origin = request_origin(uri, headers, proxy_trust)
        .map_err(|_| ApiErrorEnvelope::forbidden("Invalid request origin"))?;
    let expected = request_origin.origin().ascii_serialization();

    let origins = headers.get_all(header::ORIGIN);
    let mut origins = origins.iter();
    if let Some(origin) = origins.next() {
        if origins.next().is_some() || origin.to_str().ok() != Some(expected.as_str()) {
            return Err(ApiErrorEnvelope::forbidden("Invalid request origin"));
        }
        return Ok(());
    }

    let referers = headers.get_all(header::REFERER);
    let mut referers = referers.iter();
    let Some(referer) = referers.next() else {
        return Err(ApiErrorEnvelope::forbidden("Invalid request origin"));
    };
    if referers.next().is_some() {
        return Err(ApiErrorEnvelope::forbidden("Invalid request origin"));
    }
    let same_origin = referer
        .to_str()
        .ok()
        .and_then(|value| Url::parse(value).ok())
        .is_some_and(|value| value.origin() == request_origin.origin());
    if !same_origin {
        return Err(ApiErrorEnvelope::forbidden("Invalid request origin"));
    }
    Ok(())
}

#[cfg(feature = "server")]
fn validation_error(field: &str, message: &str) -> ApiErrorEnvelope {
    let mut errors = FieldErrors::new();
    errors.add(field, message.to_owned());
    ApiErrorEnvelope::validation("Validation error", errors)
}

#[cfg(feature = "server")]
fn parse_active_expiry(
    value: Option<&str>,
    now: chrono::DateTime<Utc>,
) -> Result<Option<chrono::DateTime<Utc>>, ApiErrorEnvelope> {
    let Some(value) = value else {
        return Ok(None);
    };
    let parsed = chrono::DateTime::parse_from_rfc3339(value)
        .map_err(|_| validation_error("expires_at", "Expiry must be a valid UTC timestamp"))?;
    if parsed.offset().local_minus_utc() != 0 {
        return Err(validation_error(
            "expires_at",
            "Expiry must be a valid UTC timestamp",
        ));
    }
    let expires_at = parsed.with_timezone(&Utc);
    if expires_at <= now {
        return Err(validation_error(
            "expires_at",
            "Expiry must be in the future",
        ));
    }
    Ok(Some(expires_at))
}

#[cfg(feature = "server")]
fn transition_error(error: PairingTransitionError) -> ApiErrorEnvelope {
    match error {
        PairingTransitionError::NotFound => ApiErrorEnvelope::not_found("Pairing not found"),
        PairingTransitionError::Conflict => {
            ApiErrorEnvelope::conflict("Pairing is no longer awaiting approval", FieldErrors::new())
        }
        PairingTransitionError::Binding => ApiErrorEnvelope::internal(),
    }
}

#[cfg(feature = "server")]
fn initialized_session() -> Result<super::session_context::InitializedSession, ApiErrorEnvelope> {
    let (headers, _, _) = current_request_parts()?;
    let cookies = axum_extra::extract::cookie::CookieJar::from_headers(&headers);
    let token = super::session_context::require_session_token(
        "pairing",
        &cookies,
        ApiErrorEnvelope::unauthorized,
    )?;
    super::session_context::require_initialized_session(
        "pairing",
        &token,
        ApiErrorEnvelope::unauthorized,
        |_| ApiErrorEnvelope::internal(),
    )
}

#[get("/_app/pairings/review")]
pub(crate) async fn review_pairing(
    code: String,
) -> Result<PairingReviewResponse, ApiErrorEnvelope> {
    let store = pairing_store()?;
    if !store.is_live_approval_code(Utc::now(), &code) {
        return Err(ApiErrorEnvelope::not_found("Pairing not found"));
    }
    if current_request_parts()?.0.get(header::COOKIE).is_none() {
        return Ok(PairingReviewResponse::LoginRequired { code });
    }
    let _session = match initialized_session() {
        Ok(session) => session,
        Err(error) if error.is_unauthorized() => {
            return Ok(PairingReviewResponse::LoginRequired { code });
        }
        Err(error) => return Err(error),
    };
    let pending = store.review(Utc::now(), &code).map_err(transition_error)?;
    let pairing = PairingReviewDetails {
        pairing_id: pending.pairing_id,
        code: pending.code,
        client_name: pending.client_name,
        permissions: pending.permissions,
        expires_at: pending.expires_at,
    };
    Ok(PairingReviewResponse::Ready { pairing })
}

#[post("/_app/pairings/approve")]
pub(crate) async fn approve_pairing(
    request: ApprovePairingRequest,
) -> Result<PairingActionResponse, ApiErrorEnvelope> {
    if !request.code_matches {
        return Err(validation_error(
            "code_matches",
            "Confirm that the browser code matches the CLI code",
        ));
    }
    if request.permissions.as_slice() != ["balances_read"] {
        return Err(validation_error(
            "permissions",
            "Permissions must be exactly [\"balances_read\"]",
        ));
    }
    let now = Utc::now();
    let active_expires_at = parse_active_expiry(request.expires_at.as_deref(), now)?;
    let capability_id = crate::client_capabilities::CapabilityId::from_str(&request.pairing_id)
        .map_err(|_| ApiErrorEnvelope::not_found("Pairing not found"))?;
    validate_csrf()?;
    let session = initialized_session()?;
    let user_id = session.session.user_id;
    pairing_store()?
        .approve(now, capability_id, &request.code, active_expires_at, || {
            let lease = crate::auth::lifecycle::acquire_pending_pairing_lease(user_id)
                .map_err(|_| ())?
                .ok_or(())?;
            let dek = crate::db::get_user_db_dek(&user_id).map_err(|_| ())?;
            Ok::<_, ()>(ApprovedPairingBinding {
                user_id,
                dek,
                lease,
            })
        })
        .map_err(transition_error)?;
    Ok(PairingActionResponse {
        status: "approved".to_owned(),
    })
}

#[post("/_app/pairings/deny")]
pub(crate) async fn deny_pairing(
    request: DenyPairingRequest,
) -> Result<PairingActionResponse, ApiErrorEnvelope> {
    let capability_id = crate::client_capabilities::CapabilityId::from_str(&request.pairing_id)
        .map_err(|_| ApiErrorEnvelope::not_found("Pairing not found"))?;
    validate_csrf()?;
    let _session = initialized_session()?;
    pairing_store()?
        .deny(Utc::now(), capability_id, &request.code)
        .map_err(transition_error)?;
    Ok(PairingActionResponse {
        status: "denied".to_owned(),
    })
}

#[get("/_app/pairings/clients")]
pub(crate) async fn list_paired_clients() -> Result<Vec<PairedClientView>, ApiErrorEnvelope> {
    let session = initialized_session()?;
    let user_id = session.session.user_id;
    let mut records = crate::db::load_client_capabilities_for_user(user_id)
        .map_err(|_| ApiErrorEnvelope::internal())?;
    let names =
        crate::db::list_paired_client_names(user_id).map_err(|_| ApiErrorEnvelope::internal())?;
    records.sort_by(|left, right| {
        left.created_at.cmp(&right.created_at).then_with(|| {
            left.capability_id
                .to_string()
                .cmp(&right.capability_id.to_string())
        })
    });
    records
        .into_iter()
        .map(|record| {
            let name = names
                .get(&record.capability_id)
                .cloned()
                .ok_or_else(ApiErrorEnvelope::internal)?;
            Ok(PairedClientView {
                capability_id: record.capability_id.to_string(),
                name,
                permission: record.permission.as_str().to_owned(),
                created_at: format_expiry(record.created_at),
                expires_at: record.expires_at.map(format_expiry),
                last_used_at: record.last_used_at.map(format_expiry),
                revoked_at: record.revoked_at.map(format_expiry),
            })
        })
        .collect()
}

#[post("/_app/pairings/revoke")]
pub(crate) async fn revoke_paired_client(
    request: RevokePairedClientRequest,
) -> Result<PairingActionResponse, ApiErrorEnvelope> {
    let capability_id = CapabilityId::from_str(&request.capability_id)
        .map_err(|_| ApiErrorEnvelope::not_found("Paired Client not found"))?;
    validate_csrf()?;
    let session = initialized_session()?;
    let user_id = session.session.user_id;
    let shutdown = crate::auth::lifecycle::begin_capability_shutdown(user_id, capability_id)
        .map_err(|_| ApiErrorEnvelope::internal())?;
    match crate::db::revoke_client_capability(user_id, capability_id, Utc::now())
        .map_err(|_| ApiErrorEnvelope::internal())?
    {
        crate::db::RevokeClientCapabilityResult::Revoked
        | crate::db::RevokeClientCapabilityResult::AlreadyRevoked => {}
        crate::db::RevokeClientCapabilityResult::NotFound => {
            return Err(ApiErrorEnvelope::not_found("Paired Client not found"));
        }
    }
    shutdown
        .wait_for_requests()
        .map_err(|_| ApiErrorEnvelope::internal())?;
    Ok(PairingActionResponse {
        status: "revoked".to_owned(),
    })
}

#[cfg(feature = "server")]
pub(super) fn api_error(error: ApiErrorEnvelope) -> Response {
    let status = StatusCode::from_u16(error.code.status_code().as_u16())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(error)).into_response()
}

#[cfg(feature = "server")]
fn api_error_with_retry_after(error: ApiErrorEnvelope, retry_after_seconds: u64) -> Response {
    let mut response = api_error(error);
    if let Ok(value) = retry_after_seconds.to_string().parse() {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use chrono::{Duration, TimeZone};
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn trusted_proxy_requires_exactly_one_ip() {
        let peer = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let mut headers = HeaderMap::new();
        assert!(trusted_source(peer, &headers, ProxyHeaderTrust::Trusted).is_err());
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("192.0.2.1, 192.0.2.2"),
        );
        assert!(trusted_source(peer, &headers, ProxyHeaderTrust::Trusted).is_err());
        headers.insert("x-forwarded-for", HeaderValue::from_static("not-an-ip"));
        assert!(trusted_source(peer, &headers, ProxyHeaderTrust::Trusted).is_err());
        headers.insert("x-forwarded-for", HeaderValue::from_static("2001:db8::1"));
        assert_eq!(
            trusted_source(peer, &headers, ProxyHeaderTrust::Trusted).unwrap(),
            IpAddr::V6("2001:db8::1".parse::<Ipv6Addr>().unwrap())
        );
    }

    #[test]
    fn untrusted_proxy_uses_peer_and_ignores_spoofed_header() {
        let peer = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 9));
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("203.0.113.1"));
        assert_eq!(
            trusted_source(peer, &headers, ProxyHeaderTrust::Untrusted).unwrap(),
            peer
        );
    }

    #[test]
    fn protocol_only_proxy_trust_uses_peer_and_ignores_spoofed_address() {
        let peer = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 9));
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("192.0.2.1, 192.0.2.2"),
        );
        assert_eq!(
            trusted_source(peer, &headers, ProxyHeaderTrust::ForwardedProtoOnly).unwrap(),
            peer
        );
    }

    #[test]
    fn origin_rejects_unsafe_authority_data() {
        let uri = Uri::from_static("/api/v1/pairings");
        for host in ["user@example.com", "example.com/path", "example.com?query"] {
            let mut headers = HeaderMap::new();
            headers.insert(header::HOST, HeaderValue::from_str(host).unwrap());
            assert!(request_origin(&uri, &headers, ProxyHeaderTrust::Untrusted).is_err());
        }
    }

    #[test]
    fn trusted_proxy_uses_forwarded_https_for_origin_form_uri() {
        let uri = Uri::from_static("/api/v1/pairings");
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("example.com"));
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));

        assert_eq!(
            request_origin(&uri, &headers, ProxyHeaderTrust::ForwardedProtoOnly)
                .unwrap()
                .as_str(),
            "https://example.com/"
        );
    }

    #[test]
    fn csrf_requires_exact_origin_or_same_origin_referer() {
        let uri = Uri::from_static("/_app/pairings/approve");
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("example.com"));
        assert!(validate_csrf_headers(&headers, &uri, ProxyHeaderTrust::Untrusted).is_err());

        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.example"),
        );
        assert!(validate_csrf_headers(&headers, &uri, ProxyHeaderTrust::Untrusted).is_err());
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://example.com"),
        );
        assert!(validate_csrf_headers(&headers, &uri, ProxyHeaderTrust::Untrusted).is_ok());

        headers.remove(header::ORIGIN);
        headers.insert(
            header::REFERER,
            HeaderValue::from_static("http://example.com/pair?code=1111-1111"),
        );
        assert!(validate_csrf_headers(&headers, &uri, ProxyHeaderTrust::Untrusted).is_ok());
    }

    #[test]
    fn active_expiry_accepts_never_and_any_future_utc_time() {
        let now = Utc
            .with_ymd_and_hms(2026, 7, 31, 15, 0, 0)
            .single()
            .unwrap();
        assert_eq!(parse_active_expiry(None, now).unwrap(), None);
        assert!(
            parse_active_expiry(Some("2126-07-31T15:00:00Z"), now)
                .unwrap()
                .is_some()
        );
        assert!(parse_active_expiry(Some("invalid"), now).is_err());
        assert!(parse_active_expiry(Some("2026-07-31T16:00:00+01:00"), now).is_err());
        assert!(
            parse_active_expiry(Some(&format_expiry(now - Duration::seconds(1))), now).is_err()
        );
    }

    #[cfg(all(feature = "db-tests", feature = "dev-config"))]
    #[test]
    fn durable_claim_recovery_rejects_inactive_and_database_mode_mismatches() {
        use crate::client_capabilities::{ClientCapabilityRecord, ClientPermission};
        use crate::db::encryption::{DbEnvelope, write_envelope};

        fn insert_record(
            user_id: crate::models::UserId,
            byte: u8,
            wrapped: bool,
            expires_at: Option<chrono::DateTime<Utc>>,
            now: chrono::DateTime<Utc>,
        ) -> (CapabilityId, ClientKeyVerifier) {
            crate::db::ensure_test_app_user(user_id);
            let raw_key = [byte; 32];
            let capability_id = CapabilityId::from_bytes([byte; 32]);
            let verifier = ClientKeyVerifier::from_raw_key(&raw_key);
            crate::db::insert_active_client_capability(&ClientCapabilityRecord {
                capability_id,
                user_id,
                key_verifier: verifier,
                wrapped_dek: wrapped.then(|| vec![1, 2, 3]),
                wrap_nonce: wrapped.then(|| vec![4_u8; 12]),
                permission: ClientPermission::BalancesRead,
                created_at: now - Duration::minutes(2),
                expires_at,
                last_used_at: None,
                revoked_at: None,
            })
            .expect("durable claim fixture should insert");
            (capability_id, verifier)
        }

        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let now = Utc::now();

        let revoked_user = crate::models::UserId::new();
        let (revoked_envelope, _) =
            DbEnvelope::new_encrypted("SecurePass123").expect("envelope should create");
        write_envelope(revoked_user, &revoked_envelope).expect("envelope should persist");
        let (revoked_id, revoked_verifier) = insert_record(revoked_user, 91, true, None, now);
        crate::db::revoke_client_capability(revoked_user, revoked_id, now)
            .expect("capability should revoke");
        assert!(matches!(
            durable_claim(revoked_id, revoked_verifier, now),
            Ok(DurableClaim::Unauthorized)
        ));

        let expired_user = crate::models::UserId::new();
        let (expired_envelope, _) =
            DbEnvelope::new_encrypted("SecurePass123").expect("envelope should create");
        write_envelope(expired_user, &expired_envelope).expect("envelope should persist");
        let (expired_id, expired_verifier) = insert_record(
            expired_user,
            92,
            true,
            Some(now - Duration::minutes(1)),
            now,
        );
        assert!(matches!(
            durable_claim(expired_id, expired_verifier, now),
            Ok(DurableClaim::Unauthorized)
        ));

        let unencrypted_user = crate::models::UserId::new();
        write_envelope(unencrypted_user, &DbEnvelope::unencrypted_dev())
            .expect("unencrypted envelope should persist");
        let (wrapped_id, wrapped_verifier) = insert_record(unencrypted_user, 93, true, None, now);
        assert!(matches!(
            durable_claim(wrapped_id, wrapped_verifier, now),
            Ok(DurableClaim::Unauthorized)
        ));

        let encrypted_user = crate::models::UserId::new();
        let (encrypted_envelope, _) =
            DbEnvelope::new_encrypted("SecurePass123").expect("envelope should create");
        write_envelope(encrypted_user, &encrypted_envelope).expect("envelope should persist");
        let (keyless_id, keyless_verifier) = insert_record(encrypted_user, 94, false, None, now);
        assert!(matches!(
            durable_claim(keyless_id, keyless_verifier, now),
            Ok(DurableClaim::Unauthorized)
        ));
    }

    #[cfg(all(feature = "db-tests", feature = "dev-config"))]
    #[test]
    fn unencrypted_pairing_activation_persists_keyless_authority_and_private_name() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let user_id = crate::models::UserId::new();
        crate::db::setup_unencrypted_dev_test_user(user_id);
        let capability_id = CapabilityId::from_bytes([74_u8; 32]);
        let now = Utc
            .with_ymd_and_hms(2026, 7, 31, 15, 0, 0)
            .single()
            .unwrap();

        let claimed = activate_pairing_claim(
            &[74_u8; 32],
            ApprovedPairingClaim {
                capability_id,
                user_id,
                dek: None,
                client_name: "unencrypted development client",
                permission: crate::client_capabilities::ClientPermission::BalancesRead,
                active_expires_at: None,
            },
            now,
        );

        assert!(claimed.is_ok());
        let record = crate::db::load_client_capability(capability_id)
            .unwrap()
            .expect("activated pairing should persist a capability record");
        assert_eq!((record.wrapped_dek, record.wrap_nonce), (None, None));
        assert_eq!(
            crate::db::load_paired_client_name(user_id, capability_id).unwrap(),
            Some("unencrypted development client".to_owned())
        );
    }

    #[cfg(all(feature = "db-tests", feature = "dev-config"))]
    #[test]
    fn unencrypted_pairing_activation_retry_after_app_commit_keeps_private_name() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let user_id = crate::models::UserId::new();
        crate::db::setup_unencrypted_dev_test_user(user_id);
        let capability_id = CapabilityId::from_bytes([75_u8; 32]);
        let raw_key = [75_u8; 32];
        let now = Utc
            .with_ymd_and_hms(2026, 7, 31, 15, 0, 0)
            .single()
            .unwrap();
        let claim = || ApprovedPairingClaim {
            capability_id,
            user_id,
            dek: None,
            client_name: "recoverable unencrypted development client",
            permission: crate::client_capabilities::ClientPermission::BalancesRead,
            active_expires_at: None,
        };

        assert!(
            activate_pairing_claim_with_check(&raw_key, claim(), now, |point| {
                (point != ActivationPoint::AfterAppCommit)
                    .then_some(())
                    .ok_or(())
            })
            .is_err()
        );
        let retry = activate_pairing_claim(&raw_key, claim(), now)
            .expect("retry after app commit should recover keyless authority");

        assert_eq!(retry.user_id, user_id);
        assert_eq!(
            crate::db::load_paired_client_name(user_id, capability_id).unwrap(),
            Some("recoverable unencrypted development client".to_owned())
        );
    }

    #[cfg(feature = "db-tests")]
    #[test]
    fn pairing_activation_faults_converge_without_partial_authority() {
        use crate::db::encryption::{DbEnvelope, UnlockAuthority, UserDbOpenMode};
        use crate::pairing::PairingStartRequest;

        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let user_id = crate::models::UserId::new();
        crate::db::ensure_test_app_user(user_id);
        let (envelope, dek) =
            DbEnvelope::new_encrypted("SecurePass123").expect("test DEK should create");
        crate::db::encryption::write_envelope(user_id, &envelope)
            .expect("test envelope should persist");
        crate::db::initialize_user_db(
            user_id,
            UserDbOpenMode::Encrypted {
                dek: dek.clone(),
                authority: UnlockAuthority::PasswordLogin,
                sqlcipher_compatibility: envelope.sqlcipher_compatibility().unwrap(),
            },
        )
        .expect("encrypted user database should initialize");
        let now = Utc
            .with_ymd_and_hms(2026, 7, 31, 15, 0, 0)
            .single()
            .unwrap();

        let failed_name_id = CapabilityId::from_bytes([71; 32]);
        let failed_name_key = [71; 32];
        let failed_name = ApprovedPairingClaim {
            capability_id: failed_name_id,
            user_id,
            dek: Some(&dek),
            client_name: "removed staged name",
            permission: crate::client_capabilities::ClientPermission::BalancesRead,
            active_expires_at: None,
        };
        assert!(
            activate_pairing_claim_with_check(&failed_name_key, failed_name, now, |point| {
                (point != ActivationPoint::AfterNameWrite)
                    .then_some(())
                    .ok_or(())
            })
            .is_err()
        );
        assert_eq!(
            crate::db::load_paired_client_name(user_id, failed_name_id).unwrap(),
            None
        );
        assert!(
            crate::db::load_active_client_capability(failed_name_id, now)
                .unwrap()
                .is_none()
        );

        let committed_id = CapabilityId::from_bytes([72; 32]);
        let committed_key = [72; 32];
        let committed_claim = || ApprovedPairingClaim {
            capability_id: committed_id,
            user_id,
            dek: Some(&dek),
            client_name: "committed private name",
            permission: crate::client_capabilities::ClientPermission::BalancesRead,
            active_expires_at: None,
        };
        assert!(
            activate_pairing_claim_with_check(&committed_key, committed_claim(), now, |point| {
                (point != ActivationPoint::AfterAppCommit)
                    .then_some(())
                    .ok_or(())
            })
            .is_err()
        );
        let retry = activate_pairing_claim(&committed_key, committed_claim(), now)
            .expect("retry after app commit should recover durably");
        assert_eq!(retry.user_id, user_id);
        assert_eq!(
            crate::db::load_paired_client_name(user_id, committed_id).unwrap(),
            Some("committed private name".to_owned())
        );

        let store = PairingStore::new();
        let pending_key = [73; 32];
        let pending_verifier = ClientKeyVerifier::from_raw_key(&pending_key);
        let start = PairingStartRequest {
            client_name: "cleanup retry".to_owned(),
            key_verifier: URL_SAFE_NO_PAD.encode(pending_verifier.as_bytes()),
            permissions: vec!["balances_read".to_owned()],
        }
        .validate()
        .unwrap();
        let started = store
            .start(
                now,
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                start,
                [([73; 32], [3; 8])],
                |_| Ok::<_, ()>(false),
            )
            .unwrap();
        store
            .approve(
                now,
                started.capability_id,
                &format_code(&started.code),
                None,
                || {
                    Ok::<_, ()>(ApprovedPairingBinding {
                        user_id,
                        dek: Some(dek.clone()),
                        lease: crate::auth::lifecycle::acquire_pending_pairing_lease(user_id)
                            .unwrap()
                            .unwrap(),
                    })
                },
            )
            .unwrap();
        assert_eq!(
            store.claim(now, started.capability_id, pending_verifier, |claim| {
                let _ = activate_pairing_claim(&pending_key, claim, now)?;
                Err::<ClaimedPairing, _>(())
            }),
            Err(PairingClaimError::Activation)
        );
        let recovered = store
            .claim(now, started.capability_id, pending_verifier, |claim| {
                activate_pairing_claim(&pending_key, claim, now)
            })
            .expect("retry before pending cleanup should converge");
        assert_eq!(recovered.user_id, user_id);
        assert_eq!(
            crate::db::load_client_capabilities_for_user(user_id)
                .unwrap()
                .len(),
            2
        );
    }
}
