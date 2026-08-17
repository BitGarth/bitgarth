#[cfg(feature = "server")]
use std::collections::HashMap;

#[cfg(feature = "server")]
use crate::balance_reliability::BalanceReliability;
#[cfg(feature = "server")]
use crate::db::{
    WalletReportLoadError, active_sync_slot_account_ids,
    get_wallet_by_fingerprint as get_wallet_by_fingerprint_db, list_wallets,
    load_account_addresses_page as load_account_addresses_page_db, load_account_sync_slot_map,
    load_account_sync_slots, load_account_transaction_counts as load_account_transaction_counts_db,
    load_account_transaction_history as load_account_transaction_history_db,
    load_all_account_balances as load_all_account_balances_db,
    load_holdings_report as load_holdings_report_db,
    load_holdings_report_range_plan as load_holdings_report_range_plan_db,
    load_manual_asset_current_balances as load_manual_asset_current_balances_db,
    load_manual_asset_wallet_report_rows as load_manual_asset_wallet_report_rows_db,
    load_settings as db_load_settings, load_wallet_report as load_wallet_report_db,
    load_wallet_report_range_plan as load_wallet_report_range_plan_db, load_wallet_summary_bundle,
};
#[cfg(feature = "server")]
use crate::transactions::AddressBalanceSummary;
#[cfg(feature = "server")]
use crate::wallets::AccountKind;
use crate::wallets::{
    GetAccountAddressesRequest, GetAccountAddressesResponse, GetWalletByFingerprintRequest,
    ReportDateParam, ReportTimezoneParam,
};
#[cfg(feature = "server")]
use axum_extra::extract::cookie::CookieJar;
#[cfg(feature = "server")]
use chrono::Utc;
#[cfg(feature = "server")]
use dioxus::logger::tracing;
use dioxus::prelude::*;

#[cfg(feature = "server")]
use super::balance_projection::{
    free_balance_unavailable_account_ids, load_wallet_balance_projection_from_summary,
};
#[cfg(feature = "server")]
use super::conversions::{
    NativeAccountManualSyncContext, WalletAccountData, account_limit_view, balance_amount_view,
    convert_wallet_to_view, custom_wallet_report_balance_state_view,
    custom_wallet_report_balance_value, sort_report_account_rows, synced_account_capacity_view,
    wallet_balance_view, wallet_report_balance_state_view,
};
#[cfg(feature = "server")]
use super::helpers::{
    internal_error, not_found_error, session_token_from_cookie, single_field_validation_error,
    unauthorized_error, validation_error,
};
#[cfg(feature = "server")]
use super::types::WalletReportAccountRow;
#[cfg(feature = "server")]
use super::types::{FiatAmountView, HoldingsReportWalletRow};
use super::types::{
    HoldingsReportResponse, WalletError, WalletReportResponse, WalletView, WalletsResponse,
};
#[cfg(feature = "server")]
use crate::account_limits::AccountActivationState;
#[cfg(feature = "server")]
use crate::backend::prices::HoldingsReportPriceRow;
#[cfg(feature = "server")]
use crate::backend::session_context::require_initialized_session;
#[cfg(feature = "server")]
use crate::services::price_overrides::{BoundaryKind, PriceSubject, price_subject_sort_key};

#[cfg(feature = "server")]
fn report_access_entitlements(
    entitlements: &crate::payments::types::FeatureEntitlements,
) -> crate::report_access::ReportAccessEntitlements {
    crate::report_access::ReportAccessEntitlements {
        tax_reports: entitlements.tax_reports,
        exchange_rates_history: entitlements.exchange_rates_history,
        price_overrides: entitlements.price_overrides,
    }
}

#[cfg(feature = "server")]
fn fiat_amount_view(value: rust_decimal::Decimal) -> FiatAmountView {
    FiatAmountView {
        raw_value: value.to_string(),
        formatted_value: value.normalize().to_string(),
    }
}

#[cfg(feature = "server")]
fn holdings_change_percent(
    opening: rust_decimal::Decimal,
    closing: rust_decimal::Decimal,
) -> Option<String> {
    if opening == rust_decimal::Decimal::ZERO {
        None
    } else {
        Some(((closing - opening) / opening * rust_decimal::Decimal::from(100)).to_string())
    }
}

#[cfg(feature = "server")]
const _: fn(rust_decimal::Decimal) -> FiatAmountView = fiat_amount_view;
#[cfg(feature = "server")]
const _: fn(rust_decimal::Decimal, rust_decimal::Decimal) -> Option<String> =
    holdings_change_percent;

#[cfg(feature = "server")]
fn sum_required_values(
    values: Vec<Option<rust_decimal::Decimal>>,
) -> Option<rust_decimal::Decimal> {
    values
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .map(|values| values.into_iter().sum())
}

#[cfg(feature = "server")]
pub(crate) fn manual_asset_search_response_for_query(
    query: &str,
) -> Result<crate::wallets::SearchManualAssetInstancesResponse, WalletError> {
    let results = crate::services::manual_asset_discovery::search_manual_asset_candidates(query)
        .map_err(|err| internal_error("manual_asset_search_catalog", err))?
        .into_iter()
        .map(|row| match row {
            crate::asset_capabilities::ManualAssetSearchResult::BitGarthCatalog {
                asset_id,
                network_id,
                unit_code,
                asset_name,
                network_name,
                decimal_precision,
                coingecko_id,
            } => crate::wallets::ManualAssetInstanceSearchRow {
                source: crate::wallets::ManualAssetSearchSource::BitGarthCatalog,
                asset_instance_id: Some(crate::asset_views::ManualAssetInstanceIdView {
                    asset_id,
                    network_id,
                }),
                coingecko_id: Some(coingecko_id),
                unit_code,
                asset_name,
                network_name,
                decimal_precision: Some(decimal_precision),
                platform_count: None,
                platform_hint: None,
            },
            crate::asset_capabilities::ManualAssetSearchResult::CoinGeckoCatalog {
                coingecko_id,
                symbol,
                name,
                platforms_json,
            } => {
                let summary = coingecko_platform_summary(platforms_json.as_deref());
                let platform_count = summary.as_ref().map(|summary| summary.count);
                let sole_platform_name = summary.and_then(|summary| summary.sole_platform_name);
                crate::wallets::ManualAssetInstanceSearchRow {
                    source: crate::wallets::ManualAssetSearchSource::CoinGeckoCatalog,
                    asset_instance_id: None,
                    coingecko_id: Some(coingecko_id),
                    unit_code: symbol.to_ascii_uppercase(),
                    asset_name: name,
                    decimal_precision: None,
                    network_name: match (&sole_platform_name, platform_count) {
                        (Some(platform), _) => platform.clone(),
                        (None, Some(count)) => format!("{count} CoinGecko platforms"),
                        (None, None) => "CoinGecko".to_string(),
                    },
                    platform_count,
                    platform_hint: match (sole_platform_name, platform_count) {
                        (Some(platform), _) => Some(platform),
                        (None, Some(count)) => Some(format!("{count} platforms")),
                        (None, None) => None,
                    },
                }
            }
        })
        .collect();
    let total_match_count = crate::services::manual_asset_discovery::match_total(query)
        .map_err(|err| internal_error("manual_asset_match_total", err))?;
    Ok(crate::wallets::SearchManualAssetInstancesResponse {
        results,
        total_match_count,
    })
}

#[cfg(feature = "server")]
struct CoingeckoPlatformSummary {
    count: usize,
    /// Set only when the asset has a single platform: its display name, or
    /// "Native" for entries with an empty platform key.
    sole_platform_name: Option<String>,
}

#[cfg(feature = "server")]
fn coingecko_platform_summary(platforms_json: Option<&str>) -> Option<CoingeckoPlatformSummary> {
    let raw = platforms_json?;
    let parsed: serde_json::Value = serde_json::from_str(raw).ok()?;
    let object = parsed.as_object()?;
    let count = object.len();
    let sole_platform_name = (count == 1).then(|| {
        object.keys().next().map_or_else(
            || "Native".to_string(),
            |key| {
                let trimmed = key.trim();
                if trimmed.is_empty() {
                    "Native".to_string()
                } else {
                    trimmed.to_string()
                }
            },
        )
    });
    Some(CoingeckoPlatformSummary {
        count,
        sole_platform_name,
    })
}

#[post("/_app/user/wallets/manual-assets/search", cookies: CookieJar)]
pub(crate) async fn search_manual_asset_instances(
    request: crate::wallets::SearchManualAssetInstancesRequest,
) -> Result<crate::wallets::SearchManualAssetInstancesResponse, WalletError> {
    tracing::debug!("wallets: manual asset search requested");
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("wallets", &session_token, unauthorized_error, |message| {
            internal_error("require_initialized_session", message)
        })?;
    let user_id = initialized_session.session.user_id;
    crate::services::manual_asset_discovery::refresh_coingecko_catalog_for_manual_asset_search(
        user_id,
        request.allow_coingecko_catalog_refresh,
    )
    .await;
    manual_asset_search_response_for_query(&request.query)
}

#[get("/_app/user/wallets/manual-assets/catalog-total", cookies: CookieJar)]
pub(crate) async fn manual_asset_catalog_total()
-> Result<crate::wallets::ManualAssetCatalogTotalResponse, WalletError> {
    tracing::debug!("wallets: manual asset catalog total requested");
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("wallets", &session_token, unauthorized_error, |message| {
            internal_error("require_initialized_session", message)
        })?;
    let _user_id = initialized_session.session.user_id;
    let (total, coingecko_catalog_empty) = crate::services::manual_asset_discovery::catalog_total()
        .map_err(|err| internal_error("manual_asset_catalog_total", err))?;
    Ok(crate::wallets::ManualAssetCatalogTotalResponse {
        total,
        coingecko_catalog_empty,
    })
}

#[post(
    "/_app/user/wallets/manual-assets/coingecko-detail",
    cookies: CookieJar
)]
pub(crate) async fn manual_asset_discovery_detail(
    request: crate::wallets::ManualAssetDiscoveryDetailRequest,
) -> Result<crate::wallets::ManualAssetDiscoveryDetailResponse, WalletError> {
    tracing::debug!("wallets: manual asset discovery detail requested");
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("wallets", &session_token, unauthorized_error, |message| {
            internal_error("require_initialized_session", message)
        })?;
    let user_id = initialized_session.session.user_id;

    crate::services::manual_asset_discovery::load_manual_asset_discovery_detail(user_id, request)
        .await
        .map_err(detail_error_to_wallet_error)
}

/// Map a manual-asset discovery detail error to the client-facing wallet error.
/// A CoinGecko rate-limit (429) is transient and retryable, so it surfaces a
/// clear "too many requests" message rather than a generic internal error —
/// the message round-trips to the UI (see `ApiErrorEnvelope::too_many_requests`).
#[cfg(feature = "server")]
fn detail_error_to_wallet_error(
    err: crate::services::manual_asset_discovery::ManualAssetDiscoveryDetailError,
) -> WalletError {
    use crate::services::manual_asset_discovery::ManualAssetDiscoveryDetailError as DetailError;
    match err {
        DetailError::InvalidCoingeckoId(message) => {
            single_field_validation_error("coingecko_id", message)
        }
        DetailError::RemoteLookupNotAllowed => single_field_validation_error(
            "allow_remote_lookup",
            "Enable price fetching or allow this one-time CoinGecko lookup.",
        ),
        DetailError::Database(err) => internal_error("manual_asset_discovery_detail", err),
        DetailError::RateLimited { .. } => WalletError::too_many_requests(
            "CoinGecko is rate-limiting requests. Wait a moment and try again.",
        ),
        DetailError::Provider(message) => internal_error("manual_asset_discovery_detail", message),
    }
}

#[post(
    "/_app/user/wallets/manual-assets/coingecko-price",
    cookies: CookieJar
)]
pub(crate) async fn manual_asset_discovery_price(
    request: crate::wallets::ManualAssetDiscoveryPriceRequest,
) -> Result<crate::wallets::ManualAssetDiscoveryPriceResponse, WalletError> {
    tracing::debug!("wallets: manual asset discovery price requested");
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("wallets", &session_token, unauthorized_error, |message| {
            internal_error("require_initialized_session", message)
        })?;
    let user_id = initialized_session.session.user_id;

    crate::asset_capabilities::AssetId::owned(request.asset_id.clone())
        .map_err(|err| single_field_validation_error("asset_id", err.to_string()))?;
    crate::asset_capabilities::unsynced::CoingeckoAssetId::parse(&request.coingecko_id)
        .map_err(|err| single_field_validation_error("coingecko_id", err.to_string()))?;

    let price = crate::services::current_prices::selected_manual_asset_current_price(
        user_id,
        request.asset_id,
        request.coingecko_id,
        request.quote_currency,
        request.allow_remote_lookup,
    )
    .await
    .map(|price| price.to_string());

    Ok(crate::wallets::ManualAssetDiscoveryPriceResponse {
        price,
        quote_currency: request.quote_currency,
    })
}

#[get("/_app/user/wallets", cookies: CookieJar)]
pub(crate) async fn get_wallets() -> Result<WalletsResponse, WalletError> {
    tracing::debug!("wallets: list requested");
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("wallets", &session_token, unauthorized_error, |message| {
            internal_error("require_initialized_session", message)
        })?;
    let user_id = initialized_session.session.user_id;
    tracing::debug!(user_id = %user_id, "wallets: list authorized");

    let summary_bundle = load_wallet_summary_bundle(user_id)
        .map_err(|e: crate::db::DbError| internal_error("wallets", e))?;
    let balance_projection =
        load_wallet_balance_projection_from_summary(user_id, summary_bundle.clone())?;
    let custom_account_balances = load_manual_asset_current_balances_db(user_id)
        .map_err(|e: crate::db::DbError| internal_error("wallets", e))?;
    let entitlements =
        crate::payments::entitlements::load_feature_entitlements(user_id, Utc::now())
            .map_err(|e| internal_error("wallets", e))?;
    let classified_accounts = crate::db::account_limits::classify_supported_accounts_for_user(
        user_id,
        usize::from(entitlements.sync_account_slots_limit),
    )
    .map_err(|e| internal_error("wallets", e))?;
    let sync_slot_records =
        load_account_sync_slots(user_id).map_err(|e| internal_error("wallets", e))?;
    let (
        wallets,
        manual_asset_accounts,
        address_balances,
        account_balances,
        account_balance_reliabilities,
        account_tx_counts,
    ) = (
        summary_bundle.wallets,
        summary_bundle.manual_asset_accounts,
        summary_bundle.address_balances,
        summary_bundle.account_balances,
        summary_bundle.account_balance_reliabilities,
        summary_bundle.account_tx_counts,
    );
    let free_balance_unavailable_account_ids =
        free_balance_unavailable_account_ids(&wallets, &entitlements.tier);
    let active_sync_slot_records = sync_slot_records
        .iter()
        .filter(|record| !free_balance_unavailable_account_ids.contains(&record.account_id))
        .cloned()
        .collect::<Vec<_>>();
    let active_sync_slots = active_sync_slot_account_ids(
        &active_sync_slot_records,
        entitlements.sync_account_slots_limit,
    );
    let sync_slot_map = sync_slot_records
        .into_iter()
        .map(|record| (record.account_id, record))
        .collect::<HashMap<_, _>>();

    let wallet_views: Result<Vec<WalletView>, WalletError> = wallets
        .into_iter()
        .map(|wallet| {
            let projected_balances = balance_projection
                .wallets
                .iter()
                .find(|projected| projected.id == wallet.wallet.id)
                .map(|projected| projected.balances.as_slice())
                .ok_or_else(|| {
                    internal_error(
                        "wallet_balance_projection",
                        "wallet missing from balance projection",
                    )
                })?;
            convert_wallet_to_view(
                wallet,
                projected_balances,
                &WalletAccountData {
                    manual_asset_accounts: &manual_asset_accounts,
                    address_balances: &address_balances,
                    account_balances: &account_balances,
                    account_balance_reliabilities: &account_balance_reliabilities,
                    custom_account_balances: &custom_account_balances,
                    account_transactions: None,
                    account_tx_counts: &account_tx_counts,
                },
                &NativeAccountManualSyncContext {
                    sync_slots: &sync_slot_map,
                    active_sync_slot_account_ids: &active_sync_slots,
                    slot_limit: entitlements.sync_account_slots_limit,
                    tier: entitlements.tier.clone(),
                    historical_backfill_enabled: entitlements.historical_backfill_enabled,
                    historical_backfill_transactions_per_account: entitlements
                        .historical_backfill_transactions_per_account,
                    free_balance_unavailable_account_ids: &free_balance_unavailable_account_ids,
                },
                &classified_accounts,
            )
        })
        .collect();

    let mut wallet_views = wallet_views?;
    let value_summary = if crate::db::get_price_fetching_enabled(user_id)
        .map_err(|e| internal_error("wallets", e))?
    {
        let currency = db_load_settings(user_id)
            .map_err(|e| internal_error("wallet_settings", e))?
            .currency
            .unwrap_or_else(|| crate::settings::default_currency(crate::i18n::Locale::default()));
        Some(
            crate::services::current_prices::apply_wallet_valuations(
                user_id,
                &mut wallet_views,
                &manual_asset_accounts,
                currency,
            )
            .await,
        )
    } else {
        None
    };

    let used_slots = u16::try_from(active_sync_slots.len()).unwrap_or(u16::MAX);
    let active_account_count = classified_accounts
        .iter()
        .filter(|account| account.state == AccountActivationState::Active)
        .count();
    let inactive_account_count = classified_accounts
        .iter()
        .filter(|account| account.state == AccountActivationState::Inactive)
        .count();
    Ok(WalletsResponse {
        wallets: wallet_views,
        value_summary,
        account_limit: account_limit_view(
            active_account_count,
            inactive_account_count,
            entitlements.sync_account_slots_limit,
        ),
        sync_capacity: synced_account_capacity_view(
            used_slots,
            entitlements.sync_account_slots_limit,
            entitlements.tier,
        ),
    })
}

#[get("/_app/user/wallets/:wallet_id/report?from&to&timezone", cookies: CookieJar)]
pub(crate) async fn get_wallet_report(
    wallet_id: crate::wallets::WalletId,
    from: Option<ReportDateParam>,
    to: Option<ReportDateParam>,
    timezone: ReportTimezoneParam,
) -> Result<WalletReportResponse, WalletError> {
    tracing::debug!(wallet_id = %wallet_id, "wallets: wallet report requested");
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("wallets", &session_token, unauthorized_error, |message| {
            internal_error("require_initialized_session", message)
        })?;
    let user_id = initialized_session.session.user_id;

    let user_timezone = timezone.into_user_timezone();
    let requested_from = from.map(ReportDateParam::into_naive_date);
    let requested_to = to.map(ReportDateParam::into_naive_date);
    let now = Utc::now();
    let timezone_for_today: chrono_tz::Tz = user_timezone.into();
    let today = now.with_timezone(&timezone_for_today).date_naive();
    let range_plan = load_wallet_report_range_plan_db(
        user_id,
        wallet_id,
        requested_from,
        requested_to,
        user_timezone,
        today,
    )
    .map_err(|err| match err {
        WalletReportLoadError::WalletNotFound => not_found_error("Wallet not found"),
        WalletReportLoadError::InvalidDateRange(date_err) => {
            single_field_validation_error("to", date_err.to_string())
        }
        WalletReportLoadError::Database(db_err) => {
            internal_error("load_wallet_report_range_plan", db_err)
        }
    })?;
    let entitlements = crate::payments::entitlements::load_feature_entitlements(user_id, now)
        .map_err(|err| internal_error("load_feature_entitlements", err))?;
    let access_decision = crate::report_access::decide_report_access(
        range_plan.requested_range,
        today,
        report_access_entitlements(&entitlements),
    );
    let report = load_wallet_report_db(
        user_id,
        wallet_id,
        Some(access_decision.access.effective_from),
        Some(access_decision.access.effective_to),
        user_timezone,
        crate::transactions::TransactionCount::from_u32(
            entitlements.historical_backfill_transactions_per_account,
        ),
    )
    .map_err(|err| match err {
        WalletReportLoadError::WalletNotFound => not_found_error("Wallet not found"),
        WalletReportLoadError::InvalidDateRange(date_err) => {
            single_field_validation_error("to", date_err.to_string())
        }
        WalletReportLoadError::Database(db_err) => internal_error("load_wallet_report", db_err),
    })?;

    let wallet_label = report.wallet_label;
    let resolved_from = report.resolved_from;
    let resolved_to = report.resolved_to;
    let default_this_year_from = range_plan.default_range.from();
    let default_this_year_to = range_plan.default_range.to();
    let native_accounts = report.accounts;

    let mut accounts = native_accounts
        .into_iter()
        .map(|row| {
            let instance = crate::asset_capabilities::asset_instance(
                &crate::asset_capabilities::synced_asset_instance(
                    crate::asset_capabilities::synced_asset_instance_id(row.asset_id),
                )
                .asset_instance_id,
            )
            .ok_or_else(|| {
                internal_error(
                    "asset_instance_lookup",
                    "synced asset instance not found in registry",
                )
            })?;
            let decimal_precision = instance.decimal_precision;
            Ok(WalletReportAccountRow {
                account_id: row.account_id.into(),
                account_label: row.account_label,
                catalog_asset_key: Some(crate::asset_views::CatalogAssetKey::from_trusted(
                    crate::asset_capabilities::asset_id_for_synced_asset(row.asset_id)
                        .as_str()
                        .to_string(),
                )),
                asset_display_name: Some(
                    crate::asset_capabilities::asset(
                        &crate::asset_capabilities::asset_id_for_synced_asset(row.asset_id),
                    )
                    .map(|a| a.canonical_name.to_string())
                    .ok_or_else(|| internal_error("asset_lookup", "asset not found in registry"))?,
                ),
                unit_code: instance.unit_code.as_str().to_string(),
                symbol: instance.symbol.as_ref().map(|s| s.to_string()),
                bitcoin_history_coverage: row.bitcoin_history_coverage.map(Into::into),
                opening_balance_state: wallet_report_balance_state_view(
                    row.opening_balance_state,
                    decimal_precision,
                ),
                opening_balance: row
                    .opening_balance
                    .map(|value| balance_amount_view(value, decimal_precision)),
                opening_balance_date: row.opening_balance_date,
                closing_balance_state: wallet_report_balance_state_view(
                    row.closing_balance_state,
                    decimal_precision,
                ),
                closing_balance: row
                    .closing_balance
                    .map(|value| balance_amount_view(value, decimal_precision)),
                closing_balance_date: row.closing_balance_date,
            })
        })
        .collect::<Result<Vec<_>, WalletError>>()?;

    let custom_accounts =
        load_manual_asset_wallet_report_rows_db(user_id, wallet_id, resolved_from, resolved_to)
            .map_err(|err| internal_error("load_manual_asset_wallet_report_rows", err))?;

    accounts.extend(custom_accounts.into_iter().map(|row| {
        let decimal_precision = row.decimal_precision.as_u8();
        let opening_balance_state = custom_wallet_report_balance_state_view(
            row.opening_balance_state.clone(),
            decimal_precision,
        );
        let opening_balance =
            custom_wallet_report_balance_value(row.opening_balance_state, decimal_precision);
        let closing_balance_state = custom_wallet_report_balance_state_view(
            row.closing_balance_state.clone(),
            decimal_precision,
        );
        let closing_balance =
            custom_wallet_report_balance_value(row.closing_balance_state, decimal_precision);

        WalletReportAccountRow {
            account_id: row.account_id,
            account_label: row.account_label.as_str().to_string(),
            catalog_asset_key: Some(crate::asset_views::CatalogAssetKey::from_trusted(
                row.asset_id.as_str().to_string(),
            )),
            asset_display_name: crate::asset_capabilities::asset(&row.asset_id)
                .map(|asset| asset.canonical_name.to_string()),
            unit_code: row.unit_code.to_string(),
            symbol: None,
            bitcoin_history_coverage: None,
            opening_balance_state,
            opening_balance,
            opening_balance_date: row.opening_balance_date,
            closing_balance_state,
            closing_balance,
            closing_balance_date: row.closing_balance_date,
        }
    }));

    accounts.sort_by(sort_report_account_rows);

    Ok(WalletReportResponse {
        wallet_label,
        resolved_from,
        resolved_to,
        default_this_year_from,
        default_this_year_to,
        access: access_decision.access,
        accounts,
    })
}

#[get("/_app/user/reports/holdings?from&to&timezone", cookies: CookieJar)]
pub(crate) async fn get_holdings_report(
    from: Option<ReportDateParam>,
    to: Option<ReportDateParam>,
    timezone: ReportTimezoneParam,
) -> Result<HoldingsReportResponse, WalletError> {
    tracing::debug!("wallets: holdings report requested");
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("wallets", &session_token, unauthorized_error, |message| {
            internal_error("require_initialized_session", message)
        })?;
    let user_id = initialized_session.session.user_id;

    let user_timezone = timezone.into_user_timezone();
    let requested_from = from.map(ReportDateParam::into_naive_date);
    let requested_to = to.map(ReportDateParam::into_naive_date);
    let now = Utc::now();
    let timezone_for_today: chrono_tz::Tz = user_timezone.into();
    let today = now.with_timezone(&timezone_for_today).date_naive();
    let range_plan = load_holdings_report_range_plan_db(
        user_id,
        requested_from,
        requested_to,
        user_timezone,
        today,
    )
    .map_err(|err| match err {
        WalletReportLoadError::WalletNotFound => not_found_error("Wallet not found"),
        WalletReportLoadError::InvalidDateRange(date_err) => {
            single_field_validation_error("to", date_err.to_string())
        }
        WalletReportLoadError::Database(db_err) => {
            internal_error("load_holdings_report_range_plan", db_err)
        }
    })?;
    let entitlements = crate::payments::entitlements::load_feature_entitlements(user_id, now)
        .map_err(|err| internal_error("load_feature_entitlements", err))?;
    let access_decision = crate::report_access::decide_report_access(
        range_plan.requested_range,
        today,
        report_access_entitlements(&entitlements),
    );
    let report = load_holdings_report_db(
        user_id,
        Some(access_decision.access.effective_from),
        Some(access_decision.access.effective_to),
        user_timezone,
        today,
        crate::transactions::TransactionCount::from_u32(
            entitlements.historical_backfill_transactions_per_account,
        ),
    )
    .map_err(|err| match err {
        WalletReportLoadError::WalletNotFound => not_found_error("Wallet not found"),
        WalletReportLoadError::InvalidDateRange(date_err) => {
            single_field_validation_error("to", date_err.to_string())
        }
        WalletReportLoadError::Database(db_err) => internal_error("load_holdings_report", db_err),
    })?;
    let price_rows = crate::backend::prices::holdings_report_price_rows(user_id, &report)?;
    let resolved_prices = crate::backend::prices::resolved_prices_for_holdings_report_price_rows(
        user_id,
        &report,
        user_timezone,
        &price_rows,
    )?;

    holdings_report_response(
        report,
        access_decision.access,
        &price_rows,
        &resolved_prices,
    )
}

#[cfg(feature = "server")]
type HoldingsPriceRequirement = (PriceSubject, BoundaryKind);

#[cfg(feature = "server")]
type HoldingsResolvedPriceMap = HashMap<HoldingsPriceRequirement, rust_decimal::Decimal>;

#[cfg(feature = "server")]
fn holdings_boundary_sort_key(boundary: BoundaryKind) -> u8 {
    match boundary {
        BoundaryKind::Opening => 0,
        BoundaryKind::Closing => 1,
    }
}

#[cfg(feature = "server")]
fn holdings_price_requirements(
    price_rows: &[HoldingsReportPriceRow],
) -> Vec<HoldingsPriceRequirement> {
    let mut requirements = Vec::new();
    for row in price_rows {
        if row.opening.needs_price() {
            requirements.push((row.subject.clone(), BoundaryKind::Opening));
        }
        if row.closing.needs_price() {
            requirements.push((row.subject.clone(), BoundaryKind::Closing));
        }
    }

    requirements.sort_by_key(|(subject, boundary)| {
        (
            price_subject_sort_key(subject),
            holdings_boundary_sort_key(*boundary),
        )
    });
    requirements.dedup();
    requirements
}

#[cfg(feature = "server")]
fn holdings_subject_labels(price_rows: &[HoldingsReportPriceRow]) -> Vec<(PriceSubject, String)> {
    let mut labels = price_rows
        .iter()
        .map(|row| (row.subject.clone(), row.label.clone()))
        .collect::<Vec<_>>();

    labels.sort_by_key(|(subject, _)| price_subject_sort_key(subject));
    labels.dedup_by(|left, right| left.0 == right.0);
    labels
}

#[cfg(feature = "server")]
fn holdings_resolved_price_map(
    views: &[crate::backend::prices::ResolvedPriceView],
) -> HoldingsResolvedPriceMap {
    views
        .iter()
        .filter_map(|view| {
            let price = view
                .price
                .as_deref()
                .and_then(|value| value.parse::<rust_decimal::Decimal>().ok())?;
            Some(((view.subject.clone(), view.boundary), price))
        })
        .collect()
}

#[cfg(feature = "server")]
fn holdings_account_boundary_value(
    row: &HoldingsReportPriceRow,
    boundary: BoundaryKind,
    prices: &HoldingsResolvedPriceMap,
) -> Option<rust_decimal::Decimal> {
    let balance = match boundary {
        BoundaryKind::Opening => row.opening,
        BoundaryKind::Closing => row.closing,
    }
    .amount()?;
    if balance == rust_decimal::Decimal::ZERO {
        return Some(rust_decimal::Decimal::ZERO);
    }

    prices
        .get(&(row.subject.clone(), boundary))
        .map(|price| balance * *price)
}

#[cfg(feature = "server")]
fn holdings_wallet_boundary_value(
    wallet_id: crate::wallets::WalletId,
    price_rows: &[HoldingsReportPriceRow],
    boundary: BoundaryKind,
    prices: &HoldingsResolvedPriceMap,
) -> Option<rust_decimal::Decimal> {
    let values = price_rows
        .iter()
        .filter(|row| row.wallet_id == wallet_id)
        .map(|row| holdings_account_boundary_value(row, boundary, prices))
        .collect::<Vec<_>>();
    sum_required_values(values)
}

#[cfg(feature = "server")]
fn holdings_report_response(
    report: crate::db::HoldingsReportData,
    access: crate::report_access::ReportAccessView,
    price_rows: &[HoldingsReportPriceRow],
    resolved_prices: &[crate::backend::prices::ResolvedPriceView],
) -> Result<HoldingsReportResponse, WalletError> {
    let price_requirements = holdings_price_requirements(price_rows);
    let subject_labels = holdings_subject_labels(price_rows);
    let resolved_price_map = holdings_resolved_price_map(resolved_prices);
    let wallets = report
        .wallets
        .into_iter()
        .map(|wallet| {
            let opening_fiat = holdings_wallet_boundary_value(
                wallet.wallet_id,
                price_rows,
                BoundaryKind::Opening,
                &resolved_price_map,
            );
            let closing_fiat = holdings_wallet_boundary_value(
                wallet.wallet_id,
                price_rows,
                BoundaryKind::Closing,
                &resolved_price_map,
            );
            let (change_fiat, change_percent) = match (opening_fiat, closing_fiat) {
                (Some(opening), Some(closing)) => (
                    Some(fiat_amount_view(closing - opening)),
                    holdings_change_percent(opening, closing),
                ),
                _ => (None, None),
            };

            Ok(HoldingsReportWalletRow {
                wallet_id: wallet.wallet_id,
                wallet_label: wallet.wallet_label,
                opening_fiat: opening_fiat.map(fiat_amount_view),
                closing_fiat: closing_fiat.map(fiat_amount_view),
                change_fiat,
                change_percent,
            })
        })
        .collect::<Result<Vec<_>, WalletError>>()?;

    Ok(HoldingsReportResponse {
        resolved_from: report.resolved_from,
        resolved_to: report.resolved_to,
        default_this_year_from: report.default_this_year_from,
        default_this_year_to: report.default_this_year_to,
        access,
        wallets,
        price_requirements,
        subject_labels,
    })
}

#[post("/_app/user/wallets/account/addresses", cookies: CookieJar)]
pub(crate) async fn get_account_addresses(
    request: GetAccountAddressesRequest,
) -> Result<GetAccountAddressesResponse, WalletError> {
    tracing::debug!("wallets: account addresses requested");
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("wallets", &session_token, unauthorized_error, |message| {
            internal_error("require_initialized_session", message)
        })?;
    let user_id = initialized_session.session.user_id;

    let validated = request.try_into_validated().map_err(validation_error)?;

    let wallets = list_wallets(user_id).map_err(|e| internal_error("wallets", e))?;
    let account = wallets
        .iter()
        .flat_map(|wallet| wallet.accounts.iter())
        .find(|account| account.id == validated.account_id)
        .ok_or_else(|| not_found_error("Account not found"))?;

    let derivation_base_path = match account.account_kind {
        AccountKind::HdPubkey => Some(
            account
                .hd_keys
                .iter()
                .find(|key| key.address_scheme == validated.address_scheme)
                .map(|key| key.derivation_path.to_string())
                .ok_or_else(|| {
                    single_field_validation_error(
                        "address_scheme",
                        "Address scheme is not linked for this account",
                    )
                })?,
        ),
        AccountKind::SingleAddress => {
            let has_address_for_scheme = account
                .addresses
                .iter()
                .any(|address| address.address_scheme == validated.address_scheme);
            if !has_address_for_scheme {
                return Err(single_field_validation_error(
                    "address_scheme",
                    "Address scheme is not linked for this account",
                ));
            }
            None
        }
    };

    let addresses_page = load_account_addresses_page_db(
        user_id,
        validated.account_id,
        validated.address_scheme,
        validated.page,
        validated.page_size,
    )
    .map_err(|e| internal_error("wallets", e))?;

    let balances_response =
        load_all_account_balances_db(user_id).map_err(|e| internal_error("wallets", e))?;
    let address_balances: HashMap<String, AddressBalanceSummary> = balances_response
        .accounts
        .iter()
        .flat_map(|account| {
            account
                .addresses
                .iter()
                .map(|addr| (addr.address.as_str().to_string(), addr.balance.clone()))
        })
        .collect();

    let rows = addresses_page
        .rows
        .iter()
        .map(|row| {
            let balance = address_balances
                .get(&row.address)
                .cloned()
                .unwrap_or_else(|| AddressBalanceSummary::unknown(account.asset_id));

            let derivation_path = match (
                derivation_base_path.as_deref(),
                row.derivation_change,
                row.derivation_index,
            ) {
                (Some(base_path), Some(change), Some(index)) => {
                    format!("{base_path}/{change}/{index}")
                }
                _ if account.account_kind == AccountKind::SingleAddress => {
                    "Single address".to_string()
                }
                _ => "Manual".to_string(),
            };

            Ok(crate::wallets::AccountAddressRowResponse {
                address: row.address.clone(),
                sync: crate::wallets::requests::AccountAddressSyncStatusResponse {
                    status: row.sync_status,
                    last_completed_at: row.sync_last_completed_at,
                    last_error: row.sync_last_error.clone(),
                },
                transaction_count: row.transaction_count,
                reported_transaction_count: row.reported_transaction_count,
                balance: wallet_balance_view(
                    account.asset_id,
                    account.network,
                    &balance,
                    BalanceReliability::finalized(),
                )?,
                derivation_path,
            })
        })
        .collect::<Result<Vec<_>, WalletError>>()?;

    Ok(GetAccountAddressesResponse {
        page: addresses_page.page,
        page_size: addresses_page.page_size,
        total: addresses_page.total,
        rows,
    })
}

#[post("/_app/user/wallets/by-fingerprint", cookies: CookieJar)]
pub(crate) async fn get_wallet_by_fingerprint(
    request: GetWalletByFingerprintRequest,
) -> Result<Option<WalletView>, WalletError> {
    tracing::debug!("wallets: get by fingerprint requested");
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("wallets", &session_token, unauthorized_error, |message| {
            internal_error("require_initialized_session", message)
        })?;
    let user_id = initialized_session.session.user_id;

    let validated = request.try_into_validated().map_err(validation_error)?;

    tracing::debug!(
        user_id = %user_id,
        fingerprint = %validated.master_fingerprint.as_str(),
        "wallets: get by fingerprint authorized"
    );

    let wallet = get_wallet_by_fingerprint_db(user_id, &validated.master_fingerprint)
        .map_err(|e| internal_error("wallets", e))?;

    let summary_bundle = load_wallet_summary_bundle(user_id)
        .map_err(|e: crate::db::DbError| internal_error("wallets", e))?;
    let balance_projection =
        load_wallet_balance_projection_from_summary(user_id, summary_bundle.clone())?;
    let custom_account_balances = load_manual_asset_current_balances_db(user_id)
        .map_err(|e: crate::db::DbError| internal_error("wallets", e))?;
    let entitlements =
        crate::payments::entitlements::load_feature_entitlements(user_id, Utc::now())
            .map_err(|e| internal_error("wallets", e))?;
    let classified_accounts = crate::db::account_limits::classify_supported_accounts_for_user(
        user_id,
        usize::from(entitlements.sync_account_slots_limit),
    )
    .map_err(|e| internal_error("wallets", e))?;
    let sync_slot_map =
        load_account_sync_slot_map(user_id).map_err(|e| internal_error("wallets", e))?;
    let sync_slot_records = sync_slot_map.values().cloned().collect::<Vec<_>>();
    let (
        wallets,
        manual_asset_accounts,
        address_balances,
        account_balances,
        account_balance_reliabilities,
    ) = (
        summary_bundle.wallets,
        summary_bundle.manual_asset_accounts,
        summary_bundle.address_balances,
        summary_bundle.account_balances,
        summary_bundle.account_balance_reliabilities,
    );
    let free_balance_unavailable_account_ids =
        free_balance_unavailable_account_ids(&wallets, &entitlements.tier);
    let active_sync_slot_records = sync_slot_records
        .iter()
        .filter(|record| !free_balance_unavailable_account_ids.contains(&record.account_id))
        .cloned()
        .collect::<Vec<_>>();
    let active_sync_slots = active_sync_slot_account_ids(
        &active_sync_slot_records,
        entitlements.sync_account_slots_limit,
    );
    let Some(requested_wallet) = wallet else {
        return Ok(None);
    };
    let wallet = wallets
        .into_iter()
        .find(|summary_wallet| summary_wallet.wallet.id == requested_wallet.wallet.id)
        .ok_or_else(|| internal_error("wallets", "Wallet not found in summary bundle"))?;
    let account_transactions =
        load_account_transaction_history_db(user_id).map_err(|e| internal_error("wallets", e))?;
    let account_tx_counts = load_account_transaction_counts_db(user_id)
        .map_err(|e: crate::db::DbError| internal_error("wallets", e))?;
    let projected_balances = balance_projection
        .wallets
        .iter()
        .find(|projected| projected.id == wallet.wallet.id)
        .map(|projected| projected.balances.as_slice())
        .ok_or_else(|| {
            internal_error(
                "wallet_balance_projection",
                "wallet missing from balance projection",
            )
        })?;

    Ok(Some(convert_wallet_to_view(
        wallet,
        projected_balances,
        &WalletAccountData {
            manual_asset_accounts: &manual_asset_accounts,
            address_balances: &address_balances,
            account_balances: &account_balances,
            account_balance_reliabilities: &account_balance_reliabilities,
            custom_account_balances: &custom_account_balances,
            account_transactions: Some(&account_transactions),
            account_tx_counts: &account_tx_counts,
        },
        &NativeAccountManualSyncContext {
            sync_slots: &sync_slot_map,
            active_sync_slot_account_ids: &active_sync_slots,
            slot_limit: entitlements.sync_account_slots_limit,
            tier: entitlements.tier.clone(),
            historical_backfill_enabled: entitlements.historical_backfill_enabled,
            historical_backfill_transactions_per_account: entitlements
                .historical_backfill_transactions_per_account,
            free_balance_unavailable_account_ids: &free_balance_unavailable_account_ids,
        },
        &classified_accounts,
    )?))
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use crate::backend::api_error::ApiErrorCode;
    use crate::backend::prices::{
        HoldingsReportBoundaryAmount, HoldingsReportPriceRow, ResolvedPriceView,
    };
    use crate::services::manual_asset_discovery::ManualAssetDiscoveryDetailError;
    use crate::services::price_overrides::{BoundaryKind, PriceSubject};
    use crate::wallets::WalletId;

    fn date(year: i32, month: u32, day: u32) -> chrono::NaiveDate {
        chrono::NaiveDate::from_ymd_opt(year, month, day).expect("valid date")
    }

    fn dec(value: &str) -> rust_decimal::Decimal {
        value.parse().expect("valid decimal")
    }

    fn full_access() -> crate::report_access::ReportAccessView {
        crate::report_access::ReportAccessView {
            requested_from: date(2026, 1, 1),
            requested_to: date(2026, 6, 30),
            effective_from: date(2026, 1, 1),
            effective_to: date(2026, 6, 30),
            gate: crate::report_access::ReportAccessGate::Full,
            range_clamped: false,
            can_edit_prices: true,
        }
    }

    #[test]
    fn rate_limited_detail_maps_to_too_many_requests_not_internal() {
        let err = detail_error_to_wallet_error(ManualAssetDiscoveryDetailError::RateLimited {
            retry_after: None,
        });
        assert_eq!(err.code, ApiErrorCode::TooManyRequests);
        assert!(!err.is_internal());
        assert!(err.message.to_lowercase().contains("coingecko"));
    }

    #[test]
    fn provider_detail_error_stays_internal() {
        let err = detail_error_to_wallet_error(ManualAssetDiscoveryDetailError::Provider(
            "boom".to_string(),
        ));
        assert!(err.is_internal());
    }

    #[test]
    fn entitlements_convert_to_report_access_entitlements() {
        let mut entitlements = crate::payments::types::FeatureEntitlements::free();
        entitlements.tax_reports = true;
        entitlements.exchange_rates_history = true;
        entitlements.price_overrides = false;

        let access = report_access_entitlements(&entitlements);

        assert!(access.tax_reports);
        assert!(access.exchange_rates_history);
        assert!(!access.price_overrides);
    }

    #[test]
    fn holdings_change_percent_is_absent_when_opening_zero() {
        assert_eq!(
            holdings_change_percent(rust_decimal::Decimal::ZERO, rust_decimal::Decimal::from(10)),
            None
        );
    }

    #[test]
    fn holdings_wallet_value_requires_all_boundary_prices() {
        let values = vec![Some(rust_decimal::Decimal::from(10)), None];
        assert_eq!(sum_required_values(values), None);
    }

    #[test]
    fn holdings_wallet_value_sums_complete_boundary_prices() {
        let values = vec![
            Some(rust_decimal::Decimal::from(10)),
            Some(rust_decimal::Decimal::from(5)),
        ];
        assert_eq!(
            sum_required_values(values),
            Some(rust_decimal::Decimal::from(15))
        );
    }

    #[test]
    fn holdings_report_response_keeps_empty_wallets_empty() {
        let report = crate::db::HoldingsReportData {
            resolved_from: date(2026, 1, 1),
            resolved_to: date(2026, 7, 4),
            default_this_year_from: date(2026, 1, 1),
            default_this_year_to: date(2026, 7, 4),
            wallets: Vec::new(),
        };
        let access = crate::report_access::ReportAccessView {
            requested_from: report.resolved_from,
            requested_to: report.resolved_to,
            effective_from: report.resolved_from,
            effective_to: report.resolved_to,
            gate: crate::report_access::ReportAccessGate::Full,
            range_clamped: false,
            can_edit_prices: false,
        };

        let response = holdings_report_response(report, access, &[], &[]).expect("response builds");

        assert!(response.wallets.is_empty());
        assert!(response.price_requirements.is_empty());
        assert!(response.subject_labels.is_empty());
    }

    #[test]
    fn holdings_report_response_includes_manual_rows_in_requirements_labels_and_totals() {
        let wallet_id = WalletId::new();
        let subject = PriceSubject::CatalogAsset(
            crate::asset_views::CatalogAssetKey::from_trusted("gold".to_string()),
        );
        let report = crate::db::HoldingsReportData {
            resolved_from: date(2026, 1, 1),
            resolved_to: date(2026, 6, 30),
            default_this_year_from: date(2026, 1, 1),
            default_this_year_to: date(2026, 12, 31),
            wallets: vec![crate::db::HoldingsReportWalletData {
                wallet_id,
                wallet_label: "Vault".to_string(),
                accounts: Vec::new(),
            }],
        };
        let price_rows = vec![HoldingsReportPriceRow {
            wallet_id,
            subject: subject.clone(),
            label: "Gold".to_string(),
            opening: HoldingsReportBoundaryAmount::Amount(dec("2")),
            closing: HoldingsReportBoundaryAmount::Amount(dec("3")),
        }];
        let resolved_prices = vec![
            ResolvedPriceView {
                subject: subject.clone(),
                boundary: BoundaryKind::Opening,
                price: Some("100".to_string()),
                source: None,
            },
            ResolvedPriceView {
                subject: subject.clone(),
                boundary: BoundaryKind::Closing,
                price: Some("110".to_string()),
                source: None,
            },
        ];

        let response =
            holdings_report_response(report, full_access(), &price_rows, &resolved_prices)
                .expect("response builds");

        assert_eq!(
            response.price_requirements,
            vec![
                (subject.clone(), BoundaryKind::Opening),
                (subject.clone(), BoundaryKind::Closing),
            ]
        );
        assert_eq!(response.subject_labels, vec![(subject, "Gold".to_string())]);
        assert_eq!(
            response.wallets[0].opening_fiat.as_ref().unwrap().raw_value,
            "200"
        );
        assert_eq!(
            response.wallets[0].closing_fiat.as_ref().unwrap().raw_value,
            "330"
        );
        assert_eq!(
            response.wallets[0].change_fiat.as_ref().unwrap().raw_value,
            "130"
        );
        assert_eq!(response.wallets[0].change_percent.as_deref(), Some("65.00"));
    }
}
