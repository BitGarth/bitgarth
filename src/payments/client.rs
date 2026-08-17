#![cfg(feature = "server")]

use super::keys::expected_signing_key_hash;
use super::types::{
    CAPABILITY_SCHEMA_VERSION_LEGACY, CAPABILITY_SCHEMA_VERSION_V3, CentralOrderNextAction,
    CentralOrderStatus, CentralOrderVerificationState, CentralRefreshStatus, EntitlementHolderId,
    PaymentAmount, PaymentAttemptId, PaymentOrderId, PaymentSecret, ProductOptionId,
    RefreshRevokedReason, TokenId, default_capability_schema_version,
};
use crate::models::UserId;
use crate::traces::client::{
    IntegrationLabel, TracedAsyncClient, TracedClientError, TransportFailure, TransportFailureStage,
};
use chrono::{DateTime, Utc};
use dioxus::logger::tracing;
#[cfg(all(test, not(bitgarth_db_unit_only)))]
use once_cell::sync::Lazy;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::fmt;
#[cfg(all(test, not(bitgarth_db_unit_only)))]
use std::sync::Mutex;
use std::time::Duration;

#[cfg(feature = "dev-config")]
const CENTRAL_BASE_URL_ENV: &str = "BITGARTH_CENTRAL_BASE_URL";
const DEFAULT_CENTRAL_BASE_URL: &str = "https://bitgarth.com";
const EXPECTED_SIGNING_KEY_HASH_HEADER: &str = "X-BitGarth-Expected-Signing-Key-Hash";
const SUPPORTED_CAPABILITY_SCHEMA_VERSION_HEADER: &str =
    "X-BitGarth-Supported-Capability-Schema-Version";
const APP_VERSION_HEADER: &str = "X-BitGarth-App-Version";
const APP_CHANNEL_HEADER: &str = "X-BitGarth-App-Channel";
const CENTRAL_TIMEOUT: Duration = Duration::from_secs(10);

fn app_metadata_header_values() -> [(&'static str, String); 2] {
    [
        (APP_VERSION_HEADER, crate::version::version().to_string()),
        (
            APP_CHANNEL_HEADER,
            crate::channel::channel().as_header_value().to_string(),
        ),
    ]
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
static CENTRAL_BASE_URL_OVERRIDE: Lazy<Mutex<Option<String>>> = Lazy::new(|| Mutex::new(None));

#[cfg(all(test, not(bitgarth_db_unit_only)))]
pub(crate) struct CentralBaseUrlOverrideGuard;

#[cfg(all(test, not(bitgarth_db_unit_only)))]
impl Drop for CentralBaseUrlOverrideGuard {
    fn drop(&mut self) {
        let mut override_value = CENTRAL_BASE_URL_OVERRIDE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *override_value = None;
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
pub(crate) fn set_central_base_url_override_for_test(
    base_url: String,
) -> CentralBaseUrlOverrideGuard {
    let mut override_value = CENTRAL_BASE_URL_OVERRIDE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *override_value = Some(base_url);
    CentralBaseUrlOverrideGuard
}

pub(crate) struct BitGarthCentralClient {
    base_url: String,
    expected_signing_key_hash: String,
    http: TracedAsyncClient,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub(crate) struct LatestAppVersionResponse {
    pub(crate) latest: String,
    pub(crate) image: Option<String>,
    pub(crate) release_url: String,
    pub(crate) published_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CentralOrderSession {
    pub(crate) order_id: PaymentOrderId,
    pub(crate) product_option_id: ProductOptionId,
    pub(crate) order_secret: PaymentSecret,
    pub(crate) merchant_id: String,
    pub(crate) order_amount: PaymentAmount,
    pub(crate) payment_attempt: CentralPaymentAttempt,
    pub(crate) management_secret: Option<PaymentSecret>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CentralPaymentAttempt {
    pub(crate) payment_attempt_id: PaymentAttemptId,
    pub(crate) atlos_order_id: String,
    pub(crate) amount: PaymentAmount,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CentralProductOptions {
    pub(crate) tiers: Vec<CentralProductTier>,
    pub(crate) options: Vec<CentralProductOption>,
    pub(crate) app_compatibility: Option<CentralAppCompatibility>,
    /// Central-authored plan-comparison paragraph with `**bold**` tier labels.
    /// Normalized to `None` when absent or blank.
    pub(crate) pricing_summary: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CentralProductTier {
    pub(crate) tier: String,
    pub(crate) display_name: String,
    pub(crate) capabilities: CentralTierCapabilities,
    pub(crate) presentation: CentralTierPresentation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CentralTierCapabilities {
    pub(crate) capability_set_id: Option<String>,
    pub(crate) capability_schema_version: u16,
    pub(crate) sync_account_slots: u16,
    pub(crate) historical_backfill_transactions_per_account: u32,
    pub(crate) historical_sync: bool,
    pub(crate) transaction_history_sync: bool,
    pub(crate) balance_sync: bool,
    pub(crate) exchange_rates_current: bool,
    pub(crate) exchange_rates_history: bool,
    pub(crate) price_overrides: bool,
    pub(crate) balance_assertions: bool,
    pub(crate) hledger_export: bool,
    pub(crate) tax_reports: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CentralTierPresentation {
    pub(crate) summary: String,
    pub(crate) bullets: Vec<String>,
    pub(crate) is_featured: bool,
    pub(crate) ribbon_label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CentralProductOption {
    pub(crate) id: ProductOptionId,
    pub(crate) tier: String,
    pub(crate) tier_display_name: String,
    pub(crate) term_quantity: u16,
    pub(crate) term_unit: String,
    pub(crate) term_label: String,
    pub(crate) price: PaymentAmount,
    pub(crate) display_order: Option<u16>,
    pub(crate) is_default: bool,
    pub(crate) badge: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CentralAppCompatibility {
    pub(crate) status: CentralAppCompatibilityStatus,
    pub(crate) detail: String,
    pub(crate) minimum_app_version: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CentralAppCompatibilityStatus {
    UpgradeRequired,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CentralOrderStatusOutcome {
    pub(crate) status: CentralOrderStatus,
    pub(crate) verification_state: CentralOrderVerificationState,
    pub(crate) next_action: CentralOrderNextAction,
    pub(crate) manual_review: Option<CentralManualReview>,
    pub(crate) payments: Vec<CentralOrderPayment>,
    pub(crate) paid_amount_minor_units: Option<u64>,
    pub(crate) remaining_amount: Option<PaymentAmount>,
    pub(crate) additional_payment_request: Option<CentralAdditionalPaymentRequest>,
    pub(crate) paid_details: Option<CentralPaidOrderDetails>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CentralPaidOrderDetails {
    pub(crate) premium_access_token: String,
    pub(crate) token_id: TokenId,
    pub(crate) subscription_valid_until: DateTime<Utc>,
    pub(crate) token_expires_at: DateTime<Utc>,
    pub(crate) paid_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CentralManualReview {
    pub(crate) reason: String,
    pub(crate) resolved: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CentralOrderPayment {
    pub(crate) payment_id: String,
    pub(crate) payment_attempt_id: Option<PaymentAttemptId>,
    pub(crate) status: String,
    pub(crate) confirmed_at: Option<DateTime<Utc>>,
    pub(crate) seen_at: Option<DateTime<Utc>>,
    pub(crate) paid_order_amount: PaymentAmount,
    pub(crate) paid_asset_amount: Option<CentralPaidAssetAmount>,
    pub(crate) recipient_address: Option<String>,
    pub(crate) blockchain_hash: Option<String>,
    pub(crate) block_number: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CentralPaidAssetAmount {
    pub(crate) amount: String,
    pub(crate) asset_code: Option<String>,
    pub(crate) blockchain_code: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CentralAdditionalPaymentRequest {
    pub(crate) payment_attempt_id: PaymentAttemptId,
    pub(crate) merchant_id: String,
    pub(crate) atlos_order_id: String,
    pub(crate) amount: PaymentAmount,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CentralRefreshOutcome {
    Active {
        premium_access_token: String,
        token_id: TokenId,
        subscription_valid_until: DateTime<Utc>,
        token_expires_at: DateTime<Utc>,
    },
    Revoked {
        reason: RefreshRevokedReason,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CentralTransferOutcome {
    Active {
        premium_access_token: String,
        token_id: TokenId,
        subscription_valid_until: DateTime<Utc>,
        token_expires_at: DateTime<Utc>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CentralHistoryOrder {
    pub(crate) order_id: PaymentOrderId,
    pub(crate) status: CentralOrderStatus,
    pub(crate) paid_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CentralHistoryOutcome {
    History {
        orders: Vec<CentralHistoryOrder>,
        premium_access_token: Option<String>,
        token_id: Option<TokenId>,
        subscription_valid_until: Option<DateTime<Utc>>,
        token_expires_at: Option<DateTime<Utc>>,
    },
}

#[derive(Debug)]
pub(crate) enum CentralClientError {
    Build(String),
    Url(String),
    Request(String),
    ResponseEncoding(String),
    ResponseJson(String),
    Contract(String),
    Http {
        status: StatusCode,
        error_code: Option<String>,
        message: String,
    },
}

impl CentralClientError {
    pub(crate) fn is_upgrade_required(&self) -> bool {
        match self {
            Self::Http {
                status, error_code, ..
            } => {
                *status == StatusCode::UPGRADE_REQUIRED
                    || matches!(
                        error_code.as_deref(),
                        Some(
                            "app_upgrade_required"
                                | "invalid_expected_signing_key"
                                | "unsupported_signing_key"
                        )
                    )
            }
            _ => false,
        }
    }
}

impl fmt::Display for CentralClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Build(message) => write!(f, "failed to build Central client: {message}"),
            Self::Url(message) => write!(f, "invalid Central URL: {message}"),
            Self::Request(message) => write!(f, "Central request failed: {message}"),
            Self::ResponseEncoding(message) => {
                write!(f, "Central response encoding failed: {message}")
            }
            Self::ResponseJson(message) => write!(f, "Central response JSON failed: {message}"),
            Self::Contract(message) => write!(f, "Central response contract failed: {message}"),
            Self::Http {
                status,
                message,
                error_code,
            } => {
                if let Some(error_code) = error_code {
                    write!(f, "Central returned {status}: {error_code}: {message}")
                } else {
                    write!(f, "Central returned {status}: {message}")
                }
            }
        }
    }
}

impl std::error::Error for CentralClientError {}

impl From<TracedClientError> for CentralClientError {
    fn from(value: TracedClientError) -> Self {
        Self::Build(value.to_string())
    }
}

impl BitGarthCentralClient {
    pub(crate) fn new(user_id: UserId) -> Result<Self, CentralClientError> {
        let http = TracedAsyncClient::builder(IntegrationLabel::new("bitgarth-central"), user_id)
            .configure(|builder| builder.timeout(CENTRAL_TIMEOUT))
            .redact_json_body_fields(&[
                "order_secret",
                "management_secret",
                "new_management_secret",
                "entitlement_token",
                "premium_access_token",
            ])
            .build()?;

        Ok(Self {
            base_url: central_base_url(),
            expected_signing_key_hash: expected_signing_key_hash()
                .map_err(|err| CentralClientError::Build(err.to_string()))?,
            http,
        })
    }

    pub(crate) async fn create_order_session(
        &self,
        entitlement_holder_id: EntitlementHolderId,
        product_option_id: ProductOptionId,
        management_secret: Option<&PaymentSecret>,
    ) -> Result<CentralOrderSession, CentralClientError> {
        let request = CreateOrderSessionRequest {
            entitlement_holder_id,
            product_option_id,
        };
        let body = serde_json::to_vec(&request)
            .map_err(|err| CentralClientError::Contract(err.to_string()))?;
        let url = self.url("/api/v1/payments/orders/session")?;

        let mut request = self.with_payment_metadata(
            self.http
                .post(url)
                .header("Content-Type", "application/json")
                .header(
                    EXPECTED_SIGNING_KEY_HASH_HEADER,
                    &self.expected_signing_key_hash,
                )
                .header("Accept-Language", "en")
                .body(body),
        );
        if let Some(secret) = management_secret {
            request = request.header("Authorization", format!("Bearer {}", secret.as_str()));
        }

        let response = request.send().await.map_err(|error| {
            let error = error.without_url();
            let failure =
                TransportFailure::from_reqwest_error(TransportFailureStage::SendFailed, &error);
            CentralClientError::Request(failure.persistence_message())
        })?;
        let status = response.status();
        let text = response
            .text()
            .map_err(|err| CentralClientError::ResponseEncoding(err.to_string()))?;
        let raw: RawCreateOrderSessionResponse = parse_success(status, &text)?;
        raw.try_into_order_session()
    }

    pub(crate) async fn payment_product_options(
        &self,
    ) -> Result<CentralProductOptions, CentralClientError> {
        let response = self
            .with_payment_metadata(
                self.http
                    .get(self.url("/api/v1/payments/product-options")?)
                    .header(
                        EXPECTED_SIGNING_KEY_HASH_HEADER,
                        &self.expected_signing_key_hash,
                    )
                    .header("Accept-Language", "en"),
            )
            .send()
            .await
            .map_err(|error| {
                let error = error.without_url();
                let failure =
                    TransportFailure::from_reqwest_error(TransportFailureStage::SendFailed, &error);
                CentralClientError::Request(failure.persistence_message())
            })?;
        let status = response.status();
        let text = response
            .text()
            .map_err(|err| CentralClientError::ResponseEncoding(err.to_string()))?;
        let raw: RawProductOptionsResponse = parse_success(status, &text)?;
        raw.try_into_product_options()
    }

    pub(crate) async fn latest_app_version(
        &self,
    ) -> Result<LatestAppVersionResponse, CentralClientError> {
        let response = self
            .with_app_metadata(self.http.get(self.url("/api/v1/latest-app-version")?))
            .send()
            .await
            .map_err(|error| {
                let error = error.without_url();
                let failure =
                    TransportFailure::from_reqwest_error(TransportFailureStage::SendFailed, &error);
                CentralClientError::Request(failure.persistence_message())
            })?;
        let status = response.status();
        let text = response
            .text()
            .map_err(|err| CentralClientError::ResponseEncoding(err.to_string()))?;
        parse_success(status, &text)
    }

    pub(crate) async fn order_status(
        &self,
        order_id: PaymentOrderId,
        order_secret: &PaymentSecret,
    ) -> Result<CentralOrderStatusOutcome, CentralClientError> {
        let url = self.url(&format!(
            "/api/v1/payments/orders/{}/status",
            order_id.to_storage_value()
        ))?;
        let response = self
            .with_payment_metadata(
                self.http
                    .get(url)
                    .header("Authorization", format!("Bearer {}", order_secret.as_str()))
                    .header(
                        EXPECTED_SIGNING_KEY_HASH_HEADER,
                        &self.expected_signing_key_hash,
                    )
                    .header("Accept-Language", "en"),
            )
            .send()
            .await
            .map_err(|error| {
                let error = error.without_url();
                let failure =
                    TransportFailure::from_reqwest_error(TransportFailureStage::SendFailed, &error);
                CentralClientError::Request(failure.persistence_message())
            })?;
        let status = response.status();
        let text = response
            .text()
            .map_err(|err| CentralClientError::ResponseEncoding(err.to_string()))?;
        let raw: RawOrderStatusResponse = parse_success(status, &text)?;
        raw.try_into_outcome()
    }

    pub(crate) async fn refresh_subscription(
        &self,
        entitlement_holder_id: EntitlementHolderId,
        token_id: TokenId,
        management_secret: &PaymentSecret,
        last_known_token: Option<String>,
    ) -> Result<CentralRefreshOutcome, CentralClientError> {
        let request = RefreshSubscriptionRequest {
            entitlement_holder_id,
            token_id,
            last_known_token,
        };
        let body = serde_json::to_vec(&request)
            .map_err(|err| CentralClientError::Contract(err.to_string()))?;
        let response = self
            .with_payment_metadata(
                self.http
                    .post(self.url("/api/v1/payments/subscription/refresh")?)
                    .header(
                        "Authorization",
                        format!("Bearer {}", management_secret.as_str()),
                    )
                    .header("Content-Type", "application/json")
                    .header(
                        EXPECTED_SIGNING_KEY_HASH_HEADER,
                        &self.expected_signing_key_hash,
                    )
                    .header("Accept-Language", "en")
                    .body(body),
            )
            .send()
            .await
            .map_err(|error| {
                let error = error.without_url();
                let failure =
                    TransportFailure::from_reqwest_error(TransportFailureStage::SendFailed, &error);
                CentralClientError::Request(failure.persistence_message())
            })?;
        let status = response.status();
        let text = response
            .text()
            .map_err(|err| CentralClientError::ResponseEncoding(err.to_string()))?;
        let raw: RawRefreshSubscriptionResponse = parse_success(status, &text)?;
        raw.try_into_outcome()
    }

    pub(crate) async fn subscription_history(
        &self,
        management_secret: &PaymentSecret,
    ) -> Result<CentralHistoryOutcome, CentralClientError> {
        let response = self
            .with_payment_metadata(
                self.http
                    .get(self.url("/api/v1/payments/subscription/history")?)
                    .header(
                        "Authorization",
                        format!("Bearer {}", management_secret.as_str()),
                    )
                    .header(
                        EXPECTED_SIGNING_KEY_HASH_HEADER,
                        &self.expected_signing_key_hash,
                    )
                    .header("Accept-Language", "en"),
            )
            .send()
            .await
            .map_err(|error| {
                let error = error.without_url();
                let failure =
                    TransportFailure::from_reqwest_error(TransportFailureStage::SendFailed, &error);
                CentralClientError::Request(failure.persistence_message())
            })?;
        let status = response.status();
        let text = response
            .text()
            .map_err(|err| CentralClientError::ResponseEncoding(err.to_string()))?;
        let raw: RawHistoryResponse = parse_success(status, &text)?;
        raw.try_into_outcome()
    }

    pub(crate) async fn transfer_subscription(
        &self,
        current_management_secret: &PaymentSecret,
        new_entitlement_holder_id: EntitlementHolderId,
        new_management_secret: &PaymentSecret,
    ) -> Result<CentralTransferOutcome, CentralClientError> {
        let request = TransferSubscriptionRequest {
            new_entitlement_holder_id,
            new_management_secret: new_management_secret.as_str().to_string(),
        };
        let body = serde_json::to_vec(&request)
            .map_err(|err| CentralClientError::Contract(err.to_string()))?;
        let response = self
            .with_payment_metadata(
                self.http
                    .post(self.url("/api/v1/payments/subscription/transfer")?)
                    .header(
                        "Authorization",
                        format!("Bearer {}", current_management_secret.as_str()),
                    )
                    .header("Content-Type", "application/json")
                    .header(
                        EXPECTED_SIGNING_KEY_HASH_HEADER,
                        &self.expected_signing_key_hash,
                    )
                    .header("Accept-Language", "en")
                    .body(body),
            )
            .send()
            .await
            .map_err(|error| {
                let error = error.without_url();
                let failure =
                    TransportFailure::from_reqwest_error(TransportFailureStage::SendFailed, &error);
                CentralClientError::Request(failure.persistence_message())
            })?;
        let status = response.status();
        let text = response
            .text()
            .map_err(|err| CentralClientError::ResponseEncoding(err.to_string()))?;
        let raw: RawTransferSubscriptionResponse = parse_success(status, &text)?;
        raw.try_into_outcome()
    }

    fn url(&self, path: &str) -> Result<String, CentralClientError> {
        let base = reqwest::Url::parse(&format!("{}/", self.base_url.trim_end_matches('/')))
            .map_err(|err| CentralClientError::Url(err.to_string()))?;
        base.join(path.trim_start_matches('/'))
            .map(|url| url.to_string())
            .map_err(|err| CentralClientError::Url(err.to_string()))
    }

    fn with_app_metadata<'a>(
        &self,
        mut request: crate::traces::client::TracedAsyncRequestBuilder<'a>,
    ) -> crate::traces::client::TracedAsyncRequestBuilder<'a> {
        for (name, value) in app_metadata_header_values() {
            request = request.header(name, value);
        }
        request
    }

    fn with_payment_metadata<'a>(
        &self,
        request: crate::traces::client::TracedAsyncRequestBuilder<'a>,
    ) -> crate::traces::client::TracedAsyncRequestBuilder<'a> {
        self.with_app_metadata(request).header(
            SUPPORTED_CAPABILITY_SCHEMA_VERSION_HEADER,
            CAPABILITY_SCHEMA_VERSION_V3.to_string(),
        )
    }
}

fn central_base_url() -> String {
    #[cfg(all(test, not(bitgarth_db_unit_only)))]
    {
        if let Some(value) = CENTRAL_BASE_URL_OVERRIDE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        {
            return value;
        }
    }

    #[cfg(feature = "dev-config")]
    if let Ok(value) = std::env::var(CENTRAL_BASE_URL_ENV) {
        return value;
    }

    DEFAULT_CENTRAL_BASE_URL.to_string()
}

fn parse_success<T: for<'de> Deserialize<'de>>(
    status: StatusCode,
    text: &str,
) -> Result<T, CentralClientError> {
    if status.is_success() {
        return serde_json::from_str(text)
            .map_err(|err| CentralClientError::ResponseJson(err.to_string()));
    }

    let error = serde_json::from_str::<CentralErrorResponse>(text).unwrap_or_default();
    Err(CentralClientError::Http {
        status,
        error_code: error.error_code,
        message: error
            .message
            .or(error.error)
            .unwrap_or_else(|| "Central request failed".to_string()),
    })
}

fn parse_secret(value: String, field: &'static str) -> Result<PaymentSecret, CentralClientError> {
    PaymentSecret::from_raw(value).map_err(|err| {
        CentralClientError::Contract(format!("invalid Central {field} secret: {err}"))
    })
}

#[derive(Serialize)]
struct CreateOrderSessionRequest {
    entitlement_holder_id: EntitlementHolderId,
    product_option_id: ProductOptionId,
}

#[derive(Serialize)]
struct RefreshSubscriptionRequest {
    entitlement_holder_id: EntitlementHolderId,
    token_id: TokenId,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_known_token: Option<String>,
}

#[derive(Serialize)]
struct TransferSubscriptionRequest {
    new_entitlement_holder_id: EntitlementHolderId,
    new_management_secret: String,
}

#[derive(Deserialize)]
struct RawCreateOrderSessionResponse {
    order_id: PaymentOrderId,
    product_option_id: ProductOptionId,
    order_secret: String,
    merchant_id: String,
    order_amount: PaymentAmount,
    payment_attempt: RawPaymentAttempt,
    management_secret: Option<String>,
}

#[derive(Deserialize)]
struct RawPaymentAttempt {
    payment_attempt_id: PaymentAttemptId,
    provider: String,
    atlos_order_id: String,
    amount: PaymentAmount,
}

#[derive(Deserialize)]
struct RawTransferSubscriptionResponse {
    status: CentralRefreshStatus,
    #[serde(alias = "entitlement_token")]
    premium_access_token: Option<String>,
    token_id: Option<TokenId>,
    subscription_valid_until: Option<DateTime<Utc>>,
    token_expires_at: Option<DateTime<Utc>>,
}

impl RawTransferSubscriptionResponse {
    fn try_into_outcome(self) -> Result<CentralTransferOutcome, CentralClientError> {
        match self.status {
            CentralRefreshStatus::Active => Ok(CentralTransferOutcome::Active {
                premium_access_token: self.premium_access_token.ok_or_else(|| {
                    CentralClientError::Contract(
                        "active transfer missing premium token".to_string(),
                    )
                })?,
                token_id: self.token_id.ok_or_else(|| {
                    CentralClientError::Contract("active transfer missing token id".to_string())
                })?,
                subscription_valid_until: self.subscription_valid_until.ok_or_else(|| {
                    CentralClientError::Contract(
                        "active transfer missing subscription expiry".to_string(),
                    )
                })?,
                token_expires_at: self.token_expires_at.ok_or_else(|| {
                    CentralClientError::Contract("active transfer missing token expiry".to_string())
                })?,
            }),
            CentralRefreshStatus::Revoked => Err(CentralClientError::Contract(
                "transfer response unexpectedly returned revoked".to_string(),
            )),
        }
    }
}

impl RawCreateOrderSessionResponse {
    fn try_into_order_session(self) -> Result<CentralOrderSession, CentralClientError> {
        Ok(CentralOrderSession {
            order_id: self.order_id,
            product_option_id: self.product_option_id,
            order_secret: parse_secret(self.order_secret, "order")?,
            merchant_id: self.merchant_id,
            order_amount: self.order_amount,
            payment_attempt: self.payment_attempt.try_into_payment_attempt()?,
            management_secret: self
                .management_secret
                .map(|secret| parse_secret(secret, "management"))
                .transpose()?,
        })
    }
}

impl RawPaymentAttempt {
    fn try_into_payment_attempt(self) -> Result<CentralPaymentAttempt, CentralClientError> {
        if self.provider != "atlos" {
            return Err(CentralClientError::Contract(format!(
                "unsupported payment attempt provider {}",
                self.provider
            )));
        }
        if self.atlos_order_id.trim().is_empty() {
            return Err(CentralClientError::Contract(
                "payment attempt missing atlos_order_id".to_string(),
            ));
        }
        if self.amount.minor_units == 0 {
            return Err(CentralClientError::Contract(
                "payment attempt amount must be positive".to_string(),
            ));
        }
        Ok(CentralPaymentAttempt {
            payment_attempt_id: self.payment_attempt_id,
            atlos_order_id: self.atlos_order_id,
            amount: self.amount,
        })
    }
}

#[derive(Deserialize)]
struct RawProductOptionsResponse {
    catalog_schema_version: u16,
    tiers: Vec<Value>,
    app_compatibility: Option<RawAppCompatibility>,
    #[serde(default)]
    pricing_summary: Option<String>,
}

#[derive(Deserialize)]
struct RawProductTier {
    tier: String,
    display_name: String,
    #[serde(default = "default_capability_schema_version")]
    capability_schema_version: u16,
    capability_set_id: Option<String>,
    capabilities: RawTierCapabilities,
    presentation: RawTierPresentation,
    purchase_options: Vec<Value>,
}

#[derive(Deserialize)]
struct RawTierPresentation {
    #[serde(default)]
    summary: String,
    #[serde(default)]
    bullets: Vec<String>,
    #[serde(default)]
    is_featured: bool,
    #[serde(default)]
    ribbon_label: Option<String>,
}

#[derive(Deserialize)]
struct RawTierCapabilities {
    limits: RawTierCapabilityLimits,
    #[serde(default)]
    features: RawTierFeatureFlags,
}

#[derive(Deserialize)]
struct RawTierCapabilityLimits {
    #[serde(default)]
    accounts: Option<RawTierAccountLimits>,
    synced_accounts: Option<u16>,
    history: RawTierCapabilityHistory,
}

#[derive(Deserialize)]
struct RawTierAccountLimits {
    total: Option<u16>,
}

#[derive(Deserialize)]
struct RawTierCapabilityHistory {
    max_transactions_per_account: u32,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
struct RawTierFeatureFlags {
    #[serde(default)]
    historical_sync: bool,
    #[serde(default)]
    transaction_history_sync: bool,
    #[serde(default)]
    balance_sync: bool,
    #[serde(default)]
    exchange_rates_current: bool,
    #[serde(default)]
    exchange_rates_history: bool,
    #[serde(default)]
    price_overrides: bool,
    #[serde(default)]
    balance_assertions: bool,
    #[serde(default)]
    hledger_export: bool,
    #[serde(default)]
    tax_reports: bool,
}

#[derive(Deserialize, Default)]
struct RawProductOptionPresentation {
    display_order: Option<u16>,
    #[serde(default)]
    is_default: bool,
    badge: Option<String>,
}

#[derive(Deserialize)]
struct RawAppCompatibility {
    status: RawAppCompatibilityStatus,
    detail: String,
    minimum_app_version: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawAppCompatibilityStatus {
    UpgradeRequired,
}

/// Lowest catalog schema the app knows how to parse. Responses below this are
/// rejected outright. Responses above it are parsed tolerantly using the fields
/// this version understands: Central guarantees additive-only changes within a
/// major, and signals genuinely breaking changes via `app_compatibility`
/// (`upgrade_required`) rather than by relying on this hard version gate.
const MIN_CATALOG_SCHEMA_VERSION: u16 = 4;

impl RawProductOptionsResponse {
    fn try_into_product_options(self) -> Result<CentralProductOptions, CentralClientError> {
        if self.catalog_schema_version < MIN_CATALOG_SCHEMA_VERSION {
            return Err(CentralClientError::Contract(format!(
                "product options response had unsupported schema version {} (expected >= {MIN_CATALOG_SCHEMA_VERSION})",
                self.catalog_schema_version
            )));
        }
        if self.catalog_schema_version > MIN_CATALOG_SCHEMA_VERSION {
            tracing::warn!(
                catalog_schema_version = self.catalog_schema_version,
                supported = MIN_CATALOG_SCHEMA_VERSION,
                "payments: Central product options used a newer catalog schema version; \
                 parsing with the fields this app version understands"
            );
        }
        if self.tiers.is_empty() {
            return Err(CentralClientError::Contract(
                "product options response contained no tiers".to_string(),
            ));
        }
        let mut seen = HashSet::new();
        let mut tiers = Vec::with_capacity(self.tiers.len());
        let mut options = Vec::new();
        for (index, raw) in self.tiers.into_iter().enumerate() {
            match parse_product_tier(raw, &mut seen) {
                Ok(Some((tier, mut tier_options))) => {
                    tiers.push(tier);
                    options.append(&mut tier_options);
                }
                Ok(None) => {
                    tracing::warn!(
                        tier_index = index,
                        "payments: skipped malformed Central product tier"
                    );
                }
                Err(error) => {
                    tracing::warn!(
                        tier_index = index,
                        error = %error,
                        "payments: skipped malformed Central product tier"
                    );
                }
            }
        }

        if tiers.is_empty() {
            return Err(CentralClientError::Contract(
                "product options response contained no usable tiers".to_string(),
            ));
        }

        Ok(CentralProductOptions {
            tiers,
            options,
            pricing_summary: self
                .pricing_summary
                .map(|summary| summary.trim().to_string())
                .filter(|summary| !summary.is_empty()),
            app_compatibility: self.app_compatibility.map(|raw| CentralAppCompatibility {
                status: match raw.status {
                    RawAppCompatibilityStatus::UpgradeRequired => {
                        CentralAppCompatibilityStatus::UpgradeRequired
                    }
                },
                detail: raw.detail,
                minimum_app_version: raw.minimum_app_version,
            }),
        })
    }
}

fn parse_product_tier(
    raw: Value,
    seen: &mut HashSet<ProductOptionId>,
) -> Result<Option<(CentralProductTier, Vec<CentralProductOption>)>, CentralClientError> {
    let raw: RawProductTier = serde_json::from_value(raw).map_err(|err| {
        CentralClientError::Contract(format!("product tier has invalid shape: {err}"))
    })?;
    let tier = raw.tier.trim().to_string();
    if tier.is_empty() {
        return Err(CentralClientError::Contract(
            "product tier has empty tier".to_string(),
        ));
    }
    let display_name = raw.display_name.trim().to_string();
    if display_name.is_empty() {
        return Err(CentralClientError::Contract(format!(
            "product tier {tier} has empty display_name"
        )));
    }

    let mut options = Vec::with_capacity(raw.purchase_options.len());
    for (index, option) in raw.purchase_options.into_iter().enumerate() {
        match parse_product_option(option, &tier, &display_name, seen) {
            Ok(Some(option)) => options.push(option),
            Ok(None) => {
                tracing::warn!(
                    product_tier = tier,
                    option_index = index,
                    "payments: skipped malformed Central product option"
                );
            }
            Err(error) => {
                tracing::warn!(
                    product_tier = tier,
                    option_index = index,
                    error = %error,
                    "payments: skipped malformed Central product option"
                );
            }
        }
    }

    let presentation = build_tier_presentation(&tier, raw.presentation)?;
    let capability_schema_version = raw.capability_schema_version;
    let sync_account_slots =
        account_limit_for_tier(capability_schema_version, &raw.capabilities.limits)?;
    let transaction_history_sync =
        transaction_history_sync_for_tier(capability_schema_version, &raw.capabilities.features);
    let features = raw.capabilities.features;

    Ok(Some((
        CentralProductTier {
            tier,
            display_name,
            capabilities: CentralTierCapabilities {
                capability_set_id: raw.capability_set_id,
                capability_schema_version,
                sync_account_slots,
                historical_backfill_transactions_per_account: raw
                    .capabilities
                    .limits
                    .history
                    .max_transactions_per_account,
                historical_sync: features.historical_sync,
                transaction_history_sync,
                balance_sync: features.balance_sync,
                exchange_rates_current: features.exchange_rates_current,
                exchange_rates_history: features.exchange_rates_history,
                price_overrides: features.price_overrides,
                balance_assertions: features.balance_assertions,
                hledger_export: features.hledger_export,
                tax_reports: features.tax_reports,
            },
            presentation,
        },
        options,
    )))
}

fn account_limit_for_tier(
    capability_schema_version: u16,
    limits: &RawTierCapabilityLimits,
) -> Result<u16, CentralClientError> {
    match capability_schema_version {
        CAPABILITY_SCHEMA_VERSION_LEGACY => limits.synced_accounts.ok_or_else(|| {
            CentralClientError::Contract(
                "legacy product tier missing limits.synced_accounts".to_string(),
            )
        }),
        CAPABILITY_SCHEMA_VERSION_V3 => limits
            .accounts
            .as_ref()
            .and_then(|accounts| accounts.total)
            .ok_or_else(|| {
                CentralClientError::Contract(
                    "v3 product tier missing limits.accounts.total".to_string(),
                )
            }),
        _ => Err(CentralClientError::Contract(format!(
            "unsupported capability schema version {capability_schema_version}"
        ))),
    }
}

fn transaction_history_sync_for_tier(
    capability_schema_version: u16,
    features: &RawTierFeatureFlags,
) -> bool {
    match capability_schema_version {
        CAPABILITY_SCHEMA_VERSION_LEGACY => features.historical_sync,
        CAPABILITY_SCHEMA_VERSION_V3 => features.transaction_history_sync,
        _ => false,
    }
}

fn build_tier_presentation(
    tier: &str,
    raw: RawTierPresentation,
) -> Result<CentralTierPresentation, CentralClientError> {
    if raw.summary.trim().is_empty() {
        return Err(CentralClientError::Contract(format!(
            "product tier {tier} has empty presentation.summary"
        )));
    }
    if raw.is_featured && raw.ribbon_label.as_deref().is_none_or(str::is_empty) {
        return Err(CentralClientError::Contract(format!(
            "product tier {tier} is featured but has no ribbon_label"
        )));
    }
    if !raw.is_featured && raw.ribbon_label.is_some() {
        return Err(CentralClientError::Contract(format!(
            "product tier {tier} has ribbon_label but is not featured"
        )));
    }
    Ok(CentralTierPresentation {
        summary: raw.summary,
        bullets: raw.bullets,
        is_featured: raw.is_featured,
        ribbon_label: raw.ribbon_label,
    })
}

fn parse_product_option(
    raw: Value,
    tier: &str,
    tier_display_name: &str,
    seen: &mut HashSet<ProductOptionId>,
) -> Result<Option<CentralProductOption>, CentralClientError> {
    let Value::Object(raw) = raw else {
        return Err(CentralClientError::Contract(
            "product option row was not an object".to_string(),
        ));
    };

    let id = raw
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| CentralClientError::Contract("product option missing id".to_string()))
        .and_then(|value| {
            ProductOptionId::from_raw(value.to_string()).map_err(|err| {
                CentralClientError::Contract(format!("product option has invalid id: {err}"))
            })
        })?;
    if !seen.insert(id.clone()) {
        return Err(CentralClientError::Contract(format!(
            "duplicate product option id {id}"
        )));
    }

    let Value::Object(term) = raw
        .get("term")
        .cloned()
        .ok_or_else(|| CentralClientError::Contract(format!("product option {id} missing term")))?
    else {
        return Err(CentralClientError::Contract(format!(
            "product option {id} has invalid term"
        )));
    };
    let term_quantity = parse_required_u16_field(&term, "quantity", &id)?;
    let term_unit = parse_required_string_field(&term, "unit", &id)?;
    let term_label = term
        .get("label")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CentralClientError::Contract(format!("product option {id} has empty term label"))
        })?
        .to_string();

    let price: PaymentAmount =
        serde_json::from_value(raw.get("price").cloned().ok_or_else(|| {
            CentralClientError::Contract(format!("product option {id} missing price"))
        })?)
        .map_err(|err| {
            CentralClientError::Contract(format!("product option {id} has invalid price: {err}"))
        })?;
    if price.currency.trim().is_empty() {
        return Err(CentralClientError::Contract(format!(
            "product option {id} has empty currency"
        )));
    }
    if price
        .currency_symbol
        .as_deref()
        .is_none_or(|symbol| symbol.trim().is_empty())
    {
        return Err(CentralClientError::Contract(format!(
            "product option {id} has empty currency_symbol"
        )));
    }

    let presentation: RawProductOptionPresentation = raw
        .get("presentation")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    Ok(Some(CentralProductOption {
        id,
        tier: tier.to_string(),
        tier_display_name: tier_display_name.to_string(),
        term_quantity,
        term_unit,
        term_label,
        price,
        display_order: presentation.display_order,
        is_default: presentation.is_default,
        badge: presentation.badge,
    }))
}

fn parse_required_u16_field(
    raw: &serde_json::Map<String, Value>,
    field: &str,
    id: &ProductOptionId,
) -> Result<u16, CentralClientError> {
    match raw.get(field) {
        Some(Value::Number(number)) => number
            .as_u64()
            .and_then(|value| u16::try_from(value).ok())
            .filter(|value| *value > 0)
            .ok_or_else(|| {
                CentralClientError::Contract(format!("product option {id} has invalid {field}"))
            }),
        Some(_) => Err(CentralClientError::Contract(format!(
            "product option {id} has invalid {field}"
        ))),
        None => Err(CentralClientError::Contract(format!(
            "product option {id} missing {field}"
        ))),
    }
}

fn parse_required_string_field(
    raw: &serde_json::Map<String, Value>,
    field: &str,
    id: &ProductOptionId,
) -> Result<String, CentralClientError> {
    match raw.get(field) {
        Some(Value::String(value)) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return Err(CentralClientError::Contract(format!(
                    "product option {id} has empty {field}"
                )));
            }
            Ok(trimmed.to_string())
        }
        Some(_) => Err(CentralClientError::Contract(format!(
            "product option {id} has invalid {field}"
        ))),
        None => Err(CentralClientError::Contract(format!(
            "product option {id} missing {field}"
        ))),
    }
}

#[derive(Deserialize)]
struct RawOrderStatusResponse {
    status: CentralOrderStatus,
    verification_state: CentralOrderVerificationState,
    next_action: CentralOrderNextAction,
    #[serde(default)]
    manual_review: Option<RawManualReviewSummary>,
    #[serde(default)]
    payments: Vec<RawOrderPayment>,
    paid_amount_minor_units: Option<u64>,
    remaining_amount: Option<PaymentAmount>,
    additional_payment_request: Option<RawAdditionalPaymentRequest>,
    #[serde(alias = "entitlement_token")]
    premium_access_token: Option<String>,
    token_id: Option<TokenId>,
    subscription_valid_until: Option<DateTime<Utc>>,
    token_expires_at: Option<DateTime<Utc>>,
    paid_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
struct RawManualReviewSummary {
    reason: String,
    resolved: bool,
}

#[derive(Deserialize)]
struct RawAdditionalPaymentRequest {
    payment_attempt_id: PaymentAttemptId,
    provider: String,
    merchant_id: String,
    atlos_order_id: String,
    amount: PaymentAmount,
}

#[derive(Deserialize)]
struct RawOrderPayment {
    payment_id: String,
    payment_attempt_id: Option<PaymentAttemptId>,
    status: String,
    confirmed_at: Option<DateTime<Utc>>,
    seen_at: Option<DateTime<Utc>>,
    paid_order_amount: PaymentAmount,
    paid_asset_amount: Option<RawPaidAssetAmount>,
    recipient_address: Option<String>,
    blockchain_hash: Option<String>,
    block_number: Option<u64>,
}

#[derive(Deserialize)]
struct RawPaidAssetAmount {
    amount: String,
    asset_code: Option<String>,
    blockchain_code: Option<String>,
}

impl RawOrderStatusResponse {
    fn try_into_outcome(self) -> Result<CentralOrderStatusOutcome, CentralClientError> {
        if self.verification_state == CentralOrderVerificationState::UnderManualReview
            && self.manual_review.is_none()
        {
            return Err(CentralClientError::Contract(
                "manual review state missing manual_review summary".to_string(),
            ));
        }

        if matches!(
            self.verification_state,
            CentralOrderVerificationState::PaymentConfirmedUnverified
                | CentralOrderVerificationState::UnderManualReview
                | CentralOrderVerificationState::AdditionalPaymentRequired
        ) && self.payments.is_empty()
        {
            return Err(CentralClientError::Contract(
                "verification state requires payment summary".to_string(),
            ));
        }

        if self.next_action == CentralOrderNextAction::RequestAdditionalPayment {
            let remaining_amount = self.remaining_amount.as_ref().ok_or_else(|| {
                CentralClientError::Contract(
                    "request_additional_payment missing remaining_amount".to_string(),
                )
            })?;
            if remaining_amount.minor_units == 0 {
                return Err(CentralClientError::Contract(
                    "remaining_amount must be positive".to_string(),
                ));
            }
            let additional_payment_request =
                self.additional_payment_request.as_ref().ok_or_else(|| {
                    CentralClientError::Contract(
                        "request_additional_payment missing additional_payment_request".to_string(),
                    )
                })?;
            if additional_payment_request.amount != *remaining_amount {
                return Err(CentralClientError::Contract(
                    "additional_payment_request amount must match remaining_amount".to_string(),
                ));
            }
            if self.verification_state != CentralOrderVerificationState::AdditionalPaymentRequired {
                return Err(CentralClientError::Contract(
                    "request_additional_payment requires additional_payment_required state"
                        .to_string(),
                ));
            }
        } else if self.additional_payment_request.is_some() {
            return Err(CentralClientError::Contract(
                "additional_payment_request requires request_additional_payment".to_string(),
            ));
        }

        let paid_details = match self.status {
            CentralOrderStatus::Paid => Some(CentralPaidOrderDetails {
                premium_access_token: self.premium_access_token.ok_or_else(|| {
                    CentralClientError::Contract("paid order missing premium token".to_string())
                })?,
                token_id: self.token_id.ok_or_else(|| {
                    CentralClientError::Contract("paid order missing token id".to_string())
                })?,
                subscription_valid_until: self.subscription_valid_until.ok_or_else(|| {
                    CentralClientError::Contract(
                        "paid order missing subscription expiry".to_string(),
                    )
                })?,
                token_expires_at: self.token_expires_at.ok_or_else(|| {
                    CentralClientError::Contract("paid order missing token expiry".to_string())
                })?,
                paid_at: self.paid_at.ok_or_else(|| {
                    CentralClientError::Contract("paid order missing paid_at".to_string())
                })?,
            }),
            CentralOrderStatus::Pending
            | CentralOrderStatus::Expired
            | CentralOrderStatus::Failed => {
                if self.premium_access_token.is_some()
                    || self.token_id.is_some()
                    || self.subscription_valid_until.is_some()
                    || self.token_expires_at.is_some()
                    || self.paid_at.is_some()
                {
                    return Err(CentralClientError::Contract(
                        "non-paid order unexpectedly included premium token fields".to_string(),
                    ));
                }
                None
            }
        };

        if self.next_action == CentralOrderNextAction::UnlockPremium
            && (self.status != CentralOrderStatus::Paid
                || self.verification_state != CentralOrderVerificationState::PremiumGranted)
        {
            return Err(CentralClientError::Contract(
                "unlock_premium requires paid premium_granted order".to_string(),
            ));
        }

        Ok(CentralOrderStatusOutcome {
            status: self.status,
            verification_state: self.verification_state,
            next_action: self.next_action,
            manual_review: self.manual_review.map(|manual_review| CentralManualReview {
                reason: manual_review.reason,
                resolved: manual_review.resolved,
            }),
            payments: self
                .payments
                .into_iter()
                .map(|payment| CentralOrderPayment {
                    payment_id: payment.payment_id,
                    payment_attempt_id: payment.payment_attempt_id,
                    status: payment.status,
                    confirmed_at: payment.confirmed_at,
                    seen_at: payment.seen_at,
                    paid_order_amount: payment.paid_order_amount,
                    paid_asset_amount: payment.paid_asset_amount.map(|asset| {
                        CentralPaidAssetAmount {
                            amount: asset.amount,
                            asset_code: asset.asset_code,
                            blockchain_code: asset.blockchain_code,
                        }
                    }),
                    recipient_address: payment.recipient_address,
                    blockchain_hash: payment.blockchain_hash,
                    block_number: payment.block_number,
                })
                .collect(),
            paid_amount_minor_units: self.paid_amount_minor_units,
            remaining_amount: self.remaining_amount,
            additional_payment_request: self
                .additional_payment_request
                .map(RawAdditionalPaymentRequest::try_into_additional_payment_request)
                .transpose()?,
            paid_details,
        })
    }
}

impl RawAdditionalPaymentRequest {
    fn try_into_additional_payment_request(
        self,
    ) -> Result<CentralAdditionalPaymentRequest, CentralClientError> {
        if self.provider != "atlos" {
            return Err(CentralClientError::Contract(format!(
                "unsupported additional payment provider {}",
                self.provider
            )));
        }
        if self.merchant_id.trim().is_empty() {
            return Err(CentralClientError::Contract(
                "additional_payment_request missing merchant_id".to_string(),
            ));
        }
        if self.atlos_order_id.trim().is_empty() {
            return Err(CentralClientError::Contract(
                "additional_payment_request missing atlos_order_id".to_string(),
            ));
        }
        if self.amount.minor_units == 0 {
            return Err(CentralClientError::Contract(
                "additional_payment_request amount must be positive".to_string(),
            ));
        }
        Ok(CentralAdditionalPaymentRequest {
            payment_attempt_id: self.payment_attempt_id,
            merchant_id: self.merchant_id,
            atlos_order_id: self.atlos_order_id,
            amount: self.amount,
        })
    }
}

#[derive(Deserialize)]
struct RawRefreshSubscriptionResponse {
    status: CentralRefreshStatus,
    #[serde(alias = "entitlement_token")]
    premium_access_token: Option<String>,
    token_id: Option<TokenId>,
    subscription_valid_until: Option<DateTime<Utc>>,
    token_expires_at: Option<DateTime<Utc>>,
    reason: Option<RefreshRevokedReason>,
}

impl RawRefreshSubscriptionResponse {
    fn try_into_outcome(self) -> Result<CentralRefreshOutcome, CentralClientError> {
        match self.status {
            CentralRefreshStatus::Active => Ok(CentralRefreshOutcome::Active {
                premium_access_token: self.premium_access_token.ok_or_else(|| {
                    CentralClientError::Contract("active refresh missing premium token".to_string())
                })?,
                token_id: self.token_id.ok_or_else(|| {
                    CentralClientError::Contract("active refresh missing token id".to_string())
                })?,
                subscription_valid_until: self.subscription_valid_until.ok_or_else(|| {
                    CentralClientError::Contract(
                        "active refresh missing subscription expiry".to_string(),
                    )
                })?,
                token_expires_at: self.token_expires_at.ok_or_else(|| {
                    CentralClientError::Contract("active refresh missing token expiry".to_string())
                })?,
            }),
            CentralRefreshStatus::Revoked => Ok(CentralRefreshOutcome::Revoked {
                reason: self.reason.ok_or_else(|| {
                    CentralClientError::Contract("revoked refresh missing reason".to_string())
                })?,
            }),
        }
    }
}

#[derive(Default, Deserialize)]
struct CentralErrorResponse {
    error_code: Option<String>,
    message: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct RawHistoryResponse {
    orders: Vec<RawHistoryOrder>,
    #[serde(alias = "entitlement_token")]
    premium_access_token: Option<String>,
    token_id: Option<TokenId>,
    subscription_valid_until: Option<DateTime<Utc>>,
    token_expires_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
struct RawHistoryOrder {
    order_id: PaymentOrderId,
    status: CentralOrderStatus,
    paid_at: Option<DateTime<Utc>>,
}

impl RawHistoryResponse {
    fn try_into_outcome(self) -> Result<CentralHistoryOutcome, CentralClientError> {
        let orders = self
            .orders
            .into_iter()
            .map(|raw| CentralHistoryOrder {
                order_id: raw.order_id,
                status: raw.status,
                paid_at: raw.paid_at,
            })
            .collect();
        Ok(CentralHistoryOutcome::History {
            orders,
            premium_access_token: self.premium_access_token,
            token_id: self.token_id,
            subscription_valid_until: self.subscription_valid_until,
            token_expires_at: self.token_expires_at,
        })
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::State,
        http::HeaderMap,
        response::{IntoResponse, Response},
        routing::post,
    };
    use serde_json::json;
    use std::str::FromStr as _;
    use std::sync::{Arc, Mutex};

    fn test_holder_id() -> EntitlementHolderId {
        EntitlementHolderId::from_str("01JQABCDEF000000000000000D")
            .expect("test holder id should parse")
    }

    #[test]
    fn create_order_request_uses_product_option_id() {
        let request = CreateOrderSessionRequest {
            entitlement_holder_id: test_holder_id(),
            product_option_id: ProductOptionId::from_str("premium_12_months_usd")
                .expect("product option id should parse"),
        };

        let serialized = serde_json::to_value(request).expect("request should serialize");

        assert_eq!(
            serialized.get("product_option_id"),
            Some(&json!("premium_12_months_usd"))
        );
        assert!(serialized.get("product_tier").is_none());
    }

    #[test]
    fn app_metadata_headers_use_version_and_declared_channel() {
        let headers = app_metadata_header_values();
        assert_eq!(headers[0].0, APP_VERSION_HEADER);
        assert_eq!(headers[0].1, crate::version::version());
        assert_eq!(headers[1].0, APP_CHANNEL_HEADER);
        assert_eq!(headers[1].1, crate::channel::channel().as_header_value());
    }

    #[tokio::test]
    async fn refresh_subscription_declares_supported_capability_schema() {
        #[derive(Default)]
        struct HeaderCapture {
            supported_capability_schema_version: Option<String>,
        }

        async fn refresh_endpoint(
            State(capture): State<Arc<Mutex<HeaderCapture>>>,
            headers: HeaderMap,
        ) -> Response {
            let observed = headers
                .get("x-bitgarth-supported-capability-schema-version")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            capture
                .lock()
                .expect("header capture lock should not be poisoned")
                .supported_capability_schema_version = observed;

            Json(json!({
                "status": "active",
                "entitlement_token": "test-token",
                "token_id": "01JQABCDEF000000000000000E",
                "subscription_valid_until": "2027-01-01T00:00:00Z",
                "token_expires_at": "2026-07-10T00:00:00Z"
            }))
            .into_response()
        }

        let capture = Arc::new(Mutex::new(HeaderCapture::default()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test Central should bind");
        let base_url = format!(
            "http://{}",
            listener
                .local_addr()
                .expect("test Central should have local address")
        );
        let router = Router::new()
            .route(
                "/api/v1/payments/subscription/refresh",
                post(refresh_endpoint),
            )
            .with_state(Arc::clone(&capture));
        tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("test Central should serve");
        });

        let _base_url_guard = set_central_base_url_override_for_test(base_url);
        let client =
            BitGarthCentralClient::new(crate::models::UserId::new()).expect("client should build");
        let management_secret =
            PaymentSecret::from_raw("frPMkDek45GSAMEAFTXV5ORxF8p3c5_MqPg7Zq-bNuI")
                .expect("management secret should parse");

        let _outcome = client
            .refresh_subscription(
                test_holder_id(),
                TokenId::from_str("01JQABCDEF000000000000000E").expect("token id should parse"),
                &management_secret,
                None,
            )
            .await
            .expect("refresh should succeed");

        assert_eq!(
            capture
                .lock()
                .expect("header capture lock should not be poisoned")
                .supported_capability_schema_version
                .as_deref(),
            Some("3")
        );
    }

    #[test]
    fn order_session_response_parses_product_option_id_and_non_default_amount() {
        let raw: RawCreateOrderSessionResponse = serde_json::from_value(json!({
            "order_id": "01JQABCDEF000000000000000E",
            "product_option_id": "premium_12_months_usd",
            "order_secret": "frPMkDek45GSAMEAFTXV5ORxF8p3c5_MqPg7Zq-bNuI",
            "merchant_id": "8MY8BXTU15",
            "order_amount": {
                "minor_units": 123,
                "currency": "USD",
                "currency_symbol": "$",
                "decimal_precision": 2
            },
            "payment_attempt": {
                "payment_attempt_id": "01JQABCDEF000000000000000F",
                "provider": "atlos",
                "atlos_order_id": "01JQABCDEF000000000000000G",
                "amount": {
                    "minor_units": 123,
                    "currency": "USD",
                    "currency_symbol": "$",
                    "decimal_precision": 2
                }
            },
            "management_secret": null
        }))
        .expect("raw response should parse");

        let session = raw
            .try_into_order_session()
            .expect("order session should convert");

        assert_eq!(session.product_option_id.as_str(), "premium_12_months_usd");
        assert_eq!(session.order_amount.atlos_decimal_amount(), "1.23");
        assert_eq!(session.order_amount.currency_symbol.as_deref(), Some("$"));
        assert_eq!(
            session.payment_attempt.payment_attempt_id.to_string(),
            "01JQABCDEF000000000000000F"
        );
        assert_eq!(
            session.payment_attempt.atlos_order_id,
            "01JQABCDEF000000000000000G"
        );
        assert_eq!(
            session.payment_attempt.amount.atlos_decimal_amount(),
            "1.23"
        );
    }

    #[test]
    fn product_options_response_parses_tier_grouped_response() {
        let raw: RawProductOptionsResponse = serde_json::from_value(json!({
            "catalog_schema_version": 4,
            "pricing_summary": "**Free** tracks holdings. **Paid** does the accounting.",
            "tiers": [
                {
                    "tier": "free",
                    "display_name": "Free",
                    "capabilities": {
                        "limits": {
                            "synced_accounts": 5,
                            "history": { "max_transactions_per_account": 0 }
                        }
                    },
                    "presentation": {
                        "summary": "Local ownership, balance-only.",
                        "bullets": ["**5** balance-synced accounts"]
                    },
                    "purchase_options": []
                },
                {
                    "tier": "premium",
                    "display_name": "Premium",
                    "capabilities": {
                        "limits": {
                            "synced_accounts": 50,
                            "history": { "max_transactions_per_account": 50000 }
                        }
                    },
                    "presentation": {
                        "summary": "Fifty synced accounts, deep histories.",
                        "bullets": ["**50** synced accounts"],
                        "is_featured": true,
                        "ribbon_label": "Best value"
                    },
                    "purchase_options": [{
                        "id": "premium_test_1_day_usd",
                        "term": {
                            "quantity": 1,
                            "unit": "day",
                            "label": "1 day (test)"
                        },
                        "price": {
                            "minor_units": 1,
                            "currency": "USD",
                            "currency_symbol": "$",
                            "decimal_precision": 2
                        }
                    }]
                }
            ]
        }))
        .expect("raw response should parse");

        let response = raw
            .try_into_product_options()
            .expect("product options should convert");

        assert_eq!(response.tiers.len(), 2);
        assert_eq!(response.tiers[0].tier, "free");
        assert_eq!(response.tiers[0].display_name, "Free");
        assert_eq!(response.tiers[0].capabilities.sync_account_slots, 5);
        assert_eq!(
            response.tiers[0].presentation.summary,
            "Local ownership, balance-only."
        );
        assert_eq!(
            response.tiers[0].presentation.bullets,
            vec!["**5** balance-synced accounts".to_string()]
        );
        assert!(!response.tiers[0].presentation.is_featured);
        assert!(response.tiers[0].presentation.ribbon_label.is_none());
        assert!(response.tiers[1].presentation.is_featured);
        assert_eq!(
            response.tiers[1].presentation.ribbon_label.as_deref(),
            Some("Best value")
        );
        assert_eq!(response.options.len(), 1);
        assert_eq!(response.options[0].id.as_str(), "premium_test_1_day_usd");
        assert_eq!(response.options[0].tier, "premium");
        assert_eq!(response.options[0].tier_display_name, "Premium");
        assert_eq!(response.options[0].term_quantity, 1);
        assert_eq!(response.options[0].term_unit, "day");
        assert_eq!(response.options[0].term_label, "1 day (test)");
        assert_eq!(response.options[0].price.atlos_decimal_amount(), "0.01");
        assert!(response.app_compatibility.is_none());
        assert_eq!(
            response.pricing_summary.as_deref(),
            Some("**Free** tracks holdings. **Paid** does the accounting.")
        );
    }

    #[test]
    fn product_options_response_normalizes_missing_or_blank_pricing_summary() {
        let tier = json!({
            "tier": "premium",
            "display_name": "Premium",
            "capabilities": {
                "limits": {
                    "synced_accounts": 50,
                    "history": { "max_transactions_per_account": 50000 }
                }
            },
            "presentation": {
                "summary": "Fifty synced accounts, deep histories.",
                "bullets": ["**50** synced accounts"]
            },
            "purchase_options": []
        });

        let missing: RawProductOptionsResponse = serde_json::from_value(json!({
            "catalog_schema_version": 4,
            "tiers": [tier.clone()]
        }))
        .expect("raw response should parse");
        let missing = missing
            .try_into_product_options()
            .expect("product options should convert");
        assert!(missing.pricing_summary.is_none());

        let blank: RawProductOptionsResponse = serde_json::from_value(json!({
            "catalog_schema_version": 4,
            "pricing_summary": "   ",
            "tiers": [tier]
        }))
        .expect("raw response should parse");
        let blank = blank
            .try_into_product_options()
            .expect("product options should convert");
        assert!(blank.pricing_summary.is_none());
    }

    #[test]
    fn product_options_response_rejects_old_schema_version() {
        let raw: RawProductOptionsResponse = serde_json::from_value(json!({
            "catalog_schema_version": 3,
            "tiers": []
        }))
        .expect("raw response should parse");

        let error = raw
            .try_into_product_options()
            .expect_err("schema 3 should be rejected");
        assert!(
            error.to_string().contains("schema version 3"),
            "expected schema error, got {error}"
        );
    }

    #[test]
    fn product_options_response_accepts_newer_schema_version_with_compatible_fields() {
        // Central bumped the catalog version and added fields the app does not
        // know about (a new `features` block on capabilities, a new per-option
        // field, a new top-level field). All v4-compatible fields are still
        // present, so the app parses tolerantly and shows the payment page.
        let raw: RawProductOptionsResponse = serde_json::from_value(json!({
            "catalog_schema_version": 5,
            "new_top_level_field": "ignored",
            "tiers": [
                {
                    "tier": "premium",
                    "display_name": "Premium",
                    "capabilities": {
                        "limits": {
                            "synced_accounts": 50,
                            "history": { "max_transactions_per_account": 50000 }
                        },
                        "features": { "historical_sync": true, "background_sync": true }
                    },
                    "presentation": {
                        "summary": "Fifty synced accounts, deep histories.",
                        "bullets": ["**50** synced accounts"]
                    },
                    "purchase_options": [{
                        "id": "premium_test_1_day_usd",
                        "term": {
                            "quantity": 1,
                            "unit": "day",
                            "label": "1 day (test)"
                        },
                        "price": {
                            "minor_units": 1,
                            "currency": "USD",
                            "currency_symbol": "$",
                            "decimal_precision": 2
                        },
                        "new_option_field": "ignored"
                    }]
                }
            ]
        }))
        .expect("raw response should parse");

        let response = raw
            .try_into_product_options()
            .expect("newer additive schema should convert");

        assert_eq!(response.tiers.len(), 1);
        assert_eq!(response.tiers[0].tier, "premium");
        assert_eq!(response.tiers[0].capabilities.sync_account_slots, 50);
        assert_eq!(
            response.tiers[0]
                .capabilities
                .historical_backfill_transactions_per_account,
            50000
        );
        assert_eq!(response.options.len(), 1);
        assert_eq!(response.options[0].id.as_str(), "premium_test_1_day_usd");
        assert_eq!(response.options[0].price.atlos_decimal_amount(), "0.01");
    }

    #[test]
    fn product_options_response_parses_v3_canonical_capabilities() {
        let raw: RawProductOptionsResponse = serde_json::from_value(json!({
            "catalog_schema_version": 4,
            "tiers": [{
                "tier": "basic",
                "display_name": "Basic",
                "capability_schema_version": 3,
                "capability_set_id": "basic.v3",
                "capabilities": {
                    "limits": {
                        "accounts": { "total": 10 },
                        "synced_accounts": 2,
                        "history": { "max_transactions_per_account": 5000 }
                    },
                    "features": {
                        "historical_sync": false,
                        "transaction_history_sync": true,
                        "balance_sync": true,
                        "exchange_rates_current": true,
                        "exchange_rates_history": true,
                        "price_overrides": true,
                        "balance_assertions": true,
                        "hledger_export": true,
                        "tax_reports": true
                    }
                },
                "presentation": {
                    "summary": "Ten accounts with transaction history.",
                    "bullets": ["**10** accounts"]
                },
                "purchase_options": []
            }]
        }))
        .expect("raw response should parse");

        let response = raw
            .try_into_product_options()
            .expect("v3 product options should parse");

        assert_eq!(response.tiers[0].capabilities.capability_schema_version, 3);
        assert_eq!(
            response.tiers[0].capabilities.capability_set_id.as_deref(),
            Some("basic.v3")
        );
        assert_eq!(response.tiers[0].capabilities.sync_account_slots, 10);
        assert!(response.tiers[0].capabilities.transaction_history_sync);
        assert_eq!(
            response.tiers[0]
                .capabilities
                .historical_backfill_transactions_per_account,
            5000
        );
    }

    #[test]
    fn product_options_response_skips_v3_tier_missing_accounts_total() {
        let raw: RawProductOptionsResponse = serde_json::from_value(json!({
            "catalog_schema_version": 4,
            "tiers": [{
                "tier": "basic",
                "display_name": "Basic",
                "capability_schema_version": 3,
                "capabilities": {
                    "limits": {
                        "history": { "max_transactions_per_account": 5000 }
                    },
                    "features": {
                        "transaction_history_sync": true
                    }
                },
                "presentation": {
                    "summary": "Ten accounts with transaction history.",
                    "bullets": ["**10** accounts"]
                },
                "purchase_options": []
            }]
        }))
        .expect("raw response should parse");

        let error = raw
            .try_into_product_options()
            .expect_err("missing v3 account total should skip all tiers");

        assert!(error.to_string().contains("no usable tiers"));
    }

    #[test]
    fn product_options_response_skips_unknown_capability_schema_version() {
        let raw: RawProductOptionsResponse = serde_json::from_value(json!({
            "catalog_schema_version": 4,
            "tiers": [{
                "tier": "basic",
                "display_name": "Basic",
                "capability_schema_version": 4,
                "capabilities": {
                    "limits": {
                        "synced_accounts": 50,
                        "history": { "max_transactions_per_account": 50000 }
                    },
                    "features": {
                        "historical_sync": true
                    }
                },
                "presentation": {
                    "summary": "Legacy-looking future schema.",
                    "bullets": ["**50** synced accounts"]
                },
                "purchase_options": []
            }]
        }))
        .expect("raw response should parse");

        let error = raw
            .try_into_product_options()
            .expect_err("unknown capability schema should skip all tiers");

        assert!(error.to_string().contains("no usable tiers"));
    }

    #[test]
    fn product_options_response_fails_when_v3_drops_accounts_total() {
        // v3 account limits are canonical under `accounts.total`. Tolerant
        // parsing cannot recover the tier when Central omits that field, so it
        // is skipped; with no usable tiers left the conversion fails and the
        // caller takes the product-options-unavailable path.
        let raw: RawProductOptionsResponse = serde_json::from_value(json!({
            "catalog_schema_version": 5,
            "tiers": [
                {
                    "tier": "premium",
                    "display_name": "Premium",
                    "capabilities": {
                        "limits": {
                            "history": { "max_transactions_per_account": 50000 }
                        }
                    },
                    "presentation": {
                        "summary": "Fifty synced accounts.",
                        "bullets": ["**50** synced accounts"]
                    },
                    "purchase_options": []
                }
            ]
        }))
        .expect("raw response should parse");

        let error = raw
            .try_into_product_options()
            .expect_err("schema 5 missing required field should fall back");
        assert!(
            error.to_string().contains("no usable tiers"),
            "expected no-usable-tiers error, got {error}"
        );
    }

    #[test]
    fn product_options_response_skips_featured_tier_without_ribbon_but_keeps_others() {
        // A tier with `is_featured: true` and no `ribbon_label` is a Central
        // catalog bug. The app stays resilient: the bad tier is skipped with a
        // log line, the good one survives. (If *every* tier is bad, the outer
        // loop surfaces a "no usable tiers" error.)
        let raw: RawProductOptionsResponse = serde_json::from_value(json!({
            "catalog_schema_version": 4,
            "tiers": [
                {
                    "tier": "premium",
                    "display_name": "Premium",
                    "capabilities": {
                        "limits": {
                            "synced_accounts": 50,
                            "history": { "max_transactions_per_account": 50000 }
                        }
                    },
                    "presentation": {
                        "summary": "Fifty synced accounts.",
                        "bullets": ["**50** synced accounts"],
                        "is_featured": true
                    },
                    "purchase_options": []
                },
                {
                    "tier": "free",
                    "display_name": "Free",
                    "capabilities": {
                        "limits": {
                            "synced_accounts": 5,
                            "history": { "max_transactions_per_account": 0 }
                        }
                    },
                    "presentation": {
                        "summary": "Local ownership.",
                        "bullets": ["**5** balance-synced accounts"]
                    },
                    "purchase_options": []
                }
            ]
        }))
        .expect("raw response should parse");

        let response = raw
            .try_into_product_options()
            .expect("good tier should survive");
        assert_eq!(response.tiers.len(), 1);
        assert_eq!(response.tiers[0].tier, "free");
    }

    #[test]
    fn product_options_response_skips_invalid_rows_when_valid_rows_remain() {
        let raw: RawProductOptionsResponse = serde_json::from_value(json!({
            "catalog_schema_version": 4,
            "tiers": [{
                "tier": "premium",
                "display_name": "Premium",
                "capabilities": {
                    "limits": {
                        "synced_accounts": 50,
                        "history": { "max_transactions_per_account": 50000 }
                    }
                },
                "presentation": {
                    "summary": "Fifty synced accounts.",
                    "bullets": ["**50** synced accounts"]
                },
                "purchase_options": [
                {
                    "id": "premium_12_months_usd",
                    "term": {
                        "quantity": 12,
                        "unit": "month",
                        "label": "1 year"
                    },
                    "price": {
                        "minor_units": 123,
                        "currency": "USD",
                        "decimal_precision": 2
                    }
                },
                {
                    "id": "premium_test_1_day_usd",
                    "term": {
                        "quantity": 1,
                        "unit": "day",
                        "label": "1 day (test)"
                    },
                    "price": {
                        "minor_units": 1,
                        "currency": "USD",
                        "currency_symbol": "$",
                        "decimal_precision": 2
                    }
                }
                ]
            }]
        }))
        .expect("raw response should parse");

        let response = raw
            .try_into_product_options()
            .expect("valid rows should survive");

        assert_eq!(response.options.len(), 1);
        assert_eq!(response.options[0].id.as_str(), "premium_test_1_day_usd");
    }

    #[test]
    fn product_options_response_keeps_first_duplicate_id() {
        let raw: RawProductOptionsResponse = serde_json::from_value(json!({
            "catalog_schema_version": 4,
            "tiers": [{
                "tier": "premium",
                "display_name": "Premium",
                "capabilities": {
                    "limits": {
                        "synced_accounts": 50,
                        "history": { "max_transactions_per_account": 50000 }
                    }
                },
                "presentation": {
                    "summary": "Fifty synced accounts.",
                    "bullets": ["**50** synced accounts"]
                },
                "purchase_options": [
                {
                    "id": "premium_12_months_usd",
                    "term": {
                        "quantity": 12,
                        "unit": "month",
                        "label": "1 year"
                    },
                    "price": {
                        "minor_units": 123,
                        "currency": "USD",
                        "currency_symbol": "$",
                        "decimal_precision": 2
                    }
                },
                {
                    "id": "premium_12_months_usd",
                    "term": {
                        "quantity": 12,
                        "unit": "month",
                        "label": "1 year"
                    },
                    "price": {
                        "minor_units": 456,
                        "currency": "USD",
                        "currency_symbol": "$",
                        "decimal_precision": 2
                    }
                }
                ]
            }]
        }))
        .expect("raw response should parse");

        let response = raw
            .try_into_product_options()
            .expect("first duplicate id should be kept");

        assert_eq!(response.options.len(), 1);
        assert_eq!(response.options[0].id.as_str(), "premium_12_months_usd");
        assert_eq!(response.options[0].price.atlos_decimal_amount(), "1.23");
    }

    #[test]
    fn product_options_response_tolerates_unknown_tiers() {
        let raw: RawProductOptionsResponse = serde_json::from_value(json!({
            "catalog_schema_version": 4,
            "tiers": [{
                "tier": "business",
                "display_name": "Business",
                "capabilities": {
                    "limits": {
                        "synced_accounts": 100,
                        "history": { "max_transactions_per_account": 100000 }
                    }
                },
                "presentation": {
                    "summary": "Hundred synced accounts.",
                    "bullets": ["**100** synced accounts"]
                },
                "purchase_options": [{
                    "id": "business_12_months_usd",
                    "term": {
                        "quantity": 12,
                        "unit": "month",
                        "label": "1 year"
                    },
                    "price": {
                        "minor_units": 123,
                        "currency": "USD",
                        "currency_symbol": "$",
                        "decimal_precision": 2
                    }
                }]
            }]
        }))
        .expect("raw response should parse");

        let response = raw
            .try_into_product_options()
            .expect("unknown tiers should not fail the whole response");

        assert_eq!(response.options.len(), 1);
        assert_eq!(response.options[0].tier, "business");
        assert_eq!(response.options[0].tier_display_name, "Business");
    }

    #[test]
    fn order_status_response_parses_verifying_state_with_payment_summary() {
        let raw: RawOrderStatusResponse = serde_json::from_value(json!({
            "status": "pending",
            "verification_state": "payment_confirmed_unverified",
            "next_action": "keep_polling",
            "payments": [{
                "payment_id": "19A2D79298D2BC37A7D9569D8A",
                "payment_attempt_id": "01JQABCDEF000000000000000F",
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
        .expect("raw order status should parse");

        let outcome = raw
            .try_into_outcome()
            .expect("verifying response should convert");

        assert_eq!(outcome.status, CentralOrderStatus::Pending);
        assert_eq!(
            outcome.verification_state,
            CentralOrderVerificationState::PaymentConfirmedUnverified
        );
        assert_eq!(outcome.next_action, CentralOrderNextAction::KeepPolling);
        assert_eq!(outcome.payments.len(), 1);
        assert_eq!(
            outcome.payments[0].paid_order_amount.atlos_decimal_amount(),
            "8.00"
        );
        assert!(outcome.manual_review.is_none());
        assert!(outcome.paid_details.is_none());
    }

    #[test]
    fn order_status_response_parses_additional_payment_request() {
        let raw: RawOrderStatusResponse = serde_json::from_value(json!({
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
                "atlos_order_id": "01JQABCDEF000000000000000H",
                "amount": {
                    "minor_units": 199,
                    "currency": "USD",
                    "decimal_precision": 2
                }
            },
            "payments": [{
                "payment_id": "19A2D79298D2BC37A7D9569D8A",
                "status": "confirmed",
                "paid_order_amount": {
                    "minor_units": 800,
                    "currency": "USD",
                    "decimal_precision": 2
                }
            }]
        }))
        .expect("raw order status should parse");

        let outcome = raw
            .try_into_outcome()
            .expect("additional payment response should convert");

        assert_eq!(
            outcome.verification_state,
            CentralOrderVerificationState::AdditionalPaymentRequired
        );
        assert_eq!(
            outcome.next_action,
            CentralOrderNextAction::RequestAdditionalPayment
        );
        assert_eq!(
            outcome
                .additional_payment_request
                .expect("request should be present")
                .atlos_order_id,
            "01JQABCDEF000000000000000H"
        );
        assert_eq!(
            outcome
                .remaining_amount
                .expect("remaining amount")
                .minor_units,
            199
        );
    }

    #[test]
    fn order_status_response_requires_manual_review_summary() {
        let raw: RawOrderStatusResponse = serde_json::from_value(json!({
            "status": "expired",
            "verification_state": "under_manual_review",
            "next_action": "show_manual_review",
            "payments": [{
                "payment_id": "19A2D79298D2BC37A7D9569D8A",
                "status": "confirmed",
                "paid_order_amount": {
                    "minor_units": 800,
                    "currency": "USD",
                    "decimal_precision": 2
                }
            }]
        }))
        .expect("raw order status should parse");

        let error = raw
            .try_into_outcome()
            .expect_err("manual review summary should be required");

        assert!(error.to_string().contains("manual_review summary"));
    }

    #[test]
    fn order_status_response_requires_paid_fields_for_unlock() {
        let raw: RawOrderStatusResponse = serde_json::from_value(json!({
            "status": "paid",
            "verification_state": "premium_granted",
            "next_action": "unlock_premium"
        }))
        .expect("raw order status should parse");

        let error = raw
            .try_into_outcome()
            .expect_err("paid response should require token fields");

        assert!(
            error
                .to_string()
                .contains("paid order missing premium token")
        );
    }
}
