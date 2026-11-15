//! Entries shown on the per-user Prices page: saved daily points, the user's
//! own prices, and gap rows, merged into one dated list and paged.

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

pub(crate) const PRICE_HISTORY_PAGE_SIZE: usize = 50;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum PriceHistorySort {
    #[default]
    NewestFirst,
    OldestFirst,
}

/// Which price sources the table shows. CoinGecko only applies while the
/// user's own CoinGecko toggle is on; the server falls back to `All` otherwise.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum PriceSourceFilter {
    #[default]
    All,
    YourPrices,
    CoinGecko,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum PriceHistoryEntry {
    /// CoinGecko's daily price for this UTC date.
    Daily { date_utc: NaiveDate, price: Decimal },
    /// A price the user set, at the instant they gave it.
    YourPrice {
        at: DateTime<Utc>,
        price: Decimal,
        note: Option<String>,
    },
    /// Days with no saved price, inclusive.
    Gap { from: NaiveDate, to: NaiveDate },
}

#[cfg(any(feature = "server", test))]
pub(crate) struct OwnPrice {
    pub(crate) at: DateTime<Utc>,
    pub(crate) local_date: NaiveDate,
    pub(crate) price: Decimal,
    pub(crate) note: Option<String>,
}

/// Shared daily points (ascending, one per date) and the dates they were read
/// for. Gap rows exist only inside this window: they describe the shared
/// data, so they are never shown when it isn't.
#[cfg(any(feature = "server", test))]
pub(crate) struct DailyInView {
    pub(crate) points: Vec<(DateTime<Utc>, Decimal)>,
    pub(crate) from: NaiveDate,
    pub(crate) to: NaiveDate,
}

#[cfg(any(feature = "server", test))]
pub(crate) struct PriceHistoryPage {
    pub(crate) entries: Vec<PriceHistoryEntry>,
    pub(crate) total_entries: usize,
}

/// Merge shared daily points and the user's own prices, add gap rows, sort
/// and return one page (1-based).
///
/// Gaps only appear when `daily` is in view. A day is covered by a daily point
/// on that UTC date or an own price on that local date. Gaps sit between
/// covered days inside the daily window, plus a trailing gap up to
/// `today_utc - 1`; never before the first saved price and never for today,
/// whose daily price may not be published yet.
#[cfg(any(feature = "server", test))]
pub(crate) fn build_price_history_page(
    daily: Option<DailyInView>,
    own: Vec<OwnPrice>,
    today_utc: NaiveDate,
    sort: PriceHistorySort,
    page: usize,
) -> PriceHistoryPage {
    let mut keyed: Vec<(DateTime<Utc>, PriceHistoryEntry)> = Vec::new();
    // Each covered day with its latest entry instant; a gap after that day sorts
    // right behind it, even when an own price's UTC instant falls on a later date.
    let mut covered: Vec<(NaiveDate, DateTime<Utc>)> = Vec::new();

    let window = daily.as_ref().map(|daily| (daily.from, daily.to));
    for (at, price) in daily.map(|daily| daily.points).unwrap_or_default() {
        let date_utc = at.date_naive();
        covered.push((date_utc, at));
        keyed.push((at, PriceHistoryEntry::Daily { date_utc, price }));
    }
    for own_price in own {
        covered.push((own_price.local_date, own_price.at));
        keyed.push((
            own_price.at,
            PriceHistoryEntry::YourPrice {
                at: own_price.at,
                price: own_price.price,
                note: own_price.note,
            },
        ));
    }

    if let Some((from, to)) = window {
        covered.retain(|(date, _)| *date < today_utc && from <= *date && *date <= to);
        covered.sort_by_key(|(date, at)| (*date, std::cmp::Reverse(*at)));
        covered.dedup_by_key(|(date, _)| *date);
        for pair in covered.windows(2) {
            if let [(before, key), (after, _)] = pair {
                push_gap(&mut keyed, *key, before.succ_opt(), after.pred_opt());
            }
        }
        if let (Some((last, key)), Some(yesterday)) = (covered.last(), today_utc.pred_opt()) {
            push_gap(&mut keyed, *key, last.succ_opt(), Some(yesterday.min(to)));
        }
    }

    keyed.sort_by_key(|(key, _)| *key);
    if sort == PriceHistorySort::NewestFirst {
        keyed.reverse();
    }

    let total_entries = keyed.len();
    let start = page
        .saturating_sub(1)
        .saturating_mul(PRICE_HISTORY_PAGE_SIZE);
    let entries = keyed
        .into_iter()
        .skip(start)
        .take(PRICE_HISTORY_PAGE_SIZE)
        .map(|(_, entry)| entry)
        .collect();

    PriceHistoryPage {
        entries,
        total_entries,
    }
}

#[cfg(any(feature = "server", test))]
fn push_gap(
    keyed: &mut Vec<(DateTime<Utc>, PriceHistoryEntry)>,
    key: DateTime<Utc>,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
) {
    // Pushed after every entry, so the stable sort keeps it behind an equal key.
    if let (Some(from), Some(to)) = (from, to)
        && from <= to
    {
        keyed.push((key, PriceHistoryEntry::Gap { from, to }));
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn d(raw: &str) -> NaiveDate {
        NaiveDate::parse_from_str(raw, "%Y-%m-%d").expect("date")
    }

    fn midnight_utc(date: NaiveDate) -> DateTime<Utc> {
        date.and_time(chrono::NaiveTime::MIN).and_utc()
    }

    fn p(raw: &str) -> Decimal {
        Decimal::from_str(raw).expect("decimal")
    }

    fn daily(dates: &[&str]) -> Vec<(DateTime<Utc>, Decimal)> {
        dates
            .iter()
            .map(|date| (midnight_utc(d(date)), p("1")))
            .collect()
    }

    /// Daily points in view from the start of 2026 up to `to`.
    fn shared(points: Vec<(DateTime<Utc>, Decimal)>, to: &str) -> Option<DailyInView> {
        Some(DailyInView {
            points,
            from: d("2026-01-01"),
            to: d(to),
        })
    }

    fn shape(entries: &[PriceHistoryEntry]) -> Vec<String> {
        entries
            .iter()
            .map(|entry| match entry {
                PriceHistoryEntry::Daily { date_utc, .. } => format!("D{date_utc}"),
                PriceHistoryEntry::YourPrice { at, .. } => format!("Y{at}"),
                PriceHistoryEntry::Gap { from, to } => format!("G{from}..{to}"),
            })
            .collect()
    }

    #[test]
    fn gaps_between_and_after_but_not_before_or_today() {
        let page = build_price_history_page(
            shared(
                daily(&["2026-11-01", "2026-11-02", "2026-11-05"]),
                "2026-12-31",
            ),
            Vec::new(),
            d("2026-11-08"),
            PriceHistorySort::OldestFirst,
            1,
        );

        assert_eq!(
            shape(&page.entries),
            vec![
                "D2026-11-01",
                "D2026-11-02",
                "G2026-11-03..2026-11-04",
                "D2026-11-05",
                "G2026-11-06..2026-11-07",
            ]
        );
    }

    #[test]
    fn trailing_gap_stops_at_range_end_and_is_absent_when_caught_up() {
        let clipped = build_price_history_page(
            shared(daily(&["2026-10-01"]), "2026-10-03"),
            Vec::new(),
            d("2026-11-08"),
            PriceHistorySort::OldestFirst,
            1,
        );
        assert_eq!(
            shape(&clipped.entries),
            vec!["D2026-10-01", "G2026-10-02..2026-10-03"]
        );

        let caught_up = build_price_history_page(
            shared(daily(&["2026-11-07"]), "2026-12-31"),
            Vec::new(),
            d("2026-11-08"),
            PriceHistorySort::OldestFirst,
            1,
        );
        assert_eq!(shape(&caught_up.entries), vec!["D2026-11-07"]);
    }

    #[test]
    fn own_price_covers_its_local_date_and_sorts_by_utc_instant() {
        let at = DateTime::parse_from_rfc3339("2026-11-02T04:00:00Z")
            .expect("instant")
            .with_timezone(&Utc);
        let page = build_price_history_page(
            shared(daily(&["2026-10-31", "2026-11-02"]), "2026-11-02"),
            vec![OwnPrice {
                at,
                local_date: d("2026-11-01"),
                price: p("2"),
                note: Some("Kraken".to_string()),
            }],
            d("2026-11-03"),
            PriceHistorySort::NewestFirst,
            1,
        );

        assert_eq!(
            shape(&page.entries),
            vec!["Y2026-11-02 04:00:00 UTC", "D2026-11-02", "D2026-10-31"]
        );
    }

    #[test]
    fn gap_after_own_price_sorts_behind_it_when_its_utc_date_is_later() {
        // 2026-11-03 20:00 in Los Angeles is 2026-11-04 04:00 UTC.
        let at = DateTime::parse_from_rfc3339("2026-11-04T04:00:00Z")
            .expect("instant")
            .with_timezone(&Utc);
        let own = || {
            vec![OwnPrice {
                at,
                local_date: d("2026-11-03"),
                price: p("2"),
                note: None,
            }]
        };
        let rows = || daily(&["2026-11-01", "2026-11-05"]);
        let expected = vec![
            "D2026-11-01",
            "G2026-11-02..2026-11-02",
            "Y2026-11-04 04:00:00 UTC",
            "G2026-11-04..2026-11-04",
            "D2026-11-05",
        ];
        let oldest = build_price_history_page(
            shared(rows(), "2026-11-05"),
            own(),
            d("2026-11-06"),
            PriceHistorySort::OldestFirst,
            1,
        );
        assert_eq!(shape(&oldest.entries), expected);

        let newest = build_price_history_page(
            shared(rows(), "2026-11-05"),
            own(),
            d("2026-11-06"),
            PriceHistorySort::NewestFirst,
            1,
        );
        let mut reversed = expected;
        reversed.reverse();
        assert_eq!(shape(&newest.entries), reversed);
    }

    #[test]
    fn future_override_does_not_extend_missing_dates_past_yesterday() {
        let future = d("2026-11-12");
        let page = build_price_history_page(
            shared(daily(&["2026-11-08"]), "2026-11-12"),
            vec![OwnPrice {
                at: midnight_utc(future),
                local_date: future,
                price: p("2"),
                note: None,
            }],
            d("2026-11-10"),
            PriceHistorySort::OldestFirst,
            1,
        );
        assert_eq!(
            shape(&page.entries),
            vec![
                "D2026-11-08",
                "G2026-11-09..2026-11-09",
                "Y2026-11-12 00:00:00 UTC",
            ]
        );
    }

    #[test]
    fn no_saved_prices_means_no_entries() {
        let page = build_price_history_page(
            shared(Vec::new(), "2026-12-31"),
            Vec::new(),
            d("2026-11-08"),
            PriceHistorySort::NewestFirst,
            1,
        );
        assert!(page.entries.is_empty());
        assert_eq!(page.total_entries, 0);
    }

    #[test]
    fn pages_count_gaps_and_stay_stable() {
        // 60 consecutive days, then a gap, then one more day: 62 entries.
        let mut dates = (0..60)
            .map(|offset| d("2026-01-01") + chrono::Days::new(offset))
            .collect::<Vec<_>>();
        dates.push(d("2026-03-10"));
        let rows = dates
            .iter()
            .map(|date| (midnight_utc(*date), p("1")))
            .collect::<Vec<_>>();

        let first = build_price_history_page(
            shared(rows.clone(), "2026-03-10"),
            Vec::new(),
            d("2026-03-11"),
            PriceHistorySort::OldestFirst,
            1,
        );
        let second = build_price_history_page(
            shared(rows, "2026-03-10"),
            Vec::new(),
            d("2026-03-11"),
            PriceHistorySort::OldestFirst,
            2,
        );

        assert_eq!(first.total_entries, 62);
        assert_eq!(second.total_entries, 62);
        assert_eq!(first.entries.len(), PRICE_HISTORY_PAGE_SIZE);
        assert_eq!(
            shape(&second.entries[10..]),
            vec!["G2026-03-02..2026-03-09", "D2026-03-10"]
        );
    }

    #[test]
    fn own_prices_alone_have_no_gap_rows() {
        let own = |date: &str| OwnPrice {
            at: midnight_utc(d(date)),
            local_date: d(date),
            price: p("2"),
            note: None,
        };
        let page = build_price_history_page(
            None,
            vec![own("2026-03-01"), own("2026-06-01")],
            d("2026-11-08"),
            PriceHistorySort::OldestFirst,
            1,
        );
        assert_eq!(
            shape(&page.entries),
            vec!["Y2026-03-01 00:00:00 UTC", "Y2026-06-01 00:00:00 UTC"]
        );
    }

    #[test]
    fn own_price_before_the_daily_window_does_not_open_a_gap() {
        let page = build_price_history_page(
            Some(DailyInView {
                points: daily(&["2026-11-06", "2026-11-07"]),
                from: d("2026-11-06"),
                to: d("2026-11-07"),
            }),
            vec![OwnPrice {
                at: midnight_utc(d("2026-01-15")),
                local_date: d("2026-01-15"),
                price: p("2"),
                note: None,
            }],
            d("2026-11-08"),
            PriceHistorySort::OldestFirst,
            1,
        );
        assert_eq!(
            shape(&page.entries),
            vec!["Y2026-01-15 00:00:00 UTC", "D2026-11-06", "D2026-11-07"]
        );
    }
}
