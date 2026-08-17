use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode as AxumStatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use dioxus::fullstack::StatusCode;
use ed25519_dalek::{Signer, SigningKey};
use serde::Deserialize;
use serde_json::{Value, json};
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use crate::models::{AuthResponse, UserId};
use crate::payments::keys::{expected_signing_key_hash, set_signing_public_key_override_for_test};
use crate::payments::types::{
    CAPABILITY_SCHEMA_VERSION_V3, EntitlementCapabilities, EntitlementHolderId, EntitlementTier,
    PaymentAmount, PaymentOrderId, PaymentOrderStatus, PaymentSecret, ProductOptionId, ProductTier,
    SubscriptionSubjectId, TokenClaims, TokenId,
};
use crate::payments::views::{
    PaymentOrderStatusView, PaymentStateStatus, PaymentStateView, PremiumOrderLaunchView,
    PremiumTopUpLaunchView,
};

use super::fixtures::register_user;
use super::setup_test_server;

const TEST_PUBLIC_KEY_B64: &str = "O2onvM62pC1io6jQKm8Nc2UyFXcd4kOmOsBIoYtZ2ik";
const ORDER_ID: &str = "01JQABCDEF000000000000000E";
const PAYMENT_ATTEMPT_ID: &str = "01JQABCDEF000000000000000F";
const ORDER_SECRET: &str = "frPMkDek45GSAMEAFTXV5ORxF8p3c5_MqPg7Zq-bNuI";
const MANAGEMENT_SECRET: &str = "5FuYMBR_MhwubKAJQeNMrUH0JD3PvFuyt3sfFh0ezLw";
const TOKEN_ID: &str = "01JQABCDEF000000000000000F";
const SUBSCRIPTION_SUBJECT_ID: &str = "01JQABCDEF000000000000000G";
const FAILED_ORDER_ID: &str = "01JQABCDEF000000000000000H";

#[derive(Clone, Copy)]
enum MockOrderOutcome {
    Pending,
    Paid,
    MalformedPaidToken,
    Verifying,
    AdditionalPaymentRequired,
    ManualReviewExpired,
}

#[derive(Clone, Copy)]
enum MockRefreshOutcome {
    Active,
    RevokedTokenSuperseded,
    RevokedExpired,
}

#[derive(Clone)]
enum MockHistoryOutcome {
    Empty,
    OrderPaid,
    BlockedOrderPaid(Arc<tokio::sync::Notify>),
    ServerError,
    MetadataOnly,
}

struct MockCentral {
    base_url: String,
    state: Arc<Mutex<MockCentralState>>,
}

struct MockCentralState {
    expected_signing_key_hash: String,
    reject_signing_key: bool,
    order_outcome: MockOrderOutcome,
    refresh_outcome: MockRefreshOutcome,
    refresh_request_count: u32,
    last_refresh_last_known_token: Option<Option<String>>,
    history_outcome: MockHistoryOutcome,
    history_request_count: u32,
    product_options_response: Option<Value>,
    product_options_request_count: u32,
}

#[derive(Deserialize)]
struct CreateOrderRequest {
    entitlement_holder_id: EntitlementHolderId,
    product_option_id: ProductOptionId,
}

#[derive(Deserialize)]
struct RefreshRequest {
    entitlement_holder_id: EntitlementHolderId,
    token_id: TokenId,
    last_known_token: Option<String>,
}

impl MockCentral {
    async fn start(expected_signing_key_hash: String) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock Central should bind");
        let base_url = format!(
            "http://{}",
            listener
                .local_addr()
                .expect("mock Central should have addr")
        );
        let state = Arc::new(Mutex::new(MockCentralState {
            expected_signing_key_hash,
            reject_signing_key: false,
            order_outcome: MockOrderOutcome::Pending,
            refresh_outcome: MockRefreshOutcome::Active,
            refresh_request_count: 0,
            last_refresh_last_known_token: None,
            history_outcome: MockHistoryOutcome::Empty,
            history_request_count: 0,
            product_options_response: None,
            product_options_request_count: 0,
        }));
        let router = Router::new()
            .route(
                "/api/v1/payments/product-options",
                get(payment_product_options),
            )
            .route("/api/v1/payments/orders/session", post(create_order))
            .route(
                "/api/v1/payments/orders/{order_id}/status",
                get(order_status),
            )
            .route(
                "/api/v1/payments/subscription/refresh",
                post(refresh_subscription),
            )
            .route(
                "/api/v1/payments/subscription/history",
                get(subscription_history),
            )
            .with_state(Arc::clone(&state));

        tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("mock Central should serve");
        });

        Self { base_url, state }
    }

    fn reject_signing_key(&self) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .reject_signing_key = true;
    }

    fn set_order_outcome(&self, outcome: MockOrderOutcome) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .order_outcome = outcome;
    }

    fn set_refresh_outcome(&self, outcome: MockRefreshOutcome) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .refresh_outcome = outcome;
    }

    fn refresh_request_count(&self) -> u32 {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .refresh_request_count
    }

    fn last_refresh_last_known_token(&self) -> Option<Option<String>> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .last_refresh_last_known_token
            .clone()
    }

    fn set_history_outcome(&self, outcome: MockHistoryOutcome) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .history_outcome = outcome;
    }

    fn history_request_count(&self) -> u32 {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .history_request_count
    }

    fn set_product_options_response(&self, response: Value) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .product_options_response = Some(response);
    }

    fn product_options_request_count(&self) -> u32 {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .product_options_request_count
    }
}

fn central_guard(mock: &MockCentral) -> crate::payments::client::CentralBaseUrlOverrideGuard {
    crate::payments::client::set_central_base_url_override_for_test(mock.base_url.clone())
}

fn unauthorized() -> Response {
    (
        AxumStatusCode::UNAUTHORIZED,
        Json(json!({"error": "unauthorized"})),
    )
        .into_response()
}

fn upgrade_required() -> Response {
    (
        AxumStatusCode::UPGRADE_REQUIRED,
        Json(json!({
            "error_code": "invalid_expected_signing_key",
            "message": "upgrade required"
        })),
    )
        .into_response()
}

fn signing_key_error(headers: &HeaderMap, state: &MockCentralState) -> Option<Response> {
    if state.reject_signing_key {
        return Some(upgrade_required());
    }

    let Some(actual) = headers
        .get("X-BitGarth-Expected-Signing-Key-Hash")
        .and_then(|value| value.to_str().ok())
    else {
        return Some(upgrade_required());
    };

    if actual != state.expected_signing_key_hash {
        return Some(upgrade_required());
    }

    None
}

async fn payment_product_options(
    State(state): State<Arc<Mutex<MockCentralState>>>,
    headers: HeaderMap,
) -> Response {
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.product_options_request_count += 1;
    if let Some(response) = signing_key_error(&headers, &state) {
        return response;
    }
    if let Some(response) = state.product_options_response.clone() {
        return Json(response).into_response();
    }

    Json(default_product_options_response()).into_response()
}

fn product_option_json(
    id: &str,
    quantity: u16,
    unit: &str,
    label: &str,
    minor_units: u64,
) -> Value {
    product_option_json_with_presentation(id, quantity, unit, label, minor_units, false, None)
}

fn product_option_json_with_presentation(
    id: &str,
    quantity: u16,
    unit: &str,
    label: &str,
    minor_units: u64,
    is_default: bool,
    badge: Option<&str>,
) -> Value {
    json!({
        "id": id,
        "term": {
            "quantity": quantity,
            "unit": unit,
            "label": label
        },
        "price": {
            "minor_units": minor_units,
            "currency": "USD",
            "currency_symbol": "$",
            "decimal_precision": 2
        },
        "presentation": {
            "is_default": is_default,
            "badge": badge
        }
    })
}

fn tier_json(
    tier: &str,
    display_name: &str,
    synced_accounts: u16,
    max_transactions_per_account: u32,
    purchase_options: Vec<Value>,
) -> Value {
    json!({
        "tier": tier,
        "display_name": display_name,
        "capability_schema_version": CAPABILITY_SCHEMA_VERSION_V3,
        "capabilities": {
            "limits": {
                "accounts": {
                    "total": synced_accounts
                },
                "synced_accounts": synced_accounts,
                "history": {
                    "max_transactions_per_account": max_transactions_per_account
                }
            },
            "features": {
                "historical_sync": false,
                "transaction_history_sync": max_transactions_per_account > 0,
                "balance_sync": true,
                "exchange_rates_current": true,
                "exchange_rates_history": true,
                "price_overrides": true,
                "balance_assertions": true,
                "hledger_export": true,
                "tax_reports": true
            }
        },
        "presentation": default_tier_presentation(tier, display_name, synced_accounts),
        "purchase_options": purchase_options
    })
}

fn default_tier_presentation(tier: &str, display_name: &str, synced_accounts: u16) -> Value {
    let summary = format!("{display_name} test tier — {synced_accounts} synced accounts.");
    let bullets = vec![format!("**{synced_accounts}** synced accounts")];
    let mut value = json!({
        "summary": summary,
        "bullets": bullets,
    });
    // Mirror production: Basic gets the featured ribbon.
    if tier == "basic" {
        value["is_featured"] = json!(true);
        value["ribbon_label"] = json!("Early adopter discount");
    }
    value
}

fn product_options_response(tiers: Vec<Value>) -> Value {
    json!({
        "catalog_schema_version": 4,
        "tiers": tiers
    })
}

fn product_options_response_with_free_accounts(accounts: u16) -> Value {
    product_options_response(vec![
        tier_json("free", "Free", accounts, 0, Vec::new()),
        tier_json("basic", "Basic", 10, 10000, Vec::new()),
        tier_json(
            "premium",
            "Premium",
            50,
            50000,
            vec![product_option_json_with_presentation(
                "premium_12_months_usd",
                12,
                "month",
                "1 year",
                123,
                true,
                Some("Best value"),
            )],
        ),
    ])
}

fn product_options_response_with_upgrade_required(mut response: Value) -> Value {
    response["app_compatibility"] = json!({
        "status": "upgrade_required",
        "detail": "BitGarth needs an update before paid plans can be purchased or refreshed.",
        "minimum_app_version": "9.9.9"
    });
    response
}

fn default_product_options_response() -> Value {
    let mut response = product_options_response(vec![
        tier_json("free", "Free", 50, 0, Vec::new()),
        tier_json("basic", "Basic", 10, 10000, Vec::new()),
        tier_json(
            "premium",
            "Premium",
            50,
            50000,
            vec![product_option_json_with_presentation(
                "premium_12_months_usd",
                12,
                "month",
                "1 year",
                123,
                true,
                Some("Best value"),
            )],
        ),
    ]);
    response["pricing_summary"] = json!("**Free** tracks holdings. **Paid** does the accounting.");
    response
}

async fn create_order(
    State(state): State<Arc<Mutex<MockCentralState>>>,
    headers: HeaderMap,
    Json(request): Json<CreateOrderRequest>,
) -> Response {
    if let Some(response) = signing_key_error(
        &headers,
        &state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    ) {
        return response;
    }

    Json(json!({
        "order_id": ORDER_ID,
        "product_option_id": request.product_option_id,
        "order_secret": ORDER_SECRET,
        "merchant_id": "8MY8BXTU15",
        "order_amount": {
            "minor_units": 999,
            "currency": "USD",
            "currency_symbol": "$",
            "decimal_precision": 2
        },
        "payment_attempt": {
            "payment_attempt_id": PAYMENT_ATTEMPT_ID,
            "provider": "atlos",
            "atlos_order_id": PAYMENT_ATTEMPT_ID,
            "amount": {
                "minor_units": 999,
                "currency": "USD",
                "currency_symbol": "$",
                "decimal_precision": 2
            }
        },
        "management_secret": MANAGEMENT_SECRET,
        "holder_seen": request.entitlement_holder_id.to_storage_value()
    }))
    .into_response()
}

async fn order_status(
    State(state): State<Arc<Mutex<MockCentralState>>>,
    Path(order_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(response) = signing_key_error(&headers, &state) {
        return response;
    }
    if headers
        .get("Authorization")
        .and_then(|value| value.to_str().ok())
        != Some(&format!("Bearer {ORDER_SECRET}"))
    {
        return unauthorized();
    }
    if order_id != ORDER_ID {
        return (AxumStatusCode::NOT_FOUND, Json(json!({"error": "missing"}))).into_response();
    }

    match state.order_outcome {
        MockOrderOutcome::Pending => Json(json!({
            "status": "pending",
            "verification_state": "awaiting_payment",
            "next_action": "keep_polling",
            "payments": []
        }))
        .into_response(),
        MockOrderOutcome::Paid => {
            let claims = token_claims(
                EntitlementHolderId::from_str(state_holder_from_order_request().as_str())
                    .expect("test holder id should parse"),
            );
            let token = sign_token(&claims);
            Json(json!({
                "status": "paid",
                "verification_state": "premium_granted",
                "next_action": "unlock_premium",
                "payments": [],
                "entitlement_token": token,
                "token_id": TOKEN_ID,
                "subscription_valid_until": claims.subscription_valid_until,
                "token_expires_at": claims.token_expires_at,
                "paid_at": Utc::now()
            }))
            .into_response()
        }
        MockOrderOutcome::MalformedPaidToken => {
            let claims = token_claims(
                EntitlementHolderId::from_str(state_holder_from_order_request().as_str())
                    .expect("test holder id should parse"),
            );
            Json(json!({
                "status": "paid",
                "verification_state": "premium_granted",
                "next_action": "unlock_premium",
                "payments": [],
                "entitlement_token": "not-a-signed-token",
                "token_id": TOKEN_ID,
                "subscription_valid_until": claims.subscription_valid_until,
                "token_expires_at": claims.token_expires_at,
                "paid_at": Utc::now()
            }))
            .into_response()
        }
        MockOrderOutcome::Verifying => Json(json!({
            "status": "pending",
            "verification_state": "payment_confirmed_unverified",
            "next_action": "keep_polling",
            "payments": [{
                "payment_id": "19A2D79298D2BC37A7D9569D8A",
                "payment_attempt_id": PAYMENT_ATTEMPT_ID,
                "status": "confirmed",
                "paid_order_amount": {
                    "minor_units": 999,
                    "currency": "USD",
                    "decimal_precision": 2
                },
                "paid_asset_amount": {
                    "amount": "0.00002614",
                    "asset_code": "XMR",
                    "blockchain_code": "XMR"
                },
                "blockchain_hash": "d4331a38c1214af749eb5c12e7343156465fce70aa2df1c62bce626be0c58613",
                "confirmed_at": "2026-04-21T19:11:45Z",
                "seen_at": "2026-04-21T19:27:31.110404Z"
            }]
        }))
        .into_response(),
        MockOrderOutcome::AdditionalPaymentRequired => Json(json!({
            "status": "pending",
            "verification_state": "additional_payment_required",
            "next_action": "request_additional_payment",
            "paid_amount_minor_units": 800,
            "remaining_amount": {
                "minor_units": 199,
                "currency": "USD",
                "decimal_precision": 2
            },
            "additional_payment_request": {
                "payment_attempt_id": "01JQABCDEF000000000000000G",
                "provider": "atlos",
                "merchant_id": "8MY8BXTU15",
                "atlos_order_id": "01JQABCDEF000000000000000G",
                "amount": {
                    "minor_units": 199,
                    "currency": "USD",
                    "decimal_precision": 2
                }
            },
            "payments": [{
                "payment_id": "19A2D79298D2BC37A7D9569D8A",
                "payment_attempt_id": PAYMENT_ATTEMPT_ID,
                "status": "confirmed",
                "paid_order_amount": {
                    "minor_units": 800,
                    "currency": "USD",
                    "decimal_precision": 2
                },
                "paid_asset_amount": {
                    "amount": "0.00002000",
                    "asset_code": "XMR",
                    "blockchain_code": "XMR"
                },
                "blockchain_hash": "d4331a38c1214af749eb5c12e7343156465fce70aa2df1c62bce626be0c58613",
                "confirmed_at": "2026-04-21T19:11:45Z",
                "seen_at": "2026-04-21T19:27:31.110404Z"
            }]
        }))
        .into_response(),
        MockOrderOutcome::ManualReviewExpired => Json(json!({
            "status": "expired",
            "verification_state": "under_manual_review",
            "next_action": "show_manual_review",
            "manual_review": {
                "reason": "amount_mismatch",
                "resolved": false
            },
            "payments": [{
                "payment_id": "19A2D79298D2BC37A7D9569D8A",
                "payment_attempt_id": PAYMENT_ATTEMPT_ID,
                "status": "confirmed",
                "paid_order_amount": {
                    "minor_units": 800,
                    "currency": "USD",
                    "decimal_precision": 2
                },
                "paid_asset_amount": {
                    "amount": "0.00002614",
                    "asset_code": "XMR",
                    "blockchain_code": "XMR"
                },
                "blockchain_hash": "d4331a38c1214af749eb5c12e7343156465fce70aa2df1c62bce626be0c58613",
                "confirmed_at": "2026-04-21T19:11:45Z",
                "seen_at": "2026-04-21T19:27:31.110404Z"
            }]
        }))
        .into_response(),
    }
}

async fn refresh_subscription(
    State(state): State<Arc<Mutex<MockCentralState>>>,
    headers: HeaderMap,
    Json(request): Json<RefreshRequest>,
) -> Response {
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(response) = signing_key_error(&headers, &state) {
        return response;
    }
    if headers
        .get("Authorization")
        .and_then(|value| value.to_str().ok())
        != Some(&format!("Bearer {MANAGEMENT_SECRET}"))
    {
        return unauthorized();
    }
    if request.token_id.to_storage_value() != TOKEN_ID {
        return (AxumStatusCode::NOT_FOUND, Json(json!({"error": "missing"}))).into_response();
    }

    state.refresh_request_count += 1;
    state.last_refresh_last_known_token = Some(request.last_known_token.clone());

    match state.refresh_outcome {
        MockRefreshOutcome::Active => {
            let claims = token_claims(request.entitlement_holder_id);
            let token = sign_token(&claims);
            Json(json!({
                "status": "active",
                "entitlement_token": token,
                "token_id": TOKEN_ID,
                "subscription_valid_until": claims.subscription_valid_until,
                "token_expires_at": claims.token_expires_at
            }))
            .into_response()
        }
        MockRefreshOutcome::RevokedExpired => Json(json!({
            "status": "revoked",
            "reason": "expired"
        }))
        .into_response(),
        MockRefreshOutcome::RevokedTokenSuperseded => Json(json!({
            "status": "revoked",
            "reason": "token_superseded"
        }))
        .into_response(),
    }
}

async fn subscription_history(
    State(state): State<Arc<Mutex<MockCentralState>>>,
    headers: HeaderMap,
) -> Response {
    let history_outcome = {
        let mut state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.history_request_count += 1;
        if let Some(response) = signing_key_error(&headers, &state) {
            return response;
        }
        if headers
            .get("Authorization")
            .and_then(|value| value.to_str().ok())
            != Some(&format!("Bearer {MANAGEMENT_SECRET}"))
        {
            return unauthorized();
        }
        state.history_outcome.clone()
    };

    paid_history_response(history_outcome).await
}

async fn paid_history_response(history_outcome: MockHistoryOutcome) -> Response {
    match history_outcome {
        MockHistoryOutcome::BlockedOrderPaid(notify) => {
            notify.notified().await;
            paid_history_payload()
        }
        MockHistoryOutcome::OrderPaid => paid_history_payload(),
        MockHistoryOutcome::Empty => Json(json!({
            "orders": [],
            "active_token": null,
            "premium_access_token": null,
            "token_id": null,
            "subscription_valid_until": null,
            "token_expires_at": null
        }))
        .into_response(),
        MockHistoryOutcome::ServerError => (
            AxumStatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "central unavailable"})),
        )
            .into_response(),
        MockHistoryOutcome::MetadataOnly => {
            let holder = EntitlementHolderId::from_str(state_holder_from_order_request().as_str())
                .expect("test holder id should parse");
            let claims = token_claims(holder);
            Json(json!({
                "active_token": {
                    "token_id": TOKEN_ID,
                    "tier": "premium",
                    "capability_set_id": claims.capability_set_id,
                    "capability_schema_version": claims.capability_schema_version,
                    "capabilities": serde_json::to_value(&claims.capabilities)
                        .expect("test capabilities should serialize"),
                    "token_expires_at": claims.token_expires_at
                },
                "token_id": null,
                "subscription_valid_until": claims.subscription_valid_until,
                "token_expires_at": null,
                "orders": []
            }))
            .into_response()
        }
    }
}

fn paid_history_payload() -> Response {
    let holder = EntitlementHolderId::from_str(state_holder_from_order_request().as_str())
        .expect("test holder id should parse");
    let claims = token_claims(holder);
    let token = sign_token(&claims);
    Json(json!({
        "entitlement_token": token,
        "token_id": TOKEN_ID,
        "subscription_valid_until": claims.subscription_valid_until,
        "token_expires_at": claims.token_expires_at,
        "active_token": {
            "token_id": TOKEN_ID,
            "tier": "premium",
            "capability_set_id": claims.capability_set_id,
            "capability_schema_version": claims.capability_schema_version,
            "capabilities": serde_json::to_value(&claims.capabilities)
                .expect("test capabilities should serialize"),
            "token_expires_at": claims.token_expires_at
        },
        "orders": [{ "order_id": ORDER_ID, "status": "paid", "paid_at": Utc::now() }]
    }))
    .into_response()
}

fn state_holder_from_order_request() -> String {
    // The app persists the generated holder ID from create-order before polling.
    // The integration test reads it from the DB and updates the mock through the token itself.
    TEST_HOLDER_ID
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

static TEST_HOLDER_ID: once_cell::sync::Lazy<Mutex<String>> =
    once_cell::sync::Lazy::new(|| Mutex::new(String::new()));

fn token_claims(holder: EntitlementHolderId) -> TokenClaims {
    let now = Utc::now();
    TokenClaims {
        token_id: TokenId::from_str(TOKEN_ID).expect("test token id should parse"),
        subscription_subject_id: SubscriptionSubjectId::from_str(SUBSCRIPTION_SUBJECT_ID)
            .expect("test subscription subject id should parse"),
        entitlement_holder_id: holder,
        tier: EntitlementTier::Premium,
        capability_set_id: Some("capset_premium_v1".to_string()),
        capability_schema_version: CAPABILITY_SCHEMA_VERSION_V3,
        capabilities: EntitlementCapabilities::v3_from_parts(50, 50000, true),
        subscription_valid_until: now + Duration::days(365),
        token_expires_at: now + Duration::days(7),
        issued_at: now - Duration::minutes(1),
    }
}

fn sign_token(claims: &TokenClaims) -> String {
    let claims_json = serde_json::to_vec(claims).expect("claims should serialize");
    let signing_key = SigningKey::from_bytes(&[0_u8; 32]);
    let signature = signing_key.sign(&claims_json);
    format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(claims_json),
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    )
}

async fn registered_user_id(server: &super::IntegrationTestServer) -> UserId {
    register_user(server).await;
    let response = server.get("/_app/auth/me").await;
    response.assert_status_ok();
    let body: AuthResponse = response.json();
    body.user.user_id
}

async fn activate_paid_entitlement(
    server: &super::IntegrationTestServer,
    user_id: UserId,
    mock: &MockCentral,
) {
    server
        .post("/_app/user/payments/premium/start")
        .json(&json!({ "product_option_id": "premium_12_months_usd" }))
        .await
        .assert_status_ok();
    let subject = crate::db::payments::load_payment_subject(user_id)
        .expect("subject query should succeed")
        .expect("subject should exist");
    *TEST_HOLDER_ID
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
        subject.entitlement_holder_id.to_storage_value();
    mock.set_order_outcome(MockOrderOutcome::Paid);
    server
        .post("/_app/user/payments/premium/poll")
        .json(&json!({ "order_id": ORDER_ID }))
        .await
        .assert_status_ok();
}

fn assert_no_payment_secrets(value: &Value) {
    let serialized = value.to_string();
    assert!(!serialized.contains(ORDER_SECRET));
    assert!(!serialized.contains(MANAGEMENT_SECRET));
    assert!(!serialized.contains("entitlement_token"));
    assert!(!serialized.contains("premium_access_token"));
}

fn assert_free_entitlement_state(state: &PaymentStateView) {
    let expected_free_account_limit = crate::payments::free_tier::baked_free_tier_snapshot()
        .capabilities
        .limits
        .accounts
        .total;

    assert_eq!(state.tier, "free");
    assert_eq!(state.tier_display_name, "Free");
    assert_eq!(state.sync_account_slots_limit, expected_free_account_limit);
    assert!(!state.historical_backfill_enabled);
    assert_eq!(state.historical_backfill_transactions_per_account, 0);
    assert!(state.paid_through.is_none());
}

fn assert_single_app_entitlement_snapshot(user_id: UserId, expected_source: &str) {
    let snapshots =
        crate::db::entitlement_snapshots::load_app_entitlement_snapshots_for_user(user_id)
            .expect("app entitlement snapshots should load");
    assert_eq!(snapshots.len(), 1);
    let snapshot = &snapshots[0];
    assert_eq!(snapshot.user_id, user_id);
    assert_eq!(snapshot.source, expected_source);
    assert_eq!(snapshot.entitlement_tier.as_str(), "premium");
    assert_eq!(
        snapshot
            .token_id
            .expect("snapshot should store token id")
            .to_storage_value(),
        TOKEN_ID
    );
    assert_eq!(
        snapshot
            .subscription_subject_id
            .expect("snapshot should store subscription subject")
            .to_storage_value(),
        SUBSCRIPTION_SUBJECT_ID
    );
    assert!(snapshot.subscription_valid_until.is_some());
    assert!(snapshot.token_expires_at.is_some());
    assert!(snapshot.capability_set_id.is_some());
    assert_eq!(
        snapshot.capability_schema_version,
        CAPABILITY_SCHEMA_VERSION_V3
    );

    let capabilities_json = snapshot
        .capabilities_json
        .as_deref()
        .expect("snapshot should store capability json");
    assert!(!capabilities_json.contains(ORDER_SECRET));
    assert!(!capabilities_json.contains(MANAGEMENT_SECRET));
}

fn assert_no_app_entitlement_snapshots(user_id: UserId) {
    let snapshots =
        crate::db::entitlement_snapshots::load_app_entitlement_snapshots_for_user(user_id)
            .expect("app entitlement snapshots should load");
    assert!(snapshots.is_empty());
}

async fn wait_for_payment_refresh_status(user_id: UserId, expected: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let subject = crate::db::payments::load_payment_subject(user_id)
            .expect("subject query should succeed")
            .expect("subject should exist");
        if subject.last_refresh_status.as_deref() == Some(expected) {
            return;
        }
        if std::time::Instant::now() >= deadline {
            panic!("payment refresh status did not become {expected} before timeout");
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

async fn wait_for_single_app_entitlement_snapshot(user_id: UserId, expected_source: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let snapshots =
            crate::db::entitlement_snapshots::load_app_entitlement_snapshots_for_user(user_id)
                .expect("app entitlement snapshots should load");
        if snapshots.len() == 1 && snapshots[0].source == expected_source {
            assert_single_app_entitlement_snapshot(user_id, expected_source);
            return;
        }
        if std::time::Instant::now() >= deadline {
            panic!("app entitlement snapshot {expected_source} was not recorded before timeout");
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

async fn wait_for_history_request_count(mock: &MockCentral, expected: u32) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if mock.history_request_count() >= expected {
            return;
        }
        if std::time::Instant::now() >= deadline {
            panic!("history request count did not reach {expected} before timeout");
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

async fn wait_for_free_tier_cache_accounts(expected: u16) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if let Some(cached) =
            crate::db::load_free_tier_entitlement_cache().expect("free tier cache should load")
            && cached.capabilities.limits.accounts.total == expected
        {
            return;
        }
        if std::time::Instant::now() >= deadline {
            panic!("free tier cache did not reach {expected} accounts before timeout");
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

async fn wait_for_login_recovery_complete(user_id: UserId) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let has_active_history = crate::db::payments::load_active_token_history(user_id)
            .expect("history query should succeed")
            .is_some();
        let subject = crate::db::payments::load_payment_subject(user_id)
            .expect("subject query should succeed")
            .expect("subject should exist");
        let snapshots =
            crate::db::entitlement_snapshots::load_app_entitlement_snapshots_for_user(user_id)
                .expect("app entitlement snapshots should load");
        if has_active_history
            && subject.last_refresh_status.as_deref() == Some("active")
            && snapshots.len() == 1
            && snapshots[0].source == "login_refresh"
        {
            return;
        }
        if std::time::Instant::now() >= deadline {
            panic!("login recovery did not complete before timeout");
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn payment_page_refreshes_stale_entitlement_state() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let user_id = registered_user_id(&server).await;
    activate_paid_entitlement(&server, user_id, &mock).await;

    crate::db::payments::record_payment_refresh_status(
        user_id,
        "error",
        Utc::now() - Duration::hours(25),
    )
    .expect("refresh timestamp should update");
    let refresh_count_before = mock.refresh_request_count();

    let response = server.get("/_app/user/payments/state-refresh").await;
    response.assert_status_ok();
    let state: PaymentStateView = response.json();

    assert_eq!(state.status, PaymentStateStatus::Active);
    assert_eq!(mock.refresh_request_count(), refresh_count_before + 1);
    let subject = crate::db::payments::load_payment_subject(user_id)
        .expect("subject query should succeed")
        .expect("subject should exist");
    assert_eq!(subject.last_refresh_status.as_deref(), Some("active"));
    assert_single_app_entitlement_snapshot(user_id, "payments_refresh");
}

#[tokio::test(flavor = "current_thread")]
async fn login_refreshes_stale_entitlement_state_without_blocking_login() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let username = register_user(&server).await;
    let response = server.get("/_app/auth/me").await;
    response.assert_status_ok();
    let body: AuthResponse = response.json();
    let user_id = body.user.user_id;
    activate_paid_entitlement(&server, user_id, &mock).await;

    crate::db::payments::record_payment_refresh_status(
        user_id,
        "error",
        Utc::now() - Duration::hours(25),
    )
    .expect("refresh timestamp should update");
    server.post("/_app/auth/logout").await.assert_status_ok();
    let refresh_count_before = mock.refresh_request_count();

    let response = server
        .post("/_app/auth/login")
        .json(&json!({
            "username": username,
            "password": "SecurePass123"
        }))
        .await;

    response.assert_status_ok();
    wait_for_payment_refresh_status(user_id, "active").await;
    assert_eq!(mock.refresh_request_count(), refresh_count_before + 1);
    wait_for_single_app_entitlement_snapshot(user_id, "login_refresh").await;
}

#[tokio::test(flavor = "current_thread")]
async fn login_refresh_updates_free_tier_cache_for_never_paid_user() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    mock.set_product_options_response(product_options_response_with_free_accounts(22));
    let _central_guard = central_guard(&mock);
    let username = register_user(&server).await;
    server.post("/_app/auth/logout").await.assert_status_ok();
    let options_before = mock.product_options_request_count();

    let response = server
        .post("/_app/auth/login")
        .json(&json!({
            "username": username,
            "password": "SecurePass123"
        }))
        .await;

    response.assert_status_ok();
    wait_for_free_tier_cache_accounts(22).await;
    assert_eq!(mock.product_options_request_count(), options_before + 1);
}

#[tokio::test(flavor = "current_thread")]
async fn login_recovers_wiped_token_state_without_user_action() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let username = register_user(&server).await;
    let response = server.get("/_app/auth/me").await;
    response.assert_status_ok();
    let body: AuthResponse = response.json();
    let user_id = body.user.user_id;
    activate_paid_entitlement(&server, user_id, &mock).await;

    crate::db::payments::clear_verified_premium_token(
        user_id,
        crate::db::payments::TokenHistoryStatus::Revoked,
        None,
        "revoked",
        Utc::now(),
    )
    .expect("clear should succeed");
    mock.set_history_outcome(MockHistoryOutcome::OrderPaid);
    server.post("/_app/auth/logout").await.assert_status_ok();
    let history_count_before = mock.history_request_count();

    let response = server
        .post("/_app/auth/login")
        .json(&json!({
            "username": username,
            "password": "SecurePass123"
        }))
        .await;
    response.assert_status_ok();

    wait_for_login_recovery_complete(user_id).await;
    assert_eq!(mock.history_request_count(), history_count_before + 1);
    let subject = crate::db::payments::load_payment_subject(user_id)
        .expect("subject query should succeed")
        .expect("subject should exist");
    assert_eq!(subject.last_refresh_status.as_deref(), Some("active"));
    assert!(subject.active_token_history_id.is_some());

    let history = crate::db::payments::load_active_token_history(user_id)
        .expect("history query should succeed")
        .expect("history should exist");
    assert_eq!(history.token_id.to_storage_value(), TOKEN_ID);

    let response = server.get("/_app/user/wallets").await;
    response.assert_status_ok();
    let value: Value = response.json();
    assert_eq!(value["sync_capacity"]["slot_limit"], 50);
    assert_eq!(
        value["sync_capacity"]["summary"],
        "0 of 50 synced accounts used"
    );
    assert_single_app_entitlement_snapshot(user_id, "login_refresh");
}

#[tokio::test(flavor = "current_thread")]
async fn login_does_not_wait_for_central_recovery() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let username = register_user(&server).await;
    let response = server.get("/_app/auth/me").await;
    response.assert_status_ok();
    let body: AuthResponse = response.json();
    let user_id = body.user.user_id;
    activate_paid_entitlement(&server, user_id, &mock).await;

    crate::db::payments::clear_verified_premium_token(
        user_id,
        crate::db::payments::TokenHistoryStatus::Revoked,
        None,
        "revoked",
        Utc::now(),
    )
    .expect("clear should succeed");
    let notify = Arc::new(tokio::sync::Notify::new());
    mock.set_history_outcome(MockHistoryOutcome::BlockedOrderPaid(Arc::clone(&notify)));
    server.post("/_app/auth/logout").await.assert_status_ok();
    let history_count_before = mock.history_request_count();

    let login_request =
        std::future::IntoFuture::into_future(server.post("/_app/auth/login").json(&json!({
            "username": username,
            "password": "SecurePass123"
        })));
    tokio::pin!(login_request);
    let request_started_deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let login = loop {
        tokio::select! {
            response = &mut login_request => break response,
            _ = tokio::time::sleep(std::time::Duration::from_millis(20)) => {
                if mock.history_request_count() > history_count_before {
                    break tokio::time::timeout(
                        std::time::Duration::from_millis(250),
                        &mut login_request,
                    )
                    .await
                    .expect("login should not wait for blocked Central recovery");
                }
                if std::time::Instant::now() >= request_started_deadline {
                    panic!("blocked Central recovery did not start before timeout");
                }
            }
        }
    };
    login.assert_status_ok();
    wait_for_history_request_count(&mock, history_count_before + 1).await;

    assert!(
        crate::db::payments::load_active_token_history(user_id)
            .expect("history query should succeed")
            .is_none()
    );

    notify.notify_waiters();
    wait_for_login_recovery_complete(user_id).await;
}

#[tokio::test(flavor = "current_thread")]
async fn login_refresh_central_unavailable_writes_no_new_app_snapshot() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let central_guard = central_guard(&mock);
    let username = register_user(&server).await;
    let response = server.get("/_app/auth/me").await;
    response.assert_status_ok();
    let body: AuthResponse = response.json();
    let user_id = body.user.user_id;
    activate_paid_entitlement(&server, user_id, &mock).await;
    assert_single_app_entitlement_snapshot(user_id, "payment_poll");

    crate::db::payments::record_payment_refresh_status(
        user_id,
        "error",
        Utc::now() - Duration::hours(25),
    )
    .expect("refresh timestamp should update");
    server.post("/_app/auth/logout").await.assert_status_ok();
    drop(central_guard);
    let _unavailable_guard = crate::payments::client::set_central_base_url_override_for_test(
        "http://127.0.0.1:9".to_string(),
    );

    let response = server
        .post("/_app/auth/login")
        .json(&json!({
            "username": username,
            "password": "SecurePass123"
        }))
        .await;

    response.assert_status_ok();
    wait_for_payment_refresh_status(user_id, "error").await;
    assert_single_app_entitlement_snapshot(user_id, "payment_poll");
}

#[tokio::test(flavor = "current_thread")]
async fn payment_page_returns_local_state_and_central_options_without_fallback_price() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let _user_id = registered_user_id(&server).await;

    let state_response = server.get("/_app/user/payments/state-local").await;
    state_response.assert_status_ok();
    let state: PaymentStateView = state_response.json();
    assert_eq!(state.status, PaymentStateStatus::NotActive);
    assert_eq!(state.display_amount, None);
    assert_eq!(state.currency, None);

    let response = server.get("/_app/user/payments/catalog").await;
    response.assert_status_ok();
    let page: crate::payments::views::PaymentCatalogView = response.json();

    assert!(page.order_history.is_empty());
    assert_eq!(page.tiers.len(), 3);
    assert_eq!(page.tiers[0].tier, "free");
    assert_eq!(page.tiers[1].tier, "basic");
    assert_eq!(page.tiers[2].tier, "premium");
    assert_eq!(page.options.len(), 1);
    assert_eq!(page.options[0].id, "premium_12_months_usd");
    assert_eq!(page.options[0].tier_display_name, "Premium");
    assert_eq!(page.options[0].display_amount, "1.23");
    assert_eq!(page.options[0].currency, "USD");
    assert_eq!(page.options[0].currency_symbol, "$");
    assert_eq!(page.options[0].term_quantity, Some(12));
    assert_eq!(page.options[0].term_unit.as_deref(), Some("month"));
    assert_eq!(
        page.pricing_summary,
        Some(crate::payments::views::parse_bullet(
            "**Free** tracks holdings. **Paid** does the accounting."
        ))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn payment_catalog_fetch_updates_free_tier_cache() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let _user_id = registered_user_id(&server).await;

    mock.set_product_options_response(product_options_response_with_free_accounts(20));

    let response = server.get("/_app/user/payments/catalog").await;
    response.assert_status_ok();

    let cached = crate::db::load_free_tier_entitlement_cache()
        .expect("cache load should succeed")
        .expect("cache should be populated");
    assert_eq!(cached.capabilities.limits.accounts.total, 20);
}

#[tokio::test(flavor = "current_thread")]
async fn product_options_upgrade_required_still_updates_free_tier_cache() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let _user_id = registered_user_id(&server).await;

    mock.set_product_options_response(product_options_response_with_upgrade_required(
        product_options_response_with_free_accounts(20),
    ));

    let response = server.get("/_app/user/payments/catalog").await;
    response.assert_status_ok();

    let cached = crate::db::load_free_tier_entitlement_cache()
        .expect("cache load should succeed")
        .expect("cache should be populated");
    assert_eq!(cached.capabilities.limits.accounts.total, 20);
}

#[tokio::test(flavor = "current_thread")]
async fn start_premium_order_updates_free_tier_cache() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let _user_id = registered_user_id(&server).await;

    mock.set_product_options_response(product_options_response_with_free_accounts(21));

    let response = server
        .post("/_app/user/payments/premium/start")
        .json(&json!({ "product_option_id": "premium_12_months_usd" }))
        .await;
    response.assert_status_ok();

    let cached = crate::db::load_free_tier_entitlement_cache()
        .expect("cache load should succeed")
        .expect("cache should be populated");
    assert_eq!(cached.capabilities.limits.accounts.total, 21);
}

#[tokio::test(flavor = "current_thread")]
async fn payment_page_returns_multiple_premium_options() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    mock.set_product_options_response(product_options_response(vec![tier_json(
        "premium",
        "Premium",
        50,
        50000,
        vec![
            product_option_json("premium_12_months_usd", 12, "month", "1 year", 123),
            product_option_json("premium_test_1_day_usd", 1, "day", "1 day (test)", 1),
        ],
    )]));
    let _central_guard = central_guard(&mock);
    let _user_id = registered_user_id(&server).await;

    let response = server.get("/_app/user/payments/catalog").await;
    response.assert_status_ok();
    let page: crate::payments::views::PaymentCatalogView = response.json();

    assert_eq!(page.options.len(), 2);
    assert_eq!(page.options[0].id, "premium_12_months_usd");
    assert_eq!(page.options[1].id, "premium_test_1_day_usd");
    assert_eq!(page.options[1].term_quantity, Some(1));
    assert_eq!(page.options[1].term_unit.as_deref(), Some("day"));
}

#[tokio::test(flavor = "current_thread")]
async fn start_order_accepts_basic_purchase_option() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    mock.set_product_options_response(product_options_response(vec![
        tier_json(
            "basic",
            "Basic",
            10,
            10000,
            vec![product_option_json(
                "basic_12_months_usd",
                12,
                "month",
                "1 year",
                123,
            )],
        ),
        tier_json(
            "premium",
            "Premium",
            50,
            50000,
            vec![product_option_json(
                "premium_12_months_usd",
                12,
                "month",
                "1 year",
                999,
            )],
        ),
    ]));
    let _central_guard = central_guard(&mock);
    let user_id = registered_user_id(&server).await;

    let response = server
        .post("/_app/user/payments/premium/start")
        .json(&json!({ "product_option_id": "basic_12_months_usd" }))
        .await;
    response.assert_status_ok();
    let value: Value = response.json();
    assert_no_payment_secrets(&value);

    let order = crate::db::payments::load_payment_order(
        user_id,
        PaymentOrderId::from_str(ORDER_ID).expect("order id should parse"),
    )
    .expect("order query should succeed")
    .expect("order should exist");
    assert_eq!(order.product_tier, ProductTier::Basic);
}

#[tokio::test(flavor = "current_thread")]
async fn payment_page_ignores_unknown_tiers_without_failing() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    mock.set_product_options_response(product_options_response(vec![
        tier_json(
            "premium",
            "Premium",
            50,
            50000,
            vec![product_option_json(
                "premium_12_months_usd",
                12,
                "month",
                "1 year",
                123,
            )],
        ),
        tier_json(
            "business",
            "Business",
            100,
            100000,
            vec![product_option_json(
                "business_12_months_usd",
                12,
                "month",
                "1 year",
                456,
            )],
        ),
    ]));
    let _central_guard = central_guard(&mock);
    let _user_id = registered_user_id(&server).await;

    let response = server.get("/_app/user/payments/catalog").await;
    response.assert_status_ok();
    let page: crate::payments::views::PaymentCatalogView = response.json();

    assert_eq!(page.options.len(), 1);
    assert_eq!(page.options[0].id, "premium_12_months_usd");
}

#[tokio::test(flavor = "current_thread")]
async fn payment_page_returns_compatibility_when_options_are_loaded() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let mut response = default_product_options_response();
    response["app_compatibility"] = json!({
        "status": "upgrade_required",
        "detail": "Install a newer build.",
        "minimum_app_version": null
    });
    mock.set_product_options_response(response);
    let _central_guard = central_guard(&mock);
    let _user_id = registered_user_id(&server).await;

    let response = server.get("/_app/user/payments/catalog").await;
    response.assert_status_ok();
    let page: crate::payments::views::PaymentCatalogView = response.json();

    assert_eq!(page.options.len(), 1);
    assert_eq!(
        page.app_compatibility.expect("compatibility").detail,
        "Install a newer build."
    );
}

#[tokio::test(flavor = "current_thread")]
async fn payment_page_skips_malformed_rows_when_valid_premium_option_remains() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    mock.set_product_options_response(product_options_response(vec![tier_json(
        "premium",
        "Premium",
        50,
        50000,
        vec![
            json!({
                "id": "premium_12_months_usd",
                "term": {
                    "label": "1 year"
                },
                "price": {
                    "minor_units": 123,
                    "currency": "USD",
                    "decimal_precision": 2
                }
            }),
            product_option_json("premium_test_1_day_usd", 1, "day", "1 day (test)", 1),
        ],
    )]));
    let _central_guard = central_guard(&mock);
    let _user_id = registered_user_id(&server).await;

    let response = server.get("/_app/user/payments/catalog").await;
    response.assert_status_ok();
    let page: crate::payments::views::PaymentCatalogView = response.json();

    assert_eq!(page.options.len(), 1);
    assert_eq!(page.options[0].id, "premium_test_1_day_usd");
}

#[tokio::test(flavor = "current_thread")]
async fn start_order_persists_secrets_without_exposing_them() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let user_id = registered_user_id(&server).await;

    let response = server
        .post("/_app/user/payments/premium/start")
        .json(&json!({ "product_option_id": "premium_12_months_usd" }))
        .await;
    response.assert_status_ok();
    let value: Value = response.json();
    assert_no_payment_secrets(&value);
    let started: PremiumOrderLaunchView =
        serde_json::from_value(value).expect("start response should deserialize");

    assert_eq!(started.central_order_id, ORDER_ID);
    assert_eq!(started.atlos_order_id, PAYMENT_ATTEMPT_ID);
    assert_eq!(started.order_amount, "9.99");

    let subject = crate::db::payments::load_payment_subject(user_id)
        .expect("subject query should succeed")
        .expect("subject should exist");
    *TEST_HOLDER_ID
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
        subject.entitlement_holder_id.to_storage_value();
    assert_eq!(
        subject
            .management_secret
            .as_ref()
            .expect("management secret should persist")
            .as_str(),
        MANAGEMENT_SECRET
    );
    let order = crate::db::payments::load_payment_order(
        user_id,
        PaymentOrderId::from_str(ORDER_ID).expect("order id should parse"),
    )
    .expect("order query should succeed")
    .expect("order should exist");
    assert_eq!(order.order_secret.as_str(), ORDER_SECRET);
    assert_eq!(order.status, PaymentOrderStatus::Pending);

    let page_response = server.get("/_app/user/payments/catalog").await;
    page_response.assert_status_ok();
    let page: crate::payments::views::PaymentCatalogView = page_response.json();
    assert_eq!(page.order_history.len(), 1);
    assert_eq!(page.order_history[0].order_id, ORDER_ID);
    assert_eq!(page.order_history[0].display_amount, "9.99");
    assert_eq!(
        page.order_history[0].status,
        PaymentOrderStatusView::Pending
    );
}

#[tokio::test(flavor = "current_thread")]
async fn start_order_rejects_selected_option_that_disappears_after_page_load() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    mock.set_product_options_response(product_options_response(vec![tier_json(
        "premium",
        "Premium",
        50,
        50000,
        vec![
            product_option_json("premium_12_months_usd", 12, "month", "1 year", 123),
            product_option_json("premium_test_1_day_usd", 1, "day", "1 day (test)", 1),
        ],
    )]));
    let _central_guard = central_guard(&mock);
    let _user_id = registered_user_id(&server).await;

    let page_response = server.get("/_app/user/payments/catalog").await;
    page_response.assert_status_ok();

    mock.set_product_options_response(product_options_response(vec![tier_json(
        "premium",
        "Premium",
        50,
        50000,
        vec![product_option_json(
            "premium_12_months_usd",
            12,
            "month",
            "1 year",
            123,
        )],
    )]));

    let response = server
        .post("/_app/user/payments/premium/start")
        .json(&json!({ "product_option_id": "premium_test_1_day_usd" }))
        .await;
    response.assert_status(StatusCode::BAD_REQUEST);
    let error: Value = response.json();

    assert_eq!(
        error.get("message").and_then(Value::as_str),
        Some("The selected paid option is no longer available. Refresh the page and try again.")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn start_order_rejects_upgrade_required_after_page_load() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    mock.set_product_options_response(default_product_options_response());
    let _central_guard = central_guard(&mock);
    let _user_id = registered_user_id(&server).await;

    let page_response = server.get("/_app/user/payments/catalog").await;
    page_response.assert_status_ok();

    let mut response = default_product_options_response();
    response["app_compatibility"] = json!({
        "status": "upgrade_required",
        "detail": "Install a newer build.",
        "minimum_app_version": null
    });
    mock.set_product_options_response(response);

    let response = server
        .post("/_app/user/payments/premium/start")
        .json(&json!({ "product_option_id": "premium_12_months_usd" }))
        .await;
    response.assert_status(StatusCode::BAD_REQUEST);
    let error: Value = response.json();

    assert_eq!(
        error.get("message").and_then(Value::as_str),
        Some("Install a newer build.")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn poll_paid_order_stores_verified_token_and_returns_active_state() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let user_id = registered_user_id(&server).await;

    server
        .post("/_app/user/payments/premium/start")
        .json(&json!({ "product_option_id": "premium_12_months_usd" }))
        .await
        .assert_status_ok();
    let subject = crate::db::payments::load_payment_subject(user_id)
        .expect("subject query should succeed")
        .expect("subject should exist");
    *TEST_HOLDER_ID
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
        subject.entitlement_holder_id.to_storage_value();
    mock.set_order_outcome(MockOrderOutcome::Paid);

    let response = server
        .post("/_app/user/payments/premium/poll")
        .json(&json!({ "order_id": ORDER_ID }))
        .await;
    response.assert_status_ok();
    let value: Value = response.json();
    assert_no_payment_secrets(&value);
    let state: PaymentStateView =
        serde_json::from_value(value).expect("payment state should deserialize");

    assert_eq!(state.status, PaymentStateStatus::Active);
    let subject = crate::db::payments::load_payment_subject(user_id)
        .expect("subject query should succeed")
        .expect("subject should exist");
    assert!(subject.active_token_history_id.is_some());
    let history = crate::db::payments::load_active_token_history(user_id)
        .expect("history query should succeed")
        .expect("history should exist");
    assert_eq!(history.token_id.to_storage_value(), TOKEN_ID);
    assert_single_app_entitlement_snapshot(user_id, "payment_poll");
}

#[tokio::test(flavor = "current_thread")]
async fn poll_malformed_paid_token_writes_no_app_entitlement_snapshot() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let user_id = registered_user_id(&server).await;

    server
        .post("/_app/user/payments/premium/start")
        .json(&json!({ "product_option_id": "premium_12_months_usd" }))
        .await
        .assert_status_ok();
    let subject = crate::db::payments::load_payment_subject(user_id)
        .expect("subject query should succeed")
        .expect("subject should exist");
    *TEST_HOLDER_ID
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
        subject.entitlement_holder_id.to_storage_value();
    mock.set_order_outcome(MockOrderOutcome::MalformedPaidToken);

    let response = server
        .post("/_app/user/payments/premium/poll")
        .json(&json!({ "order_id": ORDER_ID }))
        .await;

    assert_eq!(response.status_code(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_no_app_entitlement_snapshots(user_id);
    let subject = crate::db::payments::load_payment_subject(user_id)
        .expect("subject query should succeed")
        .expect("subject should exist");
    assert!(subject.active_token_history_id.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn poll_verifying_order_returns_verifying_state_with_payment_summary() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let _user_id = registered_user_id(&server).await;

    server
        .post("/_app/user/payments/premium/start")
        .json(&json!({ "product_option_id": "premium_12_months_usd" }))
        .await
        .assert_status_ok();
    mock.set_order_outcome(MockOrderOutcome::Verifying);

    let response = server
        .post("/_app/user/payments/premium/poll")
        .json(&json!({ "order_id": ORDER_ID }))
        .await;
    response.assert_status_ok();
    let value: Value = response.json();
    assert_no_payment_secrets(&value);
    let state: PaymentStateView =
        serde_json::from_value(value).expect("payment state should deserialize");

    assert_eq!(state.status, PaymentStateStatus::Verifying);
    let summary = state
        .payment_summary
        .expect("verifying state should include payment summary");
    assert_eq!(summary.paid_asset_code.as_deref(), Some("XMR"));
    assert_eq!(summary.paid_asset_amount.as_deref(), Some("0.00002614"));
}

#[tokio::test(flavor = "current_thread")]
async fn top_up_launch_uses_parent_order_and_central_attempt() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let _user_id = registered_user_id(&server).await;

    server
        .post("/_app/user/payments/premium/start")
        .json(&json!({ "product_option_id": "premium_12_months_usd" }))
        .await
        .assert_status_ok();
    mock.set_order_outcome(MockOrderOutcome::AdditionalPaymentRequired);

    let response = server
        .post("/_app/user/payments/premium/top-up")
        .json(&json!({ "central_order_id": ORDER_ID }))
        .await;
    response.assert_status_ok();
    let value: Value = response.json();
    assert_no_payment_secrets(&value);
    let top_up: PremiumTopUpLaunchView =
        serde_json::from_value(value).expect("top-up launch should deserialize");

    assert_eq!(
        top_up.state.status,
        PaymentStateStatus::AdditionalPaymentRequired
    );
    let additional_payment = top_up
        .state
        .additional_payment
        .expect("additional payment state");
    assert_eq!(additional_payment.paid_amount, "8.00");
    assert_eq!(additional_payment.remaining_amount, "1.99");
    let launch = top_up.launch.expect("top-up should launch");
    assert_eq!(launch.central_order_id, ORDER_ID);
    assert_eq!(launch.atlos_order_id, "01JQABCDEF000000000000000G");
    assert_eq!(launch.order_amount, "1.99");
}

#[tokio::test(flavor = "current_thread")]
async fn payment_page_reload_recovers_additional_payment_required_from_central() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let _user_id = registered_user_id(&server).await;

    server
        .post("/_app/user/payments/premium/start")
        .json(&json!({ "product_option_id": "premium_12_months_usd" }))
        .await
        .assert_status_ok();
    mock.set_order_outcome(MockOrderOutcome::AdditionalPaymentRequired);

    let response = server.get("/_app/user/payments/state-refresh").await;
    response.assert_status_ok();
    let state: PaymentStateView = response.json();

    assert_eq!(state.status, PaymentStateStatus::AdditionalPaymentRequired);
    assert_eq!(state.order_id.as_deref(), Some(ORDER_ID));
    assert_free_entitlement_state(&state);
    let additional_payment = state.additional_payment.expect("additional payment state");
    assert_eq!(additional_payment.paid_amount, "8.00");
    assert_eq!(additional_payment.remaining_amount, "1.99");
    assert!(state.payment_summary.is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn expired_order_refresh_keeps_free_entitlements() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let _user_id = registered_user_id(&server).await;

    server
        .post("/_app/user/payments/premium/start")
        .json(&json!({ "product_option_id": "premium_12_months_usd" }))
        .await
        .assert_status_ok();
    mock.set_order_outcome(MockOrderOutcome::ManualReviewExpired);

    let response = server.get("/_app/user/payments/state-refresh").await;
    response.assert_status_ok();
    let state: PaymentStateView = response.json();

    assert_eq!(state.status, PaymentStateStatus::ManualReview);
    assert_eq!(state.order_id.as_deref(), Some(ORDER_ID));
    assert_free_entitlement_state(&state);
}

#[tokio::test(flavor = "current_thread")]
async fn poll_manual_review_order_returns_review_state_and_reason_copy() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let user_id = registered_user_id(&server).await;

    server
        .post("/_app/user/payments/premium/start")
        .json(&json!({ "product_option_id": "premium_12_months_usd" }))
        .await
        .assert_status_ok();
    mock.set_order_outcome(MockOrderOutcome::ManualReviewExpired);

    let response = server
        .post("/_app/user/payments/premium/poll")
        .json(&json!({ "order_id": ORDER_ID }))
        .await;
    response.assert_status_ok();
    let value: Value = response.json();
    assert_no_payment_secrets(&value);
    let state: PaymentStateView =
        serde_json::from_value(value).expect("payment state should deserialize");

    assert_eq!(state.status, PaymentStateStatus::ManualReview);
    assert_free_entitlement_state(&state);
    assert!(
        state
            .message
            .as_deref()
            .unwrap_or_default()
            .contains("did not match the expected order amount")
    );

    let order = crate::db::payments::load_payment_order(
        user_id,
        PaymentOrderId::from_str(ORDER_ID).expect("order id should parse"),
    )
    .expect("order query should succeed")
    .expect("order should exist");
    assert_eq!(order.status, PaymentOrderStatus::Expired);
}

#[tokio::test(flavor = "current_thread")]
async fn poll_verifying_order_keeps_free_entitlements() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let _user_id = registered_user_id(&server).await;

    server
        .post("/_app/user/payments/premium/start")
        .json(&json!({ "product_option_id": "premium_12_months_usd" }))
        .await
        .assert_status_ok();
    mock.set_order_outcome(MockOrderOutcome::Verifying);

    let response = server
        .post("/_app/user/payments/premium/poll")
        .json(&json!({ "order_id": ORDER_ID }))
        .await;
    response.assert_status_ok();
    let state: PaymentStateView = response.json();

    assert_eq!(state.status, PaymentStateStatus::Verifying);
    assert_eq!(state.order_id.as_deref(), Some(ORDER_ID));
    assert!(state.payment_summary.is_some());
    assert_free_entitlement_state(&state);
}

#[tokio::test(flavor = "current_thread")]
async fn payment_page_reload_recovers_manual_review_from_central() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let _user_id = registered_user_id(&server).await;

    server
        .post("/_app/user/payments/premium/start")
        .json(&json!({ "product_option_id": "premium_12_months_usd" }))
        .await
        .assert_status_ok();
    mock.set_order_outcome(MockOrderOutcome::ManualReviewExpired);

    server
        .post("/_app/user/payments/premium/poll")
        .json(&json!({ "order_id": ORDER_ID }))
        .await
        .assert_status_ok();

    let response = server.get("/_app/user/payments/state-refresh").await;
    response.assert_status_ok();
    let state: PaymentStateView = response.json();

    assert_eq!(state.status, PaymentStateStatus::ManualReview);
    assert_free_entitlement_state(&state);
    assert!(state.payment_summary.is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn start_order_is_blocked_while_manual_review_is_unresolved() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let _user_id = registered_user_id(&server).await;

    server
        .post("/_app/user/payments/premium/start")
        .json(&json!({ "product_option_id": "premium_12_months_usd" }))
        .await
        .assert_status_ok();
    mock.set_order_outcome(MockOrderOutcome::ManualReviewExpired);

    server
        .post("/_app/user/payments/premium/poll")
        .json(&json!({ "order_id": ORDER_ID }))
        .await
        .assert_status_ok();

    let response = server
        .post("/_app/user/payments/premium/start")
        .json(&json!({ "product_option_id": "premium_12_months_usd" }))
        .await;
    assert_eq!(response.status_code(), StatusCode::BAD_REQUEST);
    let value: Value = response.json();
    assert_no_payment_secrets(&value);
    assert!(
        value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .contains("manual review")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn refresh_revoked_expired_marks_premium_not_active() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let user_id = registered_user_id(&server).await;

    server
        .post("/_app/user/payments/premium/start")
        .json(&json!({ "product_option_id": "premium_12_months_usd" }))
        .await
        .assert_status_ok();
    let subject = crate::db::payments::load_payment_subject(user_id)
        .expect("subject query should succeed")
        .expect("subject should exist");
    *TEST_HOLDER_ID
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
        subject.entitlement_holder_id.to_storage_value();
    mock.set_order_outcome(MockOrderOutcome::Paid);
    server
        .post("/_app/user/payments/premium/poll")
        .json(&json!({ "order_id": ORDER_ID }))
        .await
        .assert_status_ok();

    mock.set_refresh_outcome(MockRefreshOutcome::RevokedExpired);
    let response = server.post("/_app/user/payments/premium/refresh").await;
    response.assert_status_ok();
    let state: PaymentStateView = response.json();

    assert_eq!(state.status, PaymentStateStatus::NotActive);
    assert!(state.support_reference.is_none());
    let subject = crate::db::payments::load_payment_subject(user_id)
        .expect("subject query should succeed")
        .expect("subject should exist");
    assert!(subject.active_token_history_id.is_none());
    assert_eq!(subject.last_refresh_status.as_deref(), Some("revoked"));
}

#[tokio::test(flavor = "current_thread")]
async fn refresh_token_superseded_with_valid_local_token_and_empty_history_keeps_premium_warning() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let user_id = registered_user_id(&server).await;
    activate_paid_entitlement(&server, user_id, &mock).await;
    let before = crate::db::payments::load_payment_subject(user_id)
        .expect("subject query should succeed")
        .expect("subject should exist");
    let before_history = crate::db::payments::load_active_token_history(user_id)
        .expect("history query should succeed")
        .expect("paid subject should store active token");
    let before_token = before_history.active_token.clone();

    mock.set_refresh_outcome(MockRefreshOutcome::RevokedTokenSuperseded);
    mock.set_history_outcome(MockHistoryOutcome::Empty);
    let response = server.post("/_app/user/payments/premium/refresh").await;
    response.assert_status_ok();
    let state: PaymentStateView = response.json();

    assert_eq!(state.status, PaymentStateStatus::ActiveWithSyncWarning);
    assert_eq!(
        state
            .support_reference
            .as_ref()
            .and_then(|reference| reference.token_id.as_deref()),
        Some(TOKEN_ID)
    );
    assert_eq!(
        state
            .support_reference
            .as_ref()
            .and_then(|reference| reference.subscription_subject_id.as_deref()),
        Some(SUBSCRIPTION_SUBJECT_ID)
    );
    assert_eq!(
        state
            .support_reference
            .as_ref()
            .map(|reference| reference.entitlement_holder_id.as_str()),
        Some(before.entitlement_holder_id.to_storage_value().as_str())
    );
    let after = crate::db::payments::load_payment_subject(user_id)
        .expect("subject query should succeed")
        .expect("subject should exist");
    let after_history = crate::db::payments::load_active_token_history(user_id)
        .expect("history query should succeed")
        .expect("history should exist");
    assert_eq!(after_history.active_token.as_str(), before_token.as_str());
    assert_eq!(after.last_refresh_status.as_deref(), Some("sync_warning"));
}

#[tokio::test(flavor = "current_thread")]
async fn refresh_token_superseded_with_valid_local_token_and_history_repairs_active_state() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let user_id = registered_user_id(&server).await;
    activate_paid_entitlement(&server, user_id, &mock).await;

    mock.set_refresh_outcome(MockRefreshOutcome::RevokedTokenSuperseded);
    mock.set_history_outcome(MockHistoryOutcome::OrderPaid);
    let response = server.post("/_app/user/payments/premium/refresh").await;
    response.assert_status_ok();
    let state: PaymentStateView = response.json();

    assert_eq!(state.status, PaymentStateStatus::Active);
    assert_eq!(
        state
            .support_reference
            .as_ref()
            .and_then(|reference| reference.token_id.as_deref()),
        Some(TOKEN_ID)
    );
    let subject = crate::db::payments::load_payment_subject(user_id)
        .expect("subject query should succeed")
        .expect("subject should exist");
    assert!(subject.active_token_history_id.is_some());
    assert_eq!(subject.last_refresh_status.as_deref(), Some("active"));
}

#[tokio::test(flavor = "current_thread")]
async fn active_payment_page_includes_safe_support_reference() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let user_id = registered_user_id(&server).await;
    activate_paid_entitlement(&server, user_id, &mock).await;
    let subject = crate::db::payments::load_payment_subject(user_id)
        .expect("subject query should succeed")
        .expect("subject should exist");

    let response = server.get("/_app/user/payments/state-refresh").await;
    response.assert_status_ok();
    let value: Value = response.json();
    assert_no_payment_secrets(&value);
    let state: PaymentStateView =
        serde_json::from_value(value).expect("payment state should deserialize");

    let reference = state
        .support_reference
        .expect("active premium state should include support reference");
    assert_eq!(reference.token_id.as_deref(), Some(TOKEN_ID));
    assert_eq!(
        reference.subscription_subject_id.as_deref(),
        Some(SUBSCRIPTION_SUBJECT_ID)
    );
    assert_eq!(
        reference.entitlement_holder_id,
        subject.entitlement_holder_id.to_storage_value()
    );
    assert_eq!(reference.order_id.as_deref(), Some(ORDER_ID));
}

#[tokio::test(flavor = "current_thread")]
async fn unsupported_signing_key_returns_upgrade_error_without_secrets() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    mock.reject_signing_key();
    let _central_guard = central_guard(&mock);
    let _user_id = registered_user_id(&server).await;

    let response = server
        .post("/_app/user/payments/premium/start")
        .json(&json!({ "product_option_id": "premium_12_months_usd" }))
        .await;
    assert_eq!(response.status_code(), StatusCode::BAD_REQUEST);
    let value: Value = response.json();
    assert_no_payment_secrets(&value);
    assert!(
        value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .contains("update")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cancel_pending_order_marks_canceled() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let user_id = registered_user_id(&server).await;

    server
        .post("/_app/user/payments/premium/start")
        .json(&json!({ "product_option_id": "premium_12_months_usd" }))
        .await
        .assert_status_ok();

    let response = server
        .post("/_app/user/payments/premium/cancel")
        .json(&json!({ "order_id": ORDER_ID }))
        .await;
    response.assert_status_ok();
    let state: PaymentStateView = response.json();
    assert_eq!(state.status, PaymentStateStatus::Canceled);
    assert_free_entitlement_state(&state);

    let order = crate::db::payments::load_payment_order(
        user_id,
        PaymentOrderId::from_str(ORDER_ID).expect("order id should parse"),
    )
    .expect("order query should succeed")
    .expect("order should exist");
    assert_eq!(order.status, PaymentOrderStatus::Canceled);
}

#[tokio::test(flavor = "current_thread")]
async fn reconcile_updates_canceled_order_to_paid_and_stores_token() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let user_id = registered_user_id(&server).await;

    server
        .post("/_app/user/payments/premium/start")
        .json(&json!({ "product_option_id": "premium_12_months_usd" }))
        .await
        .assert_status_ok();

    let subject = crate::db::payments::load_payment_subject(user_id)
        .expect("subject query should succeed")
        .expect("subject should exist");
    *TEST_HOLDER_ID
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
        subject.entitlement_holder_id.to_storage_value();

    server
        .post("/_app/user/payments/premium/cancel")
        .json(&json!({ "order_id": ORDER_ID }))
        .await
        .assert_status_ok();

    mock.set_history_outcome(MockHistoryOutcome::OrderPaid);
    let response = server.post("/_app/user/payments/premium/reconcile").await;
    response.assert_status_ok();
    let value: Value = response.json();
    assert_no_payment_secrets(&value);
    let state: PaymentStateView =
        serde_json::from_value(value).expect("reconcile response should deserialize");
    assert_eq!(state.status, PaymentStateStatus::Active);

    let order = crate::db::payments::load_payment_order(
        user_id,
        PaymentOrderId::from_str(ORDER_ID).expect("order id should parse"),
    )
    .expect("order query should succeed")
    .expect("order should exist");
    assert_eq!(order.status, PaymentOrderStatus::Paid);

    let subject = crate::db::payments::load_payment_subject(user_id)
        .expect("subject query should succeed")
        .expect("subject should exist");
    assert!(subject.active_token_history_id.is_some());
    assert_single_app_entitlement_snapshot(user_id, "payment_reconcile");
}

#[tokio::test(flavor = "current_thread")]
async fn payment_page_recovers_wiped_token_state_from_subscription_history() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let user_id = registered_user_id(&server).await;
    activate_paid_entitlement(&server, user_id, &mock).await;

    // Wipe token fields to simulate the stuck-user state.
    crate::db::payments::clear_verified_premium_token(
        user_id,
        crate::db::payments::TokenHistoryStatus::Revoked,
        None,
        "revoked",
        Utc::now(),
    )
    .expect("clear should succeed");

    mock.set_history_outcome(MockHistoryOutcome::OrderPaid);
    let response = server.get("/_app/user/payments/state-refresh").await;
    response.assert_status_ok();
    let value: Value = response.json();
    assert_no_payment_secrets(&value);
    let state: PaymentStateView =
        serde_json::from_value(value).expect("payment state should deserialize");

    assert_eq!(state.status, PaymentStateStatus::Active);
    assert_no_payment_secrets(
        &serde_json::to_value(&state).expect("state should serialize for secret check"),
    );

    let reference = state
        .support_reference
        .expect("recovered state should include support reference");
    assert_eq!(reference.token_id.as_deref(), Some(TOKEN_ID));
    assert_eq!(reference.order_id.as_deref(), Some(ORDER_ID));
    assert_eq!(
        reference.subscription_subject_id.as_deref(),
        Some(SUBSCRIPTION_SUBJECT_ID)
    );

    let subject = crate::db::payments::load_payment_subject(user_id)
        .expect("subject query should succeed")
        .expect("subject should exist");
    assert!(subject.active_token_history_id.is_some());
    let history = crate::db::payments::load_active_token_history(user_id)
        .expect("history query should succeed")
        .expect("history should exist");
    assert_eq!(history.token_id.to_storage_value(), TOKEN_ID);
    assert_eq!(subject.last_refresh_status.as_deref(), Some("active"));
}

#[tokio::test(flavor = "current_thread")]
async fn history_metadata_without_signed_token_does_not_grant_premium() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let user_id = registered_user_id(&server).await;
    activate_paid_entitlement(&server, user_id, &mock).await;

    crate::db::payments::clear_verified_premium_token(
        user_id,
        crate::db::payments::TokenHistoryStatus::Revoked,
        None,
        "revoked",
        Utc::now(),
    )
    .expect("clear should succeed");

    mock.set_history_outcome(MockHistoryOutcome::MetadataOnly);
    let response = server.get("/_app/user/payments/state-refresh").await;
    response.assert_status_ok();
    let value: Value = response.json();
    assert_no_payment_secrets(&value);
    let state: PaymentStateView =
        serde_json::from_value(value).expect("payment state should deserialize");

    assert_eq!(state.status, PaymentStateStatus::RecoveryFailed);

    let subject = crate::db::payments::load_payment_subject(user_id)
        .expect("subject query should succeed")
        .expect("subject should exist");
    assert!(subject.active_token_history_id.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn payment_page_recovers_wiped_token_state_with_latest_failed_order() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let user_id = registered_user_id(&server).await;
    activate_paid_entitlement(&server, user_id, &mock).await;

    crate::db::payments::clear_verified_premium_token(
        user_id,
        crate::db::payments::TokenHistoryStatus::Revoked,
        None,
        "revoked",
        Utc::now(),
    )
    .expect("clear should succeed");
    let failed_order_id = PaymentOrderId::from_str(FAILED_ORDER_ID).expect("order id should parse");
    crate::db::payments::insert_payment_order(
        user_id,
        &crate::db::payments::NewPaymentOrder {
            order_id: failed_order_id,
            order_secret: PaymentSecret::from_raw(ORDER_SECRET).expect("order secret should parse"),
            product_tier: ProductTier::Basic,
            amount: PaymentAmount {
                minor_units: 123,
                currency: "USD".to_string(),
                currency_symbol: Some("$".to_string()),
                decimal_precision: 2,
            },
        },
        Utc::now(),
    )
    .expect("failed order insert should succeed");
    crate::db::payments::mark_payment_order_status(
        user_id,
        failed_order_id,
        PaymentOrderStatus::Failed,
        None,
        Utc::now() + Duration::seconds(1),
    )
    .expect("failed order status should update");

    mock.set_history_outcome(MockHistoryOutcome::OrderPaid);
    let response = server.get("/_app/user/payments/state-refresh").await;
    response.assert_status_ok();
    let state: PaymentStateView =
        serde_json::from_value(response.json()).expect("payment state should deserialize");

    assert_eq!(state.status, PaymentStateStatus::Active);
    assert_eq!(
        state
            .support_reference
            .as_ref()
            .and_then(|reference| reference.token_id.as_deref()),
        Some(TOKEN_ID)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn payment_page_recovery_failure_returns_recovery_failed_state() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let user_id = registered_user_id(&server).await;
    activate_paid_entitlement(&server, user_id, &mock).await;

    crate::db::payments::clear_verified_premium_token(
        user_id,
        crate::db::payments::TokenHistoryStatus::Revoked,
        None,
        "revoked",
        Utc::now(),
    )
    .expect("clear should succeed");

    mock.set_history_outcome(MockHistoryOutcome::Empty);
    let response = server.get("/_app/user/payments/state-refresh").await;
    response.assert_status_ok();
    let value: Value = response.json();
    assert_no_payment_secrets(&value);
    let state: PaymentStateView =
        serde_json::from_value(value).expect("payment state should deserialize");

    assert_eq!(state.status, PaymentStateStatus::RecoveryFailed);

    let subject = crate::db::payments::load_payment_subject(user_id)
        .expect("subject query should succeed")
        .expect("subject should exist");
    assert!(subject.active_token_history_id.is_none());
    assert_eq!(
        subject.last_refresh_status.as_deref(),
        Some("recovery_failed")
    );

    let reference = state
        .support_reference
        .expect("recovery failed should include support reference with holder id");
    assert!(reference.token_id.is_none());
    assert!(reference.order_id.is_none());
    assert!(reference.subscription_subject_id.is_none());
    assert_eq!(
        reference.entitlement_holder_id,
        subject.entitlement_holder_id.to_storage_value()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn payment_page_recovery_central_error_keeps_retryable_status() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let user_id = registered_user_id(&server).await;
    activate_paid_entitlement(&server, user_id, &mock).await;

    crate::db::payments::clear_verified_premium_token(
        user_id,
        crate::db::payments::TokenHistoryStatus::Revoked,
        None,
        "revoked",
        Utc::now(),
    )
    .expect("clear should succeed");
    mock.set_history_outcome(MockHistoryOutcome::ServerError);

    let response = server.get("/_app/user/payments/state-refresh").await;
    response.assert_status_ok();
    let state: PaymentStateView =
        serde_json::from_value(response.json()).expect("payment state should deserialize");
    assert_eq!(state.status, PaymentStateStatus::RecoveryFailed);

    let subject = crate::db::payments::load_payment_subject(user_id)
        .expect("subject query should succeed")
        .expect("subject should exist");
    assert_eq!(subject.last_refresh_status.as_deref(), Some("error"));
}

#[tokio::test(flavor = "current_thread")]
async fn payment_page_recovery_failure_is_throttled() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let user_id = registered_user_id(&server).await;
    activate_paid_entitlement(&server, user_id, &mock).await;

    crate::db::payments::clear_verified_premium_token(
        user_id,
        crate::db::payments::TokenHistoryStatus::Revoked,
        None,
        "revoked",
        Utc::now(),
    )
    .expect("clear should succeed");

    // First recovery attempt fails.
    mock.set_history_outcome(MockHistoryOutcome::Empty);
    let response = server.get("/_app/user/payments/state-refresh").await;
    response.assert_status_ok();
    let state: PaymentStateView =
        serde_json::from_value(response.json()).expect("state should deserialize");
    assert_eq!(state.status, PaymentStateStatus::RecoveryFailed);

    let history_count_before = mock.history_request_count();
    let response = server.get("/_app/user/payments/state-refresh").await;
    response.assert_status_ok();
    let state: PaymentStateView =
        serde_json::from_value(response.json()).expect("state should deserialize");
    assert_eq!(state.status, PaymentStateStatus::RecoveryFailed);
    assert_eq!(mock.history_request_count(), history_count_before);
}

#[tokio::test(flavor = "current_thread")]
async fn manual_reconcile_bypasses_recovery_failed_throttle() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let user_id = registered_user_id(&server).await;
    activate_paid_entitlement(&server, user_id, &mock).await;

    crate::db::payments::clear_verified_premium_token(
        user_id,
        crate::db::payments::TokenHistoryStatus::Revoked,
        None,
        "revoked",
        Utc::now(),
    )
    .expect("clear should succeed");

    // Trigger recovery_failed throttle.
    mock.set_history_outcome(MockHistoryOutcome::Empty);
    let response = server.get("/_app/user/payments/state-refresh").await;
    response.assert_status_ok();

    // Manual reconcile bypasses throttle.
    mock.set_history_outcome(MockHistoryOutcome::OrderPaid);
    let response = server.post("/_app/user/payments/premium/reconcile").await;
    response.assert_status_ok();
    let state: PaymentStateView = response.json();
    assert_eq!(state.status, PaymentStateStatus::Active);

    let subject = crate::db::payments::load_payment_subject(user_id)
        .expect("subject query should succeed")
        .expect("subject should exist");
    assert!(subject.active_token_history_id.is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn payment_page_does_not_attempt_recovery_without_management_secret() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let _user_id = registered_user_id(&server).await;

    // No paid entitlement activated, so no management_secret.
    let history_count_before = mock.history_request_count();
    let response = server.get("/_app/user/payments/state-refresh").await;
    response.assert_status_ok();
    let state: PaymentStateView =
        serde_json::from_value(response.json()).expect("state should deserialize");
    assert_eq!(state.status, PaymentStateStatus::NotActive);
    assert_eq!(mock.history_request_count(), history_count_before);
}

#[tokio::test(flavor = "current_thread")]
async fn refresh_active_response_keeps_order_id_in_support_reference() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let user_id = registered_user_id(&server).await;
    activate_paid_entitlement(&server, user_id, &mock).await;

    mock.set_refresh_outcome(MockRefreshOutcome::Active);
    let response = server.post("/_app/user/payments/premium/refresh").await;
    response.assert_status_ok();
    let state: PaymentStateView = response.json();
    assert_eq!(state.status, PaymentStateStatus::Active);
    let reference = state
        .support_reference
        .expect("active refresh should include support reference");
    assert_eq!(reference.order_id.as_deref(), Some(ORDER_ID));
    assert_eq!(reference.token_id.as_deref(), Some(TOKEN_ID));
}

#[tokio::test(flavor = "current_thread")]
async fn refresh_omits_empty_last_known_token() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let user_id = registered_user_id(&server).await;
    activate_paid_entitlement(&server, user_id, &mock).await;

    crate::db::payments::set_token_history_active_token_for_test(user_id, TOKEN_ID, "")
        .expect("active token blanking should succeed");

    mock.set_refresh_outcome(MockRefreshOutcome::Active);
    let response = server.post("/_app/user/payments/premium/refresh").await;
    response.assert_status_ok();

    assert_eq!(mock.last_refresh_last_known_token(), Some(None));
}

#[tokio::test(flavor = "current_thread")]
async fn token_superseded_warning_keeps_order_id_in_support_reference() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let user_id = registered_user_id(&server).await;
    activate_paid_entitlement(&server, user_id, &mock).await;

    mock.set_refresh_outcome(MockRefreshOutcome::RevokedTokenSuperseded);
    mock.set_history_outcome(MockHistoryOutcome::Empty);
    let response = server.post("/_app/user/payments/premium/refresh").await;
    response.assert_status_ok();
    let state: PaymentStateView = response.json();
    assert_eq!(state.status, PaymentStateStatus::ActiveWithSyncWarning);
    let reference = state
        .support_reference
        .expect("sync warning should include support reference");
    assert_eq!(reference.order_id.as_deref(), Some(ORDER_ID));
}

#[tokio::test(flavor = "current_thread")]
async fn canceled_order_state_includes_support_reference_with_free_entitlements() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let _user_id = registered_user_id(&server).await;

    // Start and cancel a Premium order.
    server
        .post("/_app/user/payments/premium/start")
        .json(&json!({ "product_option_id": "premium_12_months_usd" }))
        .await
        .assert_status_ok();
    let response = server
        .post("/_app/user/payments/premium/cancel")
        .json(&json!({ "order_id": ORDER_ID }))
        .await;
    response.assert_status_ok();
    let state: PaymentStateView = response.json();
    assert_eq!(state.status, PaymentStateStatus::Canceled);
    assert_free_entitlement_state(&state);
    let reference = state
        .support_reference
        .expect("canceled order should include support reference");
    assert_eq!(reference.order_id.as_deref(), Some(ORDER_ID));
    assert!(!reference.entitlement_holder_id.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn cancel_endpoint_returns_support_reference_for_paid_order() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let user_id = registered_user_id(&server).await;
    activate_paid_entitlement(&server, user_id, &mock).await;

    // Cancel the paid order directly.
    let response = server
        .post("/_app/user/payments/premium/cancel")
        .json(&json!({ "order_id": ORDER_ID }))
        .await;
    response.assert_status_ok();
    let state: PaymentStateView = response.json();
    // Cancel on a paid order returns the verified active entitlement state.
    assert_eq!(state.status, PaymentStateStatus::Active);
    assert_eq!(state.tier, "premium");
    assert!(state.paid_through.is_some());
    let reference = state
        .support_reference
        .expect("cancel on paid order should include support reference");
    assert_eq!(reference.order_id.as_deref(), Some(ORDER_ID));
    assert_eq!(reference.token_id.as_deref(), Some(TOKEN_ID));
    assert_eq!(
        reference.subscription_subject_id.as_deref(),
        Some(SUBSCRIPTION_SUBJECT_ID)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn refresh_token_superseded_with_missing_local_token_returns_unavailable() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let user_id = registered_user_id(&server).await;
    activate_paid_entitlement(&server, user_id, &mock).await;

    // Clear the active token pointer. In the new history-based design, clearing
    // the pointer also removes the token_id. Without an active token history row,
    // the refresh endpoint cannot reach Central and returns an error.
    crate::db::payments::set_active_token_for_test(user_id, None, None, None)
        .expect("set active token should succeed");

    mock.set_refresh_outcome(MockRefreshOutcome::RevokedTokenSuperseded);
    let response = server.post("/_app/user/payments/premium/refresh").await;
    // No active token means refresh cannot proceed — 400 Bad Request.
    assert_eq!(response.status_code(), 400);
}

#[tokio::test(flavor = "current_thread")]
async fn refresh_token_superseded_with_invalid_local_token_clears_not_active() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let user_id = registered_user_id(&server).await;
    activate_paid_entitlement(&server, user_id, &mock).await;

    // Corrupt the stored active_token.
    crate::db::payments::set_active_token_for_test(
        user_id,
        Some("this-is-not-a-valid-token"),
        None,
        None,
    )
    .expect("set active token should succeed");

    mock.set_refresh_outcome(MockRefreshOutcome::RevokedTokenSuperseded);
    let response = server.post("/_app/user/payments/premium/refresh").await;
    response.assert_status_ok();
    let state: PaymentStateView = response.json();
    assert_eq!(state.status, PaymentStateStatus::NotActive);

    let subject = crate::db::payments::load_payment_subject(user_id)
        .expect("subject query should succeed")
        .expect("subject should exist");
    assert!(subject.active_token_history_id.is_none());
    assert_eq!(subject.last_refresh_status.as_deref(), Some("revoked"));
}

#[tokio::test(flavor = "current_thread")]
async fn refresh_token_superseded_with_expired_local_token_clears_not_active() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let user_id = registered_user_id(&server).await;
    activate_paid_entitlement(&server, user_id, &mock).await;

    let subject = crate::db::payments::load_payment_subject(user_id)
        .expect("subject query should succeed")
        .expect("subject should exist");

    // Generate a token that expired 1 day ago.
    let now = Utc::now();
    let expired_claims = TokenClaims {
        token_id: TokenId::from_str(TOKEN_ID).expect("test token id should parse"),
        subscription_subject_id: SubscriptionSubjectId::from_str(SUBSCRIPTION_SUBJECT_ID)
            .expect("test subject id should parse"),
        entitlement_holder_id: subject.entitlement_holder_id,
        tier: EntitlementTier::Premium,
        capability_set_id: Some("capset_premium_v1".to_string()),
        capability_schema_version: CAPABILITY_SCHEMA_VERSION_V3,
        capabilities: EntitlementCapabilities::v3_from_parts(50, 50000, true),
        subscription_valid_until: now + Duration::days(365),
        token_expires_at: now - Duration::days(1),
        issued_at: now - Duration::days(8),
    };
    let expired_token = sign_token(&expired_claims);
    crate::db::payments::set_active_token_for_test(
        user_id,
        Some(&expired_token),
        Some(&(now - Duration::days(1)).to_rfc3339()),
        Some(&(now - Duration::days(8)).to_rfc3339()),
    )
    .expect("set active token should succeed");

    mock.set_refresh_outcome(MockRefreshOutcome::RevokedTokenSuperseded);
    let response = server.post("/_app/user/payments/premium/refresh").await;
    response.assert_status_ok();
    let state: PaymentStateView = response.json();
    assert_eq!(state.status, PaymentStateStatus::NotActive);

    let subject = crate::db::payments::load_payment_subject(user_id)
        .expect("subject query should succeed")
        .expect("subject should exist");
    assert!(subject.active_token_history_id.is_none());
    assert_eq!(subject.last_refresh_status.as_deref(), Some("revoked"));
}

#[tokio::test(flavor = "current_thread")]
async fn refresh_token_superseded_keeps_warning_when_reconcile_fails() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let user_id = registered_user_id(&server).await;
    activate_paid_entitlement(&server, user_id, &mock).await;

    let history_before = crate::db::payments::load_active_token_history(user_id)
        .expect("history query should succeed")
        .expect("paid subject should have token");
    let token_before = history_before.active_token.clone();

    // Reconcile returns no active token, so handle_token_superseded falls back to sync_warning.
    mock.set_history_outcome(MockHistoryOutcome::Empty);
    mock.set_refresh_outcome(MockRefreshOutcome::RevokedTokenSuperseded);
    let response = server.post("/_app/user/payments/premium/refresh").await;
    response.assert_status_ok();
    let state: PaymentStateView = response.json();
    assert_eq!(state.status, PaymentStateStatus::ActiveWithSyncWarning);

    let history_after = crate::db::payments::load_active_token_history(user_id)
        .expect("history query should succeed")
        .expect("history should exist");
    assert_eq!(history_after.active_token.as_str(), token_before.as_str());
    let subject = crate::db::payments::load_payment_subject(user_id)
        .expect("subject query should succeed")
        .expect("subject should exist");
    assert_eq!(subject.last_refresh_status.as_deref(), Some("sync_warning"));
}

#[tokio::test(flavor = "current_thread")]
async fn state_local_endpoint_requires_authentication() {
    let server = setup_test_server();
    let response = server.get("/_app/user/payments/state-local").await;
    response.assert_status_unauthorized();
}

#[tokio::test(flavor = "current_thread")]
async fn state_local_endpoint_returns_active_state_without_contacting_central() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let user_id = registered_user_id(&server).await;
    activate_paid_entitlement(&server, user_id, &mock).await;

    // Make the entitlement snapshot stale so the Central-allowed page path
    // would attempt a refresh; the local-only state endpoint must not.
    crate::db::payments::record_payment_refresh_status(
        user_id,
        "error",
        Utc::now() - Duration::hours(25),
    )
    .expect("refresh timestamp should update");
    let refresh_before = mock.refresh_request_count();
    let options_before = mock.product_options_request_count();

    let response = server.get("/_app/user/payments/state-local").await;
    response.assert_status_ok();
    let state: PaymentStateView = response.json();

    assert_eq!(state.status, PaymentStateStatus::Active);
    assert_eq!(mock.refresh_request_count(), refresh_before);
    assert_eq!(mock.product_options_request_count(), options_before);
}

#[tokio::test(flavor = "current_thread")]
async fn state_local_endpoint_returns_in_flight_order_without_contacting_central() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let central_guard = central_guard(&mock);
    let _user_id = registered_user_id(&server).await;

    let response = server
        .post("/_app/user/payments/premium/start")
        .json(&json!({ "product_option_id": "premium_12_months_usd" }))
        .await;
    response.assert_status_ok();

    // Central goes down after the order exists locally.
    drop(central_guard);
    let _unavailable_guard = crate::payments::client::set_central_base_url_override_for_test(
        "http://127.0.0.1:9".to_string(),
    );

    let response = server.get("/_app/user/payments/state-local").await;
    response.assert_status_ok();
    let state: PaymentStateView = response.json();

    assert_eq!(state.order_id.as_deref(), Some(ORDER_ID));
    assert_ne!(state.status, PaymentStateStatus::Unavailable);
}

#[tokio::test(flavor = "current_thread")]
async fn state_local_failed_order_keeps_free_entitlements() {
    let server = setup_test_server();
    let user_id = registered_user_id(&server).await;
    let now = Utc::now();
    crate::db::payments::load_or_create_payment_subject(user_id, now)
        .expect("subject should be created");
    let failed_order_id = PaymentOrderId::from_str(FAILED_ORDER_ID).expect("order id should parse");
    crate::db::payments::insert_payment_order(
        user_id,
        &crate::db::payments::NewPaymentOrder {
            order_id: failed_order_id,
            order_secret: PaymentSecret::from_raw(ORDER_SECRET).expect("order secret should parse"),
            product_tier: ProductTier::Premium,
            amount: PaymentAmount {
                minor_units: 999,
                currency: "USD".to_string(),
                currency_symbol: Some("$".to_string()),
                decimal_precision: 2,
            },
        },
        now,
    )
    .expect("order insert should succeed");
    crate::db::payments::mark_payment_order_status(
        user_id,
        failed_order_id,
        PaymentOrderStatus::Failed,
        None,
        now + Duration::seconds(1),
    )
    .expect("order status should update");

    let response = server.get("/_app/user/payments/state-local").await;
    response.assert_status_ok();
    let state: PaymentStateView = response.json();

    assert_eq!(state.status, PaymentStateStatus::Failed);
    assert_eq!(state.order_id.as_deref(), Some(FAILED_ORDER_ID));
    assert_free_entitlement_state(&state);
}

#[tokio::test(flavor = "current_thread")]
async fn state_local_expired_order_keeps_free_entitlements() {
    let server = setup_test_server();
    let user_id = registered_user_id(&server).await;
    let now = Utc::now();
    crate::db::payments::load_or_create_payment_subject(user_id, now)
        .expect("subject should be created");
    let expired_order_id =
        PaymentOrderId::from_str("01JQABCDEF000000000000000J").expect("order id should parse");
    crate::db::payments::insert_payment_order(
        user_id,
        &crate::db::payments::NewPaymentOrder {
            order_id: expired_order_id,
            order_secret: PaymentSecret::from_raw(ORDER_SECRET).expect("order secret should parse"),
            product_tier: ProductTier::Basic,
            amount: PaymentAmount {
                minor_units: 123,
                currency: "USD".to_string(),
                currency_symbol: Some("$".to_string()),
                decimal_precision: 2,
            },
        },
        now,
    )
    .expect("order insert should succeed");
    crate::db::payments::mark_payment_order_status(
        user_id,
        expired_order_id,
        PaymentOrderStatus::Expired,
        None,
        now + Duration::seconds(1),
    )
    .expect("order status should update");

    let response = server.get("/_app/user/payments/state-local").await;
    response.assert_status_ok();
    let state: PaymentStateView = response.json();

    assert_eq!(state.status, PaymentStateStatus::Expired);
    assert_eq!(
        state.order_id.as_deref(),
        Some("01JQABCDEF000000000000000J")
    );
    assert_free_entitlement_state(&state);
}

#[tokio::test(flavor = "current_thread")]
async fn state_local_pending_premium_order_keeps_free_entitlements() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let central_guard = central_guard(&mock);
    let _user_id = registered_user_id(&server).await;

    server
        .post("/_app/user/payments/premium/start")
        .json(&json!({ "product_option_id": "premium_12_months_usd" }))
        .await
        .assert_status_ok();

    drop(central_guard);
    let _unavailable_guard = crate::payments::client::set_central_base_url_override_for_test(
        "http://127.0.0.1:9".to_string(),
    );

    let response = server.get("/_app/user/payments/state-local").await;
    response.assert_status_ok();
    let state: PaymentStateView = response.json();

    assert_eq!(state.status, PaymentStateStatus::Pending);
    assert_eq!(state.order_id.as_deref(), Some(ORDER_ID));
    assert_eq!(state.display_amount.as_deref(), Some("9.99"));
    assert_eq!(state.currency.as_deref(), Some("USD"));
    assert_free_entitlement_state(&state);
}

#[tokio::test(flavor = "current_thread")]
async fn state_local_pending_basic_order_keeps_free_entitlements() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    mock.set_product_options_response(product_options_response(vec![
        tier_json(
            "basic",
            "Basic",
            10,
            10000,
            vec![product_option_json(
                "basic_12_months_usd",
                12,
                "month",
                "1 year",
                123,
            )],
        ),
        tier_json(
            "premium",
            "Premium",
            50,
            50000,
            vec![product_option_json(
                "premium_12_months_usd",
                12,
                "month",
                "1 year",
                999,
            )],
        ),
    ]));
    let central_guard = central_guard(&mock);
    let _user_id = registered_user_id(&server).await;

    server
        .post("/_app/user/payments/premium/start")
        .json(&json!({ "product_option_id": "basic_12_months_usd" }))
        .await
        .assert_status_ok();

    drop(central_guard);
    let _unavailable_guard = crate::payments::client::set_central_base_url_override_for_test(
        "http://127.0.0.1:9".to_string(),
    );

    let response = server.get("/_app/user/payments/state-local").await;
    response.assert_status_ok();
    let state: PaymentStateView = response.json();

    assert_eq!(state.status, PaymentStateStatus::Pending);
    assert_eq!(state.order_id.as_deref(), Some(ORDER_ID));
    assert_eq!(state.display_amount.as_deref(), Some("9.99"));
    assert_eq!(state.currency.as_deref(), Some("USD"));
    assert_free_entitlement_state(&state);
}

#[tokio::test(flavor = "current_thread")]
async fn catalog_endpoint_returns_central_tiers_and_options() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);
    let _user_id = registered_user_id(&server).await;

    let response = server.get("/_app/user/payments/catalog").await;
    response.assert_status_ok();
    let catalog: crate::payments::views::PaymentCatalogView = response.json();

    assert_eq!(catalog.tiers.len(), 3);
    assert_eq!(catalog.options.len(), 1);
    assert!(catalog.order_history.is_empty());
    assert_eq!(catalog.options_message, None);
}

#[tokio::test(flavor = "current_thread")]
async fn catalog_endpoint_reports_unavailable_when_central_down() {
    let server = setup_test_server();
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let _unavailable_guard = crate::payments::client::set_central_base_url_override_for_test(
        "http://127.0.0.1:9".to_string(),
    );
    let _user_id = registered_user_id(&server).await;

    let response = server.get("/_app/user/payments/catalog").await;
    response.assert_status_ok();
    let catalog: crate::payments::views::PaymentCatalogView = response.json();

    assert!(catalog.tiers.is_empty());
    assert!(catalog.options.is_empty());
    assert_eq!(
        catalog.options_message.as_deref(),
        Some("Price unavailable. Could not reach BitGarth payment service.")
    );
}
