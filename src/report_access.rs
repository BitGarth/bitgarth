#[cfg(any(feature = "server", test))]
use crate::report_dates::LocalReportDateRange;
#[cfg(any(feature = "server", test))]
use chrono::Months;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ReportAccessGate {
    Full,
    RollingNineMonthWindow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReportAccessView {
    pub(crate) requested_from: NaiveDate,
    pub(crate) requested_to: NaiveDate,
    pub(crate) effective_from: NaiveDate,
    pub(crate) effective_to: NaiveDate,
    pub(crate) gate: ReportAccessGate,
    pub(crate) range_clamped: bool,
    pub(crate) can_edit_prices: bool,
}

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReportAccessEntitlements {
    pub(crate) tax_reports: bool,
    pub(crate) exchange_rates_history: bool,
    pub(crate) price_overrides: bool,
}

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReportAccessDecision {
    pub(crate) access: ReportAccessView,
}

#[cfg(any(feature = "server", test))]
pub(crate) fn decide_report_access(
    requested_range: LocalReportDateRange,
    today: NaiveDate,
    entitlements: ReportAccessEntitlements,
) -> ReportAccessDecision {
    let requested_from = requested_range.from();
    let requested_to = requested_range.to();
    let full_report_access = entitlements.tax_reports;
    if full_report_access {
        return ReportAccessDecision {
            access: ReportAccessView {
                requested_from,
                requested_to,
                effective_from: requested_from,
                effective_to: requested_to,
                gate: ReportAccessGate::Full,
                range_clamped: false,
                can_edit_prices: entitlements.price_overrides,
            },
        };
    }

    let allowed_from = today.checked_sub_months(Months::new(9)).unwrap_or(today);
    let mut effective_from = requested_from.max(allowed_from);
    let mut effective_to = requested_to.min(today);

    if effective_from > effective_to {
        effective_from = allowed_from;
        effective_to = today;
    }

    ReportAccessDecision {
        access: ReportAccessView {
            requested_from,
            requested_to,
            effective_from,
            effective_to,
            gate: ReportAccessGate::RollingNineMonthWindow,
            range_clamped: effective_from != requested_from || effective_to != requested_to,
            can_edit_prices: entitlements.price_overrides,
        },
    }
}

#[cfg(test)]
mod tests {
    use crate::report_dates::LocalReportDateRange;

    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("valid date")
    }

    fn range(from: NaiveDate, to: NaiveDate) -> LocalReportDateRange {
        LocalReportDateRange::new(from, to).expect("valid range")
    }

    fn limited() -> ReportAccessEntitlements {
        ReportAccessEntitlements {
            tax_reports: false,
            exchange_rates_history: false,
            price_overrides: false,
        }
    }

    fn full() -> ReportAccessEntitlements {
        ReportAccessEntitlements {
            tax_reports: true,
            exchange_rates_history: true,
            price_overrides: true,
        }
    }

    fn tax_reports_only() -> ReportAccessEntitlements {
        ReportAccessEntitlements {
            tax_reports: true,
            exchange_rates_history: false,
            price_overrides: true,
        }
    }

    fn exchange_rates_history_only() -> ReportAccessEntitlements {
        ReportAccessEntitlements {
            tax_reports: false,
            exchange_rates_history: true,
            price_overrides: true,
        }
    }

    #[test]
    fn full_entitlements_do_not_clamp_range() {
        let decision = decide_report_access(
            range(date(2025, 1, 1), date(2025, 12, 31)),
            date(2026, 7, 3),
            full(),
        );

        assert_eq!(decision.access.effective_from, date(2025, 1, 1));
        assert_eq!(decision.access.effective_to, date(2025, 12, 31));
        assert_eq!(decision.access.gate, ReportAccessGate::Full);
        assert!(!decision.access.range_clamped);
        assert!(decision.access.can_edit_prices);
    }

    #[test]
    fn limited_entitlements_clip_start_to_rolling_nine_month_window() {
        let decision = decide_report_access(
            range(date(2026, 1, 1), date(2026, 12, 31)),
            date(2026, 12, 15),
            limited(),
        );

        assert_eq!(decision.access.effective_from, date(2026, 3, 15));
        assert_eq!(decision.access.effective_to, date(2026, 12, 15));
        assert_eq!(
            decision.access.gate,
            ReportAccessGate::RollingNineMonthWindow
        );
        assert!(decision.access.range_clamped);
        assert!(!decision.access.can_edit_prices);
    }

    #[test]
    fn limited_entitlements_leave_in_window_custom_range_unchanged() {
        let decision = decide_report_access(
            range(date(2026, 5, 1), date(2026, 6, 1)),
            date(2026, 7, 3),
            limited(),
        );

        assert_eq!(decision.access.effective_from, date(2026, 5, 1));
        assert_eq!(decision.access.effective_to, date(2026, 6, 1));
        assert!(!decision.access.range_clamped);
    }

    #[test]
    fn limited_entitlements_use_current_window_when_request_has_no_overlap() {
        let decision = decide_report_access(
            range(date(2025, 1, 1), date(2025, 12, 31)),
            date(2026, 12, 15),
            limited(),
        );

        assert_eq!(decision.access.effective_from, date(2026, 3, 15));
        assert_eq!(decision.access.effective_to, date(2026, 12, 15));
        assert!(decision.access.range_clamped);
    }

    #[test]
    fn tax_reports_without_exchange_rates_history_grants_full_range() {
        let decision = decide_report_access(
            range(date(2026, 1, 1), date(2026, 12, 31)),
            date(2026, 12, 15),
            tax_reports_only(),
        );

        assert_eq!(decision.access.effective_from, date(2026, 1, 1));
        assert_eq!(decision.access.effective_to, date(2026, 12, 31));
        assert_eq!(decision.access.gate, ReportAccessGate::Full);
        assert!(!decision.access.range_clamped);
    }

    #[test]
    fn exchange_rates_history_without_tax_reports_uses_rolling_nine_month_window() {
        let decision = decide_report_access(
            range(date(2026, 1, 1), date(2026, 12, 31)),
            date(2026, 12, 15),
            exchange_rates_history_only(),
        );

        assert_eq!(decision.access.effective_from, date(2026, 3, 15));
        assert_eq!(decision.access.effective_to, date(2026, 12, 15));
        assert_eq!(
            decision.access.gate,
            ReportAccessGate::RollingNineMonthWindow
        );
        assert!(decision.access.range_clamped);
    }
}
