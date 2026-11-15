use super::planner::DateSpan;
use chrono::{Duration, NaiveDate};

/// Pick the newest contiguous gap in this lane. Cooldowns and actual prices
/// are both excluded for scheduling, but only actual prices satisfy coverage.
pub(super) fn next_gap(
    window: DateSpan,
    mut excluded: Vec<DateSpan>,
    probe_end: Option<NaiveDate>,
) -> Option<DateSpan> {
    excluded.sort_unstable_by_key(|s| std::cmp::Reverse(s.end));
    let mut end = window.end;
    let mut start = window.start;
    for span in excluded {
        if span.start > end {
            continue;
        }
        if span.end < end {
            start = start.max(span.end.succ_opt()?);
            break;
        }
        end = span.start.pred_opt()?;
        if end < start {
            return None;
        }
    }
    start = start.max(end - Duration::days(364));
    if let Some(probe) = probe_end
        && start < end
        && start <= probe
    {
        let upper = end.min(probe);
        start += Duration::days((upper - start).num_days().div_euclid(2));
    }
    Some(DateSpan { start, end })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn day(n: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 5, n).unwrap()
    }
    fn span(a: u32, b: u32) -> DateSpan {
        DateSpan {
            start: day(a),
            end: day(b),
        }
    }
    #[test]
    fn gaps_survive_overlaps_and_provider_boundaries() {
        assert_eq!(
            next_gap(
                span(1, 31),
                vec![span(1, 5), span(17, 31), span(15, 20)],
                None
            ),
            Some(span(6, 14))
        );
        assert_eq!(
            next_gap(span(1, 31), vec![span(1, 15)], Some(day(31))),
            Some(span(23, 31))
        );
        assert_eq!(next_gap(span(1, 31), vec![span(1, 31)], None), None);
    }
}
