use crate::models::UserTimezone;
use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DateBoundaryKind {
    StartOfDay,
    EndOfDay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LocalReportDateRange {
    from: NaiveDate,
    to: NaiveDate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalReportDateRangeError {
    InvertedRange,
}

impl LocalReportDateRange {
    pub(crate) fn new(from: NaiveDate, to: NaiveDate) -> Result<Self, LocalReportDateRangeError> {
        if from > to {
            return Err(LocalReportDateRangeError::InvertedRange);
        }
        Ok(Self { from, to })
    }

    pub(crate) fn from(self) -> NaiveDate {
        self.from
    }

    pub(crate) fn to(self) -> NaiveDate {
        self.to
    }

    /// `Some(year)` when the range spans exactly Jan 1 – Dec 31 of one calendar year.
    pub(crate) fn full_calendar_year(self) -> Option<i32> {
        let year = self.from.year();
        let starts_on_jan_first = self.from.month() == 1 && self.from.day() == 1;
        let ends_on_dec_last = self.to == NaiveDate::from_ymd_opt(year, 12, 31)?;
        if starts_on_jan_first && ends_on_dec_last {
            Some(year)
        } else {
            None
        }
    }
}

/// Full calendar-year range (Jan 1 – Dec 31) for `year`.
pub(crate) fn calendar_year_range(year: i32) -> Option<LocalReportDateRange> {
    LocalReportDateRange::new(
        NaiveDate::from_ymd_opt(year, 1, 1)?,
        NaiveDate::from_ymd_opt(year, 12, 31)?,
    )
    .ok()
}

/// The year the dial should display for `range`, or `None` for a custom span.
///
/// A range that exactly matches `current_year_preset` (which may be year-to-date)
/// shows that preset's year; any full calendar year shows its own year.
pub(crate) fn displayed_calendar_year(
    range: LocalReportDateRange,
    current_year_preset: Option<LocalReportDateRange>,
) -> Option<i32> {
    if let Some(year) = range.full_calendar_year() {
        return Some(year);
    }
    current_year_preset
        .filter(|preset| *preset == range)
        .map(|preset| preset.from().year())
}

/// The range to apply when stepping the dial to `target_year`.
///
/// Stepping onto the current year reuses `current_year_preset` (so a year-to-date
/// span is preserved); any other year uses the full calendar year.
pub(crate) fn dial_year_range(
    target_year: i32,
    current_year: i32,
    current_year_preset: Option<LocalReportDateRange>,
) -> Option<LocalReportDateRange> {
    if target_year == current_year {
        current_year_preset
    } else {
        calendar_year_range(target_year)
    }
}

impl std::fmt::Display for LocalReportDateRangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvertedRange => write!(f, "Start date must be on or before end date"),
        }
    }
}

pub(crate) fn local_report_date_to_utc_boundary(
    date: NaiveDate,
    timezone: UserTimezone,
    kind: DateBoundaryKind,
) -> DateTime<Utc> {
    let tz: Tz = timezone.into();
    match kind {
        DateBoundaryKind::StartOfDay => {
            let naive = match date.and_hms_opt(0, 0, 0) {
                Some(value) => value,
                None => {
                    return DateTime::<Utc>::from_naive_utc_and_offset(
                        date.and_time(chrono::NaiveTime::MIN),
                        Utc,
                    );
                }
            };
            resolve_local_datetime(tz, naive).with_timezone(&Utc)
        }
        DateBoundaryKind::EndOfDay => {
            let next_day = match date.succ_opt() {
                Some(value) => value,
                None => {
                    let naive = match date.and_hms_opt(23, 59, 59) {
                        Some(value) => value,
                        None => {
                            return DateTime::<Utc>::from_naive_utc_and_offset(
                                date.and_time(chrono::NaiveTime::MIN),
                                Utc,
                            );
                        }
                    };
                    return resolve_local_datetime(tz, naive).with_timezone(&Utc);
                }
            };
            let next_day_start = match next_day.and_hms_opt(0, 0, 0) {
                Some(value) => value,
                None => {
                    return DateTime::<Utc>::from_naive_utc_and_offset(
                        next_day.and_time(chrono::NaiveTime::MIN),
                        Utc,
                    );
                }
            };
            (resolve_local_datetime(tz, next_day_start).with_timezone(&Utc)) - Duration::seconds(1)
        }
    }
}

fn resolve_local_datetime(timezone: Tz, naive: NaiveDateTime) -> DateTime<Tz> {
    match timezone.from_local_datetime(&naive) {
        LocalResult::Single(value) => value,
        LocalResult::Ambiguous(first, _) => first,
        LocalResult::None => resolve_shifted_local_datetime(timezone, naive),
    }
}

fn resolve_shifted_local_datetime(timezone: Tz, naive: NaiveDateTime) -> DateTime<Tz> {
    let mut candidate = naive;
    for _ in 0..180 {
        candidate += Duration::minutes(1);
        match timezone.from_local_datetime(&candidate) {
            LocalResult::Single(value) => return value,
            LocalResult::Ambiguous(first, _) => return first,
            LocalResult::None => {}
        }
    }

    DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc).with_timezone(&timezone)
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("valid date")
    }

    #[test]
    fn local_report_date_range_accepts_ordered_dates() {
        let from = NaiveDate::from_ymd_opt(2026, 1, 1).expect("valid date");
        let to = NaiveDate::from_ymd_opt(2026, 12, 31).expect("valid date");

        let range = LocalReportDateRange::new(from, to).expect("range should validate");
        assert_eq!(range.from(), from);
        assert_eq!(range.to(), to);
    }

    #[test]
    fn full_calendar_year_recognizes_jan_to_dec_span() {
        let range = LocalReportDateRange::new(date(2024, 1, 1), date(2024, 12, 31)).expect("valid");
        assert_eq!(range.full_calendar_year(), Some(2024));
    }

    #[test]
    fn full_calendar_year_rejects_year_to_date_span() {
        let range = LocalReportDateRange::new(date(2026, 1, 1), date(2026, 3, 31)).expect("valid");
        assert_eq!(range.full_calendar_year(), None);
    }

    #[test]
    fn full_calendar_year_rejects_non_january_start() {
        let range = LocalReportDateRange::new(date(2024, 2, 1), date(2024, 12, 31)).expect("valid");
        assert_eq!(range.full_calendar_year(), None);
    }

    #[test]
    fn calendar_year_range_spans_whole_year() {
        let range = calendar_year_range(2023).expect("range should exist");
        assert_eq!(range.from(), date(2023, 1, 1));
        assert_eq!(range.to(), date(2023, 12, 31));
    }

    #[test]
    fn displayed_calendar_year_uses_full_year_directly() {
        let range = calendar_year_range(2022).expect("range");
        assert_eq!(displayed_calendar_year(range, None), Some(2022));
    }

    #[test]
    fn displayed_calendar_year_matches_year_to_date_preset() {
        let ytd = LocalReportDateRange::new(date(2026, 1, 1), date(2026, 3, 31)).expect("valid");
        assert_eq!(displayed_calendar_year(ytd, Some(ytd)), Some(2026));
    }

    #[test]
    fn displayed_calendar_year_returns_none_for_custom_span() {
        let custom = LocalReportDateRange::new(date(2025, 3, 1), date(2026, 8, 15)).expect("valid");
        let preset = LocalReportDateRange::new(date(2026, 1, 1), date(2026, 3, 31)).expect("valid");
        assert_eq!(displayed_calendar_year(custom, Some(preset)), None);
    }

    #[test]
    fn dial_year_range_reuses_preset_for_current_year() {
        let ytd = LocalReportDateRange::new(date(2026, 1, 1), date(2026, 3, 31)).expect("valid");
        assert_eq!(dial_year_range(2026, 2026, Some(ytd)), Some(ytd));
    }

    #[test]
    fn dial_year_range_uses_full_year_for_other_years() {
        assert_eq!(dial_year_range(2024, 2026, None), calendar_year_range(2024));
    }

    #[test]
    fn local_report_date_range_rejects_inverted_dates() {
        let from = NaiveDate::from_ymd_opt(2026, 12, 31).expect("valid date");
        let to = NaiveDate::from_ymd_opt(2026, 1, 1).expect("valid date");

        let result = LocalReportDateRange::new(from, to);
        assert_eq!(result, Err(LocalReportDateRangeError::InvertedRange));
    }

    #[test]
    fn start_of_day_uses_user_timezone_in_winter() {
        let date = NaiveDate::from_ymd_opt(2026, 1, 21).expect("valid date");
        let boundary = local_report_date_to_utc_boundary(
            date,
            UserTimezone("America/New_York".parse().expect("valid timezone")),
            DateBoundaryKind::StartOfDay,
        );

        assert_eq!(
            boundary,
            Utc.with_ymd_and_hms(2026, 1, 21, 5, 0, 0)
                .single()
                .expect("valid timestamp")
        );
    }

    #[test]
    fn end_of_day_uses_user_timezone_in_winter() {
        let date = NaiveDate::from_ymd_opt(2026, 1, 21).expect("valid date");
        let boundary = local_report_date_to_utc_boundary(
            date,
            UserTimezone("America/New_York".parse().expect("valid timezone")),
            DateBoundaryKind::EndOfDay,
        );

        assert_eq!(
            boundary,
            Utc.with_ymd_and_hms(2026, 1, 22, 4, 59, 59)
                .single()
                .expect("valid timestamp")
        );
    }

    #[test]
    fn end_of_day_handles_dst_short_day() {
        let date = NaiveDate::from_ymd_opt(2026, 3, 29).expect("valid date");
        let boundary = local_report_date_to_utc_boundary(
            date,
            UserTimezone("Europe/Amsterdam".parse().expect("valid timezone")),
            DateBoundaryKind::EndOfDay,
        );

        assert_eq!(
            boundary,
            Utc.with_ymd_and_hms(2026, 3, 29, 21, 59, 59)
                .single()
                .expect("valid timestamp")
        );
    }
}
