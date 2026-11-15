//! Read-only daily price history for the per-user Prices page.

use super::{
    DailyPricePointRecord, date_text, raw_daily_price_point_to_record,
    read_raw_daily_price_point_row, select_daily_price_point,
};
use crate::db::error::DbError;
use crate::models::CurrencyCode;
use chrono::NaiveDate;
use rusqlite::{Connection, params};

/// Saved CoinGecko daily points for one asset/currency pair, ascending, one per
/// UTC date. Only CoinGecko rows: the Prices page gates each source on the
/// user's own toggle for that source. All credential scopes are included; when
/// a date has several rows the report's `select_daily_price_point` rule picks
/// the one shown.
pub(crate) fn load_daily_price_history(
    conn: &Connection,
    asset_id: &str,
    quote_currency: CurrencyCode,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<DailyPricePointRecord>, DbError> {
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
               AND date_utc BETWEEN ?3 AND ?4
               AND granularity = 'daily'
               AND price_kind = 'daily_point'
               AND provider = 'coingecko'
             ORDER BY date_utc ASC",
        )
        .map_err(|err| DbError::from_rusqlite_error("prepare daily price history", err))?;

    let rows = stmt
        .query_map(
            params![
                asset_id,
                quote_currency.code(),
                date_text(from),
                date_text(to)
            ],
            read_raw_daily_price_point_row,
        )
        .map_err(|err| DbError::from_rusqlite_error("query daily price history", err))?;

    let mut history = Vec::new();
    let mut same_date: Vec<DailyPricePointRecord> = Vec::new();
    for row in rows {
        let raw =
            row.map_err(|err| DbError::from_rusqlite_error("read daily price history row", err))?;
        let record = raw_daily_price_point_to_record(raw)?;
        if same_date
            .first()
            .is_some_and(|first| first.date_utc != record.date_utc)
        {
            history.extend(select_daily_price_point(std::mem::take(&mut same_date)));
        }
        same_date.push(record);
    }
    history.extend(select_daily_price_point(same_date));
    Ok(history)
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let mut conn = Connection::open_in_memory().expect("in-memory prices db");
        super::super::configure_and_migrate_connection(&mut conn).expect("migrate");
        conn
    }

    /// (id, asset, quote, price_time_utc, kind, license_scope, retrieved_at)
    type Row<'a> = (
        &'a str,
        &'a str,
        &'a str,
        &'a str,
        &'a str,
        &'a str,
        &'a str,
    );

    fn insert(conn: &Connection, rows: &[Row<'_>]) {
        for (id, asset, quote, time, kind, scope, retrieved_at) in rows {
            let granularity = if *kind == "daily_point" {
                "daily"
            } else {
                "point"
            };
            conn.execute(
                "INSERT INTO price_points
                 (id, asset_id, quote_currency, price_time_utc, date_utc, price, provider,
                  provider_asset_id, granularity, price_kind, license_scope, retrieved_at,
                  created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, '1', 'coingecko', ?2, ?6, ?7, ?8, ?9, ?9, ?9)",
                params![
                    id,
                    asset,
                    quote,
                    time,
                    &time[..10],
                    granularity,
                    kind,
                    scope,
                    retrieved_at
                ],
            )
            .expect("insert price point");
        }
    }

    fn d(raw: &str) -> NaiveDate {
        NaiveDate::parse_from_str(raw, "%Y-%m-%d").expect("date")
    }

    fn usd() -> CurrencyCode {
        CurrencyCode::from_code("USD").expect("USD")
    }

    #[test]
    fn history_filters_pair_and_kind_and_keeps_one_row_per_date() {
        let conn = conn();
        let t = "2026-11-01T00:00:00Z";
        let (daily, keyless, pro) = ("daily_point", "public_keyless", "coingecko_pro_key");
        insert(
            &conn,
            &[
                (
                    "a",
                    "bitcoin",
                    "USD",
                    "2026-11-01T00:00:00Z",
                    daily,
                    keyless,
                    t,
                ),
                // Legacy Pro row with a noon timestamp on the same date, retrieved later: preferred.
                (
                    "b",
                    "bitcoin",
                    "USD",
                    "2026-11-01T12:00:00Z",
                    daily,
                    pro,
                    "2026-11-02T00:00:00Z",
                ),
                ("c", "bitcoin", "USD", "2026-11-03T00:00:00Z", daily, pro, t),
                (
                    "d",
                    "bitcoin",
                    "EUR",
                    "2026-11-02T00:00:00Z",
                    daily,
                    keyless,
                    t,
                ),
                (
                    "e",
                    "ethereum",
                    "USD",
                    "2026-11-02T00:00:00Z",
                    daily,
                    keyless,
                    t,
                ),
                (
                    "f",
                    "bitcoin",
                    "USD",
                    "2026-11-02T08:00:00Z",
                    "current_snapshot",
                    keyless,
                    t,
                ),
                (
                    "g",
                    "bitcoin",
                    "USD",
                    "2026-11-05T00:00:00Z",
                    daily,
                    keyless,
                    t,
                ),
            ],
        );

        let history =
            load_daily_price_history(&conn, "bitcoin", usd(), d("2026-11-01"), d("2026-11-03"))
                .expect("history");

        let ids = history
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["b", "c"]);
    }
}
