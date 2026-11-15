use super::executor::{PUBLIC_KEYLESS_HISTORY_REQUEST_LANE, is_coingecko_not_found};
use super::planner::DateSpan;
use super::shared_planner::next_gap;
use crate::db::prices_db::shared_history::{self as store, Lane, SharedPricePair};
use crate::integrations::coingecko::{
    CoinGeckoCredentialMode, CoingeckoMarketChartRangeResponse,
    client::{CoingeckoClient, CoingeckoError},
};
use crate::traces::client::{IntegrationLabel, TracedBlockingClient};
use chrono::{DateTime, Duration, NaiveDate, NaiveTime, Utc};
use rusqlite::{Connection, params};
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::time::Instant;

type JobResult<T> = Result<T, Box<dyn std::error::Error>>;
const REQUEST_BUDGET: usize = 8;
const PAIR_REFRESH: std::time::Duration = std::time::Duration::from_secs(15 * 60);
const PARTIAL_RETRY_SECS: i64 = 24 * 60 * 60;
// ponytail: fixed hourly retry for today's not-yet-published 00:00 point.
const TODAY_RETRY_SECS: i64 = 60 * 60;
static LAST_PAIR_REFRESH: Mutex<Option<Instant>> = Mutex::new(None);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SharedPriceHistorySummary {
    pub(crate) requests_attempted: usize,
    pub(crate) requests_succeeded: usize,
    pub(crate) history_limit_responses: usize,
    pub(crate) rows_written: usize,
    pub(crate) reason: &'static str,
    pub(crate) retry_at: Option<i64>,
}

type BatchResult = Result<SharedPriceHistorySummary, SharedPriceHistorySummary>;

impl SharedPriceHistorySummary {
    fn collect(run: impl FnOnce(&mut Self) -> JobResult<()>) -> BatchResult {
        let mut summary = Self {
            requests_attempted: 0,
            requests_succeeded: 0,
            history_limit_responses: 0,
            rows_written: 0,
            reason: "no_work",
            retry_at: None,
        };
        match run(&mut summary) {
            Ok(()) => Ok(summary),
            Err(_) => {
                summary.reason = "worker_error";
                Err(summary)
            }
        }
    }

    fn backoff(&mut self, conn: &Connection, now: i64) -> rusqlite::Result<bool> {
        let retry_at = store::public_retry_at(conn)?;
        if retry_at > now {
            self.retry_at = Some(retry_at);
            self.reason = "rate_limited";
            return Ok(true);
        }
        Ok(false)
    }
}

pub(crate) fn run_shared_price_history() -> BatchResult {
    SharedPriceHistorySummary::collect(run)
}

fn run(summary: &mut SharedPriceHistorySummary) -> JobResult<()> {
    if !crate::db::any_price_fetching_enabled()? {
        summary.reason = "no_opt_in";
        return Ok(());
    }
    let mut conn = crate::db::open_migrated_prices_db()?;
    {
        let mut last = LAST_PAIR_REFRESH
            .lock()
            .map_err(|_| "price discovery mutex poisoned")?;
        if last.is_none_or(|t| t.elapsed() >= PAIR_REFRESH) {
            store::refresh_pairs(&mut conn)?;
            conn.execute(
                "DELETE FROM shared_history_deferrals WHERE retry_at<=?1",
                [Utc::now().timestamp()],
            )?;
            *last = Some(Instant::now());
        }
    }
    let now = Utc::now();
    if summary.backoff(&conn, now.timestamp())? || store::due_pair(&conn, now)?.is_none() {
        return Ok(());
    }
    let client =
        TracedBlockingClient::instance_builder(IntegrationLabel::new("coingecko-shared-history"))
            .configure(|b| b.timeout(std::time::Duration::from_secs(10)))
            .build()
            .map_err(|e| e.to_string())?;
    let api =
        CoingeckoClient::from_credential_mode(client, CoinGeckoCredentialMode::PublicKeyless)?;
    process_batch(
        summary,
        &conn,
        &PUBLIC_KEYLESS_HISTORY_REQUEST_LANE,
        || crate::tasks::TASK_MANAGER_SHUTTING_DOWN.load(Ordering::Acquire),
        Utc::now,
        crate::db::any_price_fetching_enabled,
        |pair, span| {
            let start = span.start.and_time(NaiveTime::MIN).and_utc().timestamp();
            let end = (span.end + Duration::days(1))
                .and_time(NaiveTime::MIN)
                .and_utc()
                .timestamp();
            api.market_chart_range(
                &pair.provider_id,
                &pair.currency.code().to_ascii_lowercase(),
                start,
                end,
            )
        },
    )
}

#[cfg(all(test, feature = "db-tests"))]
pub(super) mod tests;

fn daily_window(now: DateTime<Utc>, lane: Lane) -> DateSpan {
    let publish = now.date_naive().and_time(NaiveTime::MIN).and_utc() + Duration::minutes(40);
    let end = if now < publish {
        now.date_naive() - Duration::days(1)
    } else {
        now.date_naive()
    };
    DateSpan {
        end,
        start: match lane {
            Lane::Recent => end - Duration::days(6),
            Lane::History => NaiveDate::default(),
        },
    }
}

fn planned_gap(
    conn: &Connection,
    pair: &SharedPricePair,
    window: DateSpan,
    now: DateTime<Utc>,
) -> JobResult<(Option<DateSpan>, i64)> {
    let mut excluded: Vec<_> = store::covered_dates(conn, pair, window.start, window.end)?
        .into_iter()
        .map(|d| DateSpan { start: d, end: d })
        .collect();
    let mut stmt = conn.prepare(
        "SELECT from_date,to_date,retry_at,probe_end FROM shared_history_deferrals
        WHERE asset_id=?1 AND quote_currency=?2 AND provider_asset_id=?3 AND retry_at>?4 AND to_date>=?5 AND from_date<=?6",
    )?;
    let mut rows = stmt.query(params![
        pair.asset_id,
        pair.currency.code(),
        pair.provider_id,
        now.timestamp(),
        window.start,
        window.end
    ])?;
    let publish = now.date_naive().and_time(NaiveTime::MIN).and_utc() + Duration::minutes(40);
    let mut next = if now < publish {
        publish
    } else {
        publish + Duration::days(1)
    }
    .timestamp();
    let mut probe = None;
    while let Some(row) = rows.next()? {
        excluded.push(DateSpan {
            start: row.get(0)?,
            end: row.get(1)?,
        });
        next = next.min(row.get(2)?);
        if let Some(end) = row.get::<_, Option<NaiveDate>>(3)? {
            probe = Some(end);
        }
    }
    Ok((next_gap(window, excluded, probe), next))
}

fn defer(
    conn: &Connection,
    pair: &SharedPricePair,
    span: DateSpan,
    until: i64,
    probe: Option<NaiveDate>,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO shared_history_deferrals VALUES(?1,?2,?3,?4,?5,?6,?7)
        ON CONFLICT(asset_id,quote_currency,provider_asset_id,from_date,to_date) DO UPDATE SET
        retry_at=excluded.retry_at,probe_end=excluded.probe_end",
        params![
            pair.asset_id,
            pair.currency.code(),
            pair.provider_id,
            span.start,
            span.end,
            until,
            probe
        ],
    )?;
    Ok(())
}

fn finish_window(
    conn: &Connection,
    pair: &SharedPricePair,
    lane: Lane,
    window: DateSpan,
    now: DateTime<Utc>,
    next_due: i64,
) -> JobResult<()> {
    let mut due = next_due;
    if lane == Lane::History {
        // Skip a proven unavailable prefix in one turn. Otherwise walk one bounded window.
        let prefix: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM shared_history_deferrals
                    WHERE asset_id=?1 AND quote_currency=?2 AND provider_asset_id=?3 AND retry_at>?4
                    AND from_date<=?5 AND to_date>=?6)",
            params![
                pair.asset_id,
                pair.currency.code(),
                pair.provider_id,
                now.timestamp(),
                NaiveDate::default(),
                window.start
            ],
            |r| r.get(0),
        )?;
        let next_window = if window.start == NaiveDate::default() || prefix {
            None
        } else {
            window.start.pred_opt()
        };
        conn.execute(
            "UPDATE shared_history_pairs SET history_end=?3 WHERE asset_id=?1 AND quote_currency=?2",
            params![pair.asset_id, pair.currency.code(), next_window],
        )?;
        if next_window.is_some() {
            due = 0;
        } else {
            let retry: Option<i64> = conn.query_row(
                "SELECT MIN(retry_at) FROM shared_history_deferrals
                 WHERE asset_id=?1 AND quote_currency=?2 AND provider_asset_id=?3 AND retry_at>?4",
                params![
                    pair.asset_id,
                    pair.currency.code(),
                    pair.provider_id,
                    now.timestamp()
                ],
                |row| row.get(0),
            )?;
            due = due.min(retry.unwrap_or(due));
        }
    }
    store::finish_turn(conn, pair, lane, due)?;
    Ok(())
}

#[cfg(all(test, feature = "db-tests"))]
pub(super) use tests::run_batch;

fn process_batch(
    summary: &mut SharedPriceHistorySummary,
    conn: &Connection,
    // User-initiated keyless history holds this same lane until its rows are committed.
    request_lane: &Mutex<()>,
    shutting_down: impl Fn() -> bool,
    clock: impl Fn() -> DateTime<Utc>,
    consent: impl Fn() -> Result<bool, crate::db::DbError>,
    mut fetch: impl FnMut(
        &SharedPricePair,
        DateSpan,
    ) -> Result<CoingeckoMarketChartRangeResponse, CoingeckoError>,
) -> JobResult<()> {
    let mut partial_response = false;
    for _ in 0..32 {
        let now = clock();
        if shutting_down() {
            summary.reason = "shutdown";
            break;
        }
        if summary.requests_attempted == REQUEST_BUDGET || summary.backoff(conn, now.timestamp())? {
            break;
        }
        let _guard = request_lane
            .lock()
            .map_err(|_| "public history lane poisoned")?;
        let now = clock();
        if summary.backoff(conn, now.timestamp())? {
            break;
        }
        let Some((pair, lane, cursor)) = store::due_pair(conn, now)? else {
            break;
        };
        let mut window = daily_window(now, lane);
        if lane == Lane::History {
            window.end = cursor.unwrap_or(window.end).min(window.end);
            window.start = (window.end - Duration::days(364)).max(NaiveDate::default());
        }
        let (gap, next_due) = planned_gap(conn, &pair, window, now)?;
        let Some(span) = gap else {
            finish_window(conn, &pair, lane, window, now, next_due)?;
            continue;
        };
        if !store::still_current(conn, &pair)? {
            conn.execute(
                "DELETE FROM shared_history_pairs WHERE asset_id=?1 AND quote_currency=?2",
                params![pair.asset_id, pair.currency.code()],
            )?;
            continue;
        }
        if !consent()? {
            summary.reason = "no_opt_in";
            break;
        }
        if shutting_down() {
            summary.reason = "shutdown";
            break;
        }
        summary.requests_attempted += 1;
        let result = fetch(&pair, span);
        let at = clock();
        partial_response |= save_outcome(summary, conn, &pair, span, now, at, result)?;
        summary.reason = if summary.retry_at.is_some() {
            "rate_limited"
        } else if summary.history_limit_responses == summary.requests_attempted {
            // Expected one-off rejections of dates beyond the keyless 365-day window.
            "history_limit"
        } else if summary.requests_succeeded == 0 {
            "provider_error"
        } else if summary.rows_written == 0 {
            "no_new_rows"
        } else if partial_response || summary.requests_succeeded < summary.requests_attempted {
            "partial_progress"
        } else {
            "completed"
        };
        store::finish_turn(conn, &pair, lane, 0)?;
    }
    Ok(())
}

// Returns whether a valid response omitted one or more requested days.
fn save_outcome(
    summary: &mut SharedPriceHistorySummary,
    conn: &Connection,
    pair: &SharedPricePair,
    span: DateSpan,
    requested_at: DateTime<Utc>,
    at: DateTime<Utc>,
    result: Result<CoingeckoMarketChartRangeResponse, CoingeckoError>,
) -> JobResult<bool> {
    match result {
        Ok(response) => {
            let points = response.into_daily_prices_at(at.date_naive());
            match points {
                Ok(points) => {
                    let rows: Vec<_> = points
                        .into_iter()
                        .filter(|p| (span.start..=span.end).contains(&p.date_utc))
                        .map(|p| crate::db::DailyPricePointUpsert {
                            asset_id: pair.asset_id.clone(),
                            quote_currency: pair.currency,
                            price_time_utc: p.price_time,
                            date_utc: p.date_utc,
                            price: p.price.as_decimal(),
                            provider_asset_id: pair.provider_id.clone(),
                            provider_quote_id: Some(pair.currency.code().to_ascii_lowercase()),
                            license_scope: "public_keyless".into(),
                            retrieved_at: at,
                        })
                        .collect();
                    summary.requests_succeeded += 1;
                    summary.rows_written +=
                        crate::db::prices_db::write_daily_points(conn, &rows, Some(pair))?;
                    let today = at.date_naive();
                    let mut older = span;
                    if span.end == today && !rows.iter().any(|row| row.date_utc == today) {
                        // Today's 00:00 point may simply not be published yet.
                        let today_span = DateSpan {
                            start: today,
                            end: today,
                        };
                        defer(
                            conn,
                            pair,
                            today_span,
                            at.timestamp() + TODAY_RETRY_SECS,
                            None,
                        )?;
                        older.end = today - Duration::days(1);
                    }
                    if older.start <= older.end
                        && rows.len() as i64 != (older.end - older.start).num_days() + 1
                    {
                        defer(conn, pair, older, at.timestamp() + PARTIAL_RETRY_SECS, None)?;
                    }
                    return Ok(rows.len() as i64 != (span.end - span.start).num_days() + 1);
                }
                Err(_) => defer(conn, pair, span, at.timestamp() + 300, None)?,
            }
        }
        Err(err) if err.is_history_limit() => {
            summary.history_limit_responses += 1;
            let (end, probe) = match err.public_history_unavailable_through(requested_at) {
                Some(cutoff) if cutoff >= span.start => (cutoff, None),
                _ => (span.start, Some(span.end)),
            };
            defer(
                conn,
                pair,
                DateSpan {
                    start: NaiveDate::default(),
                    end,
                },
                at.timestamp() + 7 * 86400,
                probe,
            )?;
        }
        Err(err) if is_coingecko_not_found(&err) => {
            defer(
                conn,
                pair,
                DateSpan {
                    start: NaiveDate::default(),
                    end: at.date_naive() + Duration::days(7),
                },
                at.timestamp() + 7 * 86400,
                None,
            )?;
        }
        Err(err) => {
            if let Some(retry) = err.history_retry_at(at) {
                store::set_public_retry_at(conn, retry)?;
                summary.retry_at = Some(retry);
            } else {
                defer(conn, pair, span, at.timestamp() + 300, None)?;
            }
        }
    }
    Ok(false)
}
