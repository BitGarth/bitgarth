//! `/api/v1` routes for paired clients such as the `bitgarth` CLI.
//!
//! Every handler authenticates the Client Key, checks the paired client's
//! permission, and then calls the same `*_for_user` core the web app's server
//! functions use, so both surfaces share validation and business rules.

use std::future::Future;
use std::str::FromStr;

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use chrono::{DateTime, Utc};
use dioxus::server::FullstackState;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::authorize_client_request;
use crate::backend::ApiErrorEnvelope;
use crate::backend::pairing::api_error;
use crate::backend::{prices, settings, transactions, wallets};
use crate::client_capabilities::ClientPermission;
use crate::models::{
    CurrencyCode, DateTimeFormat, FieldErrors, NumberFormat, RawEtherscanApiKey,
    RawEtherscanBaseUrl, RawMempoolBaseUrl, SimpleApiKey, UserId, UserTimezone,
};
use crate::wallets::{
    AddBtcAddressRequest, AddEthAddressRequest, AddManualAssetAccountRequest,
    AddManualAssetBalanceAssertionRequest, AddXpubRequest, AddressScheme, DeleteAccountRequest,
    DeleteManualAssetBalanceAssertionRequest, DeleteWalletRequest, DigitalAssetAccountId,
    GetAccountAddressesRequest, ManualAssetBalanceAssertionId, MoveAccountRequest, MoveDestination,
    RawLabel, ReportDateParam, ReportTimezoneParam, SearchManualAssetInstancesRequest,
    UpdateAccountLabelRequest, UpdateManualAssetBalanceAssertionRequest, UpdateWalletLabelRequest,
    ValidateXpubRequest, WalletAccountId, WalletId,
};

use ClientPermission::{Full, Read};

pub(super) fn routes() -> Router<FullstackState> {
    Router::new()
        .route("/me", get(me))
        .route("/clients/self", axum::routing::delete(revoke_self))
        .route("/exports/wallet-data", post(export_wallet_data))
        .route(
            "/imports/wallet-data",
            post(import_wallet_data).layer(axum::extract::DefaultBodyLimit::max(14 * 1024 * 1024)),
        )
        .route("/wallets", get(list_wallets))
        .route(
            "/wallets/{wallet_id}",
            axum::routing::patch(rename_wallet).delete(delete_wallet),
        )
        .route("/wallets/{wallet_id}/report", get(wallet_report))
        .route("/reports/holdings", get(holdings_report))
        .route("/accounts/bitcoin-address", post(add_bitcoin_address))
        .route("/accounts/ethereum-address", post(add_ethereum_address))
        .route("/accounts/xpub", post(add_xpub))
        .route("/accounts/manual", post(add_manual_account))
        .route(
            "/accounts/{account_id}",
            axum::routing::patch(rename_account).delete(delete_account),
        )
        .route("/accounts/{account_id}/move", post(move_account))
        .route("/accounts/{account_id}/addresses", get(account_addresses))
        .route(
            "/accounts/{account_id}/transactions",
            get(account_transactions),
        )
        .route(
            "/accounts/{account_id}/balance-assertions",
            post(add_balance_assertion),
        )
        .route(
            "/accounts/{account_id}/balance-assertions/{assertion_id}",
            put(update_balance_assertion).delete(delete_balance_assertion),
        )
        .route("/xpubs/inspect", post(inspect_xpub))
        .route("/assets/search", get(search_assets))
        .route("/sync", get(sync_status).post(start_sync))
        .route("/sync/{run_id}", get(sync_run_status))
        .route("/settings", get(get_settings))
        .route("/settings/{key}", put(put_setting))
        .route("/secrets/{name}", put(put_secret).delete(delete_secret))
        .route("/prices/history", get(price_history))
        .route(
            "/prices/overrides",
            get(list_price_overrides)
                .put(upsert_price_override)
                .delete(delete_price_override),
        )
}

// ============ Plumbing ============

/// Authorize, run `core` for the paired client's user, and encode the result.
/// The request lease is held until `core` finishes.
async fn respond<T, F, Fut>(headers: &HeaderMap, required: ClientPermission, core: F) -> Response
where
    T: Serialize,
    F: FnOnce(UserId) -> Fut,
    Fut: Future<Output = Result<T, ApiErrorEnvelope>>,
{
    let authorized = match authorize_client_request(headers, required) {
        Ok(authorized) => authorized,
        Err(error) => return api_error(error),
    };
    let result = if required == Full {
        crate::tasks::with_paired_job_authority(
            authorized.user_id,
            authorized.capability_id,
            core(authorized.user_id),
        )
        .await
    } else {
        core(authorized.user_id).await
    };
    drop(authorized);
    match result {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => api_error(error),
    }
}

fn json_body<T: DeserializeOwned>(body: &Bytes) -> Result<T, ApiErrorEnvelope> {
    serde_json::from_slice(body)
        .map_err(|error| ApiErrorEnvelope::bad_request(format!("Invalid JSON body: {error}")))
}

fn query<T>(query: Result<Query<T>, QueryRejection>) -> Result<T, ApiErrorEnvelope> {
    query
        .map(|Query(value)| value)
        .map_err(|error| ApiErrorEnvelope::bad_request(error.body_text()))
}

fn path_id<T: FromStr>(raw: &str, what: &str) -> Result<T, ApiErrorEnvelope> {
    raw.parse()
        .map_err(|_| ApiErrorEnvelope::not_found(format!("{what} not found")))
}

fn validation(field: &str, message: impl Into<String>) -> ApiErrorEnvelope {
    let mut errors = FieldErrors::new();
    errors.add(field, message.into());
    ApiErrorEnvelope::validation("Validation error", errors)
}

/// Parse inputs first; on failure answer before touching the database, but
/// only after authenticating so unauthenticated callers learn nothing.
macro_rules! parsed {
    ($headers:expr, $permission:expr, $parse:expr, |$user:ident, $input:ident| $core:expr) => {{
        match $parse {
            Ok($input) => respond(&$headers, $permission, |$user| $core).await,
            Err(error) => match authorize_client_request(&$headers, $permission) {
                Ok(_) => api_error(error),
                Err(auth_error) => api_error(auth_error),
            },
        }
    }};
}

// ============ Identity ============

#[derive(Serialize)]
struct MeResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    username: Option<String>,
    client: ClientIdentity,
    server_version: String,
}

#[derive(Serialize)]
struct ClientIdentity {
    name: String,
    permission: &'static str,
    created_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
}

async fn me(headers: HeaderMap) -> Response {
    let authorized = match authorize_client_request(&headers, ClientPermission::BalancesRead) {
        Ok(authorized) => authorized,
        Err(error) => return api_error(error),
    };
    let result = (|| {
        let username = if authorized.permission.satisfies(Read) {
            Some(
                crate::backend::auth::username_for_user(authorized.user_id)
                    .ok_or_else(ApiErrorEnvelope::internal)?,
            )
        } else {
            None
        };
        let record = crate::db::load_client_capability(authorized.capability_id)
            .map_err(|_| ApiErrorEnvelope::internal())?
            .ok_or_else(ApiErrorEnvelope::internal)?;
        let name = crate::db::load_paired_client_name(authorized.user_id, authorized.capability_id)
            .map_err(|_| ApiErrorEnvelope::internal())?
            .ok_or_else(ApiErrorEnvelope::internal)?;
        Ok::<_, ApiErrorEnvelope>(MeResponse {
            username,
            client: ClientIdentity {
                name,
                permission: authorized.permission.as_str(),
                created_at: record.created_at,
                expires_at: record.expires_at,
            },
            server_version: crate::version::version().to_string(),
        })
    })();
    drop(authorized);
    match result {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => api_error(error),
    }
}

#[derive(Serialize)]
struct RevokedResponse {
    status: &'static str,
}

/// Revoke the calling client's own Client Key (`bitgarth auth logout`).
async fn revoke_self(headers: HeaderMap) -> Response {
    let authorized = match authorize_client_request(&headers, ClientPermission::BalancesRead) {
        Ok(authorized) => authorized,
        Err(error) => return api_error(error),
    };
    let (user_id, capability_id) = (authorized.user_id, authorized.capability_id);
    // A competing shutdown waits for this lease. Release it before joining
    // the blocking lifecycle operation, which only needs the verified IDs.
    drop(authorized);
    let Ok(shutdown) =
        crate::auth::lifecycle::begin_capability_shutdown_async(user_id, capability_id).await
    else {
        return api_error(ApiErrorEnvelope::internal());
    };
    if crate::db::revoke_client_capability(user_id, capability_id, Utc::now()).is_err()
        || shutdown.drain().await.is_err()
    {
        return api_error(ApiErrorEnvelope::internal());
    }
    tracing::info!(user_id = %user_id, "public API: paired client revoked itself");
    (StatusCode::OK, Json(RevokedResponse { status: "revoked" })).into_response()
}

// ============ Encrypted wallet archives ============

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchivePassword {
    password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchiveUpload {
    password: String,
    payload_base64: String,
}

fn archive_error(error: crate::backend::ExportError) -> ApiErrorEnvelope {
    use crate::backend::ExportError;
    match error {
        ExportError::BadRequest(message) => ApiErrorEnvelope::bad_request(message),
        ExportError::Validation(message)
        | ExportError::PasswordRequired(message)
        | ExportError::EncryptedZipAuthFailed(message) => validation("archive", message),
        ExportError::Internal(_) => ApiErrorEnvelope::internal(),
        ExportError::Unauthorized(message) => ApiErrorEnvelope::unauthorized(message),
    }
}

async fn export_wallet_data(headers: HeaderMap, body: Bytes) -> Response {
    respond(&headers, Full, |user| async move {
        let request: ArchivePassword = json_body(&body)?;
        crate::backend::exports::export_paired_wallet_data(user, request.password)
            .map_err(archive_error)
    })
    .await
}

async fn import_wallet_data(headers: HeaderMap, body: Bytes) -> Response {
    respond(&headers, Full, |user| async move {
        let request: ArchiveUpload = json_body(&body)?;
        crate::backend::exports::import_paired_wallet_data(
            user,
            &request.payload_base64,
            request.password,
        )
        .await
        .map_err(archive_error)
    })
    .await
}

// ============ Wallets ============

async fn list_wallets(headers: HeaderMap) -> Response {
    respond(&headers, Read, wallets::get_wallets_for_user).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LabelBody {
    label: RawLabel,
}

async fn rename_wallet(headers: HeaderMap, Path(wallet_id): Path<String>, body: Bytes) -> Response {
    let parse = path_id::<WalletId>(&wallet_id, "Wallet")
        .and_then(|wallet_id| Ok((wallet_id, json_body::<LabelBody>(&body)?)));
    parsed!(headers, Full, parse, |user, input| {
        wallets::update_wallet_label_for_user(
            user,
            UpdateWalletLabelRequest {
                wallet_id: input.0,
                label: input.1.label,
            },
        )
    })
}

async fn delete_wallet(headers: HeaderMap, Path(wallet_id): Path<String>) -> Response {
    let parse = path_id::<WalletId>(&wallet_id, "Wallet");
    parsed!(headers, Full, parse, |user, wallet_id| {
        wallets::delete_wallet_for_user(
            user,
            DeleteWalletRequest {
                wallet_id,
                delete_accounts: crate::wallets::DeleteAccountsChoice::new(true),
            },
        )
    })
}

#[derive(Deserialize)]
struct ReportQuery {
    from: Option<ReportDateParam>,
    to: Option<ReportDateParam>,
    timezone: Option<UserTimezone>,
}

/// Reports use the requested timezone, else the user's saved one, else UTC.
fn report_timezone(
    user_id: UserId,
    requested: Option<UserTimezone>,
) -> Result<ReportTimezoneParam, ApiErrorEnvelope> {
    if let Some(timezone) = requested {
        return Ok(ReportTimezoneParam(timezone));
    }
    let saved = crate::db::load_settings(user_id)
        .map_err(|_| ApiErrorEnvelope::internal())?
        .timezone;
    Ok(ReportTimezoneParam(
        saved.unwrap_or(UserTimezone(chrono_tz::Tz::UTC)),
    ))
}

async fn wallet_report(
    headers: HeaderMap,
    Path(wallet_id): Path<String>,
    report: Result<Query<ReportQuery>, QueryRejection>,
) -> Response {
    let parse = path_id::<WalletId>(&wallet_id, "Wallet")
        .and_then(|wallet_id| Ok((wallet_id, query(report)?)));
    parsed!(headers, Read, parse, |user, input| async move {
        let (wallet_id, report) = input;
        let timezone = report_timezone(user, report.timezone)?;
        wallets::get_wallet_report_for_user(user, wallet_id, report.from, report.to, timezone).await
    })
}

async fn holdings_report(
    headers: HeaderMap,
    report: Result<Query<ReportQuery>, QueryRejection>,
) -> Response {
    let parse = query(report);
    parsed!(headers, Read, parse, |user, report| async move {
        let timezone = report_timezone(user, report.timezone)?;
        wallets::get_holdings_report_for_user(user, report.from, report.to, timezone).await
    })
}

// ============ Accounts ============

async fn add_bitcoin_address(headers: HeaderMap, body: Bytes) -> Response {
    let parse = json_body::<AddBtcAddressRequest>(&body);
    parsed!(headers, Full, parse, |user, request| {
        wallets::add_bitcoin_address_for_user(user, request)
    })
}

async fn add_ethereum_address(headers: HeaderMap, body: Bytes) -> Response {
    let parse = json_body::<AddEthAddressRequest>(&body);
    parsed!(headers, Full, parse, |user, request| {
        wallets::add_ethereum_address_for_user(user, request)
    })
}

async fn add_xpub(headers: HeaderMap, body: Bytes) -> Response {
    let parse = json_body::<AddXpubRequest>(&body);
    parsed!(headers, Full, parse, |user, request| {
        wallets::add_xpub_for_user(user, request)
    })
}

async fn add_manual_account(headers: HeaderMap, body: Bytes) -> Response {
    let parse = json_body::<AddManualAssetAccountRequest>(&body);
    parsed!(headers, Full, parse, |user, request| {
        wallets::add_manual_asset_account_for_user(user, request)
    })
}

async fn inspect_xpub(headers: HeaderMap, body: Bytes) -> Response {
    let parse = json_body::<ValidateXpubRequest>(&body);
    parsed!(headers, Read, parse, |user, request| {
        wallets::validate_xpub_for_user(user, request)
    })
}

async fn rename_account(
    headers: HeaderMap,
    Path(account_id): Path<String>,
    body: Bytes,
) -> Response {
    let parse = path_id::<WalletAccountId>(&account_id, "Account")
        .and_then(|account_id| Ok((account_id, json_body::<LabelBody>(&body)?)));
    parsed!(headers, Full, parse, |user, input| {
        wallets::update_account_label_for_user(
            user,
            UpdateAccountLabelRequest {
                account_id: input.0,
                label: input.1.label,
            },
        )
    })
}

async fn delete_account(headers: HeaderMap, Path(account_id): Path<String>) -> Response {
    let parse = path_id::<WalletAccountId>(&account_id, "Account");
    parsed!(headers, Full, parse, |user, account_id| {
        wallets::delete_account_for_user(user, DeleteAccountRequest { account_id })
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MoveBody {
    destination: MoveDestination,
}

async fn move_account(headers: HeaderMap, Path(account_id): Path<String>, body: Bytes) -> Response {
    let parse = path_id::<WalletAccountId>(&account_id, "Account")
        .and_then(|account_id| Ok((account_id, json_body::<MoveBody>(&body)?)));
    parsed!(headers, Full, parse, |user, input| {
        wallets::move_wallet_account_for_user(
            user,
            MoveAccountRequest {
                account_id: input.0,
                destination: input.1.destination,
            },
        )
    })
}

#[derive(Deserialize)]
struct AddressesQuery {
    scheme: AddressScheme,
    page: Option<u32>,
    page_size: Option<u32>,
}

async fn account_addresses(
    headers: HeaderMap,
    Path(account_id): Path<String>,
    addresses: Result<Query<AddressesQuery>, QueryRejection>,
) -> Response {
    let parse = path_id::<DigitalAssetAccountId>(&account_id, "Account")
        .and_then(|account_id| Ok((account_id, query(addresses)?)));
    parsed!(headers, Read, parse, |user, input| {
        wallets::get_account_addresses_for_user(
            user,
            GetAccountAddressesRequest {
                account_id: input.0,
                address_scheme: input.1.scheme,
                page: input.1.page,
                page_size: input.1.page_size,
            },
        )
    })
}

#[derive(Deserialize)]
struct TransactionsQuery {
    pending_page: Option<u32>,
    confirmed_page: Option<u32>,
    sort: Option<String>,
    filters: Option<String>,
}

async fn account_transactions(
    headers: HeaderMap,
    Path(account_id): Path<String>,
    history: Result<Query<TransactionsQuery>, QueryRejection>,
) -> Response {
    let parse = path_id::<WalletAccountId>(&account_id, "Account")
        .and_then(|account_id| Ok((account_id, query(history)?)));
    parsed!(headers, Read, parse, |user, input| {
        let (account_id, history) = input;
        transactions::get_account_transactions_for_user(
            user,
            account_id,
            history.pending_page,
            history.confirmed_page,
            history.sort,
            history.filters,
        )
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BalanceAssertionBody {
    asserted_on: ReportDateParam,
    balance: crate::wallets::RawManualAssetBalance,
    #[serde(default)]
    note: Option<crate::wallets::RawManualAssetAssertionNote>,
}

async fn add_balance_assertion(
    headers: HeaderMap,
    Path(account_id): Path<String>,
    body: Bytes,
) -> Response {
    let parse = path_id::<WalletAccountId>(&account_id, "Account")
        .and_then(|account_id| Ok((account_id, json_body::<BalanceAssertionBody>(&body)?)));
    parsed!(headers, Full, parse, |user, input| {
        let (account_id, body) = input;
        transactions::add_manual_asset_balance_assertion_for_user(
            user,
            AddManualAssetBalanceAssertionRequest {
                account_id,
                asserted_on: body.asserted_on,
                balance: body.balance,
                note: body.note,
            },
        )
    })
}

async fn update_balance_assertion(
    headers: HeaderMap,
    Path((account_id, assertion_id)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let parse = path_id::<WalletAccountId>(&account_id, "Account").and_then(|account_id| {
        Ok((
            account_id,
            path_id::<ManualAssetBalanceAssertionId>(&assertion_id, "Balance assertion")?,
            json_body::<BalanceAssertionBody>(&body)?,
        ))
    });
    parsed!(headers, Full, parse, |user, input| {
        let (account_id, assertion_id, body) = input;
        transactions::update_manual_asset_balance_assertion_for_user(
            user,
            UpdateManualAssetBalanceAssertionRequest {
                assertion_id,
                account_id,
                asserted_on: body.asserted_on,
                balance: body.balance,
                note: body.note,
            },
        )
    })
}

async fn delete_balance_assertion(
    headers: HeaderMap,
    Path((account_id, assertion_id)): Path<(String, String)>,
) -> Response {
    let parse = path_id::<WalletAccountId>(&account_id, "Account").and_then(|account_id| {
        Ok((
            account_id,
            path_id::<ManualAssetBalanceAssertionId>(&assertion_id, "Balance assertion")?,
        ))
    });
    parsed!(headers, Full, parse, |user, ids| {
        transactions::delete_manual_asset_balance_assertion_for_user(
            user,
            Some(ids.0),
            DeleteManualAssetBalanceAssertionRequest {
                assertion_id: ids.1,
            },
        )
    })
}

#[derive(Deserialize)]
struct AssetSearchQuery {
    query: String,
}

async fn search_assets(
    headers: HeaderMap,
    search: Result<Query<AssetSearchQuery>, QueryRejection>,
) -> Response {
    let parse = query(search);
    parsed!(headers, Read, parse, |user, search| {
        wallets::search_manual_asset_instances_for_user(
            user,
            SearchManualAssetInstancesRequest {
                query: search.query,
                allow_coingecko_catalog_refresh: false,
            },
        )
    })
}

// ============ Sync ============

#[derive(Serialize)]
struct SyncStatusResponse {
    overall: crate::transactions::AggregateSyncSnapshot,
    accounts: Vec<crate::transactions::AccountSyncSnapshot>,
}

async fn sync_run_status(headers: HeaderMap, Path(run_id): Path<String>) -> Response {
    let parse = path_id::<crate::transactions::TransactionSyncRunId>(&run_id, "Sync run");
    parsed!(headers, Read, parse, |user, run_id| async move {
        crate::tasks::client_sync_status(user, run_id)
            .await
            .map_err(|_| ApiErrorEnvelope::internal())?
            .ok_or_else(|| ApiErrorEnvelope::not_found("Sync run not found"))
    })
}

async fn sync_status(headers: HeaderMap) -> Response {
    respond(&headers, Read, |user| async move {
        Ok(SyncStatusResponse {
            overall: transactions::get_sync_state_for_user(user).await?,
            accounts: transactions::get_account_sync_snapshots_for_user(user).await?,
        })
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartSyncBody {
    #[serde(default)]
    scope: crate::transactions::RawTransactionSyncScope,
}

async fn start_sync(headers: HeaderMap, body: Bytes) -> Response {
    let parse = if body.is_empty() {
        Ok(StartSyncBody {
            scope: crate::transactions::RawTransactionSyncScope::default(),
        })
    } else {
        json_body::<StartSyncBody>(&body)
    };
    parsed!(headers, Full, parse, |user, body| {
        transactions::trigger_sync_for_user(
            user,
            crate::transactions::RawTransactionSyncTriggerRequest {
                source: crate::transactions::RawTransactionSyncTriggerSource::manual(),
                scope: body.scope,
            },
        )
    })
}

// ============ Settings and secrets ============

/// Settings as paired clients see them. Secret values are never returned;
/// `secrets` reports only whether each one is configured.
#[derive(Serialize)]
struct ClientSettingsView {
    currency: Option<CurrencyCode>,
    timezone: Option<UserTimezone>,
    number_format: Option<&'static str>,
    date_format: Option<&'static str>,
    price_fetching: bool,
    mempool_url: Option<String>,
    effective_mempool_url: Option<String>,
    etherscan_url: Option<String>,
    effective_etherscan_url: Option<String>,
    hledger_prefix: Option<String>,
    default_hledger_prefix: String,
    secrets: SecretsView,
}

#[derive(Serialize)]
struct SecretsView {
    etherscan_api_key: bool,
    coingecko_api_key: bool,
}

fn public_provider_url(raw: &str) -> Option<String> {
    let mut url = url::Url::parse(raw).ok()?;
    url.set_password(None).ok()?;
    url.set_username("").ok()?;
    Some(url.to_string())
}

async fn load_client_settings(user_id: UserId) -> Result<ClientSettingsView, ApiErrorEnvelope> {
    let stored = settings::get_settings_for_user(user_id).await?;
    let default_hledger_prefix = settings::default_hledger_account_prefix(user_id)?;
    let effective_mempool_url =
        crate::models::resolve_effective_mempool_base_url(stored.mempool_base_url.as_ref())
            .ok()
            .and_then(|(url, _)| public_provider_url(url.as_str()));
    let effective_etherscan_url =
        crate::models::resolve_effective_etherscan_base_url(stored.etherscan_base_url.as_ref())
            .ok()
            .and_then(|(url, _)| public_provider_url(url.as_str()));
    Ok(ClientSettingsView {
        currency: stored.currency,
        timezone: stored.timezone,
        number_format: stored.number_format.map(|format| format.code()),
        date_format: stored.date_time_format.map(|format| format.code()),
        price_fetching: stored.price_fetching_enabled,
        mempool_url: stored
            .mempool_base_url
            .and_then(|url| public_provider_url(url.as_str())),
        effective_mempool_url,
        etherscan_url: stored
            .etherscan_base_url
            .and_then(|url| public_provider_url(url.as_str())),
        effective_etherscan_url,
        hledger_prefix: stored
            .hledger_account_prefix
            .map(|prefix| prefix.as_str().to_owned()),
        default_hledger_prefix,
        secrets: SecretsView {
            etherscan_api_key: stored.has_etherscan_api_key,
            coingecko_api_key: stored.has_coingecko_api_key,
        },
    })
}

async fn get_settings(headers: HeaderMap) -> Response {
    respond(&headers, Read, load_client_settings).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ValueBody {
    value: serde_json::Value,
}

/// A validated settings change. `None` payloads restore the default.
#[derive(Debug)]
enum SettingChange {
    Currency(CurrencyCode),
    Timezone(UserTimezone),
    NumberFormat(NumberFormat),
    DateFormat(DateTimeFormat),
    PriceFetching(bool),
    MempoolUrl(Option<RawMempoolBaseUrl>),
    EtherscanUrl(Option<RawEtherscanBaseUrl>),
    HledgerPrefix(Option<String>),
}

const SETTING_KEYS: &str = "currency, timezone, number-format, date-format, price-fetching, mempool-url, etherscan-url, hledger-prefix";

fn parse_setting_change(
    key: &str,
    value: serde_json::Value,
) -> Result<SettingChange, ApiErrorEnvelope> {
    use serde_json::Value;
    let text = |value: &Value| value.as_str().map(str::trim).map(str::to_owned);
    let required_text = |value: &Value| {
        text(value).filter(|text| !text.is_empty()).ok_or_else(|| {
            validation(
                "value",
                format!("{key} needs a text value and cannot be cleared"),
            )
        })
    };
    let optional_text = |value: &Value| match value {
        Value::Null => Ok(None),
        Value::String(text) if text.trim().is_empty() => Ok(None),
        Value::String(text) => Ok(Some(text.trim().to_owned())),
        _ => Err(validation(
            "value",
            format!("{key} needs a text value or null"),
        )),
    };
    match key {
        "currency" => {
            let code = required_text(&value)?.to_ascii_uppercase();
            CurrencyCode::from_code(&code)
                .map(SettingChange::Currency)
                .ok_or_else(|| {
                    validation("value", format!("{code} is not an ISO 4217 currency code"))
                })
        }
        "timezone" => {
            let name = required_text(&value)?;
            name.parse::<chrono_tz::Tz>()
                .map(|tz| SettingChange::Timezone(UserTimezone(tz)))
                .map_err(|_| {
                    validation(
                        "value",
                        format!("{name} is not an IANA timezone such as Europe/Amsterdam"),
                    )
                })
        }
        "number-format" => {
            let code = required_text(&value)?;
            NumberFormat::from_code(&code)
                .map(SettingChange::NumberFormat)
                .ok_or_else(|| {
                    validation(
                        "value",
                        "use dot_comma (1,234.56), comma_dot (1.234,56) or comma_space (1 234,56)",
                    )
                })
        }
        "date-format" => {
            let code = required_text(&value)?;
            DateTimeFormat::from_code(&code)
                .map(SettingChange::DateFormat)
                .ok_or_else(|| validation("value", "use ymd_24, dmy_24 or mdy_12"))
        }
        "price-fetching" => value
            .as_bool()
            .map(SettingChange::PriceFetching)
            .ok_or_else(|| validation("value", "price-fetching needs true or false")),
        "mempool-url" => optional_text(&value)
            .map(|url| SettingChange::MempoolUrl(url.map(RawMempoolBaseUrl::new))),
        "etherscan-url" => optional_text(&value)
            .map(|url| SettingChange::EtherscanUrl(url.map(RawEtherscanBaseUrl::new))),
        "hledger-prefix" => optional_text(&value).map(SettingChange::HledgerPrefix),
        _ => Err(ApiErrorEnvelope::not_found(format!(
            "Unknown setting {key}. Known settings: {SETTING_KEYS}"
        ))),
    }
}

async fn apply_setting_change(
    user_id: UserId,
    change: SettingChange,
) -> Result<(), ApiErrorEnvelope> {
    match change {
        SettingChange::Currency(value) => settings::save_currency_for_user(user_id, value).await,
        SettingChange::Timezone(value) => settings::save_timezone_for_user(user_id, value).await,
        SettingChange::NumberFormat(value) => {
            settings::save_number_format_for_user(user_id, value).await
        }
        SettingChange::DateFormat(value) => {
            settings::save_date_time_format_for_user(user_id, value).await
        }
        SettingChange::PriceFetching(value) => {
            settings::set_price_fetching_enabled_for_user(user_id, value)
                .await
                .map(|_| ())
        }
        SettingChange::MempoolUrl(value) => {
            settings::save_mempool_base_url_for_user(user_id, value)
                .await
                .map(|_| ())
        }
        SettingChange::EtherscanUrl(value) => {
            let origin = |configured: Option<&RawEtherscanBaseUrl>| {
                crate::models::resolve_effective_etherscan_base_url(configured)
                    .ok()
                    .map(|(url, _)| url.origin())
            };
            let requested_origin = origin(value.as_ref());
            settings::save_etherscan_base_url_for_user(user_id, value, move |current| {
                requested_origin.is_some() && requested_origin == origin(current.as_ref())
            })
            .await
            .map(|_| ())
        }
        SettingChange::HledgerPrefix(value) => {
            settings::save_hledger_account_prefix_for_user(user_id, value)
                .await
                .map(|_| ())
        }
    }
}

async fn put_setting(headers: HeaderMap, Path(key): Path<String>, body: Bytes) -> Response {
    let parse =
        json_body::<ValueBody>(&body).and_then(|body| parse_setting_change(&key, body.value));
    parsed!(headers, Full, parse, |user, change| async move {
        apply_setting_change(user, change).await?;
        load_client_settings(user).await
    })
}

#[derive(Debug)]
enum SecretName {
    EtherscanApiKey,
    CoinGeckoApiKey,
}

fn parse_secret_name(name: &str) -> Result<SecretName, ApiErrorEnvelope> {
    match name {
        "etherscan-api-key" => Ok(SecretName::EtherscanApiKey),
        "coingecko-api-key" => Ok(SecretName::CoinGeckoApiKey),
        _ => Err(ApiErrorEnvelope::not_found(format!(
            "Unknown secret {name}. Known secrets: etherscan-api-key, coingecko-api-key"
        ))),
    }
}

async fn save_secret(
    user_id: UserId,
    name: SecretName,
    value: Option<String>,
) -> Result<SecretsView, ApiErrorEnvelope> {
    match name {
        SecretName::EtherscanApiKey => {
            settings::save_etherscan_api_key_for_user(user_id, value.map(RawEtherscanApiKey::new))
                .await?;
        }
        SecretName::CoinGeckoApiKey => {
            settings::save_coingecko_api_key_for_user(user_id, value.and_then(SimpleApiKey::new))
                .await?;
        }
    }
    Ok(load_client_settings(user_id).await?.secrets)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretBody {
    value: String,
}

async fn put_secret(headers: HeaderMap, Path(name): Path<String>, body: Bytes) -> Response {
    let parse = parse_secret_name(&name).and_then(|name| {
        let body = json_body::<SecretBody>(&body)?;
        let value = body.value.trim().to_owned();
        if value.is_empty() {
            return Err(validation(
                "value",
                "The secret must not be empty; delete it instead",
            ));
        }
        Ok((name, value))
    });
    parsed!(headers, Full, parse, |user, input| save_secret(
        user,
        input.0,
        Some(input.1)
    ))
}

async fn delete_secret(headers: HeaderMap, Path(name): Path<String>) -> Response {
    let parse = parse_secret_name(&name);
    parsed!(headers, Full, parse, |user, name| save_secret(
        user, name, None
    ))
}

// ============ Prices ============

async fn price_history(
    headers: HeaderMap,
    history: Result<Query<prices::PriceHistoryQuery>, QueryRejection>,
) -> Response {
    parsed!(headers, Read, query(history), |user, history| {
        prices::get_price_history_for_user(user, history, prices::PriceSelection::Exact)
    })
}

#[derive(Deserialize)]
struct OverridesQuery {
    asset: String,
    currency: CurrencyCode,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
}

async fn list_price_overrides(
    headers: HeaderMap,
    overrides: Result<Query<OverridesQuery>, QueryRejection>,
) -> Response {
    let parse = query(overrides);
    parsed!(headers, Read, parse, |user, overrides| {
        prices::list_price_overrides_for_user(
            user,
            "catalog_asset".to_owned(),
            overrides.asset,
            overrides.currency,
            overrides.from,
            overrides.to,
        )
    })
}

async fn upsert_price_override(headers: HeaderMap, body: Bytes) -> Response {
    let parse = json_body::<prices::UpsertPriceOverrideInput>(&body);
    parsed!(headers, Full, parse, |user, input| {
        prices::upsert_price_override_for_user(user, input)
    })
}

async fn delete_price_override(headers: HeaderMap, body: Bytes) -> Response {
    let parse = json_body::<prices::DeletePriceOverrideInput>(&body);
    parsed!(headers, Full, parse, |user, input| {
        prices::delete_price_override_for_user(user, input)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn setting_changes_parse_and_reject_with_guidance() {
        assert!(matches!(
            parse_setting_change("currency", json!("eur")),
            Ok(SettingChange::Currency(code)) if code.code() == "EUR"
        ));
        assert!(matches!(
            parse_setting_change("price-fetching", json!(true)),
            Ok(SettingChange::PriceFetching(true))
        ));
        assert!(matches!(
            parse_setting_change("mempool-url", json!(null)),
            Ok(SettingChange::MempoolUrl(None))
        ));
        assert!(matches!(
            parse_setting_change("hledger-prefix", json!("  ")),
            Ok(SettingChange::HledgerPrefix(None))
        ));
        assert!(
            parse_setting_change("currency", json!(null))
                .unwrap_err()
                .is_validation()
        );
        assert!(
            parse_setting_change("timezone", json!("Mars/Base"))
                .unwrap_err()
                .is_validation()
        );
        assert!(
            parse_setting_change("price-fetching", json!("yes"))
                .unwrap_err()
                .is_validation()
        );
        assert!(
            parse_setting_change("colour", json!("red"))
                .unwrap_err()
                .is_not_found()
        );
    }

    #[test]
    fn secret_names_are_an_explicit_allow_list() {
        assert!(matches!(
            parse_secret_name("etherscan-api-key"),
            Ok(SecretName::EtherscanApiKey)
        ));
        assert!(matches!(
            parse_secret_name("coingecko-api-key"),
            Ok(SecretName::CoinGeckoApiKey)
        ));
        assert!(
            parse_secret_name("session-token")
                .unwrap_err()
                .is_not_found()
        );
    }
}
