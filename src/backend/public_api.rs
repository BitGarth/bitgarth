use crate::backend::ProxyHeaderTrust;
use crate::client_capabilities::{CapabilityId, ClientCapabilityRecord, ClientPermission};
use crate::db::encryption::{ClientKeyWrapper, DbEnvelope, UnlockAuthority, UserDbOpenMode};
use crate::models::UserId;
use crate::pairing::PairingStore;
use axum::Json;
use axum::Router;
use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use chrono::{DateTime, Utc};
use dioxus::server::FullstackState;
use serde::Serialize;
use std::sync::Arc;

use super::ApiErrorEnvelope;
use super::wallets::{
    ProjectedBalanceReason, ProjectedBalanceStatus, WalletBalanceProjection,
    load_wallet_balance_projection,
};

pub(crate) fn router(
    store: Arc<PairingStore>,
    proxy_trust: ProxyHeaderTrust,
) -> Router<FullstackState> {
    let api = Router::<FullstackState>::new()
        .route("/pairings", post(super::pairing::start_pairing))
        .route(
            "/pairings/{pairing_id}/claim",
            post(super::pairing::claim_pairing),
        )
        .route("/wallet-balances", get(wallet_balances))
        .fallback(super::pairing::unimplemented_public_endpoint)
        .layer(middleware::from_fn(no_store))
        .layer(axum::Extension(store))
        .layer(axum::Extension(proxy_trust));
    Router::new().nest("/api/v1", api)
}

struct AuthorizedClientRequest {
    user_id: UserId,
    capability_id: CapabilityId,
    permission: ClientPermission,
    _lease: crate::auth::lifecycle::UserRequestLease,
}

fn unauthorized() -> ApiErrorEnvelope {
    ApiErrorEnvelope::unauthorized("Invalid Client Key")
}

fn record_is_expired(record: &ClientCapabilityRecord, now: DateTime<Utc>) -> bool {
    record
        .expires_at
        .is_some_and(|expires_at| expires_at <= now)
}

fn record_has_active_authority(record: &ClientCapabilityRecord) -> bool {
    record.revoked_at.is_none()
}

fn ensure_permission(
    granted: Option<ClientPermission>,
    required: ClientPermission,
) -> Result<(), ApiErrorEnvelope> {
    if granted == Some(required) {
        Ok(())
    } else {
        Err(ApiErrorEnvelope::forbidden(
            "This paired client does not have balances_read permission.",
        ))
    }
}

fn expire_record(
    record: &ClientCapabilityRecord,
    now: DateTime<Utc>,
) -> Result<(), ApiErrorEnvelope> {
    crate::auth::lifecycle::shutdown_expired_client_capability(
        record.user_id,
        record.capability_id,
        now,
    )
    .map_err(|error| {
        tracing::error!(
            user_id = %record.user_id,
            capability_id = %record.capability_id,
            error = %error,
            "public API: failed to retire expired Client Key"
        );
        ApiErrorEnvelope::internal()
    })?;
    Ok(())
}

fn client_key_open_mode(
    envelope: &DbEnvelope,
    record: &ClientCapabilityRecord,
    raw_key: &[u8; 32],
) -> Result<UserDbOpenMode, ApiErrorEnvelope> {
    match (
        envelope,
        record.wrapped_dek.as_ref(),
        record.wrap_nonce.as_ref(),
    ) {
        (
            DbEnvelope::Encrypted {
                sqlcipher_version, ..
            },
            Some(wrapped_dek),
            Some(wrap_nonce),
        ) => {
            let wrapper = ClientKeyWrapper {
                nonce: wrap_nonce.clone(),
                ciphertext: wrapped_dek.clone(),
            };
            let dek = wrapper
                .unwrap(
                    raw_key,
                    record.user_id,
                    record.capability_id,
                    record.permission,
                )
                .map_err(|_| unauthorized())?;
            Ok(UserDbOpenMode::Encrypted {
                dek,
                authority: UnlockAuthority::ClientKey {
                    capability_id: record.capability_id,
                },
                sqlcipher_compatibility: sqlcipher_version.clone(),
            })
        }
        #[cfg(feature = "dev-config")]
        (DbEnvelope::UnencryptedDev, None, None) => Ok(UserDbOpenMode::UnencryptedDev),
        _ => Err(unauthorized()),
    }
}

fn authorize_client_request(
    request: &Request,
    required_permission: ClientPermission,
) -> Result<AuthorizedClientRequest, ApiErrorEnvelope> {
    let raw_key =
        super::pairing::parse_client_key(request.headers()).map_err(|_| unauthorized())?;
    let verifier = crate::client_capabilities::ClientKeyVerifier::from_raw_key(&raw_key);
    let Some(initial) =
        crate::db::find_capability_identity_by_verifier(verifier).map_err(|error| {
            tracing::error!(error = %error, "public API: Client Key identity lookup failed");
            ApiErrorEnvelope::internal()
        })?
    else {
        return Err(unauthorized());
    };
    let now = Utc::now();
    if !record_has_active_authority(&initial) {
        return Err(unauthorized());
    }
    if record_is_expired(&initial, now) {
        expire_record(&initial, now)?;
        return Err(unauthorized());
    }
    ensure_permission(Some(initial.permission), required_permission)?;

    let lease =
        crate::auth::lifecycle::acquire_client_key_request(initial.user_id, initial.capability_id)
            .map_err(|error| {
                tracing::error!(error = %error, "public API: Client Key lease failed");
                ApiErrorEnvelope::internal()
            })?
            .ok_or_else(unauthorized)?;
    let Some(record) =
        crate::db::load_client_capability(initial.capability_id).map_err(|error| {
            tracing::error!(error = %error, "public API: Client Key recheck failed");
            ApiErrorEnvelope::internal()
        })?
    else {
        return Err(unauthorized());
    };
    if record.user_id != initial.user_id
        || record.key_verifier != verifier
        || !record_has_active_authority(&record)
    {
        return Err(unauthorized());
    }
    let rechecked_at = Utc::now();
    if record_is_expired(&record, rechecked_at) {
        drop(lease);
        expire_record(&record, rechecked_at)?;
        return Err(unauthorized());
    }
    ensure_permission(Some(record.permission), required_permission)?;

    let envelope = crate::db::encryption::read_envelope(record.user_id).map_err(|error| {
        tracing::error!(
            user_id = %record.user_id,
            error = %error,
            "public API: failed to load user database envelope"
        );
        ApiErrorEnvelope::internal()
    })?;
    let open_mode = client_key_open_mode(&envelope, &record, &raw_key)?;
    crate::db::initialize_user_db(record.user_id, open_mode).map_err(|error| {
        tracing::error!(
            user_id = %record.user_id,
            error = %error,
            "public API: failed to open user database"
        );
        ApiErrorEnvelope::internal()
    })?;
    let activity_at = Utc::now();
    if record_is_expired(&record, activity_at) {
        drop(lease);
        expire_record(&record, activity_at)?;
        return Err(unauthorized());
    }
    crate::db::record_client_capability_activity(record.capability_id, record.user_id, activity_at)
        .map_err(|error| {
            tracing::warn!(
                user_id = %record.user_id,
                capability_id = %record.capability_id,
                error = %error,
                "public API: Client Key lost authority before activity update"
            );
            unauthorized()
        })?;

    Ok(AuthorizedClientRequest {
        user_id: record.user_id,
        capability_id: record.capability_id,
        permission: record.permission,
        _lease: lease,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct WalletBalancesResponse {
    wallets: Vec<WalletBalanceResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct WalletBalanceResponse {
    id: String,
    name: String,
    balances: Vec<AssetBalanceResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct AssetBalanceResponse {
    asset_id: String,
    network_id: String,
    unit: String,
    amount: Option<String>,
    status: PublicBalanceStatus,
    reasons: Vec<PublicBalanceReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum PublicBalanceStatus {
    Final,
    Provisional,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum PublicBalanceReason {
    FirstSuccessfulSyncPending,
    InactiveAccountNotSyncing,
}

fn serialize_projection(mut projection: WalletBalanceProjection) -> WalletBalancesResponse {
    projection.wallets.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.id.to_string().cmp(&right.id.to_string()))
    });
    WalletBalancesResponse {
        wallets: projection
            .wallets
            .into_iter()
            .map(|mut wallet| {
                wallet.balances.sort_by(|left, right| {
                    left.asset_id
                        .cmp(&right.asset_id)
                        .then_with(|| left.network_id.cmp(&right.network_id))
                });
                WalletBalanceResponse {
                    id: wallet.id.to_string(),
                    name: wallet.name,
                    balances: wallet
                        .balances
                        .into_iter()
                        .map(|balance| AssetBalanceResponse {
                            asset_id: balance.asset_id,
                            network_id: balance.network_id,
                            unit: balance.unit,
                            amount: balance.amount.map(|amount| {
                                crate::amounts::format_unsigned_amount(
                                    amount,
                                    balance.decimal_precision,
                                )
                            }),
                            status: match balance.status {
                                ProjectedBalanceStatus::Final => PublicBalanceStatus::Final,
                                ProjectedBalanceStatus::Provisional => {
                                    PublicBalanceStatus::Provisional
                                }
                                ProjectedBalanceStatus::Unknown => PublicBalanceStatus::Unknown,
                            },
                            reasons: balance
                                .reasons
                                .into_iter()
                                .map(|reason| match reason {
                                    ProjectedBalanceReason::FirstSuccessfulSyncPending => {
                                        PublicBalanceReason::FirstSuccessfulSyncPending
                                    }
                                    ProjectedBalanceReason::InactiveAccountNotSyncing => {
                                        PublicBalanceReason::InactiveAccountNotSyncing
                                    }
                                })
                                .collect(),
                        })
                        .collect(),
                }
            })
            .collect(),
    }
}

async fn wallet_balances(request: Request) -> Response {
    let authorized = match authorize_client_request(&request, ClientPermission::BalancesRead) {
        Ok(authorized) => authorized,
        Err(error) => {
            tracing::warn!(
                status = error.code.status_code().as_u16(),
                error_code = ?error.code,
                "public API: wallet balances failed"
            );
            return super::pairing::api_error(error);
        }
    };
    debug_assert_eq!(authorized.permission, ClientPermission::BalancesRead);
    let projection = match load_wallet_balance_projection(authorized.user_id) {
        Ok(projection) => projection,
        Err(error) => {
            tracing::warn!(
                user_id = %authorized.user_id,
                capability_id = %authorized.capability_id,
                status = StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                error = %error,
                "public API: wallet balances failed"
            );
            return super::pairing::api_error(ApiErrorEnvelope::internal());
        }
    };
    tracing::info!(
        user_id = %authorized.user_id,
        capability_id = %authorized.capability_id,
        "public API: wallet balances retrieved"
    );
    (StatusCode::OK, Json(serialize_projection(projection))).into_response()
}

async fn no_store(request: axum::extract::Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, max-age=0"),
    );
    response
}

pub(crate) async fn browser_pairing_no_store(
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let no_store = matches!(
        request.uri().path(),
        "/pair"
            | "/_app/pairings/review"
            | "/_app/pairings/approve"
            | "/_app/pairings/deny"
            | "/_app/pairings/clients"
            | "/_app/pairings/revoke"
    );
    let mut response = next.run(request).await;
    if no_store {
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store, max-age=0"),
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amounts::UnsignedAmount;
    use crate::backend::wallets::{ProjectedAssetBalance, ProjectedWalletBalance};
    use std::str::FromStr;

    #[cfg(feature = "dev-config")]
    fn fixed_client_capability_record(
        wrapped_dek: Option<Vec<u8>>,
        wrap_nonce: Option<Vec<u8>>,
    ) -> ClientCapabilityRecord {
        ClientCapabilityRecord {
            capability_id: CapabilityId::from_bytes([41_u8; 32]),
            user_id: UserId::from_str("01KGQYDBAH5B0JD0BSF2VX95FR").unwrap(),
            key_verifier: crate::client_capabilities::ClientKeyVerifier::from_raw_key(&[42_u8; 32]),
            wrapped_dek,
            wrap_nonce,
            permission: ClientPermission::BalancesRead,
            created_at: DateTime::from_str("2026-08-06T12:00:00Z").unwrap(),
            expires_at: None,
            last_used_at: None,
            revoked_at: None,
        }
    }

    #[cfg(feature = "dev-config")]
    #[test]
    fn client_key_open_mode_rejects_database_mode_mismatches() {
        let (encrypted, _) = DbEnvelope::new_encrypted("SecurePass123").unwrap();
        let keyless_record = fixed_client_capability_record(None, None);
        assert!(
            client_key_open_mode(&encrypted, &keyless_record, &[42_u8; 32])
                .unwrap_err()
                .is_unauthorized()
        );

        let wrapped_record = fixed_client_capability_record(Some(vec![1_u8]), Some(vec![2_u8; 12]));
        assert!(
            client_key_open_mode(
                &DbEnvelope::unencrypted_dev(),
                &wrapped_record,
                &[42_u8; 32],
            )
            .unwrap_err()
            .is_unauthorized()
        );
    }

    #[cfg(feature = "dev-config")]
    #[test]
    fn client_key_open_mode_accepts_keyless_unencrypted_dev() {
        let record = fixed_client_capability_record(None, None);
        let open_mode =
            client_key_open_mode(&DbEnvelope::unencrypted_dev(), &record, &[42_u8; 32]).unwrap();

        assert!(matches!(open_mode, UserDbOpenMode::UnencryptedDev));
    }

    #[test]
    fn serializer_matches_the_fixed_wallet_balance_contract() {
        let projection = WalletBalanceProjection {
            wallets: vec![
                ProjectedWalletBalance {
                    id: crate::wallets::WalletId::from_str("01ARZ3NDEKTSV4RRFFQ69G5FAX").unwrap(),
                    name: "Manual".to_string(),
                    balances: vec![ProjectedAssetBalance {
                        asset_id: "cardano".to_string(),
                        network_id: "cardano-mainnet".to_string(),
                        unit: "ADA".to_string(),
                        amount: Some(UnsignedAmount::from_u128(1_234_000)),
                        decimal_precision: 6,
                        status: ProjectedBalanceStatus::Final,
                        reasons: Vec::new(),
                    }],
                },
                ProjectedWalletBalance {
                    id: crate::wallets::WalletId::from_str("01ARZ3NDEKTSV4RRFFQ69G5FAW").unwrap(),
                    name: "Empty".to_string(),
                    balances: Vec::new(),
                },
                ProjectedWalletBalance {
                    id: crate::wallets::WalletId::from_str("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap(),
                    name: "Alpha".to_string(),
                    balances: vec![
                        ProjectedAssetBalance {
                            asset_id: "usd-coin".to_string(),
                            network_id: "polygon-mainnet".to_string(),
                            unit: "USDC".to_string(),
                            amount: None,
                            decimal_precision: 6,
                            status: ProjectedBalanceStatus::Unknown,
                            reasons: Vec::new(),
                        },
                        ProjectedAssetBalance {
                            asset_id: "ethereum".to_string(),
                            network_id: "ethereum-mainnet".to_string(),
                            unit: "ETH".to_string(),
                            amount: Some(UnsignedAmount::from_u128(2_000_000_000_000_000_000)),
                            decimal_precision: 18,
                            status: ProjectedBalanceStatus::Provisional,
                            reasons: vec![
                                ProjectedBalanceReason::FirstSuccessfulSyncPending,
                                ProjectedBalanceReason::InactiveAccountNotSyncing,
                            ],
                        },
                        ProjectedAssetBalance {
                            asset_id: "bitcoin".to_string(),
                            network_id: "bitcoin-mainnet".to_string(),
                            unit: "BTC".to_string(),
                            amount: Some(UnsignedAmount::from_u128(123_450_000)),
                            decimal_precision: 8,
                            status: ProjectedBalanceStatus::Final,
                            reasons: Vec::new(),
                        },
                    ],
                },
            ],
        };

        let actual = serde_json::to_value(serialize_projection(projection)).unwrap();
        let expected: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/client_api/wallet-balances.json"
        ))
        .unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn active_client_without_the_required_permission_is_forbidden() {
        let error = ensure_permission(None, ClientPermission::BalancesRead).unwrap_err();
        assert_eq!(error.code.status_code(), StatusCode::FORBIDDEN);
    }
}
