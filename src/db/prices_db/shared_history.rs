use crate::asset_capabilities::{self, AssetId};
use crate::models::CurrencyCode;
use rusqlite::Connection;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SharedPricePair {
    pub asset_id: String,
    pub provider_id: String,
    pub currency: CurrencyCode,
}

pub(crate) fn known_coingecko_pairs(conn: &Connection) -> rusqlite::Result<Vec<SharedPricePair>> {
    let mut stmt = conn.prepare(
        "SELECT asset_id, quote_currency FROM price_points WHERE provider='coingecko'
         UNION SELECT asset_id, quote_currency FROM current_price_cache WHERE provider='coingecko'
         ORDER BY asset_id, quote_currency",
    )?;
    let mut rows = stmt.query([])?;
    let mut pairs = Vec::new();
    while let Some(row) = rows.next()? {
        let asset_id: String = row.get(0)?;
        let code: String = row.get(1)?;
        if let Some(currency) = CurrencyCode::from_code(&code)
            && let Some(provider_id) = resolved_provider_id(conn, &asset_id, currency)?
        {
            pairs.push(SharedPricePair {
                asset_id,
                currency,
                provider_id,
            });
        }
    }
    Ok(pairs)
}

fn catalog_id(asset_id: &str) -> Option<String> {
    AssetId::owned(asset_id.to_owned())
        .ok()
        .and_then(|id| asset_capabilities::asset(&id))
        .and_then(|asset| asset.price_refs.coingecko.as_deref().map(str::to_owned))
        .or_else(|| asset_capabilities::resolve_manual_coingecko_id(asset_id, None))
}

/// Revalidate just this local pair before dispatch and again in the write transaction.
pub(crate) fn still_current(conn: &Connection, pair: &SharedPricePair) -> rusqlite::Result<bool> {
    Ok(
        resolved_provider_id(conn, &pair.asset_id, pair.currency)?.as_deref()
            == Some(&pair.provider_id),
    )
}

fn resolved_provider_id(
    conn: &Connection,
    asset_id: &str,
    currency: CurrencyCode,
) -> rusqlite::Result<Option<String>> {
    let (count, first, last): (i64, Option<String>, Option<String>) = conn.query_row(
        "SELECT COUNT(*), MIN(id), MAX(id) FROM (
             SELECT DISTINCT NULLIF(TRIM(provider_asset_id),'') AS id FROM price_points
             WHERE provider='coingecko' AND asset_id=?1 AND quote_currency=?2
             UNION SELECT NULLIF(TRIM(provider_asset_id),'') FROM current_price_cache
             WHERE provider='coingecko' AND asset_id=?1 AND quote_currency=?2)",
        rusqlite::params![asset_id, currency.code()],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    Ok(if count > 0 && first == last {
        first.or_else(|| catalog_id(asset_id))
    } else {
        None
    })
}

pub(crate) fn refresh_pairs(conn: &mut Connection) -> rusqlite::Result<()> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let known = known_coingecko_pairs(&tx)?;
    tx.execute_batch("CREATE TEMP TABLE IF NOT EXISTS active_pair_keys(asset_id TEXT, quote_currency TEXT, PRIMARY KEY(asset_id, quote_currency));
                      DELETE FROM active_pair_keys")?;
    for pair in known {
        let args = rusqlite::params![pair.asset_id, pair.currency.code(), pair.provider_id];
        tx.execute("INSERT INTO shared_history_pairs VALUES (?1,?2,?3,0,0,0,NULL)
                    ON CONFLICT(asset_id,quote_currency) DO UPDATE SET provider_asset_id=excluded.provider_asset_id,
                    recent_due=0,history_due=0,last_turn=0,history_end=NULL WHERE provider_asset_id<>excluded.provider_asset_id",args)?;
        tx.execute(
            "INSERT INTO active_pair_keys VALUES (?1,?2)",
            rusqlite::params![pair.asset_id, pair.currency.code()],
        )?;
    }
    tx.execute(
        "DELETE FROM shared_history_pairs WHERE (asset_id,quote_currency) NOT IN
                (SELECT asset_id,quote_currency FROM active_pair_keys)",
        [],
    )?;
    tx.commit()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lane {
    Recent,
    History,
}

pub(crate) fn due_pair(
    conn: &Connection,
    now: chrono::DateTime<chrono::Utc>,
) -> rusqlite::Result<Option<(SharedPricePair, Lane, Option<chrono::NaiveDate>)>> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT asset_id,quote_currency,provider_asset_id,recent_due<=?1 AS recent,history_end
                    FROM shared_history_pairs WHERE (recent_due<=?1 OR history_due<=?1)
                    ORDER BY recent DESC,last_turn, asset_id,quote_currency LIMIT 1",
        [now.timestamp()],
        |row| {
            let code: String = row.get(1)?;
            let currency = CurrencyCode::from_code(&code).ok_or(rusqlite::Error::InvalidQuery)?;
            Ok((
                SharedPricePair {
                    asset_id: row.get(0)?,
                    currency,
                    provider_id: row.get(2)?,
                },
                if row.get::<_, bool>(3)? {
                    Lane::Recent
                } else {
                    Lane::History
                },
                row.get(4)?,
            ))
        },
    )
    .optional()
}

pub(crate) fn finish_turn(
    conn: &Connection,
    pair: &SharedPricePair,
    lane: Lane,
    due: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE shared_history_pairs SET
        recent_due=CASE WHEN ?4 THEN ?3 ELSE recent_due END,
        history_due=CASE WHEN ?4 THEN history_due ELSE ?3 END,
        last_turn=(SELECT COALESCE(MAX(last_turn),0)+1 FROM shared_history_pairs)
        WHERE asset_id=?1 AND quote_currency=?2",
        rusqlite::params![
            pair.asset_id,
            pair.currency.code(),
            due,
            lane == Lane::Recent
        ],
    )?;
    Ok(())
}

pub(crate) fn public_retry_at(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COALESCE(MAX(retry_at),0) FROM public_history_backoff",
        [],
        |r| r.get(0),
    )
}

/// Keyless requests are serialized and check this backoff first, so the newest
/// Retry-After is the provider's latest answer and replaces the stored one.
pub(crate) fn set_public_retry_at(conn: &Connection, retry: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO public_history_backoff VALUES (1,?1)
        ON CONFLICT(singleton) DO UPDATE SET retry_at=excluded.retry_at",
        [retry],
    )?;
    Ok(())
}

pub(crate) fn covered_dates(
    conn: &Connection,
    pair: &SharedPricePair,
    start: chrono::NaiveDate,
    end: chrono::NaiveDate,
) -> rusqlite::Result<Vec<chrono::NaiveDate>> {
    // A non-midnight row is final only if it was retrieved after its date ended;
    // a same-day live sample must not hide the published 00:00 price.
    let mut stmt=conn.prepare("SELECT DISTINCT date_utc FROM price_points
        WHERE asset_id=?1 AND quote_currency=?2 AND provider='coingecko' AND granularity='daily'
        AND price_kind='daily_point' AND (TRIM(provider_asset_id)=?3 OR NULLIF(TRIM(provider_asset_id),'') IS NULL)
        AND date_utc BETWEEN ?4 AND ?5
        AND (date_utc < substr(retrieved_at,1,10) OR price_time_utc=date_utc || 'T00:00:00Z')")?;
    stmt.query_map(
        rusqlite::params![
            pair.asset_id,
            pair.currency.code(),
            pair.provider_id,
            start,
            end
        ],
        |r| r.get(0),
    )?
    .collect()
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;

    #[test]
    fn known_pairs_resolve_legacy_and_skip_conflicts() {
        let mut conn = Connection::open_in_memory().unwrap();
        super::super::configure_and_migrate_connection(&mut conn).unwrap();
        conn.execute_batch("INSERT INTO current_price_cache
            (asset_id, quote_currency, provider, price, license_scope, retrieved_at, created_at, updated_at)
            VALUES ('bitcoin','USD','coingecko','1','public_keyless','old','old','old'),
                   ('ambiguous','EUR','coingecko','1','public_keyless','old','old','old');
            UPDATE current_price_cache SET provider_asset_id='a' WHERE asset_id='ambiguous';
            INSERT INTO price_points
            (id, asset_id, quote_currency, price_time_utc, price, provider, provider_asset_id, granularity, price_kind, license_scope, retrieved_at, created_at, updated_at)
            VALUES ('id','ambiguous','EUR','time','1','coingecko','b','daily','daily_point','coingecko_pro_key','old','old','old');").unwrap();
        let pairs = known_coingecko_pairs(&conn).unwrap();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].provider_id, "bitcoin");
        assert_eq!(pairs[0].currency.code(), "USD");
    }
}
