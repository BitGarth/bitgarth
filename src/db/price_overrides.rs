use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use rust_decimal::Decimal;
use std::str::FromStr;
use ulid::Ulid;

use crate::db::{DbError, with_user_db, with_user_db_mut};
use crate::models::{CurrencyCode, UserId};
use crate::services::price_overrides::{NewPriceOverride, OverrideLookup, PriceSubject};

mod storage_codec {
    use crate::db::DbError;
    use crate::services::price_overrides::PriceSubject;

    pub(super) fn to_storage(subject: &PriceSubject) -> String {
        match subject {
            PriceSubject::CatalogAsset(id) => id.as_str().to_string(),
        }
    }

    pub(super) fn from_storage(asset_id: &str) -> Result<PriceSubject, DbError> {
        let key = crate::asset_views::CatalogAssetKey::try_new(asset_id.to_string())
            .map_err(|_| DbError::new("invalid catalog asset key in storage"))?;
        Ok(PriceSubject::CatalogAsset(key))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PriceOverrideRecord {
    pub(crate) id: String,
    pub(crate) subject: PriceSubject,
    pub(crate) quote_currency: CurrencyCode,
    pub(crate) price_time_utc: DateTime<Utc>,
    pub(crate) price: Decimal,
    pub(crate) source_note: Option<String>,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
}

fn parse_utc(value: &str, field: &str) -> Result<DateTime<Utc>, DbError> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|err| DbError::new(format!("Invalid {field} in price override row: {err}")))
}

fn parse_decimal(value: &str) -> Result<Decimal, DbError> {
    Decimal::from_str(value).map_err(|err| {
        DbError::new(format!(
            "Invalid price decimal in price override row: {err}"
        ))
    })
}

struct CatalogRawRow {
    id: String,
    asset_id: String,
    quote_currency: String,
    price_time_utc: String,
    price: String,
    source_note: Option<String>,
    created_at: String,
    updated_at: String,
}

fn read_catalog_raw_row(row: &rusqlite::Row) -> rusqlite::Result<CatalogRawRow> {
    Ok(CatalogRawRow {
        id: row.get(0)?,
        asset_id: row.get(1)?,
        quote_currency: row.get(2)?,
        price_time_utc: row.get(3)?,
        price: row.get(4)?,
        source_note: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

fn parse_quote_currency(raw: &str) -> Result<CurrencyCode, DbError> {
    CurrencyCode::from_code(raw).ok_or_else(|| {
        DbError::new(format!(
            "Invalid quote currency in price override row: {raw}"
        ))
    })
}

fn catalog_raw_to_record(raw: CatalogRawRow) -> Result<PriceOverrideRecord, DbError> {
    Ok(PriceOverrideRecord {
        id: raw.id,
        subject: storage_codec::from_storage(&raw.asset_id)?,
        quote_currency: parse_quote_currency(&raw.quote_currency)?,
        price_time_utc: parse_utc(&raw.price_time_utc, "price_time_utc")?,
        price: parse_decimal(&raw.price)?,
        source_note: raw.source_note,
        created_at: parse_utc(&raw.created_at, "created_at")?,
        updated_at: parse_utc(&raw.updated_at, "updated_at")?,
    })
}

const CATALOG_SELECT_COLUMNS: &str =
    "id, asset_id, quote_currency, price_time_utc, price, source_note, created_at, updated_at";

fn lookup_exact_on_connection(
    conn: &rusqlite::Connection,
    subject: &PriceSubject,
    quote_currency: CurrencyCode,
    at: DateTime<Utc>,
) -> Result<Option<PriceOverrideRecord>, DbError> {
    let at_text = at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let asset_id = storage_codec::to_storage(subject);
    let raw = conn
        .query_row(
            &format!(
                "SELECT {CATALOG_SELECT_COLUMNS} FROM user_price_overrides \
                 WHERE asset_id = ?1 \
                 AND quote_currency = ?2 AND price_time_utc = ?3"
            ),
            params![asset_id, quote_currency.code(), at_text],
            read_catalog_raw_row,
        )
        .optional()
        .map_err(|err| DbError::new(format!("Failed to lookup price override: {err}")))?;
    raw.map(catalog_raw_to_record).transpose()
}

pub(crate) fn upsert_price_override(
    user_id: UserId,
    input: NewPriceOverride,
    now: DateTime<Utc>,
) -> Result<PriceOverrideRecord, DbError> {
    let id = Ulid::new().to_string();
    let price_time_utc = input
        .price_time_utc
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let price = input.price.to_string();
    let now_text = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let asset_id = storage_codec::to_storage(&input.subject);
    with_user_db_mut(user_id, |conn| {
        conn.execute(
            "INSERT INTO user_price_overrides (
                id, asset_id, quote_currency, price_time_utc,
                price, source_note, created_at, updated_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(asset_id, quote_currency, price_time_utc)
             DO UPDATE SET
                price = excluded.price,
                source_note = excluded.source_note,
                updated_at = excluded.updated_at",
            params![
                id,
                asset_id,
                input.quote_currency.code(),
                price_time_utc,
                price,
                input.source_note,
                now_text,
                now_text,
            ],
        )
        .map_err(|err| DbError::from_rusqlite_error("upsert catalog price override", err))?;
        lookup_exact_on_connection(
            conn,
            &input.subject,
            input.quote_currency,
            input.price_time_utc,
        )?
        .ok_or_else(|| DbError::new("Upserted price override could not be loaded"))
    })
}

pub(crate) fn delete_price_override(
    user_id: UserId,
    subject: PriceSubject,
    quote_currency: CurrencyCode,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    let at_text = at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let asset_id = storage_codec::to_storage(&subject);
    with_user_db_mut(user_id, |conn| {
        conn.execute(
            "DELETE FROM user_price_overrides \
             WHERE asset_id = ?1 \
             AND quote_currency = ?2 AND price_time_utc = ?3",
            params![asset_id, quote_currency.code(), at_text],
        )
        .map_err(|err| DbError::from_rusqlite_error("delete catalog price override", err))?;
        Ok(())
    })
}

pub(crate) fn lookup_price_override(
    user_id: UserId,
    subject: PriceSubject,
    quote_currency: CurrencyCode,
    mode: OverrideLookup,
) -> Result<Option<PriceOverrideRecord>, DbError> {
    let asset_id = storage_codec::to_storage(&subject);
    with_user_db(user_id, |conn| match mode {
        OverrideLookup::SameDayLatestAtOrBefore {
            at,
            local_day_start_utc,
            next_local_day_start_utc,
        } => {
            let day_start_text =
                local_day_start_utc.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            let next_day_start_text =
                next_local_day_start_utc.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            let at_text = at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            let raw = conn
                .query_row(
                    &format!(
                        "SELECT {CATALOG_SELECT_COLUMNS} FROM user_price_overrides \
                         WHERE asset_id = ?1 \
                         AND quote_currency = ?2 \
                         AND price_time_utc >= ?3 AND price_time_utc < ?4 \
                         AND price_time_utc <= ?5 \
                         ORDER BY price_time_utc DESC LIMIT 1"
                    ),
                    params![
                        asset_id,
                        quote_currency.code(),
                        day_start_text,
                        next_day_start_text,
                        at_text,
                    ],
                    read_catalog_raw_row,
                )
                .optional()
                .map_err(|err| DbError::new(format!("Failed to lookup price override: {err}")))?;
            raw.map(catalog_raw_to_record).transpose()
        }
    })
}

pub(crate) fn list_price_overrides_in_range(
    user_id: UserId,
    subject: PriceSubject,
    quote_currency: CurrencyCode,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<Vec<PriceOverrideRecord>, DbError> {
    let from_text = from.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let to_text = to.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let asset_id = storage_codec::to_storage(&subject);
    with_user_db(user_id, |conn| {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {CATALOG_SELECT_COLUMNS} FROM user_price_overrides \
                 WHERE asset_id = ?1 \
                 AND quote_currency = ?2 \
                 AND price_time_utc >= ?3 AND price_time_utc <= ?4 \
                 ORDER BY price_time_utc"
            ))
            .map_err(|err| {
                DbError::new(format!("Failed to prepare list price overrides: {err}"))
            })?;
        let raw_rows = stmt
            .query_map(
                params![asset_id, quote_currency.code(), from_text, to_text],
                read_catalog_raw_row,
            )
            .map_err(|err| DbError::new(format!("Failed to query list price overrides: {err}")))?;
        let mut records = Vec::new();
        for raw in raw_rows {
            let raw = raw
                .map_err(|err| DbError::new(format!("Failed to read price override row: {err}")))?;
            records.push(catalog_raw_to_record(raw)?);
        }
        Ok(records)
    })
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;
    use crate::asset_views::CatalogAssetKey;
    use crate::db::{acquire_test_runtime, initialize_user_db_for_test};

    fn fixed_utc(
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
    ) -> DateTime<Utc> {
        DateTime::from_naive_utc_and_offset(
            chrono::NaiveDate::from_ymd_opt(year, month, day)
                .expect("valid date")
                .and_hms_opt(hour, minute, second)
                .expect("valid time"),
            Utc,
        )
    }

    fn btc() -> PriceSubject {
        PriceSubject::CatalogAsset(CatalogAssetKey::try_new("bitcoin").expect("valid key"))
    }

    fn usd() -> CurrencyCode {
        CurrencyCode::from_code("USD").expect("USD should parse")
    }

    fn eur() -> CurrencyCode {
        CurrencyCode::from_code("EUR").expect("EUR should parse")
    }

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).expect("valid decimal")
    }

    fn same_utc_day_lookup(at: DateTime<Utc>) -> OverrideLookup {
        let day_start = DateTime::from_naive_utc_and_offset(
            at.date_naive()
                .and_hms_opt(0, 0, 0)
                .expect("valid day start"),
            Utc,
        );
        let next_day_start = DateTime::from_naive_utc_and_offset(
            at.date_naive()
                .succ_opt()
                .expect("valid next day")
                .and_hms_opt(0, 0, 0)
                .expect("valid next day start"),
            Utc,
        );
        OverrideLookup::SameDayLatestAtOrBefore {
            at,
            local_day_start_utc: day_start,
            next_local_day_start_utc: next_day_start,
        }
    }

    #[test]
    fn upsert_and_lookup_round_trips_native_asset_price() {
        let _rt = acquire_test_runtime();
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("init user db");

        let at = fixed_utc(2025, 6, 15, 12, 0, 0);
        let now = fixed_utc(2025, 6, 15, 10, 0, 0);

        let record = upsert_price_override(
            user_id,
            NewPriceOverride {
                subject: btc(),
                quote_currency: usd(),
                price_time_utc: at,
                price: d("50000.00"),
                source_note: Some("coinbase".to_string()),
            },
            now,
        )
        .expect("upsert");

        assert_eq!(record.subject, btc());
        assert_eq!(record.quote_currency, usd());
        assert_eq!(record.price, d("50000.00"));
        assert_eq!(record.source_note, Some("coinbase".to_string()));
        assert_eq!(record.created_at, now);
        assert_eq!(record.updated_at, now);

        let found = lookup_price_override(user_id, btc(), usd(), same_utc_day_lookup(at))
            .expect("lookup")
            .expect("should find");
        assert_eq!(found.id, record.id);
        assert_eq!(found.price, d("50000.00"));
    }

    #[test]
    fn upsert_conflict_updates_price_and_keeps_created_at() {
        let _rt = acquire_test_runtime();
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("init user db");

        let at = fixed_utc(2025, 6, 15, 12, 0, 0);
        let now1 = fixed_utc(2025, 6, 15, 10, 0, 0);
        let now2 = fixed_utc(2025, 6, 15, 11, 0, 0);

        let first = upsert_price_override(
            user_id,
            NewPriceOverride {
                subject: btc(),
                quote_currency: usd(),
                price_time_utc: at,
                price: d("50000.00"),
                source_note: None,
            },
            now1,
        )
        .expect("first upsert");

        let second = upsert_price_override(
            user_id,
            NewPriceOverride {
                subject: btc(),
                quote_currency: usd(),
                price_time_utc: at,
                price: d("51000.00"),
                source_note: Some("updated".to_string()),
            },
            now2,
        )
        .expect("second upsert");

        assert_eq!(first.id, second.id, "upsert should preserve id on conflict");
        assert_eq!(second.created_at, now1, "created_at should be preserved");
        assert_eq!(
            second.updated_at, now2,
            "updated_at should reflect latest write"
        );
        assert_eq!(second.price, d("51000.00"));
        assert_eq!(second.source_note, Some("updated".to_string()));
    }

    #[test]
    fn delete_removes_override() {
        let _rt = acquire_test_runtime();
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("init user db");

        let at = fixed_utc(2025, 6, 15, 12, 0, 0);
        let now = fixed_utc(2025, 6, 15, 10, 0, 0);

        upsert_price_override(
            user_id,
            NewPriceOverride {
                subject: btc(),
                quote_currency: usd(),
                price_time_utc: at,
                price: d("50000.00"),
                source_note: None,
            },
            now,
        )
        .expect("upsert");

        delete_price_override(user_id, btc(), usd(), at).expect("delete");

        let found =
            lookup_price_override(user_id, btc(), usd(), same_utc_day_lookup(at)).expect("lookup");
        assert!(found.is_none(), "should be deleted");

        // Idempotent: deleting again is fine
        delete_price_override(user_id, btc(), usd(), at).expect("delete again");
    }

    #[test]
    fn same_day_latest_at_or_before_uses_local_day_bounds() {
        let _rt = acquire_test_runtime();
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("init user db");

        let now = fixed_utc(2025, 6, 15, 0, 0, 0);

        // Amsterdam is UTC+2 in summer (CEST). Local day 2025-06-15 runs
        // from 2025-06-14T22:00:00Z to 2025-06-15T22:00:00Z.
        let local_day_start = fixed_utc(2025, 6, 14, 22, 0, 0);
        let next_local_day_start = fixed_utc(2025, 6, 15, 22, 0, 0);

        // Insert two overrides within the local day
        upsert_price_override(
            user_id,
            NewPriceOverride {
                subject: btc(),
                quote_currency: usd(),
                price_time_utc: fixed_utc(2025, 6, 15, 8, 0, 0),
                price: d("50000.00"),
                source_note: None,
            },
            now,
        )
        .expect("upsert 1");

        upsert_price_override(
            user_id,
            NewPriceOverride {
                subject: btc(),
                quote_currency: usd(),
                price_time_utc: fixed_utc(2025, 6, 15, 16, 0, 0),
                price: d("51000.00"),
                source_note: None,
            },
            now,
        )
        .expect("upsert 2");

        // Insert an override just before the local day — should not match
        upsert_price_override(
            user_id,
            NewPriceOverride {
                subject: btc(),
                quote_currency: usd(),
                price_time_utc: fixed_utc(2025, 6, 14, 21, 59, 59),
                price: d("49000.00"),
                source_note: None,
            },
            now,
        )
        .expect("upsert before day");

        // Look for latest at or before 15:00 UTC (which is 17:00 CEST, within the day)
        let found = lookup_price_override(
            user_id,
            btc(),
            usd(),
            OverrideLookup::SameDayLatestAtOrBefore {
                at: fixed_utc(2025, 6, 15, 15, 0, 0),
                local_day_start_utc: local_day_start,
                next_local_day_start_utc: next_local_day_start,
            },
        )
        .expect("lookup")
        .expect("should find");

        // Should pick 08:00 UTC (price 50000), not 16:00 UTC (51000) since 16:00 > 15:00
        assert_eq!(found.price, d("50000.00"));
    }

    #[test]
    fn list_in_range_includes_equal_from_and_to() {
        let _rt = acquire_test_runtime();
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("init user db");

        let now = fixed_utc(2025, 6, 15, 0, 0, 0);
        let from = fixed_utc(2025, 6, 15, 0, 0, 0);
        let to = fixed_utc(2025, 6, 15, 23, 59, 59);

        upsert_price_override(
            user_id,
            NewPriceOverride {
                subject: btc(),
                quote_currency: usd(),
                price_time_utc: from,
                price: d("50000.00"),
                source_note: None,
            },
            now,
        )
        .expect("upsert at from");

        upsert_price_override(
            user_id,
            NewPriceOverride {
                subject: btc(),
                quote_currency: usd(),
                price_time_utc: to,
                price: d("52000.00"),
                source_note: None,
            },
            now,
        )
        .expect("upsert at to");

        let records = list_price_overrides_in_range(user_id, btc(), usd(), from, to).expect("list");

        assert_eq!(records.len(), 2);
        assert_eq!(records[0].price, d("50000.00"));
        assert_eq!(records[1].price, d("52000.00"));
    }

    #[test]
    fn quote_currency_is_part_of_identity() {
        let _rt = acquire_test_runtime();
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("init user db");

        let at = fixed_utc(2025, 6, 15, 12, 0, 0);
        let now = fixed_utc(2025, 6, 15, 10, 0, 0);

        upsert_price_override(
            user_id,
            NewPriceOverride {
                subject: btc(),
                quote_currency: usd(),
                price_time_utc: at,
                price: d("50000.00"),
                source_note: None,
            },
            now,
        )
        .expect("upsert usd");

        upsert_price_override(
            user_id,
            NewPriceOverride {
                subject: btc(),
                quote_currency: eur(),
                price_time_utc: at,
                price: d("45000.00"),
                source_note: None,
            },
            now,
        )
        .expect("upsert eur");

        let usd_record = lookup_price_override(user_id, btc(), usd(), same_utc_day_lookup(at))
            .expect("lookup usd")
            .expect("should find usd");
        assert_eq!(usd_record.price, d("50000.00"));

        let eur_record = lookup_price_override(user_id, btc(), eur(), same_utc_day_lookup(at))
            .expect("lookup eur")
            .expect("should find eur");
        assert_eq!(eur_record.price, d("45000.00"));
    }
}
