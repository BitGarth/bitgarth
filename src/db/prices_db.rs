//! Prices database - installation-local public market data cache.
//!
//! This database is unencrypted and must not store user-private wallet data.

use super::error::DbError;
use super::sqlite_config::configure_connection;
use crate::models::CurrencyCode;
use crate::project_paths::get_price_database_path;
use chrono::{DateTime, Duration, NaiveDate, SecondsFormat, Utc};
use dioxus::logger::tracing;
use rusqlite::{OptionalExtension, params};
use rust_decimal::Decimal;
use std::collections::HashSet;
use std::path::Path;
use std::str::FromStr;

pub(crate) const CURRENT_PRICE_CACHE_TTL: Duration = Duration::seconds(5 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CurrentPriceCacheRequest {
    pub(crate) asset_id: String,
    pub(crate) provider_asset_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CurrentPriceCacheRecord {
    pub(crate) asset_id: String,
    pub(crate) quote_currency: CurrencyCode,
    pub(crate) provider: String,
    pub(crate) provider_asset_id: String,
    pub(crate) provider_quote_id: Option<String>,
    pub(crate) price: Decimal,
    pub(crate) observed_at: Option<DateTime<Utc>>,
    pub(crate) retrieved_at: DateTime<Utc>,
    pub(crate) license_scope: String,
    pub(crate) updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CurrentPriceCacheUpsert {
    pub(crate) asset_id: String,
    pub(crate) quote_currency: CurrencyCode,
    pub(crate) provider: String,
    pub(crate) provider_asset_id: String,
    pub(crate) provider_quote_id: Option<String>,
    pub(crate) price: Decimal,
    pub(crate) observed_at: Option<DateTime<Utc>>,
    pub(crate) retrieved_at: DateTime<Utc>,
    pub(crate) license_scope: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DailyPriceDateQuery {
    pub(crate) asset_id: String,
    pub(crate) quote_currency: CurrencyCode,
    pub(crate) provider_asset_id: String,
    pub(crate) license_scope: String,
    pub(crate) start: NaiveDate,
    pub(crate) end: NaiveDate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DailyPricePointQuery {
    pub(crate) asset_id: String,
    pub(crate) quote_currency: CurrencyCode,
    pub(crate) date_utc: NaiveDate,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DailyPricePointUpsert {
    pub(crate) asset_id: String,
    pub(crate) quote_currency: CurrencyCode,
    pub(crate) price_time_utc: DateTime<Utc>,
    pub(crate) date_utc: NaiveDate,
    pub(crate) price: Decimal,
    pub(crate) provider_asset_id: String,
    pub(crate) provider_quote_id: Option<String>,
    pub(crate) license_scope: String,
    pub(crate) retrieved_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DailyPricePointRecord {
    pub(crate) id: String,
    pub(crate) asset_id: String,
    pub(crate) quote_currency: CurrencyCode,
    pub(crate) price_time_utc: DateTime<Utc>,
    pub(crate) date_utc: NaiveDate,
    pub(crate) price: Decimal,
    pub(crate) provider: String,
    pub(crate) provider_asset_id: Option<String>,
    pub(crate) provider_quote_id: Option<String>,
    pub(crate) license_scope: String,
    pub(crate) retrieved_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoricalPriceAttemptStatus {
    SuccessWithPrices,
    SuccessEmpty,
    TransientFailure,
    RateLimited,
    PermanentFailure,
}

impl HistoricalPriceAttemptStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            HistoricalPriceAttemptStatus::SuccessWithPrices => "success_with_prices",
            HistoricalPriceAttemptStatus::SuccessEmpty => "success_empty",
            HistoricalPriceAttemptStatus::TransientFailure => "transient_failure",
            HistoricalPriceAttemptStatus::RateLimited => "rate_limited",
            HistoricalPriceAttemptStatus::PermanentFailure => "permanent_failure",
        }
    }
}

pub(crate) fn parse_historical_price_attempt_status(
    raw: &str,
) -> Result<HistoricalPriceAttemptStatus, DbError> {
    match raw {
        "success_with_prices" => Ok(HistoricalPriceAttemptStatus::SuccessWithPrices),
        "success_empty" => Ok(HistoricalPriceAttemptStatus::SuccessEmpty),
        "transient_failure" => Ok(HistoricalPriceAttemptStatus::TransientFailure),
        "rate_limited" => Ok(HistoricalPriceAttemptStatus::RateLimited),
        "permanent_failure" => Ok(HistoricalPriceAttemptStatus::PermanentFailure),
        _ => Err(DbError::new(format!(
            "historical price attempt: invalid status {raw}"
        ))),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistoricalPriceAttemptUpsert {
    pub(crate) asset_id: String,
    pub(crate) provider: String,
    pub(crate) from_date: NaiveDate,
    pub(crate) to_date: NaiveDate,
    pub(crate) status: HistoricalPriceAttemptStatus,
    pub(crate) attempted_at: DateTime<Utc>,
    pub(crate) rows_returned: u32,
    pub(crate) next_retry_after: Option<DateTime<Utc>>,
    pub(crate) error_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistoricalPriceAttemptQuery {
    pub(crate) asset_id: String,
    pub(crate) provider: String,
    pub(crate) from_date: NaiveDate,
    pub(crate) to_date: NaiveDate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistoricalPriceAttemptCooldownQuery {
    pub(crate) asset_id: String,
    pub(crate) provider: String,
    pub(crate) min_attempted_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistoricalPriceAttemptRecord {
    pub(crate) id: String,
    pub(crate) asset_id: String,
    pub(crate) provider: String,
    pub(crate) from_date: NaiveDate,
    pub(crate) to_date: NaiveDate,
    pub(crate) status: HistoricalPriceAttemptStatus,
    pub(crate) attempted_at: DateTime<Utc>,
    pub(crate) rows_returned: u32,
    pub(crate) next_retry_after: Option<DateTime<Utc>>,
    pub(crate) error_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CoinGeckoCatalogUpsert {
    pub(crate) provider_asset_id: String,
    pub(crate) symbol: String,
    pub(crate) normalized_symbol: String,
    pub(crate) name: String,
    pub(crate) platforms_json: Option<String>,
    pub(crate) status: String,
    pub(crate) retrieved_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CoinGeckoCatalogSearchRow {
    pub(crate) provider_asset_id: String,
    pub(crate) symbol: String,
    pub(crate) normalized_symbol: String,
    pub(crate) name: String,
    pub(crate) platforms_json: Option<String>,
    pub(crate) status: String,
    pub(crate) retrieved_at: DateTime<Utc>,
}

const MIGRATIONS: &[(&str, &str)] = &[
    (
        "V0__price_cache",
        include_str!("../../migrations/prices/V0__price_cache.sql"),
    ),
    (
        "V1__asset_id_price_rows",
        include_str!("../../migrations/prices/V1__asset_id_price_rows.sql"),
    ),
    (
        "V2__historical_price_attempts",
        include_str!("../../migrations/prices/V2__historical_price_attempts.sql"),
    ),
];

fn migrations_runner() -> Result<refinery::Runner, DbError> {
    let migrations = MIGRATIONS
        .iter()
        .map(|(name, sql)| {
            refinery::Migration::unapplied(name, sql)
                .map_err(|err| DbError::new(format!("Invalid prices migration {name}: {err}")))
        })
        .collect::<Result<Vec<_>, DbError>>()?;

    Ok(refinery::Runner::new(&migrations))
}

pub(crate) fn initialize_prices_db() -> Result<rusqlite::Connection, DbError> {
    initialize_prices_db_at_path(&get_price_database_path()?)
}

fn initialize_prices_db_at_path(db_path: &Path) -> Result<rusqlite::Connection, DbError> {
    let mut conn = rusqlite::Connection::open(db_path).map_err(|err| {
        DbError::new(format!(
            "Failed to open prices database at {}: {err}",
            db_path.display()
        ))
    })?;
    configure_and_migrate_connection(&mut conn)?;
    Ok(conn)
}

fn configure_and_migrate_connection(conn: &mut rusqlite::Connection) -> Result<(), DbError> {
    configure_connection(conn, "prices db", true);

    let runner = migrations_runner()?;
    let report = runner
        .run(conn)
        .map_err(|err| DbError::new(format!("Failed to run prices migrations: {err}")))?;

    let applied_count = report.applied_migrations().len();
    match runner
        .get_last_applied_migration(conn)
        .map_err(|err| DbError::new(format!("Failed to query prices schema version: {err}")))?
    {
        Some(migration) => {
            tracing::info!(
                "prices db: migrations completed - schema version V{}__{}, applied {} new migration(s)",
                migration.version(),
                migration.name(),
                applied_count,
            );
        }
        None => {
            tracing::info!("prices db: migrations completed - no migrations applied");
        }
    }

    Ok(())
}

struct RawCurrentPriceCacheRow {
    asset_id: String,
    quote_currency: String,
    provider: String,
    provider_asset_id: String,
    provider_quote_id: Option<String>,
    price: String,
    observed_at: Option<String>,
    retrieved_at: String,
    license_scope: String,
    updated_at: String,
}

fn read_raw_current_price_cache_row(
    row: &rusqlite::Row,
) -> rusqlite::Result<RawCurrentPriceCacheRow> {
    Ok(RawCurrentPriceCacheRow {
        asset_id: row.get(0)?,
        quote_currency: row.get(1)?,
        provider: row.get(2)?,
        provider_asset_id: row.get(3)?,
        provider_quote_id: row.get(4)?,
        price: row.get(5)?,
        observed_at: row.get(6)?,
        retrieved_at: row.get(7)?,
        license_scope: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

struct RawDailyPricePointRow {
    id: String,
    asset_id: String,
    quote_currency: String,
    price_time_utc: String,
    date_utc: String,
    price: String,
    provider: String,
    provider_asset_id: Option<String>,
    provider_quote_id: Option<String>,
    license_scope: String,
    retrieved_at: String,
}

fn read_raw_daily_price_point_row(row: &rusqlite::Row) -> rusqlite::Result<RawDailyPricePointRow> {
    Ok(RawDailyPricePointRow {
        id: row.get(0)?,
        asset_id: row.get(1)?,
        quote_currency: row.get(2)?,
        price_time_utc: row.get(3)?,
        date_utc: row.get(4)?,
        price: row.get(5)?,
        provider: row.get(6)?,
        provider_asset_id: row.get(7)?,
        provider_quote_id: row.get(8)?,
        license_scope: row.get(9)?,
        retrieved_at: row.get(10)?,
    })
}

struct RawHistoricalPriceAttemptRow {
    id: String,
    asset_id: String,
    provider: String,
    from_date: String,
    to_date: String,
    status: String,
    attempted_at: String,
    rows_returned: i64,
    next_retry_after: Option<String>,
    error_code: Option<String>,
}

fn read_raw_historical_price_attempt_row(
    row: &rusqlite::Row,
) -> rusqlite::Result<RawHistoricalPriceAttemptRow> {
    Ok(RawHistoricalPriceAttemptRow {
        id: row.get(0)?,
        asset_id: row.get(1)?,
        provider: row.get(2)?,
        from_date: row.get(3)?,
        to_date: row.get(4)?,
        status: row.get(5)?,
        attempted_at: row.get(6)?,
        rows_returned: row.get(7)?,
        next_retry_after: row.get(8)?,
        error_code: row.get(9)?,
    })
}

fn cache_time_text(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

fn date_text(value: NaiveDate) -> String {
    value.format("%Y-%m-%d").to_string()
}

fn parse_prices_db_utc(raw: &str, context: &str) -> Result<DateTime<Utc>, DbError> {
    DateTime::parse_from_rfc3339(raw)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|err| DbError::new(format!("{context}: invalid timestamp {raw}: {err}")))
}

fn parse_prices_db_date(raw: &str, context: &str) -> Result<NaiveDate, DbError> {
    NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .map_err(|err| DbError::new(format!("{context}: invalid date {raw}: {err}")))
}

fn parse_current_price_cache_utc(
    raw: &str,
    field: &str,
    asset_id: &str,
    provider: &str,
) -> Option<DateTime<Utc>> {
    match DateTime::parse_from_rfc3339(raw) {
        Ok(value) => Some(value.with_timezone(&Utc)),
        Err(err) => {
            tracing::warn!(
                asset_id = %asset_id,
                provider = %provider,
                field = %field,
                error = %err,
                "prices db: invalid current price cache timestamp; treating row as cache miss"
            );
            None
        }
    }
}

fn parse_current_price_cache_decimal(raw: &str, asset_id: &str, provider: &str) -> Option<Decimal> {
    match Decimal::from_str(raw) {
        Ok(value) => Some(value),
        Err(err) => {
            tracing::warn!(
                asset_id = %asset_id,
                provider = %provider,
                error = %err,
                "prices db: invalid current price cache decimal; treating row as cache miss"
            );
            None
        }
    }
}

fn raw_current_price_cache_to_record(
    raw: RawCurrentPriceCacheRow,
    quote_currency: CurrencyCode,
    cutoff: DateTime<Utc>,
) -> Option<CurrentPriceCacheRecord> {
    let price = parse_current_price_cache_decimal(&raw.price, &raw.asset_id, &raw.provider)?;
    let observed_at = match raw.observed_at.as_deref() {
        Some(value) => Some(parse_current_price_cache_utc(
            value,
            "observed_at",
            &raw.asset_id,
            &raw.provider,
        )?),
        None => None,
    };
    let retrieved_at = parse_current_price_cache_utc(
        &raw.retrieved_at,
        "retrieved_at",
        &raw.asset_id,
        &raw.provider,
    )?;
    if retrieved_at < cutoff {
        return None;
    }
    let updated_at =
        parse_current_price_cache_utc(&raw.updated_at, "updated_at", &raw.asset_id, &raw.provider)?;

    Some(CurrentPriceCacheRecord {
        asset_id: raw.asset_id,
        quote_currency,
        provider: raw.provider,
        provider_asset_id: raw.provider_asset_id,
        provider_quote_id: raw.provider_quote_id,
        price,
        observed_at,
        retrieved_at,
        license_scope: raw.license_scope,
        updated_at,
    })
}

fn raw_daily_price_point_to_record(
    raw: RawDailyPricePointRow,
) -> Result<DailyPricePointRecord, DbError> {
    let quote_currency = CurrencyCode::from_code(&raw.quote_currency).ok_or_else(|| {
        DbError::new(format!(
            "daily price point: invalid quote currency {}",
            raw.quote_currency
        ))
    })?;
    let price = Decimal::from_str(&raw.price)
        .map_err(|err| DbError::new(format!("daily price point: invalid decimal: {err}")))?;

    Ok(DailyPricePointRecord {
        id: raw.id,
        asset_id: raw.asset_id,
        quote_currency,
        price_time_utc: parse_prices_db_utc(&raw.price_time_utc, "daily price point time")?,
        date_utc: parse_prices_db_date(&raw.date_utc, "daily price point date")?,
        price,
        provider: raw.provider,
        provider_asset_id: raw.provider_asset_id,
        provider_quote_id: raw.provider_quote_id,
        license_scope: raw.license_scope,
        retrieved_at: parse_prices_db_utc(&raw.retrieved_at, "daily price point retrieval")?,
    })
}

fn raw_historical_price_attempt_to_record(
    raw: RawHistoricalPriceAttemptRow,
) -> Result<HistoricalPriceAttemptRecord, DbError> {
    let rows_returned = u32::try_from(raw.rows_returned).map_err(|_| {
        DbError::new(format!(
            "historical price attempt: rows returned out of range {}",
            raw.rows_returned
        ))
    })?;
    let next_retry_after = raw
        .next_retry_after
        .as_deref()
        .map(|value| parse_prices_db_utc(value, "historical price attempt next retry"))
        .transpose()?;

    Ok(HistoricalPriceAttemptRecord {
        id: raw.id,
        asset_id: raw.asset_id,
        provider: raw.provider,
        from_date: parse_prices_db_date(&raw.from_date, "historical price attempt from date")?,
        to_date: parse_prices_db_date(&raw.to_date, "historical price attempt to date")?,
        status: parse_historical_price_attempt_status(&raw.status)?,
        attempted_at: parse_prices_db_utc(&raw.attempted_at, "historical price attempt time")?,
        rows_returned,
        next_retry_after,
        error_code: raw.error_code,
    })
}

pub(crate) fn load_fresh_current_price_cache(
    conn: &rusqlite::Connection,
    requests: &[CurrentPriceCacheRequest],
    quote_currency: CurrencyCode,
    provider: &str,
    now: DateTime<Utc>,
) -> Result<Vec<CurrentPriceCacheRecord>, DbError> {
    if requests.is_empty() {
        return Ok(Vec::new());
    }

    let cutoff = now - CURRENT_PRICE_CACHE_TTL;
    let mut stmt = conn
        .prepare(
            "SELECT
                asset_id,
                quote_currency,
                provider,
                provider_asset_id,
                provider_quote_id,
                price,
                observed_at,
                retrieved_at,
                license_scope,
                updated_at
             FROM current_price_cache
             WHERE asset_id = ?1
                AND quote_currency = ?2
                AND provider = ?3
                AND provider_asset_id = ?4",
        )
        .map_err(|err| DbError::from_rusqlite_error("prepare current price cache lookup", err))?;

    let mut hits = Vec::new();
    let mut seen_requests = HashSet::new();
    for request in requests {
        if !seen_requests.insert((
            request.asset_id.as_str(),
            request.provider_asset_id.as_str(),
        )) {
            continue;
        }

        let mut rows = stmt
            .query(params![
                &request.asset_id,
                quote_currency.code(),
                provider,
                &request.provider_asset_id
            ])
            .map_err(|err| DbError::from_rusqlite_error("query current price cache", err))?;

        while let Some(row) = rows
            .next()
            .map_err(|err| DbError::from_rusqlite_error("read current price cache row", err))?
        {
            let raw = read_raw_current_price_cache_row(row).map_err(|err| {
                DbError::from_rusqlite_error("decode current price cache row", err)
            })?;
            if raw.quote_currency != quote_currency.code() {
                tracing::warn!(
                    asset_id = %raw.asset_id,
                    quote_currency = %raw.quote_currency,
                    "prices db: invalid current price cache quote currency; treating row as cache miss"
                );
                continue;
            }
            if let Some(record) = raw_current_price_cache_to_record(raw, quote_currency, cutoff) {
                hits.push(record);
            }
        }
    }

    Ok(hits)
}

pub(crate) fn upsert_current_price_cache(
    conn: &rusqlite::Connection,
    row: CurrentPriceCacheUpsert,
) -> Result<(), DbError> {
    let observed_at = row.observed_at.map(cache_time_text);
    let retrieved_at = cache_time_text(row.retrieved_at);
    let price = row.price.to_string();

    conn.execute(
        "INSERT INTO current_price_cache (
            asset_id,
            quote_currency,
            provider,
            provider_asset_id,
            provider_quote_id,
            price,
            observed_at,
            retrieved_at,
            license_scope,
            created_at,
            updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?8, ?8)
        ON CONFLICT(asset_id, quote_currency, provider)
        DO UPDATE SET
            provider_asset_id = excluded.provider_asset_id,
            provider_quote_id = excluded.provider_quote_id,
            price = excluded.price,
            observed_at = excluded.observed_at,
            retrieved_at = excluded.retrieved_at,
            license_scope = excluded.license_scope,
            updated_at = excluded.updated_at",
        params![
            row.asset_id,
            row.quote_currency.code(),
            row.provider,
            row.provider_asset_id,
            row.provider_quote_id,
            price,
            observed_at,
            retrieved_at,
            row.license_scope,
        ],
    )
    .map_err(|err| DbError::from_rusqlite_error("upsert current price cache", err))?;

    Ok(())
}

fn historical_price_attempt_id(row: &HistoricalPriceAttemptUpsert) -> String {
    format!(
        "{}:{}:{}:{}",
        row.provider,
        row.asset_id,
        date_text(row.from_date),
        date_text(row.to_date)
    )
}

fn validate_historical_price_attempt_span(
    from_date: NaiveDate,
    to_date: NaiveDate,
) -> Result<(), DbError> {
    if from_date > to_date {
        return Err(DbError::new(format!(
            "historical price attempt: invalid date span {} > {}",
            date_text(from_date),
            date_text(to_date)
        )));
    }
    Ok(())
}

pub(crate) fn upsert_historical_price_attempt(
    conn: &rusqlite::Connection,
    row: HistoricalPriceAttemptUpsert,
) -> Result<(), DbError> {
    validate_historical_price_attempt_span(row.from_date, row.to_date)?;
    let id = historical_price_attempt_id(&row);
    let from_date = date_text(row.from_date);
    let to_date = date_text(row.to_date);
    let attempted_at = cache_time_text(row.attempted_at);
    let next_retry_after = row.next_retry_after.map(cache_time_text);
    let now = cache_time_text(Utc::now());

    conn.execute(
        "INSERT INTO historical_price_attempts (
            id,
            provider,
            asset_id,
            from_date,
            to_date,
            status,
            attempted_at,
            rows_returned,
            next_retry_after,
            error_code,
            created_at,
            updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)
        ON CONFLICT(provider, asset_id, from_date, to_date)
        DO UPDATE SET
            status = excluded.status,
            attempted_at = excluded.attempted_at,
            rows_returned = excluded.rows_returned,
            next_retry_after = excluded.next_retry_after,
            error_code = excluded.error_code,
            updated_at = excluded.updated_at
        WHERE historical_price_attempts.attempted_at <= excluded.attempted_at",
        params![
            id,
            row.provider,
            row.asset_id,
            from_date,
            to_date,
            row.status.as_str(),
            attempted_at,
            i64::from(row.rows_returned),
            next_retry_after,
            row.error_code,
            now,
        ],
    )
    .map_err(|err| DbError::from_rusqlite_error("upsert historical price attempt", err))?;

    Ok(())
}

pub(crate) fn latest_historical_price_attempt(
    conn: &rusqlite::Connection,
    query: &HistoricalPriceAttemptQuery,
) -> Result<Option<HistoricalPriceAttemptRecord>, DbError> {
    validate_historical_price_attempt_span(query.from_date, query.to_date)?;
    let mut stmt = conn
        .prepare(
            "SELECT
                id,
                asset_id,
                provider,
                from_date,
                to_date,
                status,
                attempted_at,
                rows_returned,
                next_retry_after,
                error_code
             FROM historical_price_attempts
             WHERE asset_id = ?1
               AND provider = ?2
               AND from_date <= ?3
               AND to_date >= ?4
             ORDER BY attempted_at DESC, id DESC
             LIMIT 1",
        )
        .map_err(|err| {
            DbError::from_rusqlite_error("prepare historical price attempt lookup", err)
        })?;

    let raw = stmt
        .query_row(
            params![
                &query.asset_id,
                &query.provider,
                date_text(query.from_date),
                date_text(query.to_date),
            ],
            read_raw_historical_price_attempt_row,
        )
        .optional()
        .map_err(|err| {
            DbError::from_rusqlite_error("query historical price attempt lookup", err)
        })?;

    raw.map(raw_historical_price_attempt_to_record).transpose()
}

pub(crate) fn latest_historical_price_cooldown_attempt(
    conn: &rusqlite::Connection,
    query: &HistoricalPriceAttemptCooldownQuery,
) -> Result<Option<HistoricalPriceAttemptRecord>, DbError> {
    let mut stmt = conn
        .prepare(
            "SELECT
                id,
                asset_id,
                provider,
                from_date,
                to_date,
                status,
                attempted_at,
                rows_returned,
                next_retry_after,
                error_code
             FROM historical_price_attempts
             WHERE asset_id = ?1
               AND provider = ?2
               AND status IN ('success_empty', 'rate_limited')
               AND attempted_at >= ?3
             ORDER BY attempted_at DESC, id DESC
             LIMIT 1",
        )
        .map_err(|err| {
            DbError::from_rusqlite_error("prepare historical price cooldown attempt lookup", err)
        })?;

    let raw = stmt
        .query_row(
            params![
                &query.asset_id,
                &query.provider,
                cache_time_text(query.min_attempted_at),
            ],
            read_raw_historical_price_attempt_row,
        )
        .optional()
        .map_err(|err| {
            DbError::from_rusqlite_error("query historical price cooldown attempt lookup", err)
        })?;

    raw.map(raw_historical_price_attempt_to_record).transpose()
}

pub(crate) fn load_daily_price_dates(
    conn: &rusqlite::Connection,
    query: &DailyPriceDateQuery,
) -> Result<Vec<NaiveDate>, DbError> {
    let mut stmt = conn
        .prepare(
            "SELECT date_utc
             FROM price_points
             WHERE asset_id = ?1
               AND quote_currency = ?2
               AND provider = 'coingecko'
               AND provider_asset_id = ?3
               AND granularity = 'daily'
               AND price_kind = 'daily_point'
               AND license_scope = ?4
               AND date_utc BETWEEN ?5 AND ?6
             ORDER BY date_utc ASC",
        )
        .map_err(|err| DbError::from_rusqlite_error("prepare daily price date query", err))?;

    let rows = stmt
        .query_map(
            params![
                &query.asset_id,
                query.quote_currency.code(),
                &query.provider_asset_id,
                &query.license_scope,
                date_text(query.start),
                date_text(query.end),
            ],
            |row| row.get::<_, String>(0),
        )
        .map_err(|err| DbError::from_rusqlite_error("query daily price dates", err))?;

    let mut dates = Vec::new();
    for row in rows {
        let raw = row.map_err(|err| DbError::from_rusqlite_error("read daily price date", err))?;
        dates.push(parse_prices_db_date(&raw, "daily price date")?);
    }
    Ok(dates)
}

pub(crate) fn lookup_daily_price_point(
    conn: &rusqlite::Connection,
    query: &DailyPricePointQuery,
) -> Result<Option<DailyPricePointRecord>, DbError> {
    let mut stmt = conn
        .prepare(
            "SELECT
                id,
                asset_id,
                quote_currency,
                price_time_utc,
                date_utc,
                price,
                provider,
                provider_asset_id,
                provider_quote_id,
                license_scope,
                retrieved_at
             FROM price_points
             WHERE asset_id = ?1
               AND quote_currency = ?2
               AND date_utc = ?3
               AND granularity = 'daily'
               AND price_kind = 'daily_point'",
        )
        .map_err(|err| DbError::from_rusqlite_error("prepare daily price point lookup", err))?;

    let rows = stmt
        .query_map(
            params![
                &query.asset_id,
                query.quote_currency.code(),
                date_text(query.date_utc),
            ],
            read_raw_daily_price_point_row,
        )
        .map_err(|err| DbError::from_rusqlite_error("query daily price point lookup", err))?;

    let mut decoded = Vec::new();
    for row in rows {
        let raw =
            row.map_err(|err| DbError::from_rusqlite_error("read daily price point row", err))?;
        decoded.push(raw_daily_price_point_to_record(raw)?);
    }

    Ok(select_daily_price_point(decoded))
}

fn provider_preference_rank(provider: &str) -> u8 {
    match provider {
        "coingecko" => 0,
        _ => 1,
    }
}

pub(crate) fn select_daily_price_point(
    mut rows: Vec<DailyPricePointRecord>,
) -> Option<DailyPricePointRecord> {
    rows.sort_by(|left, right| {
        provider_preference_rank(&left.provider)
            .cmp(&provider_preference_rank(&right.provider))
            .then_with(|| left.provider.cmp(&right.provider))
            .then_with(|| right.retrieved_at.cmp(&left.retrieved_at))
            .then_with(|| left.id.cmp(&right.id))
    });
    rows.into_iter().next()
}

pub(crate) fn upsert_daily_price_points(
    conn: &rusqlite::Connection,
    rows: &[DailyPricePointUpsert],
) -> Result<(), DbError> {
    if rows.is_empty() {
        return Ok(());
    }

    conn.execute("BEGIN IMMEDIATE", [])
        .map_err(|err| DbError::from_rusqlite_error("begin daily price upsert", err))?;
    let result = upsert_daily_price_points_in_transaction(conn, rows);
    match result {
        Ok(()) => conn
            .execute("COMMIT", [])
            .map(|_| ())
            .map_err(|err| DbError::from_rusqlite_error("commit daily price upsert", err)),
        Err(err) => {
            let _ = conn.execute("ROLLBACK", []);
            Err(err)
        }
    }
}

fn upsert_daily_price_points_in_transaction(
    conn: &rusqlite::Connection,
    rows: &[DailyPricePointUpsert],
) -> Result<(), DbError> {
    let now = Utc::now();
    let mut stmt = conn
        .prepare(
            "INSERT INTO price_points (
                id, asset_id, quote_currency, price_time_utc, date_utc, price,
                provider, provider_asset_id, provider_quote_id, granularity,
                price_kind, license_scope, retrieved_at, created_at, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'coingecko', ?7, ?8, 'daily',
                    'daily_point', ?9, ?10, ?11, ?12)
            ON CONFLICT(asset_id, quote_currency, provider, price_time_utc, granularity, price_kind)
            DO UPDATE SET
                date_utc = excluded.date_utc,
                price = excluded.price,
                provider_asset_id = excluded.provider_asset_id,
                provider_quote_id = excluded.provider_quote_id,
                license_scope = excluded.license_scope,
                retrieved_at = excluded.retrieved_at,
                updated_at = excluded.updated_at",
        )
        .map_err(|err| DbError::from_rusqlite_error("prepare daily price upsert", err))?;

    let now = cache_time_text(now);
    for row in rows {
        let price_time_utc = cache_time_text(row.price_time_utc);
        let id = format!(
            "coingecko:{}:{}:{}:daily",
            row.asset_id,
            row.quote_currency.code(),
            price_time_utc
        );
        let price = row.price.to_string();
        let retrieved_at = cache_time_text(row.retrieved_at);
        stmt.execute(params![
            id,
            &row.asset_id,
            row.quote_currency.code(),
            price_time_utc,
            date_text(row.date_utc),
            price,
            &row.provider_asset_id,
            row.provider_quote_id.as_deref(),
            &row.license_scope,
            retrieved_at,
            &now,
            &now,
        ])
        .map_err(|err| DbError::from_rusqlite_error("execute daily price upsert", err))?;
    }

    Ok(())
}

pub(crate) fn latest_coingecko_catalog_retrieved_at(
    conn: &rusqlite::Connection,
) -> Result<Option<DateTime<Utc>>, DbError> {
    let raw = conn
        .query_row(
            "SELECT retrieved_at
             FROM coingecko_asset_catalog
             ORDER BY retrieved_at DESC
             LIMIT 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|err| {
            DbError::from_rusqlite_error("query latest coingecko catalog retrieval", err)
        })?;

    raw.as_deref()
        .map(|value| parse_prices_db_utc(value, "latest coingecko catalog retrieval"))
        .transpose()
}

pub(crate) fn replace_or_upsert_coingecko_catalog_rows(
    conn: &rusqlite::Connection,
    rows: &[CoinGeckoCatalogUpsert],
    retrieved_at: DateTime<Utc>,
) -> Result<(), DbError> {
    if rows.is_empty() {
        return Ok(());
    }

    conn.execute("BEGIN IMMEDIATE", [])
        .map_err(|err| DbError::from_rusqlite_error("begin coingecko catalog upsert", err))?;
    let result = upsert_coingecko_catalog_rows_in_transaction(conn, rows, retrieved_at);
    match result {
        Ok(()) => conn
            .execute("COMMIT", [])
            .map(|_| ())
            .map_err(|err| DbError::from_rusqlite_error("commit coingecko catalog upsert", err)),
        Err(err) => {
            let _ = conn.execute("ROLLBACK", []);
            Err(err)
        }
    }
}

fn upsert_coingecko_catalog_rows_in_transaction(
    conn: &rusqlite::Connection,
    rows: &[CoinGeckoCatalogUpsert],
    retrieved_at: DateTime<Utc>,
) -> Result<(), DbError> {
    let retrieved_at = cache_time_text(retrieved_at);
    let mut stmt = conn
        .prepare(
            "INSERT INTO coingecko_asset_catalog (
                provider_asset_id,
                symbol,
                normalized_symbol,
                name,
                platforms_json,
                status,
                retrieved_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(provider_asset_id)
            DO UPDATE SET
                symbol = excluded.symbol,
                normalized_symbol = excluded.normalized_symbol,
                name = excluded.name,
                platforms_json = excluded.platforms_json,
                status = excluded.status,
                retrieved_at = excluded.retrieved_at",
        )
        .map_err(|err| DbError::from_rusqlite_error("prepare coingecko catalog upsert", err))?;

    for row in rows {
        stmt.execute(params![
            row.provider_asset_id,
            row.symbol,
            row.normalized_symbol,
            row.name,
            row.platforms_json,
            row.status,
            retrieved_at,
        ])
        .map_err(|err| DbError::from_rusqlite_error("upsert coingecko catalog row", err))?;
    }

    Ok(())
}

pub(crate) fn search_coingecko_asset_catalog(
    conn: &rusqlite::Connection,
    query: &str,
    limit: usize,
) -> Result<Vec<CoinGeckoCatalogSearchRow>, DbError> {
    let normalized_query = query.trim().to_ascii_lowercase();
    if normalized_query.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }

    let prefix_query = format!("{normalized_query}%");
    let name_prefix_query = format!("{normalized_query}%");
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let mut stmt = conn
        .prepare(
            "SELECT
                provider_asset_id,
                symbol,
                normalized_symbol,
                name,
                platforms_json,
                status,
                retrieved_at
             FROM coingecko_asset_catalog
             WHERE status = 'active'
                AND (
                    normalized_symbol = ?1
                    OR normalized_symbol LIKE ?2
                    OR lower(name) LIKE ?3
                )
             ORDER BY
                CASE
                    WHEN normalized_symbol = ?1 THEN 0
                    WHEN normalized_symbol LIKE ?2 THEN 1
                    ELSE 2
                END,
                normalized_symbol,
                lower(name),
                provider_asset_id
             LIMIT ?4",
        )
        .map_err(|err| DbError::from_rusqlite_error("prepare coingecko catalog search", err))?;

    let rows = stmt
        .query_map(
            params![normalized_query, prefix_query, name_prefix_query, limit],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .map_err(|err| DbError::from_rusqlite_error("query coingecko catalog search", err))?;

    let mut results = Vec::new();
    for row in rows {
        let (
            provider_asset_id,
            symbol,
            normalized_symbol,
            name,
            platforms_json,
            status,
            retrieved_at,
        ) = row.map_err(|err| DbError::from_rusqlite_error("read coingecko catalog row", err))?;
        results.push(CoinGeckoCatalogSearchRow {
            provider_asset_id,
            symbol,
            normalized_symbol,
            name,
            platforms_json,
            status,
            retrieved_at: parse_prices_db_utc(&retrieved_at, "coingecko catalog search row")?,
        });
    }

    Ok(results)
}

/// The match `WHERE` fragment shared by catalog search and count, mirroring
/// `search_coingecko_asset_catalog` exactly so counts agree with results.
/// Bind order for the three params: (normalized_query, prefix_query, name_prefix_query).
const COINGECKO_MATCH_PREDICATE: &str =
    "(normalized_symbol = ?1 OR normalized_symbol LIKE ?2 OR lower(name) LIKE ?3)";

/// Counts active CoinGecko catalog rows. When `query` is `Some` and non-empty,
/// restricts to rows matching the same conditions as `search_coingecko_asset_catalog`.
pub(crate) fn count_active_coingecko_catalog(
    conn: &rusqlite::Connection,
    query: Option<&str>,
) -> Result<usize, DbError> {
    let normalized_query = query
        .map(|q| q.trim().to_ascii_lowercase())
        .filter(|q| !q.is_empty());
    let count: i64 = match normalized_query {
        Some(q) => {
            let prefix = format!("{q}%");
            let sql = format!(
                "SELECT COUNT(*) FROM coingecko_asset_catalog WHERE status = 'active' AND {COINGECKO_MATCH_PREDICATE}"
            );
            conn.query_row(&sql, params![q, prefix, prefix], |row| row.get(0))
                .map_err(|err| {
                    DbError::from_rusqlite_error("count coingecko catalog matches", err)
                })?
        }
        None => conn
            .query_row(
                "SELECT COUNT(*) FROM coingecko_asset_catalog WHERE status = 'active'",
                [],
                |row| row.get(0),
            )
            .map_err(|err| DbError::from_rusqlite_error("count coingecko catalog", err))?,
    };
    Ok(usize::try_from(count).unwrap_or(0))
}

/// Counts active CoinGecko catalog rows whose `provider_asset_id` is in `ids`,
/// optionally restricted to the same match conditions. Returns 0 for empty `ids`.
///
/// `ids` binds one SQLite variable each; the caller's exclusion set (synced
/// registry + unsynced catalog) is tens of static entries, far below SQLite's
/// 999-variable limit. Chunk the `IN` list if that set ever grows past ~996.
pub(crate) fn count_active_coingecko_in_set(
    conn: &rusqlite::Connection,
    ids: &[String],
    query: Option<&str>,
) -> Result<usize, DbError> {
    if ids.is_empty() {
        return Ok(0);
    }
    let normalized_query = query
        .map(|q| q.trim().to_ascii_lowercase())
        .filter(|q| !q.is_empty());
    let base = if normalized_query.is_some() { 3 } else { 0 };
    let in_placeholders = (0..ids.len())
        .map(|i| format!("?{}", base + i + 1))
        .collect::<Vec<_>>()
        .join(", ");

    let count: i64 = match normalized_query {
        Some(q) => {
            let prefix = format!("{q}%");
            let sql = format!(
                "SELECT COUNT(*) FROM coingecko_asset_catalog \
                 WHERE status = 'active' AND {COINGECKO_MATCH_PREDICATE} \
                 AND provider_asset_id IN ({in_placeholders})"
            );
            let mut sql_params: Vec<&dyn rusqlite::ToSql> = vec![&q, &prefix, &prefix];
            sql_params.extend(ids.iter().map(|id| id as &dyn rusqlite::ToSql));
            conn.query_row(&sql, sql_params.as_slice(), |row| row.get(0))
                .map_err(|err| {
                    DbError::from_rusqlite_error("count coingecko overlap matches", err)
                })?
        }
        None => {
            let sql = format!(
                "SELECT COUNT(*) FROM coingecko_asset_catalog \
                 WHERE status = 'active' AND provider_asset_id IN ({in_placeholders})"
            );
            let sql_params: Vec<&dyn rusqlite::ToSql> =
                ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
            conn.query_row(&sql, sql_params.as_slice(), |row| row.get(0))
                .map_err(|err| DbError::from_rusqlite_error("count coingecko overlap", err))?
        }
    };
    Ok(usize::try_from(count).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rusqlite::params;
    use std::path::{Path, PathBuf};

    fn migrated_memory_connection() -> rusqlite::Connection {
        let mut conn =
            rusqlite::Connection::open_in_memory().expect("in-memory prices db should open");
        super::configure_and_migrate_connection(&mut conn).expect("prices db should migrate");
        conn
    }

    fn object_exists(conn: &rusqlite::Connection, object_type: &str, name: &str) -> bool {
        conn.query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM sqlite_master
                WHERE type = ?1 AND name = ?2
            )",
            params![object_type, name],
            |row| row.get::<_, bool>(0),
        )
        .expect("sqlite_master query should succeed")
    }

    fn column_exists(conn: &rusqlite::Connection, table: &str, column: &str) -> bool {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .expect("pragma should prepare");
        let columns = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .expect("pragma should query")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("columns should load");
        columns.iter().any(|name| name == column)
    }

    fn cleanup_prices_db(path: &Path) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(sqlite_sidecar_path(path, "-wal"));
        let _ = std::fs::remove_file(sqlite_sidecar_path(path, "-shm"));
    }

    fn sqlite_sidecar_path(path: &Path, suffix: &str) -> PathBuf {
        let mut path = path.as_os_str().to_os_string();
        path.push(suffix);
        PathBuf::from(path)
    }

    fn utc(raw: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(raw)
            .expect("test timestamp should parse")
            .with_timezone(&Utc)
    }

    fn usd() -> CurrencyCode {
        CurrencyCode::from_code("USD").expect("USD should parse")
    }

    fn decimal(raw: &str) -> Decimal {
        Decimal::from_str(raw).expect("test decimal should parse")
    }

    fn test_daily_record(
        id: &str,
        provider: &str,
        retrieved_at: DateTime<Utc>,
    ) -> DailyPricePointRecord {
        DailyPricePointRecord {
            id: id.to_string(),
            asset_id: "bitcoin".to_string(),
            quote_currency: usd(),
            price_time_utc: utc("2026-06-10T00:00:00Z"),
            date_utc: NaiveDate::from_ymd_opt(2026, 6, 10).expect("test date"),
            price: decimal("100.00"),
            provider: provider.to_string(),
            provider_asset_id: Some("bitcoin".to_string()),
            provider_quote_id: Some("usd".to_string()),
            license_scope: "public_keyless".to_string(),
            retrieved_at,
        }
    }

    #[test]
    fn daily_price_point_order_prefers_coingecko() {
        let older = utc("2026-06-10T12:00:00Z");
        let newer = utc("2026-06-10T13:00:00Z");
        let rows = vec![
            test_daily_record("row-provider", "other-provider", newer),
            test_daily_record("row-coingecko", "coingecko", older),
        ];

        let selected = select_daily_price_point(rows).expect("selected row");

        assert_eq!(selected.id, "row-coingecko");
        assert_eq!(selected.provider, "coingecko");
    }

    #[test]
    fn daily_price_point_order_uses_stable_fallback_for_unknown_providers() {
        let retrieved_at = utc("2026-06-10T12:00:00Z");
        let rows = vec![
            test_daily_record("row-z", "z-provider", retrieved_at),
            test_daily_record("row-a", "a-provider", retrieved_at),
        ];

        let selected = select_daily_price_point(rows).expect("selected row");

        assert_eq!(selected.id, "row-a");
        assert_eq!(selected.provider, "a-provider");
    }

    #[test]
    fn daily_price_point_order_prefers_newer_within_same_provider() {
        let rows = vec![
            test_daily_record("old", "coingecko", utc("2026-06-10T12:00:00Z")),
            test_daily_record("new", "coingecko", utc("2026-06-10T13:00:00Z")),
        ];

        let selected = select_daily_price_point(rows).expect("selected row");

        assert_eq!(selected.id, "new");
    }

    fn cache_request(asset_id: &str, provider_asset_id: &str) -> CurrentPriceCacheRequest {
        CurrentPriceCacheRequest {
            asset_id: asset_id.to_string(),
            provider_asset_id: provider_asset_id.to_string(),
        }
    }

    fn cache_upsert(
        asset_id: &str,
        provider_asset_id: &str,
        price: &str,
        retrieved_at: DateTime<Utc>,
    ) -> CurrentPriceCacheUpsert {
        CurrentPriceCacheUpsert {
            asset_id: asset_id.to_string(),
            quote_currency: usd(),
            provider: "coingecko".to_string(),
            provider_asset_id: provider_asset_id.to_string(),
            provider_quote_id: Some("usd".to_string()),
            price: decimal(price),
            observed_at: Some(retrieved_at),
            retrieved_at,
            license_scope: "public_keyless".to_string(),
        }
    }

    fn catalog_upsert(
        provider_asset_id: &str,
        symbol: &str,
        name: &str,
        retrieved_at: DateTime<Utc>,
    ) -> CoinGeckoCatalogUpsert {
        CoinGeckoCatalogUpsert {
            provider_asset_id: provider_asset_id.to_string(),
            symbol: symbol.to_string(),
            normalized_symbol: symbol.to_ascii_lowercase(),
            name: name.to_string(),
            platforms_json: Some(r#"{"ethereum":"0xabc"}"#.to_string()),
            status: "active".to_string(),
            retrieved_at,
        }
    }

    #[test]
    fn fresh_prices_db_applies_schema() {
        let conn = migrated_memory_connection();

        for table in [
            "coingecko_asset_catalog",
            "price_points",
            "current_price_cache",
            "historical_price_attempts",
        ] {
            assert!(object_exists(&conn, "table", table), "{table} should exist");
        }
        assert!(
            !object_exists(&conn, "table", "asset_price_sources"),
            "asset_price_sources should be removed by V1"
        );
    }

    #[test]
    fn fresh_prices_db_creates_lookup_indexes() {
        let conn = migrated_memory_connection();

        for index in [
            "idx_coingecko_asset_catalog_symbol",
            "idx_price_points_lookup",
            "idx_price_points_daily_lookup",
            "idx_historical_price_attempts_scheduler",
            "idx_historical_price_attempts_latest",
            "idx_historical_price_attempts_retry_cooldown",
        ] {
            assert!(object_exists(&conn, "index", index), "{index} should exist");
        }
        assert!(
            !object_exists(&conn, "index", "idx_asset_price_sources_provider_asset"),
            "asset_price_sources provider index should be removed"
        );
    }

    #[test]
    fn coingecko_asset_catalog_upsert_writes_and_updates_rows() {
        let conn = migrated_memory_connection();
        let first_retrieved_at = utc("2026-06-06T12:00:00Z");
        let second_retrieved_at = utc("2026-06-07T12:00:00Z");

        replace_or_upsert_coingecko_catalog_rows(
            &conn,
            &[catalog_upsert(
                "usd-coin",
                "usdc",
                "USD Coin",
                first_retrieved_at,
            )],
            first_retrieved_at,
        )
        .expect("first catalog upsert should succeed");
        let mut replacement = catalog_upsert(
            "usd-coin",
            "usdc.e",
            "USD Coin Ethereum",
            second_retrieved_at,
        );
        replacement.normalized_symbol = "usdc.e".to_string();
        replacement.platforms_json = Some(r#"{"ethereum":"0xdef"}"#.to_string());
        replace_or_upsert_coingecko_catalog_rows(&conn, &[replacement], second_retrieved_at)
            .expect("second catalog upsert should succeed");

        let row = search_coingecko_asset_catalog(&conn, "usdc", 10)
            .expect("catalog search should succeed")
            .into_iter()
            .next()
            .expect("updated row should be returned");

        assert_eq!(row.provider_asset_id, "usd-coin");
        assert_eq!(row.symbol, "usdc.e");
        assert_eq!(row.normalized_symbol, "usdc.e");
        assert_eq!(row.name, "USD Coin Ethereum");
        assert_eq!(
            row.platforms_json.as_deref(),
            Some(r#"{"ethereum":"0xdef"}"#)
        );
        assert_eq!(row.status, "active");
        assert_eq!(row.retrieved_at, second_retrieved_at);
        assert_eq!(
            latest_coingecko_catalog_retrieved_at(&conn).expect("latest retrieval should load"),
            Some(second_retrieved_at)
        );
    }

    #[test]
    fn coingecko_asset_catalog_search_orders_exact_before_prefix_and_name() {
        let conn = migrated_memory_connection();
        let retrieved_at = utc("2026-06-06T12:00:00Z");
        let rows = vec![
            catalog_upsert("cardano", "ada", "Cardano", retrieved_at),
            catalog_upsert("adax", "adax", "ADAX", retrieved_at),
            catalog_upsert("foo", "zzz", "Ada Name Fallback", retrieved_at),
            catalog_upsert("inactive-ada", "ada2", "Inactive ADA", retrieved_at),
        ];
        replace_or_upsert_coingecko_catalog_rows(&conn, &rows, retrieved_at)
            .expect("catalog upsert should succeed");
        conn.execute(
            "UPDATE coingecko_asset_catalog SET status = 'inactive' WHERE provider_asset_id = 'inactive-ada'",
            [],
        )
        .expect("fixture update should succeed");

        let results =
            search_coingecko_asset_catalog(&conn, "ada", 10).expect("catalog search should work");
        let ids = results
            .iter()
            .map(|row| row.provider_asset_id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["cardano", "adax", "foo"]);
    }

    #[test]
    fn coingecko_asset_catalog_search_respects_limit_and_blank_query() {
        let conn = migrated_memory_connection();
        let retrieved_at = utc("2026-06-06T12:00:00Z");
        let rows = vec![
            catalog_upsert("ada-one", "ada1", "ADA One", retrieved_at),
            catalog_upsert("ada-two", "ada2", "ADA Two", retrieved_at),
        ];
        replace_or_upsert_coingecko_catalog_rows(&conn, &rows, retrieved_at)
            .expect("catalog upsert should succeed");

        assert_eq!(
            search_coingecko_asset_catalog(&conn, "ada", 1)
                .expect("limited search should work")
                .len(),
            1
        );
        assert!(
            search_coingecko_asset_catalog(&conn, "   ", 10)
                .expect("blank search should work")
                .is_empty()
        );
    }

    #[test]
    fn coingecko_asset_catalog_upsert_rejects_bad_rows() {
        let conn = migrated_memory_connection();
        let retrieved_at = utc("2026-06-06T12:00:00Z");

        let blank_provider_id = catalog_upsert(" ", "btc", "Bitcoin", retrieved_at);
        assert!(
            replace_or_upsert_coingecko_catalog_rows(&conn, &[blank_provider_id], retrieved_at)
                .is_err()
        );

        let mut invalid_status = catalog_upsert("bitcoin", "btc", "Bitcoin", retrieved_at);
        invalid_status.status = "pending".to_string();
        assert!(
            replace_or_upsert_coingecko_catalog_rows(&conn, &[invalid_status], retrieved_at)
                .is_err()
        );

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM coingecko_asset_catalog", [], |row| {
                row.get(0)
            })
            .expect("row count should load");
        assert_eq!(count, 0);
    }

    #[test]
    fn prices_db_uses_asset_id_for_price_rows() {
        let conn = migrated_memory_connection();

        for table in ["price_points", "current_price_cache"] {
            assert!(column_exists(&conn, table, "asset_id"));
            assert!(!column_exists(&conn, table, "subject_type"));
            assert!(!column_exists(&conn, table, "subject_id"));
        }
    }

    #[test]
    fn prices_db_reopen_is_idempotent() {
        let db_path =
            std::env::temp_dir().join(format!("bitgarth-prices-db-{}.sqlite3", ulid::Ulid::new()));
        cleanup_prices_db(&db_path);

        {
            let _conn =
                super::initialize_prices_db_at_path(&db_path).expect("first open should migrate");
        }
        let conn = super::initialize_prices_db_at_path(&db_path).expect("second open should work");
        let migration_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM refinery_schema_history", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("migration history count should load");

        assert_eq!(migration_count, 3);
        drop(conn);
        cleanup_prices_db(&db_path);
    }

    #[test]
    fn current_price_cache_upsert_overwrites_existing_row() {
        let conn = migrated_memory_connection();
        let first_retrieved_at = utc("2026-06-06T12:00:00Z");
        let second_retrieved_at = utc("2026-06-06T12:02:00Z");
        upsert_current_price_cache(
            &conn,
            cache_upsert("bitcoin", "bitcoin", "100.00", first_retrieved_at),
        )
        .expect("first cache upsert should succeed");

        let mut replacement = cache_upsert("bitcoin", "bitcoin", "101.25", second_retrieved_at);
        replacement.provider_quote_id = Some("usd-market".to_string());
        replacement.license_scope = "coingecko_pro_key".to_string();
        upsert_current_price_cache(&conn, replacement).expect("second cache upsert should succeed");

        let row_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM current_price_cache WHERE asset_id = 'bitcoin'",
                [],
                |row| row.get(0),
            )
            .expect("row count should load");
        assert_eq!(row_count, 1);

        let rows = load_fresh_current_price_cache(
            &conn,
            &[cache_request("bitcoin", "bitcoin")],
            usd(),
            "coingecko",
            utc("2026-06-06T12:03:00Z"),
        )
        .expect("cache lookup should succeed");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].price, decimal("101.25"));
        assert_eq!(rows[0].provider_quote_id.as_deref(), Some("usd-market"));
        assert_eq!(rows[0].retrieved_at, second_retrieved_at);
        assert_eq!(rows[0].license_scope, "coingecko_pro_key");
    }

    #[test]
    fn historical_price_attempt_round_trips_latest_covering_span() {
        let conn = migrated_memory_connection();
        let older_attempted_at = utc("2026-07-04T00:00:00Z");
        let newer_attempted_at = utc("2026-07-04T01:00:00Z");
        let from_date = NaiveDate::from_ymd_opt(2026, 7, 1).expect("test date");
        let to_date = NaiveDate::from_ymd_opt(2026, 7, 4).expect("test date");
        let next_retry_after = utc("2026-07-04T02:00:00Z");

        upsert_historical_price_attempt(
            &conn,
            HistoricalPriceAttemptUpsert {
                asset_id: "based-baby".to_string(),
                provider: "coingecko".to_string(),
                from_date,
                to_date,
                status: HistoricalPriceAttemptStatus::SuccessEmpty,
                attempted_at: older_attempted_at,
                rows_returned: 0,
                next_retry_after: None,
                error_code: None,
            },
        )
        .expect("older attempt should insert");
        upsert_historical_price_attempt(
            &conn,
            HistoricalPriceAttemptUpsert {
                asset_id: "based-baby".to_string(),
                provider: "coingecko".to_string(),
                from_date,
                to_date,
                status: HistoricalPriceAttemptStatus::RateLimited,
                attempted_at: newer_attempted_at,
                rows_returned: 0,
                next_retry_after: Some(next_retry_after),
                error_code: Some("rate_limit".to_string()),
            },
        )
        .expect("newer attempt should insert");

        let row = latest_historical_price_attempt(
            &conn,
            &HistoricalPriceAttemptQuery {
                asset_id: "based-baby".to_string(),
                provider: "coingecko".to_string(),
                from_date: NaiveDate::from_ymd_opt(2026, 7, 2).expect("test date"),
                to_date,
            },
        )
        .expect("attempt query should succeed")
        .expect("covering attempt should exist");

        assert_eq!(row.asset_id, "based-baby");
        assert_eq!(row.provider, "coingecko");
        assert_eq!(row.from_date, from_date);
        assert_eq!(row.to_date, to_date);
        assert_eq!(row.status, HistoricalPriceAttemptStatus::RateLimited);
        assert_eq!(row.attempted_at, newer_attempted_at);
        assert_eq!(row.rows_returned, 0);
        assert_eq!(row.next_retry_after, Some(next_retry_after));
        assert_eq!(row.error_code.as_deref(), Some("rate_limit"));

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM historical_price_attempts
                 WHERE provider = 'coingecko'
                   AND asset_id = 'based-baby'
                   AND from_date = '2026-07-01'
                   AND to_date = '2026-07-04'",
                [],
                |row| row.get(0),
            )
            .expect("row count should load");
        assert_eq!(count, 1);
    }

    #[test]
    fn historical_price_cooldown_attempt_finds_recent_non_covering_span() {
        let conn = migrated_memory_connection();

        upsert_historical_price_attempt(
            &conn,
            HistoricalPriceAttemptUpsert {
                asset_id: "based-baby".to_string(),
                provider: "coingecko".to_string(),
                from_date: NaiveDate::from_ymd_opt(2026, 7, 4).expect("test date"),
                to_date: NaiveDate::from_ymd_opt(2026, 7, 10).expect("test date"),
                status: HistoricalPriceAttemptStatus::SuccessEmpty,
                attempted_at: utc("2026-07-11T00:00:00Z"),
                rows_returned: 0,
                next_retry_after: None,
                error_code: None,
            },
        )
        .expect("recent cooldown attempt should insert");
        upsert_historical_price_attempt(
            &conn,
            HistoricalPriceAttemptUpsert {
                asset_id: "based-baby".to_string(),
                provider: "coingecko".to_string(),
                from_date: NaiveDate::from_ymd_opt(2026, 7, 5).expect("test date"),
                to_date: NaiveDate::from_ymd_opt(2026, 7, 11).expect("test date"),
                status: HistoricalPriceAttemptStatus::TransientFailure,
                attempted_at: utc("2026-07-11T00:05:00Z"),
                rows_returned: 0,
                next_retry_after: None,
                error_code: Some("temporary".to_string()),
            },
        )
        .expect("non-cooldown attempt should insert");

        let row = latest_historical_price_cooldown_attempt(
            &conn,
            &HistoricalPriceAttemptCooldownQuery {
                asset_id: "based-baby".to_string(),
                provider: "coingecko".to_string(),
                min_attempted_at: utc("2026-07-10T12:00:00Z"),
            },
        )
        .expect("cooldown lookup should succeed")
        .expect("recent cooldown attempt should exist");

        assert_eq!(row.status, HistoricalPriceAttemptStatus::SuccessEmpty);
        assert_eq!(
            row.from_date,
            NaiveDate::from_ymd_opt(2026, 7, 4).expect("test date")
        );
        assert_eq!(
            row.to_date,
            NaiveDate::from_ymd_opt(2026, 7, 10).expect("test date")
        );
        assert_eq!(row.attempted_at, utc("2026-07-11T00:00:00Z"));

        let stale = latest_historical_price_cooldown_attempt(
            &conn,
            &HistoricalPriceAttemptCooldownQuery {
                asset_id: "based-baby".to_string(),
                provider: "coingecko".to_string(),
                min_attempted_at: utc("2026-07-11T00:00:01Z"),
            },
        )
        .expect("cooldown lookup should succeed");

        assert!(stale.is_none());
    }

    #[test]
    fn historical_price_attempt_status_strings_match_schema() {
        assert_eq!(
            HistoricalPriceAttemptStatus::SuccessWithPrices.as_str(),
            "success_with_prices"
        );
        assert_eq!(
            HistoricalPriceAttemptStatus::SuccessEmpty.as_str(),
            "success_empty"
        );
        assert_eq!(
            HistoricalPriceAttemptStatus::TransientFailure.as_str(),
            "transient_failure"
        );
        assert_eq!(
            HistoricalPriceAttemptStatus::RateLimited.as_str(),
            "rate_limited"
        );
        assert_eq!(
            HistoricalPriceAttemptStatus::PermanentFailure.as_str(),
            "permanent_failure"
        );
        assert_eq!(
            parse_historical_price_attempt_status("success_with_prices")
                .expect("status should parse"),
            HistoricalPriceAttemptStatus::SuccessWithPrices
        );
    }

    #[test]
    fn historical_price_attempt_status_constraint_rejects_invalid_status() {
        let conn = migrated_memory_connection();
        let now = cache_time_text(utc("2026-07-04T01:00:00Z"));
        let result = conn.execute(
            "INSERT INTO historical_price_attempts (
                id,
                asset_id,
                provider,
                from_date,
                to_date,
                status,
                attempted_at,
                rows_returned,
                next_retry_after,
                error_code,
                created_at,
                updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                "coingecko:based-baby:2026-07-01:2026-07-04",
                "based-baby",
                "coingecko",
                "2026-07-01",
                "2026-07-04",
                "bogus",
                &now,
                0_i64,
                Option::<&str>::None,
                Option::<&str>::None,
                &now,
                &now,
            ],
        );

        assert!(result.is_err());
    }

    #[test]
    fn historical_price_attempt_rejects_invalid_span() {
        let conn = migrated_memory_connection();
        let from_date = NaiveDate::from_ymd_opt(2026, 7, 4).expect("test date");
        let to_date = NaiveDate::from_ymd_opt(2026, 7, 1).expect("test date");

        let upsert_result = upsert_historical_price_attempt(
            &conn,
            HistoricalPriceAttemptUpsert {
                asset_id: "based-baby".to_string(),
                provider: "coingecko".to_string(),
                from_date,
                to_date,
                status: HistoricalPriceAttemptStatus::TransientFailure,
                attempted_at: utc("2026-07-04T01:00:00Z"),
                rows_returned: 0,
                next_retry_after: None,
                error_code: Some("bad_span".to_string()),
            },
        );
        assert!(upsert_result.is_err());

        let lookup_result = latest_historical_price_attempt(
            &conn,
            &HistoricalPriceAttemptQuery {
                asset_id: "based-baby".to_string(),
                provider: "coingecko".to_string(),
                from_date,
                to_date,
            },
        );
        assert!(lookup_result.is_err());

        let now = cache_time_text(utc("2026-07-04T01:00:00Z"));
        let sql_result = conn.execute(
            "INSERT INTO historical_price_attempts (
                id,
                asset_id,
                provider,
                from_date,
                to_date,
                status,
                attempted_at,
                rows_returned,
                created_at,
                updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                "coingecko:based-baby:2026-07-04:2026-07-01",
                "based-baby",
                "coingecko",
                "2026-07-04",
                "2026-07-01",
                "transient_failure",
                &now,
                0_i64,
                &now,
                &now,
            ],
        );
        assert!(sql_result.is_err());
    }

    #[test]
    fn historical_price_attempt_stale_upsert_keeps_newer_metadata() {
        let conn = migrated_memory_connection();
        let from_date = NaiveDate::from_ymd_opt(2026, 7, 1).expect("test date");
        let to_date = NaiveDate::from_ymd_opt(2026, 7, 4).expect("test date");
        let newer_attempted_at = utc("2026-07-04T02:00:00Z");
        let older_attempted_at = utc("2026-07-04T01:00:00Z");
        let newer_next_retry_after = utc("2026-07-04T03:00:00Z");

        upsert_historical_price_attempt(
            &conn,
            HistoricalPriceAttemptUpsert {
                asset_id: "based-baby".to_string(),
                provider: "coingecko".to_string(),
                from_date,
                to_date,
                status: HistoricalPriceAttemptStatus::RateLimited,
                attempted_at: newer_attempted_at,
                rows_returned: 0,
                next_retry_after: Some(newer_next_retry_after),
                error_code: Some("rate_limit".to_string()),
            },
        )
        .expect("newer attempt should insert");
        upsert_historical_price_attempt(
            &conn,
            HistoricalPriceAttemptUpsert {
                asset_id: "based-baby".to_string(),
                provider: "coingecko".to_string(),
                from_date,
                to_date,
                status: HistoricalPriceAttemptStatus::SuccessWithPrices,
                attempted_at: older_attempted_at,
                rows_returned: 7,
                next_retry_after: None,
                error_code: None,
            },
        )
        .expect("older stale attempt should not overwrite");

        let row = latest_historical_price_attempt(
            &conn,
            &HistoricalPriceAttemptQuery {
                asset_id: "based-baby".to_string(),
                provider: "coingecko".to_string(),
                from_date,
                to_date,
            },
        )
        .expect("attempt query should succeed")
        .expect("attempt should exist");

        assert_eq!(row.status, HistoricalPriceAttemptStatus::RateLimited);
        assert_eq!(row.attempted_at, newer_attempted_at);
        assert_eq!(row.rows_returned, 0);
        assert_eq!(row.next_retry_after, Some(newer_next_retry_after));
        assert_eq!(row.error_code.as_deref(), Some("rate_limit"));
    }

    #[test]
    fn load_daily_price_dates_returns_matching_dates_only() {
        let conn = migrated_memory_connection();
        let retrieved_at = Utc
            .with_ymd_and_hms(2026, 6, 10, 12, 0, 0)
            .single()
            .expect("test time");

        upsert_daily_price_points(
            &conn,
            &[
                DailyPricePointUpsert {
                    asset_id: "bitcoin".to_string(),
                    quote_currency: usd(),
                    price_time_utc: Utc
                        .with_ymd_and_hms(2026, 6, 9, 0, 0, 0)
                        .single()
                        .expect("test time"),
                    date_utc: NaiveDate::from_ymd_opt(2026, 6, 9).expect("test date"),
                    price: Decimal::new(1000000, 2),
                    provider_asset_id: "bitcoin".to_string(),
                    provider_quote_id: Some("usd".to_string()),
                    license_scope: "public_keyless".to_string(),
                    retrieved_at,
                },
                DailyPricePointUpsert {
                    asset_id: "bitcoin".to_string(),
                    quote_currency: usd(),
                    price_time_utc: Utc
                        .with_ymd_and_hms(2026, 6, 8, 0, 0, 0)
                        .single()
                        .expect("test time"),
                    date_utc: NaiveDate::from_ymd_opt(2026, 6, 8).expect("test date"),
                    price: Decimal::new(990000, 2),
                    provider_asset_id: "wrapped-bitcoin".to_string(),
                    provider_quote_id: Some("usd".to_string()),
                    license_scope: "public_keyless".to_string(),
                    retrieved_at,
                },
                DailyPricePointUpsert {
                    asset_id: "bitcoin".to_string(),
                    quote_currency: usd(),
                    price_time_utc: Utc
                        .with_ymd_and_hms(2026, 6, 7, 0, 0, 0)
                        .single()
                        .expect("test time"),
                    date_utc: NaiveDate::from_ymd_opt(2026, 6, 7).expect("test date"),
                    price: Decimal::new(980000, 2),
                    provider_asset_id: "bitcoin".to_string(),
                    provider_quote_id: Some("usd".to_string()),
                    license_scope: "coingecko_pro_key".to_string(),
                    retrieved_at,
                },
                DailyPricePointUpsert {
                    asset_id: "bitcoin".to_string(),
                    quote_currency: usd(),
                    price_time_utc: Utc
                        .with_ymd_and_hms(2026, 7, 1, 0, 0, 0)
                        .single()
                        .expect("test time"),
                    date_utc: NaiveDate::from_ymd_opt(2026, 7, 1).expect("test date"),
                    price: Decimal::new(1010000, 2),
                    provider_asset_id: "bitcoin".to_string(),
                    provider_quote_id: Some("usd".to_string()),
                    license_scope: "public_keyless".to_string(),
                    retrieved_at,
                },
            ],
        )
        .expect("upsert should succeed");

        let dates = load_daily_price_dates(
            &conn,
            &DailyPriceDateQuery {
                asset_id: "bitcoin".to_string(),
                quote_currency: usd(),
                provider_asset_id: "bitcoin".to_string(),
                license_scope: "public_keyless".to_string(),
                start: NaiveDate::from_ymd_opt(2026, 6, 1).expect("test date"),
                end: NaiveDate::from_ymd_opt(2026, 6, 30).expect("test date"),
            },
        )
        .expect("date query should succeed");

        assert_eq!(
            dates,
            vec![NaiveDate::from_ymd_opt(2026, 6, 9).expect("test date")]
        );
    }

    #[test]
    fn daily_price_upsert_is_idempotent() {
        let conn = migrated_memory_connection();
        let retrieved_at = Utc
            .with_ymd_and_hms(2026, 6, 10, 12, 0, 0)
            .single()
            .expect("test time");
        let row = DailyPricePointUpsert {
            asset_id: "ethereum".to_string(),
            quote_currency: usd(),
            price_time_utc: Utc
                .with_ymd_and_hms(2026, 6, 9, 0, 0, 0)
                .single()
                .expect("test time"),
            date_utc: NaiveDate::from_ymd_opt(2026, 6, 9).expect("test date"),
            price: Decimal::new(250000, 2),
            provider_asset_id: "ethereum".to_string(),
            provider_quote_id: Some("usd".to_string()),
            license_scope: "public_keyless".to_string(),
            retrieved_at,
        };

        upsert_daily_price_points(&conn, std::slice::from_ref(&row))
            .expect("first upsert should succeed");
        upsert_daily_price_points(&conn, &[row]).expect("second upsert should succeed");

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM price_points WHERE asset_id = 'ethereum'",
                [],
                |row| row.get(0),
            )
            .expect("count should load");
        assert_eq!(count, 1);
    }

    #[test]
    fn current_price_cache_dedupes_duplicate_lookup_requests() {
        let conn = migrated_memory_connection();
        let retrieved_at = utc("2026-06-06T12:00:00Z");
        upsert_current_price_cache(
            &conn,
            cache_upsert("bitcoin", "bitcoin", "100.00", retrieved_at),
        )
        .expect("cache upsert should succeed");

        let rows = load_fresh_current_price_cache(
            &conn,
            &[
                cache_request("bitcoin", "bitcoin"),
                cache_request("bitcoin", "bitcoin"),
            ],
            usd(),
            "coingecko",
            utc("2026-06-06T12:01:00Z"),
        )
        .expect("cache lookup should succeed");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].asset_id, "bitcoin");
        assert_eq!(rows[0].price, decimal("100.00"));
    }

    #[test]
    fn current_price_cache_preserves_subsecond_freshness_boundary() {
        let conn = migrated_memory_connection();
        let now = utc("2026-06-06T12:00:00.500Z");
        let retrieved_at = now - CURRENT_PRICE_CACHE_TTL + Duration::milliseconds(250);
        upsert_current_price_cache(
            &conn,
            cache_upsert("bitcoin", "bitcoin", "100.00", retrieved_at),
        )
        .expect("cache upsert should succeed");

        let rows = load_fresh_current_price_cache(
            &conn,
            &[cache_request("bitcoin", "bitcoin")],
            usd(),
            "coingecko",
            now,
        )
        .expect("cache lookup should succeed");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].retrieved_at, retrieved_at);
    }

    #[test]
    fn current_price_cache_reads_fresh_rows_and_skips_stale_rows() {
        let conn = migrated_memory_connection();
        let now = utc("2026-06-06T12:00:00Z");
        let fresh_at = now - CURRENT_PRICE_CACHE_TTL + Duration::seconds(1);
        let stale_at = now - CURRENT_PRICE_CACHE_TTL - Duration::seconds(1);

        upsert_current_price_cache(
            &conn,
            cache_upsert("bitcoin", "bitcoin", "100.00", fresh_at),
        )
        .expect("fresh cache upsert should succeed");
        upsert_current_price_cache(
            &conn,
            cache_upsert("ethereum", "ethereum", "50.00", stale_at),
        )
        .expect("stale cache upsert should succeed");

        let rows = load_fresh_current_price_cache(
            &conn,
            &[
                cache_request("bitcoin", "bitcoin"),
                cache_request("ethereum", "ethereum"),
            ],
            usd(),
            "coingecko",
            now,
        )
        .expect("cache lookup should succeed");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].asset_id, "bitcoin");
        assert_eq!(rows[0].price, decimal("100.00"));
    }

    #[test]
    fn current_price_cache_skips_invalid_stored_decimal_and_timestamp_rows() {
        let conn = migrated_memory_connection();
        let now = utc("2026-06-06T12:00:00Z");
        conn.execute(
            "INSERT INTO current_price_cache (
                asset_id,
                quote_currency,
                provider,
                provider_asset_id,
                price,
                retrieved_at,
                license_scope,
                created_at,
                updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                "bad-decimal",
                "USD",
                "coingecko",
                "bad-decimal",
                "not-a-decimal",
                cache_time_text(now),
                "public_keyless",
                cache_time_text(now),
                cache_time_text(now),
            ],
        )
        .expect("invalid decimal fixture should insert");
        conn.execute(
            "INSERT INTO current_price_cache (
                asset_id,
                quote_currency,
                provider,
                provider_asset_id,
                price,
                retrieved_at,
                license_scope,
                created_at,
                updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                "bad-time",
                "USD",
                "coingecko",
                "bad-time",
                "42.00",
                "not-a-timestamp",
                "public_keyless",
                cache_time_text(now),
                cache_time_text(now),
            ],
        )
        .expect("invalid timestamp fixture should insert");

        let rows = load_fresh_current_price_cache(
            &conn,
            &[
                cache_request("bad-decimal", "bad-decimal"),
                cache_request("bad-time", "bad-time"),
            ],
            usd(),
            "coingecko",
            now,
        )
        .expect("cache lookup should succeed");

        assert!(rows.is_empty());
    }

    #[test]
    fn current_price_cache_rejects_blank_asset_id() {
        let conn = migrated_memory_connection();
        let result = conn.execute(
            "INSERT INTO current_price_cache (
                asset_id,
                quote_currency,
                provider,
                provider_asset_id,
                price,
                retrieved_at,
                license_scope,
                created_at,
                updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                " ",
                "USD",
                "coingecko",
                "bitcoin",
                "100.00",
                "2026-06-06T00:00:00Z",
                "public_keyless",
                "2026-06-06T00:00:00Z",
                "2026-06-06T00:00:00Z"
            ],
        );

        assert!(result.is_err());
    }

    #[test]
    fn coingecko_asset_catalog_rejects_null_provider_asset_id() {
        let conn = migrated_memory_connection();
        let result = conn.execute(
            "INSERT INTO coingecko_asset_catalog (
                provider_asset_id,
                symbol,
                normalized_symbol,
                name,
                status,
                retrieved_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                Option::<&str>::None,
                "btc",
                "btc",
                "Bitcoin",
                "active",
                "2026-06-06T00:00:00Z"
            ],
        );

        assert!(result.is_err());
    }

    #[test]
    fn coingecko_asset_catalog_rejects_blank_provider_asset_id() {
        let conn = migrated_memory_connection();
        let result = conn.execute(
            "INSERT INTO coingecko_asset_catalog (
                provider_asset_id,
                symbol,
                normalized_symbol,
                name,
                status,
                retrieved_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                " ",
                "btc",
                "btc",
                "Bitcoin",
                "active",
                "2026-06-06T00:00:00Z"
            ],
        );

        assert!(result.is_err());
    }

    #[test]
    fn price_points_rejects_null_id() {
        let conn = migrated_memory_connection();
        let result = conn.execute(
            "INSERT INTO price_points (
                id,
                asset_id,
                quote_currency,
                price_time_utc,
                price,
                provider,
                granularity,
                price_kind,
                license_scope,
                retrieved_at,
                created_at,
                updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                Option::<&str>::None,
                "bitcoin",
                "USD",
                "2026-06-06T00:00:00Z",
                "100.00",
                "coingecko",
                "point",
                "current_snapshot",
                "public_keyless",
                "2026-06-06T00:00:00Z",
                "2026-06-06T00:00:00Z",
                "2026-06-06T00:00:00Z"
            ],
        );

        assert!(result.is_err());
    }

    #[test]
    fn price_points_rejects_blank_id() {
        let conn = migrated_memory_connection();
        let result = conn.execute(
            "INSERT INTO price_points (
                id,
                asset_id,
                quote_currency,
                price_time_utc,
                price,
                provider,
                granularity,
                price_kind,
                license_scope,
                retrieved_at,
                created_at,
                updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                " ",
                "bitcoin",
                "USD",
                "2026-06-06T00:00:00Z",
                "100.00",
                "coingecko",
                "point",
                "current_snapshot",
                "public_keyless",
                "2026-06-06T00:00:00Z",
                "2026-06-06T00:00:00Z",
                "2026-06-06T00:00:00Z"
            ],
        );

        assert!(result.is_err());
    }

    #[test]
    fn count_active_coingecko_catalog_counts_active_rows_and_matches() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let conn = crate::db::initialize_prices_db().expect("prices db should initialize");
        let retrieved_at = chrono::DateTime::parse_from_rfc3339("2026-06-08T12:00:00Z")
            .expect("timestamp")
            .with_timezone(&chrono::Utc);
        replace_or_upsert_coingecko_catalog_rows(
            &conn,
            &[
                CoinGeckoCatalogUpsert {
                    provider_asset_id: "bitcoin".to_string(),
                    symbol: "btc".to_string(),
                    normalized_symbol: "btc".to_string(),
                    name: "Bitcoin".to_string(),
                    platforms_json: None,
                    status: "active".to_string(),
                    retrieved_at,
                },
                CoinGeckoCatalogUpsert {
                    provider_asset_id: "bitcoin-cash".to_string(),
                    symbol: "bch".to_string(),
                    normalized_symbol: "bch".to_string(),
                    name: "Bitcoin Cash".to_string(),
                    platforms_json: None,
                    status: "active".to_string(),
                    retrieved_at,
                },
            ],
            retrieved_at,
        )
        .expect("seed catalog rows");

        assert_eq!(count_active_coingecko_catalog(&conn, None).unwrap(), 2);
        // Name prefix "bitcoin" matches both; symbol "btc" matches one.
        assert_eq!(
            count_active_coingecko_catalog(&conn, Some("bitcoin")).unwrap(),
            2
        );
        assert_eq!(
            count_active_coingecko_catalog(&conn, Some("btc")).unwrap(),
            1
        );
        assert_eq!(
            count_active_coingecko_catalog(&conn, Some("   ")).unwrap(),
            2
        );

        let ids = vec!["bitcoin".to_string(), "missing".to_string()];
        assert_eq!(count_active_coingecko_in_set(&conn, &ids, None).unwrap(), 1);
        assert_eq!(
            count_active_coingecko_in_set(&conn, &ids, Some("btc")).unwrap(),
            1
        );
        assert_eq!(
            count_active_coingecko_in_set(&conn, &ids, Some("bch")).unwrap(),
            0
        );
        assert_eq!(count_active_coingecko_in_set(&conn, &[], None).unwrap(), 0);
    }

    #[test]
    fn count_active_coingecko_catalog_ignores_inactive_rows() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let conn = crate::db::initialize_prices_db().expect("prices db should initialize");
        let retrieved_at = chrono::DateTime::parse_from_rfc3339("2026-06-08T12:00:00Z")
            .expect("timestamp")
            .with_timezone(&chrono::Utc);
        replace_or_upsert_coingecko_catalog_rows(
            &conn,
            &[CoinGeckoCatalogUpsert {
                provider_asset_id: "inactive-coin".to_string(),
                symbol: "inc".to_string(),
                normalized_symbol: "inc".to_string(),
                name: "Inactive Coin".to_string(),
                platforms_json: None,
                status: "active".to_string(),
                retrieved_at,
            }],
            retrieved_at,
        )
        .expect("seed catalog row");
        conn.execute(
            "UPDATE coingecko_asset_catalog SET status = 'inactive' WHERE provider_asset_id = 'inactive-coin'",
            [],
        )
        .expect("mark inactive");

        assert_eq!(count_active_coingecko_catalog(&conn, None).unwrap(), 0);
        assert_eq!(
            count_active_coingecko_catalog(&conn, Some("inc")).unwrap(),
            0
        );
    }
}
