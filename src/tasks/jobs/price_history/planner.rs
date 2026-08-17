use chrono::{Duration, NaiveDate};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DateSpan {
    pub(crate) start: NaiveDate,
    pub(crate) end: NaiveDate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProviderRange {
    pub(crate) start: NaiveDate,
    pub(crate) end: NaiveDate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PriceHistoryHorizon {
    PublicKeyless,
    Pro,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FairPriceHistorySpanWork {
    pub(crate) asset_index: usize,
    pub(crate) span: DateSpan,
}

pub(crate) fn requested_start_date(first_owned: NaiveDate, today: NaiveDate) -> NaiveDate {
    let ten_year_cap = today - Duration::days((365 * 10) - 1);
    first_owned.max(ten_year_cap)
}

pub(crate) fn requested_start_date_for_horizon(
    first_owned: NaiveDate,
    today: NaiveDate,
    horizon: PriceHistoryHorizon,
) -> NaiveDate {
    match horizon {
        PriceHistoryHorizon::PublicKeyless => {
            let public_cap = today - Duration::days(364);
            first_owned.max(public_cap)
        }
        PriceHistoryHorizon::Pro => requested_start_date(first_owned, today),
    }
}

pub(crate) fn missing_daily_spans(requested: DateSpan, existing: &[NaiveDate]) -> Vec<DateSpan> {
    if requested.start > requested.end {
        return Vec::new();
    }

    let existing: std::collections::HashSet<NaiveDate> = existing.iter().copied().collect();
    let mut spans = Vec::new();
    let mut cursor = requested.start;
    let mut span_start = None;

    while cursor <= requested.end {
        if existing.contains(&cursor) {
            if let Some(start) = span_start.take() {
                spans.push(DateSpan {
                    start,
                    end: cursor - Duration::days(1),
                });
            }
        } else if span_start.is_none() {
            span_start = Some(cursor);
        }
        cursor += Duration::days(1);
    }

    if let Some(start) = span_start {
        spans.push(DateSpan {
            start,
            end: requested.end,
        });
    }

    spans
}

pub(crate) fn missing_daily_spans_newest_first(
    requested: DateSpan,
    existing: &[NaiveDate],
) -> Vec<DateSpan> {
    let mut spans = missing_daily_spans(requested, existing);
    spans.reverse();
    spans
}

pub(crate) fn fair_missing_span_work(
    per_asset_spans: Vec<(usize, Vec<DateSpan>)>,
) -> Vec<FairPriceHistorySpanWork> {
    let max_rounds = per_asset_spans
        .iter()
        .map(|(_asset_index, spans)| spans.len())
        .max()
        .unwrap_or(0);
    let mut work = Vec::new();

    for round in 0..max_rounds {
        for (asset_index, spans) in &per_asset_spans {
            if let Some(span) = spans.get(round).copied() {
                work.push(FairPriceHistorySpanWork {
                    asset_index: *asset_index,
                    span,
                });
            }
        }
    }

    work
}

pub(crate) fn backward_provider_ranges(start: NaiveDate, end: NaiveDate) -> Vec<ProviderRange> {
    if start > end {
        return Vec::new();
    }

    let mut ranges = Vec::new();
    let mut cursor_end = end;
    loop {
        let cursor_start = (cursor_end - Duration::days(364)).max(start);
        ranges.push(ProviderRange {
            start: cursor_start,
            end: cursor_end,
        });
        if cursor_start == start {
            break;
        }
        cursor_end = cursor_start - Duration::days(1);
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(value: &str) -> NaiveDate {
        NaiveDate::parse_from_str(value, "%Y-%m-%d").expect("test date")
    }

    #[test]
    fn missing_daily_spans_group_contiguous_missing_dates() {
        let requested = DateSpan {
            start: d("2026-01-01"),
            end: d("2026-01-07"),
        };
        let existing = vec![d("2026-01-02"), d("2026-01-03"), d("2026-01-06")];

        assert_eq!(
            missing_daily_spans(requested, &existing),
            vec![
                DateSpan {
                    start: d("2026-01-01"),
                    end: d("2026-01-01"),
                },
                DateSpan {
                    start: d("2026-01-04"),
                    end: d("2026-01-05"),
                },
                DateSpan {
                    start: d("2026-01-07"),
                    end: d("2026-01-07"),
                },
            ]
        );
    }

    #[test]
    fn missing_daily_spans_returns_empty_for_fully_covered_range() {
        let requested = DateSpan {
            start: d("2026-01-01"),
            end: d("2026-01-03"),
        };
        let existing = vec![d("2026-01-01"), d("2026-01-02"), d("2026-01-03")];

        assert_eq!(
            missing_daily_spans(requested, &existing),
            Vec::<DateSpan>::new()
        );
    }

    #[test]
    fn backward_provider_ranges_are_365_days_or_less() {
        let ranges = backward_provider_ranges(d("2024-01-01"), d("2026-01-10"));

        assert_eq!(
            ranges,
            vec![
                ProviderRange {
                    start: d("2025-01-11"),
                    end: d("2026-01-10"),
                },
                ProviderRange {
                    start: d("2024-01-12"),
                    end: d("2025-01-10"),
                },
                ProviderRange {
                    start: d("2024-01-01"),
                    end: d("2024-01-11"),
                },
            ]
        );
        assert_eq!(ranges.first().map(|range| range.end), Some(d("2026-01-10")));
        assert_eq!(
            ranges.last().map(|range| range.start),
            Some(d("2024-01-01"))
        );
        assert!(
            ranges
                .windows(2)
                .all(|pair| pair[0].start == pair[1].end + Duration::days(1))
        );
        assert!(ranges.iter().all(|range| {
            let days = (range.end - range.start).num_days() + 1;
            (1..=365).contains(&days)
        }));
    }

    #[test]
    fn requested_start_uses_first_owned_or_ten_year_cap() {
        let today = d("2026-06-10");
        assert_eq!(
            requested_start_date(d("2020-01-01"), today),
            d("2020-01-01")
        );
        assert_eq!(
            requested_start_date(d("2010-01-01"), today),
            d("2016-06-13")
        );
    }

    #[test]
    fn requested_start_uses_public_keyless_365_day_horizon() {
        let today = d("2026-06-12");

        assert_eq!(
            requested_start_date_for_horizon(
                d("2010-01-01"),
                today,
                PriceHistoryHorizon::PublicKeyless
            ),
            d("2025-06-13")
        );
        assert_eq!(
            requested_start_date_for_horizon(
                d("2026-02-07"),
                today,
                PriceHistoryHorizon::PublicKeyless
            ),
            d("2026-02-07")
        );
    }

    #[test]
    fn requested_start_uses_pro_ten_year_horizon() {
        let today = d("2026-06-12");

        assert_eq!(
            requested_start_date_for_horizon(d("2010-01-01"), today, PriceHistoryHorizon::Pro),
            d("2016-06-15")
        );
        assert_eq!(
            requested_start_date_for_horizon(d("2020-01-01"), today, PriceHistoryHorizon::Pro),
            d("2020-01-01")
        );
    }

    #[test]
    fn missing_daily_spans_newest_first_reverses_contiguous_gaps() {
        let requested = DateSpan {
            start: d("2026-01-01"),
            end: d("2026-01-10"),
        };
        let existing = vec![d("2026-01-03"), d("2026-01-04"), d("2026-01-08")];

        assert_eq!(
            missing_daily_spans_newest_first(requested, &existing),
            vec![
                DateSpan {
                    start: d("2026-01-09"),
                    end: d("2026-01-10"),
                },
                DateSpan {
                    start: d("2026-01-05"),
                    end: d("2026-01-07"),
                },
                DateSpan {
                    start: d("2026-01-01"),
                    end: d("2026-01-02"),
                },
            ]
        );
    }

    #[test]
    fn fair_missing_span_work_takes_one_span_per_asset_per_round() {
        let plan = fair_missing_span_work(vec![
            (
                0,
                vec![
                    DateSpan {
                        start: d("2026-01-08"),
                        end: d("2026-01-10"),
                    },
                    DateSpan {
                        start: d("2026-01-01"),
                        end: d("2026-01-03"),
                    },
                ],
            ),
            (
                1,
                vec![DateSpan {
                    start: d("2026-01-09"),
                    end: d("2026-01-10"),
                }],
            ),
            (
                2,
                vec![
                    DateSpan {
                        start: d("2026-01-07"),
                        end: d("2026-01-10"),
                    },
                    DateSpan {
                        start: d("2026-01-04"),
                        end: d("2026-01-06"),
                    },
                ],
            ),
        ]);

        assert_eq!(
            plan,
            vec![
                FairPriceHistorySpanWork {
                    asset_index: 0,
                    span: DateSpan {
                        start: d("2026-01-08"),
                        end: d("2026-01-10"),
                    },
                },
                FairPriceHistorySpanWork {
                    asset_index: 1,
                    span: DateSpan {
                        start: d("2026-01-09"),
                        end: d("2026-01-10"),
                    },
                },
                FairPriceHistorySpanWork {
                    asset_index: 2,
                    span: DateSpan {
                        start: d("2026-01-07"),
                        end: d("2026-01-10"),
                    },
                },
                FairPriceHistorySpanWork {
                    asset_index: 0,
                    span: DateSpan {
                        start: d("2026-01-01"),
                        end: d("2026-01-03"),
                    },
                },
                FairPriceHistorySpanWork {
                    asset_index: 2,
                    span: DateSpan {
                        start: d("2026-01-04"),
                        end: d("2026-01-06"),
                    },
                },
            ]
        );
    }
}
