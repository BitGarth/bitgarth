use super::BudgetResult;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct RequestOutcome {
    pub(super) latency_ms: f64,
    pub(super) success: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct LatencySummary {
    pub(super) median_ms: f64,
    pub(super) p95_ms: f64,
    pub(super) max_ms: f64,
    pub(super) min_ms: f64,
    pub(super) error_count: u32,
    pub(super) success_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PerfBudget {
    pub(super) median_ms: Option<f64>,
    pub(super) p95_ms: Option<f64>,
    pub(super) max_ms: Option<f64>,
    pub(super) max_error_count: Option<u32>,
    pub(super) strict: bool,
}

pub(super) fn summarize_outcomes(outcomes: &[RequestOutcome]) -> LatencySummary {
    let mut successes = outcomes
        .iter()
        .filter(|outcome| outcome.success)
        .map(|outcome| outcome.latency_ms)
        .collect::<Vec<_>>();
    successes.sort_by(f64::total_cmp);

    let error_count = outcomes.iter().filter(|outcome| !outcome.success).count() as u32;
    let success_count = successes.len() as u32;

    if successes.is_empty() {
        return LatencySummary {
            median_ms: 0.0,
            p95_ms: 0.0,
            max_ms: 0.0,
            min_ms: 0.0,
            error_count,
            success_count,
        };
    }

    LatencySummary {
        median_ms: percentile(&successes, 0.50),
        p95_ms: percentile(&successes, 0.95),
        max_ms: *successes.last().unwrap_or(&0.0),
        min_ms: *successes.first().unwrap_or(&0.0),
        error_count,
        success_count,
    }
}

fn percentile(sorted_samples: &[f64], percentile: f64) -> f64 {
    if sorted_samples.is_empty() {
        return 0.0;
    }

    let last_index = sorted_samples.len().saturating_sub(1);
    let index = ((last_index as f64) * percentile).round() as usize;
    sorted_samples[index.min(last_index)]
}

pub(super) fn evaluate_budget(summary: LatencySummary, budget: PerfBudget) -> BudgetResult {
    let within_limits = budget
        .median_ms
        .is_none_or(|limit| summary.median_ms <= limit)
        && budget.p95_ms.is_none_or(|limit| summary.p95_ms <= limit)
        && budget.max_ms.is_none_or(|limit| summary.max_ms <= limit)
        && budget
            .max_error_count
            .is_none_or(|limit| summary.error_count <= limit);

    if !budget.strict {
        return BudgetResult::ReportOnly;
    }

    if within_limits {
        BudgetResult::Passed
    } else {
        BudgetResult::Failed
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn summarize_outcomes_calculates_expected_percentiles() {
        let summary = summarize_outcomes(&[
            RequestOutcome {
                latency_ms: 10.0,
                success: true,
            },
            RequestOutcome {
                latency_ms: 20.0,
                success: true,
            },
            RequestOutcome {
                latency_ms: 30.0,
                success: true,
            },
            RequestOutcome {
                latency_ms: 40.0,
                success: true,
            },
            RequestOutcome {
                latency_ms: 50.0,
                success: true,
            },
            RequestOutcome {
                latency_ms: 60.0,
                success: false,
            },
        ]);

        assert_eq!(summary.min_ms, 10.0);
        assert_eq!(summary.median_ms, 30.0);
        assert_eq!(summary.p95_ms, 50.0);
        assert_eq!(summary.max_ms, 50.0);
        assert_eq!(summary.success_count, 5);
        assert_eq!(summary.error_count, 1);
    }

    #[test]
    fn evaluate_budget_returns_report_only_when_not_strict() {
        let result = evaluate_budget(
            LatencySummary {
                median_ms: 10.0,
                p95_ms: 20.0,
                max_ms: 30.0,
                min_ms: 5.0,
                error_count: 0,
                success_count: 1,
            },
            PerfBudget {
                median_ms: Some(1.0),
                p95_ms: Some(1.0),
                max_ms: Some(1.0),
                max_error_count: Some(0),
                strict: false,
            },
        );

        assert_eq!(result, BudgetResult::ReportOnly);
    }
}
