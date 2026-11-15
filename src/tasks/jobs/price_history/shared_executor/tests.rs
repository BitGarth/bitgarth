use super::*;
use std::cell::Cell;
use std::path::Path;

pub(in crate::tasks::jobs::price_history) fn run_batch(
    conn: &Connection,
    request_lane: &Mutex<()>,
    shutting_down: impl Fn() -> bool,
    clock: impl Fn() -> DateTime<Utc>,
    consent: impl Fn() -> Result<bool, crate::db::DbError>,
    fetch: impl FnMut(
        &SharedPricePair,
        DateSpan,
    ) -> Result<CoingeckoMarketChartRangeResponse, CoingeckoError>,
) -> BatchResult {
    SharedPriceHistorySummary::collect(|summary| {
        process_batch(
            summary,
            conn,
            request_lane,
            shutting_down,
            clock,
            consent,
            fetch,
        )
    })
}

pub(in crate::tasks::jobs::price_history) fn open(path: &Path) -> Connection {
    let mut conn = Connection::open(path).unwrap();
    for sql in [
        include_str!("../../../../../migrations/prices/V0__price_cache.sql"),
        include_str!("../../../../../migrations/prices/V1__asset_id_price_rows.sql"),
        include_str!("../../../../../migrations/prices/V2__historical_price_attempts.sql"),
        include_str!("../../../../../migrations/prices/V3__shared_history_work.sql"),
        include_str!("../../../../../migrations/prices/V4__coingecko_pair_interest.sql"),
        include_str!("../../../../../migrations/prices/V5__drop_coingecko_pair_interest.sql"),
    ] {
        conn.execute_batch(sql).unwrap();
    }
    conn.execute_batch("INSERT INTO current_price_cache
        (asset_id,quote_currency,provider,provider_asset_id,price,license_scope,retrieved_at,created_at,updated_at)
        VALUES ('bitcoin','USD','coingecko','bitcoin','10','coingecko_pro_key','old','old','old'),
               ('solana','EUR','coingecko','solana','7','public_keyless','old','old','old');").unwrap();
    store::refresh_pairs(&mut conn).unwrap();
    conn
}

fn date(day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 5, day).unwrap()
}
fn clock(day: u32) -> DateTime<Utc> {
    date(day).and_time(NaiveTime::MIN).and_utc() + Duration::hours(2)
}
pub(in crate::tasks::jobs::price_history) fn series(
    span: DateSpan,
) -> CoingeckoMarketChartRangeResponse {
    let values: Vec<_> = span
        .start
        .iter_days()
        .take_while(|d| *d <= span.end)
        .map(|d| (d.and_time(NaiveTime::MIN).and_utc().timestamp_millis(), 10))
        .collect();
    serde_json::from_value(serde_json::json!({"prices":values})).unwrap()
}
fn provider(
    _: &SharedPricePair,
    span: DateSpan,
) -> Result<CoingeckoMarketChartRangeResponse, CoingeckoError> {
    if span.start < date(17) {
        return Err(CoingeckoError::Api {
            status_code: 401,
            error_code: 10012,
            error_message: "history limit".into(),
            retry_after: None,
        });
    }
    Ok(series(span))
}

#[test]
fn nested_history_limit_defers_the_unavailable_prefix_for_seven_days() {
    let conn = open(Path::new(":memory:"));
    let now = clock(30);
    // Start the history lane on an old window and stop after its first response.
    let end = date(30) - Duration::days(730);
    conn.execute(
        "UPDATE shared_history_pairs SET recent_due=?1, history_end=?2",
        params![i64::MAX, end],
    )
    .unwrap();
    let body = r#"{"error":{"status":{
        "error_code":10012,"error_message":"Your request exceeds the allowed time range."
    }}}"#;
    let attempted = Cell::new(None);
    let summary = run_batch(
        &conn,
        &Mutex::new(()),
        || attempted.get().is_some(),
        || now,
        || Ok(true),
        |_, span| {
            attempted.set(Some(span));
            Err(CoingeckoError::from_http_response(
                401,
                Default::default(),
                body.to_owned(),
            ))
        },
    )
    .unwrap();
    assert_eq!(summary.history_limit_responses, 1);
    let span = attempted.get().expect("history request");
    assert_eq!(span.end, end);
    let deferral: (NaiveDate, NaiveDate, i64, Option<NaiveDate>) = conn
        .query_row(
            "SELECT from_date, to_date, retry_at, probe_end FROM shared_history_deferrals",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        deferral,
        (
            NaiveDate::default(),
            span.start,
            (now + Duration::days(7)).timestamp(),
            Some(span.end)
        )
    );
}

#[test]
fn run_with_only_history_limit_refusals_is_not_a_provider_error() {
    let conn = open(Path::new(":memory:"));
    let body = r#"{"error":{"status":{"error_code":10012,
        "error_message":"Public API users are limited to querying historical data within the past 365 days."
    }}}"#;
    let summary = run_batch(
        &conn,
        &Mutex::new(()),
        || false,
        || clock(30),
        || Ok(true),
        |_, _| {
            Err(CoingeckoError::from_http_response(
                401,
                Default::default(),
                body.to_owned(),
            ))
        },
    )
    .unwrap();
    assert!(summary.requests_attempted > 0);
    assert_eq!(summary.history_limit_responses, summary.requests_attempted);
    assert_eq!(summary.reason, "history_limit");
}

#[test]
fn explicit_public_window_stops_after_one_refusal_per_pair_and_keeps_its_suffix() {
    let conn = open(Path::new(":memory:"));
    let now = clock(30);
    let first_available = now.date_naive() - Duration::days(364);
    // Seed the recent seven days and the rest of each pair's 365-day cache.
    let calls = Cell::new(0);
    let seeded = run_batch(
        &conn,
        &Mutex::new(()),
        || false,
        || now,
        || Ok(calls.get() < 4),
        |_, span| {
            assert!(span.start >= first_available);
            calls.set(calls.get() + 1);
            Ok(series(span))
        },
    )
    .unwrap();
    assert_eq!(seeded.rows_written, 2 * 365);

    // One pair has a cached year, the other needs the oldest accessible day in a straddling range.
    conn.execute(
        "DELETE FROM price_points WHERE asset_id='solana' AND date_utc=?1",
        [first_available],
    )
    .unwrap();
    conn.execute(
        "UPDATE shared_history_pairs SET history_end=?1 WHERE asset_id='solana'",
        [first_available],
    )
    .unwrap();
    let body = r#"{"error":{"status":{"error_code":10012,
        "error_message":"Public API users are limited to querying historical data within the past 365 days."
    }}}"#;
    let mut refused = Vec::new();
    let mut saved = 0;
    for _ in 0..6 {
        let summary = run_batch(
            &conn,
            &Mutex::new(()),
            || false,
            || now,
            || Ok(true),
            |pair, span| {
                if span.start < first_available {
                    refused.push(pair.asset_id.clone());
                    Err(CoingeckoError::from_http_response(
                        401,
                        Default::default(),
                        body.to_owned(),
                    ))
                } else {
                    Ok(series(span))
                }
            },
        )
        .unwrap();
        saved += summary.rows_written;
        if summary.requests_attempted == 0 {
            break;
        }
    }
    refused.sort();
    assert_eq!(refused, ["bitcoin", "solana"]);
    assert_eq!(saved, 1);
    let prefixes: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM shared_history_deferrals
         WHERE from_date=?1 AND to_date=?2 AND retry_at=?3 AND probe_end IS NULL",
            params![
                NaiveDate::default(),
                first_available.pred_opt().unwrap(),
                (now + Duration::days(7)).timestamp()
            ],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(prefixes, 2);
}

#[test]
fn restart_collects_every_available_date_for_every_currency() {
    let temp = std::env::temp_dir().join(format!("bitgarth-shared-{}.db", ulid::Ulid::new()));
    let mut conn = open(&temp);
    run_batch(
        &conn,
        &Mutex::new(()),
        || false,
        || clock(23),
        || Ok(true),
        provider,
    )
    .unwrap();
    drop(conn);
    conn = Connection::open(&temp).unwrap();
    for _ in 0..12 {
        run_batch(
            &conn,
            &Mutex::new(()),
            || false,
            || clock(30),
            || Ok(true),
            provider,
        )
        .unwrap();
    }
    for pair in store::known_coingecko_pairs(&conn).unwrap() {
        let days = store::covered_dates(&conn, &pair, date(1), date(30)).unwrap();
        assert_eq!(days.len(), 14);
        assert!((17..=30).all(|n| days.contains(&date(n))));
    }
    let summary = run_batch(
        &conn,
        &Mutex::new(()),
        || false,
        || clock(30),
        || Ok(true),
        |_, _| panic!("covered and refused dates should be idle"),
    )
    .unwrap();
    assert_eq!(summary.reason, "no_work");
    assert_eq!(summary.requests_attempted, 0);
    drop(conn);
    std::fs::remove_file(temp).unwrap();
}

#[test]
fn last_opt_out_and_consent_failure_suppress_new_requests() {
    let conn = open(Path::new(":memory:"));
    let calls = Cell::new(0);
    let summary = run_batch(
        &conn,
        &Mutex::new(()),
        || false,
        || clock(23),
        || Ok(calls.get() == 0),
        |_, span| {
            calls.set(calls.get() + 1);
            Ok(series(span))
        },
    )
    .unwrap();
    assert_eq!(summary.reason, "no_opt_in");
    assert_eq!(summary.requests_attempted, 1);
    assert_eq!(summary.requests_succeeded, 1);
    assert_eq!(summary.rows_written, 7);
    assert_eq!(calls.get(), 1);
    assert!(
        run_batch(
            &conn,
            &Mutex::new(()),
            || false,
            || clock(23),
            || Err(crate::db::DbError::new("unavailable")),
            |_, _| panic!("unauthorized")
        )
        .is_err()
    );
    run_batch(
        &conn,
        &Mutex::new(()),
        || false,
        || clock(30),
        || Ok(false),
        |_, _| panic!("disabled"),
    )
    .unwrap();
    run_batch(
        &conn,
        &Mutex::new(()),
        || false,
        || clock(30),
        || Ok(true),
        provider,
    )
    .unwrap();
    for pair in store::known_coingecko_pairs(&conn).unwrap() {
        assert_eq!(
            store::covered_dates(&conn, &pair, date(24), date(30))
                .unwrap()
                .len(),
            7
        );
    }
}

#[test]
fn public_backoff_survives_the_batch_and_partial_rows_remain_missing() {
    let conn = open(Path::new(":memory:"));
    let now = clock(30) + Duration::milliseconds(836);
    let first_dispatch = clock(30) + Duration::seconds(1801);
    let summary = run_batch(
        &conn,
        &Mutex::new(()),
        || false,
        || now,
        || Ok(true),
        |_, _| {
            Err(CoingeckoError::UnexpectedResponse {
                status_code: 429,
                headers: vec![("Retry-After".into(), "1800".into())],
                body: String::new(),
            })
        },
    )
    .unwrap();
    assert_eq!(summary.reason, "rate_limited");
    assert_eq!(summary.retry_at, Some(first_dispatch.timestamp()));
    assert_eq!(summary.requests_attempted, 1);
    assert_eq!(summary.requests_succeeded, 0);
    assert_eq!(summary.rows_written, 0);
    assert_eq!(
        store::public_retry_at(&conn).unwrap(),
        first_dispatch.timestamp()
    );
    let summary = run_batch(
        &conn,
        &Mutex::new(()),
        || false,
        || first_dispatch - Duration::nanoseconds(1),
        || Ok(true),
        |_, _| panic!("backoff active"),
    )
    .unwrap();
    assert_eq!(summary.reason, "rate_limited");
    assert_eq!(summary.retry_at, Some(first_dispatch.timestamp()));
    assert_eq!(summary.requests_attempted, 0);
    let calls = Cell::new(0);
    run_batch(
        &conn,
        &Mutex::new(()),
        || false,
        || first_dispatch,
        || Ok(calls.get() < 2),
        |_, span| {
            calls.set(calls.get() + 1);
            Ok(series(DateSpan {
                start: span.end,
                end: span.end,
            }))
        },
    )
    .unwrap();
    assert_eq!(calls.get(), 2);
    let pair = &store::known_coingecko_pairs(&conn).unwrap()[0];
    assert_eq!(
        store::covered_dates(&conn, pair, date(24), date(30)).unwrap(),
        vec![date(30)]
    );
}

#[test]
fn shutdown_during_fetch_commits_only_the_in_flight_request() {
    let conn = open(Path::new(":memory:"));
    let stopping = Cell::new(false);
    run_batch(
        &conn,
        &Mutex::new(()),
        || stopping.get(),
        || clock(30),
        || Ok(true),
        |pair, span| {
            assert!(
                !stopping.replace(true),
                "dispatched another request during shutdown"
            );
            assert_eq!(pair.asset_id, "bitcoin");
            Ok(series(span))
        },
    )
    .unwrap();
    let pair = &store::known_coingecko_pairs(&conn).unwrap()[0];
    assert_eq!(
        store::covered_dates(&conn, pair, date(24), date(30))
            .unwrap()
            .len(),
        7
    );
}

#[test]
fn service_unavailable_retry_after_pauses_all_pairs() {
    let conn = open(Path::new(":memory:"));
    let now = clock(30);
    let mut calls = 0;
    run_batch(
        &conn,
        &Mutex::new(()),
        || false,
        || now,
        || Ok(true),
        |_, _| {
            calls += 1;
            Err(CoingeckoError::UnexpectedResponse {
                status_code: 503,
                headers: vec![("Retry-After".into(), "3600".into())],
                body: String::new(),
            })
        },
    )
    .unwrap();
    assert_eq!(calls, 1);
    assert_eq!(
        store::public_retry_at(&conn).unwrap(),
        now.timestamp() + 3600
    );
    run_batch(
        &conn,
        &Mutex::new(()),
        || false,
        || now + Duration::minutes(59),
        || Ok(true),
        |_, _| panic!("provider still unavailable"),
    )
    .unwrap();
}

#[test]
fn remapping_between_discovery_dispatch_and_save_preserves_public_history() {
    let conn = open(Path::new(":memory:"));
    conn.execute(
        "UPDATE current_price_cache SET provider_asset_id='changed' WHERE asset_id='bitcoin'",
        [],
    )
    .unwrap();
    let summary = run_batch(
        &conn,
        &Mutex::new(()),
        || false,
        || clock(30),
        || Ok(true),
        |pair, span| {
            assert_eq!(pair.asset_id, "solana");
            conn.execute(
                "UPDATE current_price_cache SET provider_asset_id='new-solana' WHERE asset_id='solana'",
                [],
            ).unwrap();
            Ok(series(span))
        },
    ).unwrap();
    assert_eq!(summary.reason, "no_new_rows");
    assert_eq!(summary.requests_attempted, 1);
    assert_eq!(summary.requests_succeeded, 1);
    assert_eq!(summary.rows_written, 0);
    assert_eq!(
        conn.query_row("SELECT count(*) FROM price_points", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn unknown_pair_remains_cooled_across_publication() {
    let conn = open(Path::new(":memory:"));
    let summary = run_batch(
        &conn,
        &Mutex::new(()),
        || false,
        || clock(23),
        || Ok(true),
        |_, _| {
            Err(CoingeckoError::UnexpectedResponse {
                status_code: 404,
                headers: Vec::new(),
                body: String::new(),
            })
        },
    )
    .unwrap();
    assert_eq!(summary.reason, "provider_error");
    assert_eq!(summary.requests_succeeded, 0);
    assert_eq!(summary.rows_written, 0);
    run_batch(
        &conn,
        &Mutex::new(()),
        || false,
        || clock(24),
        || Ok(true),
        |_, _| panic!("404 cooldown covers new day"),
    )
    .unwrap();
}

#[test]
fn shared_waiter_checks_consent_after_acquiring_request_lane() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    let conn = open(Path::new(":memory:"));
    let lane = Arc::new(Mutex::new(()));
    let guard = lane.lock().unwrap();
    let queued_lane = Arc::clone(&lane);
    let consent = Arc::new(AtomicBool::new(true));
    let observed = Arc::clone(&consent);
    let (started, waiting) = mpsc::channel();
    let h = std::thread::spawn(move || {
        run_batch(
            &conn,
            &queued_lane,
            || false,
            || {
                let _ = started.send(());
                clock(30)
            },
            || Ok(observed.load(Ordering::SeqCst)),
            |_, _| panic!("opted out while queued"),
        )
        .unwrap();
    });
    waiting
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    consent.store(false, Ordering::SeqCst);
    drop(guard);
    h.join().unwrap();
}

#[test]
fn padded_provider_id_in_legacy_pro_row_counts_as_covered() {
    let mut conn = open(Path::new(":memory:"));
    conn.execute_batch(
        "DELETE FROM current_price_cache;
        INSERT INTO price_points (id,asset_id,quote_currency,price_time_utc,date_utc,price,provider,
        provider_asset_id,granularity,price_kind,license_scope,retrieved_at,created_at,updated_at)
        VALUES ('old','bitcoin','USD','2026-05-28T00:00:00Z','2026-05-28','99','coingecko',
        ' bitcoin ','daily','daily_point','coingecko_pro_key','2026-05-29','old','old')",
    )
    .unwrap();
    store::refresh_pairs(&mut conn).unwrap();
    run_batch(
        &conn,
        &Mutex::new(()),
        || false,
        || clock(30),
        || Ok(true),
        |pair, span| {
            assert_eq!(pair.provider_id, "bitcoin");
            assert!(
                !(span.start..=span.end).contains(&date(28)),
                "already covered legacy date was requested"
            );
            provider(pair, span)
        },
    )
    .unwrap();
    let provenance: String = conn
        .query_row(
            "SELECT license_scope FROM price_points WHERE id='old'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(provenance, "coingecko_pro_key");
}

#[test]
fn unpublished_todays_point_is_retried_within_the_hour() {
    let conn = open(Path::new(":memory:"));
    let today_ms = date(30)
        .and_time(NaiveTime::MIN)
        .and_utc()
        .timestamp_millis();
    let without_today = |pair: &SharedPricePair, span: DateSpan| {
        provider(pair, span).map(|mut response| {
            response.prices.retain(|(millis, _)| *millis < today_ms);
            response
        })
    };
    for _ in 0..12 {
        run_batch(
            &conn,
            &Mutex::new(()),
            || false,
            || clock(30),
            || Ok(true),
            without_today,
        )
        .unwrap();
    }
    run_batch(
        &conn,
        &Mutex::new(()),
        || false,
        || clock(30) + Duration::minutes(59),
        || Ok(true),
        |_, _| panic!("today's point retried before the hour"),
    )
    .unwrap();
    let asked = Cell::new(0);
    run_batch(
        &conn,
        &Mutex::new(()),
        || false,
        || clock(30) + Duration::minutes(61),
        || Ok(true),
        |pair, span| {
            assert_eq!(
                span,
                DateSpan {
                    start: date(30),
                    end: date(30)
                }
            );
            asked.set(asked.get() + 1);
            provider(pair, span)
        },
    )
    .unwrap();
    assert_eq!(asked.get(), 2);
    for pair in store::known_coingecko_pairs(&conn).unwrap() {
        assert!(
            store::covered_dates(&conn, &pair, date(30), date(30))
                .unwrap()
                .contains(&date(30))
        );
    }
}

#[test]
fn newer_shorter_retry_after_replaces_longer_backoff() {
    let conn = open(Path::new(":memory:"));
    let now = clock(30).timestamp();
    store::set_public_retry_at(&conn, now + 3600).unwrap();
    store::set_public_retry_at(&conn, now + 60).unwrap();
    assert_eq!(store::public_retry_at(&conn).unwrap(), now + 60);
}

#[test]
fn partial_responses_count_only_rows_inserted_by_this_batch() {
    let conn = open(Path::new(":memory:"));
    conn.execute("UPDATE shared_history_pairs SET history_due=?1", [i64::MAX])
        .unwrap();
    let now = clock(30);
    let summary = run_batch(
        &conn,
        &Mutex::new(()),
        || false,
        || now,
        || Ok(true),
        |pair, span| {
            // Another writer covers one of the two returned days while the request is in flight.
            let end = span.end;
            conn.execute(
                "INSERT INTO price_points (id,asset_id,quote_currency,price_time_utc,date_utc,price,provider,
                 provider_asset_id,granularity,price_kind,license_scope,retrieved_at,created_at,updated_at)
                 VALUES (?1,?1,?2,?3,?4,'10','coingecko',?5,'daily','daily_point','coingecko_pro_key',?3,?3,?3)",
                params![pair.asset_id, pair.currency.code(), format!("{end}T00:00:00Z"), end, pair.provider_id],
            ).unwrap();
            Ok(series(DateSpan {
                start: end.pred_opt().unwrap(),
                end,
            }))
        },
    ).unwrap();
    assert_eq!(summary.reason, "partial_progress");
    assert_eq!(summary.requests_attempted, 2);
    assert_eq!(summary.requests_succeeded, 2);
    assert_eq!(summary.rows_written, 2);
}

#[test]
fn later_scheduler_failure_retains_committed_counts_and_hides_error_text() {
    let conn = open(Path::new(":memory:"));
    conn.execute_batch(
        "CREATE TRIGGER reject_turn BEFORE UPDATE ON shared_history_pairs
         BEGIN SELECT RAISE(ABORT, 'private database failure detail'); END;",
    )
    .unwrap();
    let summary = run_batch(
        &conn,
        &Mutex::new(()),
        || false,
        || clock(30),
        || Ok(true),
        |_, span| Ok(series(span)),
    )
    .unwrap_err();
    assert_eq!(
        summary,
        SharedPriceHistorySummary {
            reason: "worker_error",
            requests_attempted: 1,
            requests_succeeded: 1,
            history_limit_responses: 0,
            rows_written: 7,
            retry_at: None,
        }
    );
}

#[test]
fn rolled_back_price_rows_are_excluded_from_the_summary() {
    let conn = open(Path::new(":memory:"));
    conn.execute_batch(
        "CREATE TRIGGER reject_today BEFORE INSERT ON price_points WHEN NEW.date_utc='2026-05-30'
         BEGIN SELECT RAISE(ABORT, 'private write failure detail'); END;",
    )
    .unwrap();
    let summary = run_batch(
        &conn,
        &Mutex::new(()),
        || false,
        || clock(30),
        || Ok(true),
        |_, span| Ok(series(span)),
    )
    .unwrap_err();
    assert_eq!(summary.reason, "worker_error");
    assert_eq!(summary.requests_attempted, 1);
    assert_eq!(summary.requests_succeeded, 1);
    assert_eq!(summary.rows_written, 0);
    let rows: usize = conn
        .query_row("SELECT COUNT(*) FROM price_points", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 0);
}
