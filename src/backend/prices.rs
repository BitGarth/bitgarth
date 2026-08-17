use chrono::{DateTime, Utc};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use super::ApiErrorEnvelope;
#[cfg(feature = "server")]
use super::session_context::{
    InitializedSession, require_initialized_session, require_session_token,
};
#[cfg(feature = "server")]
use crate::asset_capabilities::AssetId;
#[cfg(feature = "server")]
use crate::db::{self, PriceOverrideRecord};
use crate::models::CurrencyCode;
#[cfg(feature = "server")]
use crate::models::{FieldErrors, SessionToken, UserTimezone};
#[cfg(feature = "server")]
use crate::services::price_overrides::price_subject_sort_key;
use crate::services::price_overrides::{BoundaryKind, PriceSubject};
#[cfg(feature = "server")]
use crate::services::price_overrides::{
    NewPriceOverride, OverrideLookup, PriceOverride, PriceOverrideValidationError, PriceSource,
    ResolvedPrice, local_timestamp_to_utc, report_boundary_utc, validate_price_decimal,
    validate_source_note,
};
#[cfg(feature = "server")]
use crate::wallets::SyncedAssetId;
use crate::wallets::{ReportDateParam, ReportTimezoneParam, WalletId};

#[cfg(feature = "server")]
use axum_extra::extract::cookie::CookieJar;
#[cfg(feature = "server")]
use dioxus::logger::tracing;

pub(crate) type PriceOverrideError = ApiErrorEnvelope;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct PriceOverrideView {
    pub subject: PriceSubject,
    pub quote_currency: CurrencyCode,
    pub price_time_utc: DateTime<Utc>,
    pub price: String,
    pub source_note: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct UpsertPriceOverrideInput {
    pub subject: PriceSubject,
    pub quote_currency: CurrencyCode,
    pub price_time_local: String,
    pub price: String,
    pub source_note: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct DeletePriceOverrideInput {
    pub subject: PriceSubject,
    pub quote_currency: CurrencyCode,
    pub price_time_local: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct ResolvedPriceView {
    pub subject: PriceSubject,
    pub boundary: BoundaryKind,
    pub price: Option<String>,
    pub source: Option<PriceSourceView>,
}

#[cfg(feature = "server")]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum HoldingsReportBoundaryAmount {
    Zero,
    Amount(rust_decimal::Decimal),
    Unknown,
}

#[cfg(feature = "server")]
impl HoldingsReportBoundaryAmount {
    pub(crate) fn needs_price(self) -> bool {
        matches!(self, Self::Amount(_))
    }

    pub(crate) fn amount(self) -> Option<rust_decimal::Decimal> {
        match self {
            Self::Zero => Some(rust_decimal::Decimal::ZERO),
            Self::Amount(amount) => Some(amount),
            Self::Unknown => None,
        }
    }
}

#[cfg(feature = "server")]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HoldingsReportPriceRow {
    pub(crate) wallet_id: WalletId,
    pub(crate) subject: PriceSubject,
    pub(crate) label: String,
    pub(crate) opening: HoldingsReportBoundaryAmount,
    pub(crate) closing: HoldingsReportBoundaryAmount,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum PriceSourceView {
    UserOverride {
        source_note: Option<String>,
        updated_at: DateTime<Utc>,
    },
    ProviderPrice {
        provider: String,
        provider_asset_id: Option<String>,
        provider_quote_id: Option<String>,
        retrieved_at: DateTime<Utc>,
        license_scope: String,
    },
}

#[cfg(feature = "server")]
impl From<PriceOverrideRecord> for PriceOverride {
    fn from(record: PriceOverrideRecord) -> Self {
        Self {
            subject: record.subject,
            quote_currency: record.quote_currency,
            price_time_utc: record.price_time_utc,
            price: record.price,
            source_note: record.source_note,
            updated_at: record.updated_at,
        }
    }
}

#[cfg(feature = "server")]
impl From<PriceOverride> for PriceOverrideView {
    fn from(override_price: PriceOverride) -> Self {
        Self {
            subject: override_price.subject,
            quote_currency: override_price.quote_currency,
            price_time_utc: override_price.price_time_utc,
            price: override_price.price.to_string(),
            source_note: override_price.source_note,
            updated_at: override_price.updated_at,
        }
    }
}

#[cfg(feature = "server")]
impl From<PriceOverrideRecord> for PriceOverrideView {
    fn from(record: PriceOverrideRecord) -> Self {
        PriceOverride::from(record).into()
    }
}

#[cfg(feature = "server")]
fn unauthorized_error(message: String) -> PriceOverrideError {
    PriceOverrideError::unauthorized(message)
}

#[cfg(feature = "server")]
fn forbidden_error(message: impl Into<String>) -> PriceOverrideError {
    PriceOverrideError::forbidden(message)
}

#[cfg(feature = "server")]
fn validation_error(field: &str, message: String) -> PriceOverrideError {
    let mut errors = FieldErrors::new();
    errors.add(field, message);
    PriceOverrideError::validation("Validation error", errors)
}

#[cfg(feature = "server")]
fn invalid_price_subject_type_error() -> PriceOverrideError {
    let message = "Invalid price subject type";
    let mut errors = FieldErrors::new();
    errors.add("subject_type", message.to_string());
    PriceOverrideError::validation(message, errors)
}

#[cfg(feature = "server")]
fn internal_error(context: &str, detail: impl std::fmt::Display) -> PriceOverrideError {
    tracing::error!(context, error = %detail, "prices: internal failure");
    PriceOverrideError::internal()
}

#[cfg(feature = "server")]
fn session_token_from_cookie(cookies: &CookieJar) -> Result<SessionToken, PriceOverrideError> {
    require_session_token("prices", cookies, unauthorized_error)
}

#[cfg(feature = "server")]
fn current_session(cookies: &CookieJar) -> Result<InitializedSession, PriceOverrideError> {
    let session_token = session_token_from_cookie(cookies)?;
    require_initialized_session("prices", &session_token, unauthorized_error, |_message| {
        PriceOverrideError::internal()
    })
}

#[cfg(feature = "server")]
fn load_user_timezone(user_id: crate::models::UserId) -> Result<UserTimezone, PriceOverrideError> {
    let settings =
        db::load_settings(user_id).map_err(|err| internal_error("load_settings", err))?;
    Ok(settings
        .timezone
        .unwrap_or_else(|| UserTimezone::from(chrono_tz::Tz::UTC)))
}

#[cfg(feature = "server")]
fn validation_to_error(field: &str, err: PriceOverrideValidationError) -> PriceOverrideError {
    validation_error(field, err.to_string())
}

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
fn ensure_price_override_mutation_allowed(
    entitlements: &crate::payments::types::FeatureEntitlements,
) -> Result<(), PriceOverrideError> {
    if entitlements.price_overrides {
        Ok(())
    } else {
        Err(forbidden_error("Upgrade to add or edit report prices."))
    }
}

#[cfg(feature = "server")]
pub(crate) fn parse_price_subject(
    subject_type: &str,
    subject_id: &str,
) -> Result<PriceSubject, PriceOverrideError> {
    match subject_type {
        "catalog_asset" | "native_asset" => {
            let key = crate::asset_views::CatalogAssetKey::try_new(subject_id.to_string())
                .map_err(|_| {
                    validation_error("subject_id", "Invalid native asset id".to_string())
                })?;
            let is_known = matches!(
                key.as_str(),
                "bitcoin" | "ethereum" | "usd-coin" | "cardano"
            );
            if !is_known {
                return Err(validation_error(
                    "subject_id",
                    "Invalid native asset id".to_string(),
                ));
            }
            Ok(PriceSubject::CatalogAsset(key))
        }
        _ => Err(invalid_price_subject_type_error()),
    }
}

#[get("/_app/user/prices/overrides?subject_type&subject_id&quote_currency&from&to", cookies: CookieJar)]
pub(crate) async fn list_price_overrides(
    subject_type: String,
    subject_id: String,
    quote_currency: CurrencyCode,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<Vec<PriceOverrideView>, PriceOverrideError> {
    let initialized_session = current_session(&cookies)?;
    let user_id = initialized_session.session.user_id;
    let subject = parse_price_subject(&subject_type, &subject_id)?;
    let records = db::list_price_overrides_in_range(user_id, subject, quote_currency, from, to)
        .map_err(|err| internal_error("list_price_overrides_in_range", err))?;
    Ok(records.into_iter().map(PriceOverrideView::from).collect())
}

#[post("/_app/user/prices/overrides", cookies: CookieJar)]
pub(crate) async fn upsert_price_override(
    input: UpsertPriceOverrideInput,
) -> Result<PriceOverrideView, PriceOverrideError> {
    let initialized_session = current_session(&cookies)?;
    let user_id = initialized_session.session.user_id;
    let entitlements =
        crate::payments::entitlements::load_feature_entitlements(user_id, Utc::now())
            .map_err(|err| internal_error("load_feature_entitlements", err))?;
    ensure_price_override_mutation_allowed(&entitlements)?;
    let timezone = load_user_timezone(user_id)?;
    let price_time_utc = local_timestamp_to_utc(&input.price_time_local, timezone)
        .map_err(|err| validation_to_error("price_time_local", err))?;
    let price =
        validate_price_decimal(&input.price).map_err(|err| validation_to_error("price", err))?;
    let source_note = validate_source_note(input.source_note)
        .map_err(|err| validation_to_error("source_note", err))?;
    let record = db::upsert_price_override(
        user_id,
        NewPriceOverride {
            subject: input.subject,
            quote_currency: input.quote_currency,
            price_time_utc,
            price,
            source_note,
        },
        Utc::now(),
    )
    .map_err(|err| internal_error("upsert_price_override", err))?;
    Ok(PriceOverrideView::from(record))
}

#[post("/_app/user/prices/overrides/delete", cookies: CookieJar)]
pub(crate) async fn delete_price_override(
    input: DeletePriceOverrideInput,
) -> Result<(), PriceOverrideError> {
    let initialized_session = current_session(&cookies)?;
    let user_id = initialized_session.session.user_id;
    let entitlements =
        crate::payments::entitlements::load_feature_entitlements(user_id, Utc::now())
            .map_err(|err| internal_error("load_feature_entitlements", err))?;
    ensure_price_override_mutation_allowed(&entitlements)?;
    let timezone = load_user_timezone(user_id)?;
    let price_time_utc = local_timestamp_to_utc(&input.price_time_local, timezone)
        .map_err(|err| validation_to_error("price_time_local", err))?;
    db::delete_price_override(user_id, input.subject, input.quote_currency, price_time_utc)
        .map_err(|err| internal_error("delete_price_override", err))
}

#[cfg(feature = "server")]
fn source_view_from_domain(source: PriceSource) -> PriceSourceView {
    match source {
        PriceSource::UserOverride {
            source_note,
            updated_at,
        } => PriceSourceView::UserOverride {
            source_note,
            updated_at,
        },
        PriceSource::ProviderPrice {
            provider,
            provider_asset_id,
            provider_quote_id,
            retrieved_at,
            license_scope,
        } => PriceSourceView::ProviderPrice {
            provider,
            provider_asset_id,
            provider_quote_id,
            retrieved_at,
            license_scope,
        },
    }
}

#[cfg(feature = "server")]
fn provider_price_query_for_subject(
    subject: &PriceSubject,
    quote: CurrencyCode,
    boundary: BoundaryKind,
    from: ReportDateParam,
    to: ReportDateParam,
) -> Option<db::DailyPricePointQuery> {
    let PriceSubject::CatalogAsset(asset_id) = subject;
    let date_utc = match boundary {
        BoundaryKind::Opening => from.into_naive_date(),
        BoundaryKind::Closing => to.into_naive_date(),
    };
    Some(db::DailyPricePointQuery {
        asset_id: asset_id.as_str().to_string(),
        quote_currency: quote,
        date_utc,
    })
}

#[cfg(feature = "server")]
fn resolve_report_price_from_sources(
    subject: PriceSubject,
    boundary: BoundaryKind,
    manual: Option<PriceOverride>,
    provider: Option<db::DailyPricePointRecord>,
) -> ResolvedPriceView {
    if let Some(manual) = manual {
        let resolved = ResolvedPrice {
            price: manual.price,
            source: PriceSource::UserOverride {
                source_note: manual.source_note,
                updated_at: manual.updated_at,
            },
        };
        return ResolvedPriceView {
            subject,
            boundary,
            price: Some(resolved.price.to_string()),
            source: Some(source_view_from_domain(resolved.source)),
        };
    }

    if let Some(provider) = provider {
        let resolved = ResolvedPrice {
            price: provider.price,
            source: PriceSource::ProviderPrice {
                provider: provider.provider,
                provider_asset_id: provider.provider_asset_id,
                provider_quote_id: provider.provider_quote_id,
                retrieved_at: provider.retrieved_at,
                license_scope: provider.license_scope,
            },
        };
        return ResolvedPriceView {
            subject,
            boundary,
            price: Some(resolved.price.to_string()),
            source: Some(source_view_from_domain(resolved.source)),
        };
    }

    ResolvedPriceView {
        subject,
        boundary,
        price: None,
        source: None,
    }
}

#[cfg(feature = "server")]
struct BoundaryResolutionContext<'a> {
    user_id: crate::models::UserId,
    prices_conn: &'a rusqlite::Connection,
    quote: CurrencyCode,
    from: ReportDateParam,
    to: ReportDateParam,
}

#[cfg(feature = "server")]
fn resolve_boundary_view(
    context: &BoundaryResolutionContext<'_>,
    subject: PriceSubject,
    boundary: BoundaryKind,
    lookup: OverrideLookup,
) -> Result<ResolvedPriceView, PriceOverrideError> {
    let manual = db::lookup_price_override(context.user_id, subject.clone(), context.quote, lookup)
        .map_err(|err| internal_error("lookup_price_override", err))?;

    let manual = manual.map(PriceOverride::from);
    let provider = if manual.is_none() {
        match provider_price_query_for_subject(
            &subject,
            context.quote,
            boundary,
            context.from,
            context.to,
        ) {
            Some(query) => db::lookup_daily_price_point(context.prices_conn, &query)
                .map_err(|err| internal_error("lookup_daily_price_point", err))?,
            None => None,
        }
    } else {
        None
    };

    Ok(resolve_report_price_from_sources(
        subject, boundary, manual, provider,
    ))
}

#[cfg(feature = "server")]
fn sort_and_dedupe_subjects(subjects: &mut Vec<PriceSubject>) {
    subjects.sort_by_key(price_subject_sort_key);
    subjects.dedup();
}

#[cfg(feature = "server")]
#[cfg(feature = "server")]
fn catalog_asset_price_subject(asset_id: &AssetId) -> PriceSubject {
    PriceSubject::CatalogAsset(crate::asset_views::CatalogAssetKey::from_trusted(
        asset_id.as_str().to_string(),
    ))
}

#[cfg(feature = "server")]
fn native_asset_price_subject(asset_id: SyncedAssetId) -> PriceSubject {
    catalog_asset_price_subject(&crate::asset_capabilities::asset_id_for_synced_asset(
        asset_id,
    ))
}

#[cfg(feature = "server")]
fn manual_asset_price_subject(asset_id: &AssetId) -> PriceSubject {
    catalog_asset_price_subject(asset_id)
}

#[cfg(feature = "server")]
fn holdings_amount_from_unsigned(
    amount: crate::amounts::UnsignedAmount,
    decimal_precision: u8,
) -> Result<HoldingsReportBoundaryAmount, PriceOverrideError> {
    if amount == crate::amounts::UnsignedAmount::zero() {
        return Ok(HoldingsReportBoundaryAmount::Zero);
    }

    crate::amounts::format_unsigned_amount(amount, decimal_precision)
        .parse::<rust_decimal::Decimal>()
        .map(HoldingsReportBoundaryAmount::Amount)
        .map_err(|err| internal_error("holdings_report_price_row_amount", err))
}

#[cfg(feature = "server")]
fn native_holdings_boundary_amount(
    state: db::WalletReportBalanceState,
    decimal_precision: u8,
) -> Result<HoldingsReportBoundaryAmount, PriceOverrideError> {
    match state {
        db::WalletReportBalanceState::CanonicalZero => Ok(HoldingsReportBoundaryAmount::Zero),
        db::WalletReportBalanceState::KnownAmount(amount) => {
            holdings_amount_from_unsigned(amount, decimal_precision)
        }
        db::WalletReportBalanceState::Unknown => Ok(HoldingsReportBoundaryAmount::Unknown),
    }
}

#[cfg(feature = "server")]
fn manual_holdings_boundary_amount(
    state: db::ManualAssetBalanceState,
    decimal_precision: u8,
) -> Result<HoldingsReportBoundaryAmount, PriceOverrideError> {
    match state {
        db::ManualAssetBalanceState::Known(amount) => {
            holdings_amount_from_unsigned(amount, decimal_precision)
        }
        db::ManualAssetBalanceState::Unknown => Ok(HoldingsReportBoundaryAmount::Unknown),
    }
}

#[cfg(feature = "server")]
fn holdings_native_asset_label(asset_id: SyncedAssetId) -> Result<String, PriceOverrideError> {
    crate::asset_capabilities::asset(&crate::asset_capabilities::asset_id_for_synced_asset(
        asset_id,
    ))
    .map(|asset| asset.canonical_name.to_string())
    .ok_or_else(|| internal_error("asset_lookup_for_holdings_price_row", "asset not found"))
}

#[cfg(feature = "server")]
fn holdings_native_decimal_precision(asset_id: SyncedAssetId) -> Result<u8, PriceOverrideError> {
    let instance = crate::asset_capabilities::asset_instance(
        &crate::asset_capabilities::synced_asset_instance(
            crate::asset_capabilities::synced_asset_instance_id(asset_id),
        )
        .asset_instance_id,
    )
    .ok_or_else(|| {
        internal_error(
            "asset_instance_lookup_for_holdings_price_row",
            "synced asset instance not found",
        )
    })?;
    Ok(instance.decimal_precision)
}

#[cfg(feature = "server")]
fn holdings_manual_asset_label(
    asset_id: &AssetId,
    unit_code: &crate::wallets::ValidatedManualAssetUnitCode,
) -> String {
    crate::asset_capabilities::asset(asset_id)
        .map(|asset| asset.canonical_name.to_string())
        .unwrap_or_else(|| unit_code.to_string())
}

#[cfg(feature = "server")]
pub(crate) fn holdings_report_price_rows(
    user_id: crate::models::UserId,
    report: &db::HoldingsReportData,
) -> Result<Vec<HoldingsReportPriceRow>, PriceOverrideError> {
    let mut rows = Vec::new();
    for wallet in &report.wallets {
        for row in &wallet.accounts {
            let decimal_precision = holdings_native_decimal_precision(row.asset_id)?;
            rows.push(HoldingsReportPriceRow {
                wallet_id: wallet.wallet_id,
                subject: native_asset_price_subject(row.asset_id),
                label: holdings_native_asset_label(row.asset_id)?,
                opening: native_holdings_boundary_amount(
                    row.opening_balance_state,
                    decimal_precision,
                )?,
                closing: native_holdings_boundary_amount(
                    row.closing_balance_state,
                    decimal_precision,
                )?,
            });
        }

        let manual_rows = db::load_manual_asset_wallet_report_rows(
            user_id,
            wallet.wallet_id,
            report.resolved_from,
            report.resolved_to,
        )
        .map_err(|err| {
            internal_error(
                "load_manual_asset_wallet_report_rows_for_holdings_prices",
                err,
            )
        })?;
        for row in manual_rows {
            let decimal_precision = row.decimal_precision.as_u8();
            rows.push(HoldingsReportPriceRow {
                wallet_id: wallet.wallet_id,
                subject: manual_asset_price_subject(&row.asset_id),
                label: holdings_manual_asset_label(&row.asset_id, &row.unit_code),
                opening: manual_holdings_boundary_amount(
                    row.opening_balance_state,
                    decimal_precision,
                )?,
                closing: manual_holdings_boundary_amount(
                    row.closing_balance_state,
                    decimal_precision,
                )?,
            });
        }
    }
    Ok(rows)
}

#[cfg(feature = "server")]
fn resolve_boundary_views_for_subjects(
    context: &BoundaryResolutionContext<'_>,
    subjects: Vec<PriceSubject>,
    user_timezone: UserTimezone,
) -> Result<Vec<ResolvedPriceView>, PriceOverrideError> {
    let opening_at = report_boundary_utc(context.from, BoundaryKind::Opening, user_timezone)
        .map_err(|err| validation_to_error("from", err))?;
    let next_from = context
        .from
        .into_naive_date()
        .succ_opt()
        .ok_or_else(|| validation_error("from", "Report date is out of range".to_string()))?;
    let next_opening_at = report_boundary_utc(
        ReportDateParam::from_naive_date(next_from),
        BoundaryKind::Opening,
        user_timezone,
    )
    .map_err(|err| validation_to_error("from", err))?;
    let closing_day_start = report_boundary_utc(context.to, BoundaryKind::Opening, user_timezone)
        .map_err(|err| validation_to_error("to", err))?;
    let closing_at = report_boundary_utc(context.to, BoundaryKind::Closing, user_timezone)
        .map_err(|err| validation_to_error("to", err))?;
    let next_to = context
        .to
        .into_naive_date()
        .succ_opt()
        .ok_or_else(|| validation_error("to", "Report date is out of range".to_string()))?;
    let next_closing_day_start = report_boundary_utc(
        ReportDateParam::from_naive_date(next_to),
        BoundaryKind::Opening,
        user_timezone,
    )
    .map_err(|err| validation_to_error("to", err))?;

    let mut output = Vec::with_capacity(subjects.len() * 2);
    for subject in subjects {
        output.push(resolve_boundary_view(
            context,
            subject.clone(),
            BoundaryKind::Opening,
            OverrideLookup::SameDayLatestAtOrBefore {
                at: opening_at,
                local_day_start_utc: opening_at,
                next_local_day_start_utc: next_opening_at,
            },
        )?);
        output.push(resolve_boundary_view(
            context,
            subject,
            BoundaryKind::Closing,
            OverrideLookup::SameDayLatestAtOrBefore {
                at: closing_at,
                local_day_start_utc: closing_day_start,
                next_local_day_start_utc: next_closing_day_start,
            },
        )?);
    }
    Ok(output)
}

#[get("/_app/user/wallets/:wallet_id/report/resolved-prices?from&to&timezone", cookies: CookieJar)]
pub(crate) async fn list_resolved_prices_for_report(
    wallet_id: WalletId,
    from: ReportDateParam,
    to: ReportDateParam,
    timezone: ReportTimezoneParam,
) -> Result<Vec<ResolvedPriceView>, PriceOverrideError> {
    let initialized_session = current_session(&cookies)?;
    let user_id = initialized_session.session.user_id;
    let now = Utc::now();
    let entitlements = crate::payments::entitlements::load_feature_entitlements(user_id, now)
        .map_err(|err| internal_error("load_feature_entitlements", err))?;
    let requested_from = from.into_naive_date();
    let requested_to = to.into_naive_date();
    let user_timezone = timezone.into_user_timezone();
    let timezone_for_today: chrono_tz::Tz = user_timezone.into();
    let today = now.with_timezone(&timezone_for_today).date_naive();
    let access_decision = crate::report_access::decide_report_access(
        crate::report_dates::LocalReportDateRange::new(requested_from, requested_to)
            .map_err(|err| validation_error("to", err.to_string()))?,
        today,
        report_access_entitlements(&entitlements),
    );
    let effective_from = ReportDateParam::from_naive_date(access_decision.access.effective_from);
    let effective_to = ReportDateParam::from_naive_date(access_decision.access.effective_to);

    let report = db::load_wallet_report(
        user_id,
        wallet_id,
        Some(effective_from.into_naive_date()),
        Some(effective_to.into_naive_date()),
        user_timezone,
        crate::transactions::TransactionCount::from_u32(
            entitlements.historical_backfill_transactions_per_account,
        ),
    )
    .map_err(|err| internal_error("load_wallet_report_for_prices", err))?;

    let custom_rows = db::load_manual_asset_wallet_report_rows(
        user_id,
        wallet_id,
        report.resolved_from,
        report.resolved_to,
    )
    .map_err(|err| internal_error("load_manual_asset_wallet_report_rows_for_prices", err))?;

    let mut subjects = report
        .accounts
        .into_iter()
        .map(|row| native_asset_price_subject(row.asset_id))
        .collect::<Vec<_>>();
    subjects.extend(
        custom_rows
            .into_iter()
            .map(|row| manual_asset_price_subject(&row.asset_id)),
    );
    sort_and_dedupe_subjects(&mut subjects);

    let settings = db::load_settings(user_id)
        .map_err(|err| internal_error("load_settings_for_prices", err))?;
    let quote = settings.currency.unwrap_or_else(|| {
        crate::settings::default_currency(settings.language.unwrap_or_default())
    });
    let prices_conn = db::initialize_prices_db()
        .map_err(|err| internal_error("initialize_prices_db_for_report_prices", err))?;
    let boundary_context = BoundaryResolutionContext {
        user_id,
        prices_conn: &prices_conn,
        quote,
        from: effective_from,
        to: effective_to,
    };
    resolve_boundary_views_for_subjects(&boundary_context, subjects, user_timezone)
}

#[get("/_app/user/reports/holdings/resolved-prices?from&to&timezone", cookies: CookieJar)]
pub(crate) async fn list_resolved_prices_for_holdings_report(
    from: ReportDateParam,
    to: ReportDateParam,
    timezone: ReportTimezoneParam,
) -> Result<Vec<ResolvedPriceView>, PriceOverrideError> {
    let initialized_session = current_session(&cookies)?;
    let user_id = initialized_session.session.user_id;
    let now = Utc::now();
    let entitlements = crate::payments::entitlements::load_feature_entitlements(user_id, now)
        .map_err(|err| internal_error("load_feature_entitlements", err))?;
    let requested_from = from.into_naive_date();
    let requested_to = to.into_naive_date();
    let user_timezone = timezone.into_user_timezone();
    let timezone_for_today: chrono_tz::Tz = user_timezone.into();
    let today = now.with_timezone(&timezone_for_today).date_naive();
    let access_decision = crate::report_access::decide_report_access(
        crate::report_dates::LocalReportDateRange::new(requested_from, requested_to)
            .map_err(|err| validation_error("to", err.to_string()))?,
        today,
        report_access_entitlements(&entitlements),
    );

    let report = db::load_holdings_report(
        user_id,
        Some(access_decision.access.effective_from),
        Some(access_decision.access.effective_to),
        user_timezone,
        today,
        crate::transactions::TransactionCount::from_u32(
            entitlements.historical_backfill_transactions_per_account,
        ),
    )
    .map_err(|err| internal_error("load_holdings_report_for_prices", err))?;

    let price_rows = holdings_report_price_rows(user_id, &report)?;
    resolved_prices_for_holdings_report_price_rows(user_id, &report, user_timezone, &price_rows)
}

#[cfg(feature = "server")]
pub(crate) fn resolved_prices_for_holdings_report_price_rows(
    user_id: crate::models::UserId,
    report: &db::HoldingsReportData,
    user_timezone: UserTimezone,
    price_rows: &[HoldingsReportPriceRow],
) -> Result<Vec<ResolvedPriceView>, PriceOverrideError> {
    if !price_rows
        .iter()
        .any(|row| row.opening.needs_price() || row.closing.needs_price())
    {
        return Ok(Vec::new());
    }

    let mut subjects = price_rows
        .iter()
        .map(|row| row.subject.clone())
        .collect::<Vec<_>>();
    sort_and_dedupe_subjects(&mut subjects);

    let settings = db::load_settings(user_id)
        .map_err(|err| internal_error("load_settings_for_holdings_prices", err))?;
    let quote = settings.currency.unwrap_or_else(|| {
        crate::settings::default_currency(settings.language.unwrap_or_default())
    });
    let prices_conn = db::initialize_prices_db()
        .map_err(|err| internal_error("initialize_prices_db_for_holdings_prices", err))?;
    let boundary_context = BoundaryResolutionContext {
        user_id,
        prices_conn: &prices_conn,
        quote,
        from: ReportDateParam::from_naive_date(report.resolved_from),
        to: ReportDateParam::from_naive_date(report.resolved_to),
    };

    resolve_boundary_views_for_subjects(&boundary_context, subjects, user_timezone)
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod holdings_price_tests {
    use super::*;
    use crate::asset_views::CatalogAssetKey;
    use crate::wallets::WalletId;

    fn catalog_subject(id: &str) -> PriceSubject {
        PriceSubject::CatalogAsset(CatalogAssetKey::from_trusted(id.to_string()))
    }

    fn date(year: i32, month: u32, day: u32) -> chrono::NaiveDate {
        chrono::NaiveDate::from_ymd_opt(year, month, day).expect("valid date")
    }

    #[test]
    fn holdings_price_reads_do_not_require_edit_or_history_entitlements() {
        let mut entitlements = crate::payments::types::FeatureEntitlements::free();
        entitlements.tax_reports = true;
        entitlements.exchange_rates_history = false;
        entitlements.price_overrides = false;

        let access = report_access_entitlements(&entitlements);

        assert!(access.tax_reports);
        assert!(!access.exchange_rates_history);
        assert!(!access.price_overrides);
    }

    #[test]
    fn report_subjects_are_sorted_and_deduped() {
        let mut subjects = vec![
            catalog_subject("cardano"),
            PriceSubject::CatalogAsset(CatalogAssetKey::from_trusted("bitcoin".to_string())),
            PriceSubject::CatalogAsset(CatalogAssetKey::from_trusted("bitcoin".to_string())),
        ];

        sort_and_dedupe_subjects(&mut subjects);

        assert_eq!(
            subjects,
            vec![
                PriceSubject::CatalogAsset(CatalogAssetKey::from_trusted("bitcoin".to_string())),
                catalog_subject("cardano"),
            ]
        );
    }

    #[test]
    fn holdings_resolved_prices_skip_storage_when_no_boundary_needs_price() {
        let report = db::HoldingsReportData {
            resolved_from: date(2026, 1, 1),
            resolved_to: date(2026, 6, 30),
            default_this_year_from: date(2026, 1, 1),
            default_this_year_to: date(2026, 12, 31),
            wallets: Vec::new(),
        };
        let price_rows = vec![
            HoldingsReportPriceRow {
                wallet_id: WalletId::new(),
                subject: catalog_subject("bitcoin"),
                label: "Bitcoin".to_string(),
                opening: HoldingsReportBoundaryAmount::Zero,
                closing: HoldingsReportBoundaryAmount::Unknown,
            },
            HoldingsReportPriceRow {
                wallet_id: WalletId::new(),
                subject: catalog_subject("cardano"),
                label: "Gold".to_string(),
                opening: HoldingsReportBoundaryAmount::Unknown,
                closing: HoldingsReportBoundaryAmount::Zero,
            },
        ];

        let resolved = resolved_prices_for_holdings_report_price_rows(
            crate::models::UserId::new(),
            &report,
            crate::models::UserTimezone::from(chrono_tz::Tz::UTC),
            &price_rows,
        )
        .expect("no price storage needed");

        assert!(resolved.is_empty());
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    fn catalog_subject(id: &str) -> PriceSubject {
        PriceSubject::CatalogAsset(
            crate::asset_views::CatalogAssetKey::try_new(id).expect("asset key"),
        )
    }

    fn fixed_time(raw: &str) -> DateTime<Utc> {
        chrono::DateTime::parse_from_rfc3339(raw)
            .expect("time")
            .with_timezone(&Utc)
    }

    fn provider_record(
        asset_id: &str,
        date: chrono::NaiveDate,
        price: &str,
    ) -> db::DailyPricePointRecord {
        db::DailyPricePointRecord {
            id: format!("row-{asset_id}-{date}"),
            asset_id: asset_id.to_string(),
            quote_currency: CurrencyCode::from_code("USD").expect("USD"),
            price_time_utc: fixed_time("2025-01-01T00:00:00Z"),
            date_utc: date,
            price: price.parse().expect("decimal"),
            provider: "coingecko".to_string(),
            provider_asset_id: Some(asset_id.to_string()),
            provider_quote_id: Some("usd".to_string()),
            license_scope: "public_keyless".to_string(),
            retrieved_at: fixed_time("2025-01-02T00:00:00Z"),
        }
    }

    #[test]
    fn parse_price_subject_rejects_malformed_catalog_key() {
        let err = parse_price_subject("native_asset", "USD Coin").expect_err("invalid key");
        assert!(err.to_string().contains("Validation error"));
    }

    #[test]
    fn parse_price_subject_rejects_unknown_catalog_key() {
        let err = parse_price_subject("native_asset", "not-a-real-asset").expect_err("unknown key");
        assert!(err.to_string().contains("Validation error"));
    }

    #[test]
    fn parse_price_subject_accepts_catalog_and_native_alias() {
        let catalog = parse_price_subject("catalog_asset", "bitcoin")
            .expect("catalog subject remains supported");
        assert!(matches!(catalog, PriceSubject::CatalogAsset(_)));

        let native =
            parse_price_subject("native_asset", "bitcoin").expect("native alias remains supported");
        assert!(matches!(native, PriceSubject::CatalogAsset(_)));
    }

    #[test]
    fn parse_price_subject_rejects_custom_unit_code() {
        let err = parse_price_subject("custom_unit_code", "ADA")
            .expect_err("legacy price subjects are unavailable");
        assert!(err.to_string().contains("Invalid price subject type"));
    }

    #[test]
    fn price_override_mutation_requires_price_override_entitlement() {
        let mut entitlements = crate::payments::types::FeatureEntitlements::free();
        entitlements.price_overrides = false;

        let err = ensure_price_override_mutation_allowed(&entitlements)
            .expect_err("missing entitlement should block mutation");

        assert_eq!(err.message, "Upgrade to add or edit report prices.");
    }

    #[test]
    fn price_override_mutation_allowed_when_entitlement_present() {
        let mut entitlements = crate::payments::types::FeatureEntitlements::free();
        entitlements.price_overrides = true;

        ensure_price_override_mutation_allowed(&entitlements)
            .expect("entitlement should allow mutation");
    }

    #[test]
    fn report_price_resolver_prefers_manual_override() {
        let subject = catalog_subject("ethereum");
        let manual = PriceOverride {
            subject: subject.clone(),
            quote_currency: CurrencyCode::from_code("USD").expect("USD"),
            price_time_utc: fixed_time("2025-01-01T00:00:00Z"),
            price: "3000".parse().expect("decimal"),
            source_note: Some("statement".to_string()),
            updated_at: fixed_time("2025-01-02T00:00:00Z"),
        };
        let provider = provider_record(
            "ethereum",
            chrono::NaiveDate::from_ymd_opt(2025, 1, 1).expect("date"),
            "2800",
        );

        let resolved = resolve_report_price_from_sources(
            subject.clone(),
            BoundaryKind::Opening,
            Some(manual),
            Some(provider),
        );

        assert_eq!(resolved.subject, subject);
        assert_eq!(resolved.boundary, BoundaryKind::Opening);
        assert_eq!(resolved.price.as_deref(), Some("3000"));
        assert!(matches!(
            resolved.source,
            Some(PriceSourceView::UserOverride { .. })
        ));
    }

    #[test]
    fn report_price_resolver_uses_provider_when_manual_missing() {
        let subject = catalog_subject("ethereum");
        let provider = provider_record(
            "ethereum",
            chrono::NaiveDate::from_ymd_opt(2025, 1, 1).expect("date"),
            "2800",
        );

        let resolved =
            resolve_report_price_from_sources(subject, BoundaryKind::Closing, None, Some(provider));

        assert_eq!(resolved.price.as_deref(), Some("2800"));
        assert_eq!(
            resolved.source,
            Some(PriceSourceView::ProviderPrice {
                provider: "coingecko".to_string(),
                provider_asset_id: Some("ethereum".to_string()),
                provider_quote_id: Some("usd".to_string()),
                retrieved_at: fixed_time("2025-01-02T00:00:00Z"),
                license_scope: "public_keyless".to_string(),
            })
        );
    }

    #[test]
    fn report_price_resolver_missing_when_all_sources_missing() {
        let subject = catalog_subject("ethereum");

        let resolved =
            resolve_report_price_from_sources(subject, BoundaryKind::Opening, None, None);

        assert_eq!(resolved.price, None);
        assert_eq!(resolved.source, None);
    }

    #[test]
    fn report_price_provider_lookup_request_uses_report_dates() {
        let from = ReportDateParam::from_naive_date(
            chrono::NaiveDate::from_ymd_opt(2025, 1, 1).expect("date"),
        );
        let to = ReportDateParam::from_naive_date(
            chrono::NaiveDate::from_ymd_opt(2025, 12, 31).expect("date"),
        );
        let quote = CurrencyCode::from_code("USD").expect("USD");

        let opening = provider_price_query_for_subject(
            &catalog_subject("ethereum"),
            quote,
            BoundaryKind::Opening,
            from,
            to,
        )
        .expect("opening query");
        let closing = provider_price_query_for_subject(
            &catalog_subject("ethereum"),
            quote,
            BoundaryKind::Closing,
            from,
            to,
        )
        .expect("closing query");
        assert_eq!(opening.asset_id, "ethereum");
        assert_eq!(opening.quote_currency, quote);
        assert_eq!(
            opening.date_utc,
            chrono::NaiveDate::from_ymd_opt(2025, 1, 1).expect("date")
        );
        assert_eq!(closing.quote_currency, quote);
        assert_eq!(
            closing.date_utc,
            chrono::NaiveDate::from_ymd_opt(2025, 12, 31).expect("date")
        );
    }
}
