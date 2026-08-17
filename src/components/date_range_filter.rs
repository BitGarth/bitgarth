use crate::report_dates::{LocalReportDateRange, LocalReportDateRangeError};
use crate::wallets::ReportDateParam;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DateRangeFilterPolicy {
    RequiredCanonicalRange,
    OptionalRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DateRangeSelection {
    Empty,
    Range(LocalReportDateRange),
}

impl DateRangeSelection {
    pub(crate) fn start_param(self) -> Option<ReportDateParam> {
        match self {
            Self::Empty => None,
            Self::Range(range) => Some(ReportDateParam::from_naive_date(range.from())),
        }
    }

    pub(crate) fn end_param(self) -> Option<ReportDateParam> {
        match self {
            Self::Empty => None,
            Self::Range(range) => Some(ReportDateParam::from_naive_date(range.to())),
        }
    }

    pub(crate) fn start_query_value(self) -> Option<String> {
        self.start_param().map(|value| value.to_string())
    }

    pub(crate) fn end_query_value(self) -> Option<String> {
        self.end_param().map(|value| value.to_string())
    }

    fn matches_route(self, route: &DateRangeRouteParams) -> bool {
        match self {
            Self::Empty => route.is_empty(),
            Self::Range(range) => route.matches_range(range),
        }
    }
}

impl From<LocalReportDateRange> for DateRangeSelection {
    fn from(value: LocalReportDateRange) -> Self {
        Self::Range(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParsedDateRangeRoute {
    Empty,
    Complete(LocalReportDateRange),
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DateRangeRouteParams {
    start: Option<String>,
    end: Option<String>,
}

impl DateRangeRouteParams {
    pub(crate) fn new(start: Option<String>, end: Option<String>) -> Self {
        Self { start, end }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.start.is_none() && self.end.is_none()
    }

    fn parsed(&self) -> ParsedDateRangeRoute {
        match (self.start.as_deref(), self.end.as_deref()) {
            (None, None) => ParsedDateRangeRoute::Empty,
            (Some(raw_start), Some(raw_end)) => {
                let parsed_start = raw_start.parse::<ReportDateParam>().ok();
                let parsed_end = raw_end.parse::<ReportDateParam>().ok();
                match (parsed_start, parsed_end) {
                    (Some(start), Some(end)) => {
                        match LocalReportDateRange::new(
                            start.into_naive_date(),
                            end.into_naive_date(),
                        ) {
                            Ok(range) => ParsedDateRangeRoute::Complete(range),
                            Err(_) => ParsedDateRangeRoute::Invalid,
                        }
                    }
                    _ => ParsedDateRangeRoute::Invalid,
                }
            }
            _ => ParsedDateRangeRoute::Invalid,
        }
    }

    pub(crate) fn selection(&self) -> DateRangeSelection {
        match self.parsed() {
            ParsedDateRangeRoute::Complete(range) => DateRangeSelection::Range(range),
            ParsedDateRangeRoute::Empty | ParsedDateRangeRoute::Invalid => {
                DateRangeSelection::Empty
            }
        }
    }

    fn input_values(&self) -> DateRangeInputs {
        match self.parsed() {
            ParsedDateRangeRoute::Complete(range) => DateRangeInputs::from_selection(range.into()),
            ParsedDateRangeRoute::Empty | ParsedDateRangeRoute::Invalid => DateRangeInputs::empty(),
        }
    }

    fn allows_server_canonicalization(&self, policy: DateRangeFilterPolicy) -> bool {
        match policy {
            DateRangeFilterPolicy::RequiredCanonicalRange => {
                !matches!(self.parsed(), ParsedDateRangeRoute::Complete(_))
            }
            DateRangeFilterPolicy::OptionalRange => false,
        }
    }

    fn should_replace_invalid_route_with_empty(&self, policy: DateRangeFilterPolicy) -> bool {
        matches!(policy, DateRangeFilterPolicy::OptionalRange)
            && matches!(self.parsed(), ParsedDateRangeRoute::Invalid)
    }

    fn matches_range(&self, range: LocalReportDateRange) -> bool {
        self.start.as_deref() == Some(&format_report_date(range.from()))
            && self.end.as_deref() == Some(&format_report_date(range.to()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DateRangeInputs {
    start: String,
    end: String,
}

impl DateRangeInputs {
    fn empty() -> Self {
        Self {
            start: String::new(),
            end: String::new(),
        }
    }

    fn from_selection(selection: DateRangeSelection) -> Self {
        match selection {
            DateRangeSelection::Empty => Self::empty(),
            DateRangeSelection::Range(range) => Self {
                start: format_report_date(range.from()),
                end: format_report_date(range.to()),
            },
        }
    }

    fn are_both_empty(&self) -> bool {
        self.start.trim().is_empty() && self.end.trim().is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DateRangeFilterSyncMode {
    FollowingRoute { allow_server_canonicalization: bool },
    DetachedFromRoute,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DateRangeFilterState {
    route: DateRangeRouteParams,
    selection: DateRangeSelection,
    inputs: DateRangeInputs,
    validation_message: Option<String>,
    sync_mode: DateRangeFilterSyncMode,
}

impl DateRangeFilterState {
    fn from_route(policy: DateRangeFilterPolicy, route: DateRangeRouteParams) -> Self {
        let selection = route.selection();
        let inputs = route.input_values();
        let allow_server_canonicalization = route.allows_server_canonicalization(policy);

        Self {
            route,
            selection,
            inputs,
            validation_message: None,
            sync_mode: DateRangeFilterSyncMode::FollowingRoute {
                allow_server_canonicalization,
            },
        }
    }

    pub(crate) fn route(&self) -> &DateRangeRouteParams {
        &self.route
    }

    pub(crate) fn selection(&self) -> DateRangeSelection {
        self.selection
    }

    pub(crate) fn start_input_value(&self) -> &str {
        &self.inputs.start
    }

    pub(crate) fn end_input_value(&self) -> &str {
        &self.inputs.end
    }

    pub(crate) fn validation_message(&self) -> Option<&str> {
        self.validation_message.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DateRangeFilterEvent {
    RouteChanged(DateRangeRouteParams),
    StartEdited(String),
    EndEdited(String),
    PresetChosen(LocalReportDateRange),
    ServerResolved(LocalReportDateRange),
    ClampedDisplay(LocalReportDateRange),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DateRangeFilterEffect {
    None,
    ReplaceRoute(DateRangeSelection),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DateRangeFilterTransition {
    pub(crate) state: DateRangeFilterState,
    pub(crate) effect: DateRangeFilterEffect,
}

pub(crate) fn initialize_date_range_filter(
    policy: DateRangeFilterPolicy,
    route: DateRangeRouteParams,
) -> DateRangeFilterTransition {
    transition_from_route(policy, route)
}

pub(crate) fn transition_date_range_filter(
    policy: DateRangeFilterPolicy,
    state: &DateRangeFilterState,
    event: DateRangeFilterEvent,
) -> DateRangeFilterTransition {
    match event {
        DateRangeFilterEvent::RouteChanged(route) => transition_from_route(policy, route),
        DateRangeFilterEvent::StartEdited(next_start) => transition_with_inputs(
            policy,
            state,
            DateRangeInputs {
                start: next_start,
                end: state.inputs.end.clone(),
            },
        ),
        DateRangeFilterEvent::EndEdited(next_end) => transition_with_inputs(
            policy,
            state,
            DateRangeInputs {
                start: state.inputs.start.clone(),
                end: next_end,
            },
        ),
        DateRangeFilterEvent::PresetChosen(range) => transition_to_selection(
            state,
            DateRangeInputs::from_selection(range.into()),
            range.into(),
        ),
        DateRangeFilterEvent::ServerResolved(range) => {
            transition_server_resolved(state, DateRangeSelection::Range(range))
        }
        DateRangeFilterEvent::ClampedDisplay(range) => transition_clamped_display(state, range),
    }
}

/// Show the server's effective (clamped) range in the controls without touching
/// the requested route. The route still drives the report fetch, so the report
/// keeps reporting that the requested range was clamped (the Free-window badge).
fn transition_clamped_display(
    state: &DateRangeFilterState,
    range: LocalReportDateRange,
) -> DateRangeFilterTransition {
    let selection = DateRangeSelection::Range(range);

    DateRangeFilterTransition {
        state: DateRangeFilterState {
            selection,
            inputs: DateRangeInputs::from_selection(selection),
            validation_message: None,
            sync_mode: DateRangeFilterSyncMode::DetachedFromRoute,
            ..state.clone()
        },
        effect: DateRangeFilterEffect::None,
    }
}

fn transition_from_route(
    policy: DateRangeFilterPolicy,
    route: DateRangeRouteParams,
) -> DateRangeFilterTransition {
    let effect = if route.should_replace_invalid_route_with_empty(policy) {
        DateRangeFilterEffect::ReplaceRoute(DateRangeSelection::Empty)
    } else {
        DateRangeFilterEffect::None
    };

    DateRangeFilterTransition {
        state: DateRangeFilterState::from_route(policy, route),
        effect,
    }
}

fn transition_with_inputs(
    policy: DateRangeFilterPolicy,
    state: &DateRangeFilterState,
    inputs: DateRangeInputs,
) -> DateRangeFilterTransition {
    match resolve_input_range(&inputs.start, &inputs.end) {
        Ok(Some(range)) => transition_to_selection(state, inputs, range.into()),
        Ok(None) if inputs.are_both_empty() => {
            transition_to_selection(state, inputs, DateRangeSelection::Empty)
        }
        Ok(None) => DateRangeFilterTransition {
            state: DateRangeFilterState {
                inputs,
                validation_message: None,
                sync_mode: DateRangeFilterSyncMode::DetachedFromRoute,
                ..state.clone()
            },
            effect: DateRangeFilterEffect::None,
        },
        Err(err) => {
            let effect = if matches!(policy, DateRangeFilterPolicy::OptionalRange)
                && inputs.are_both_empty()
            {
                DateRangeFilterEffect::ReplaceRoute(DateRangeSelection::Empty)
            } else {
                DateRangeFilterEffect::None
            };

            DateRangeFilterTransition {
                state: DateRangeFilterState {
                    inputs,
                    validation_message: Some(err.to_string()),
                    sync_mode: DateRangeFilterSyncMode::DetachedFromRoute,
                    ..state.clone()
                },
                effect,
            }
        }
    }
}

fn transition_to_selection(
    state: &DateRangeFilterState,
    inputs: DateRangeInputs,
    selection: DateRangeSelection,
) -> DateRangeFilterTransition {
    let sync_mode = if selection.matches_route(&state.route) {
        DateRangeFilterSyncMode::FollowingRoute {
            allow_server_canonicalization: false,
        }
    } else {
        DateRangeFilterSyncMode::DetachedFromRoute
    };

    let effect = if selection.matches_route(&state.route) {
        DateRangeFilterEffect::None
    } else {
        DateRangeFilterEffect::ReplaceRoute(selection)
    };

    DateRangeFilterTransition {
        state: DateRangeFilterState {
            selection,
            inputs,
            validation_message: None,
            sync_mode,
            ..state.clone()
        },
        effect,
    }
}

fn transition_server_resolved(
    state: &DateRangeFilterState,
    selection: DateRangeSelection,
) -> DateRangeFilterTransition {
    match state.sync_mode {
        DateRangeFilterSyncMode::DetachedFromRoute => DateRangeFilterTransition {
            state: state.clone(),
            effect: DateRangeFilterEffect::None,
        },
        DateRangeFilterSyncMode::FollowingRoute {
            allow_server_canonicalization: false,
        } => DateRangeFilterTransition {
            state: state.clone(),
            effect: DateRangeFilterEffect::None,
        },
        DateRangeFilterSyncMode::FollowingRoute {
            allow_server_canonicalization: true,
        } => transition_to_selection(state, DateRangeInputs::from_selection(selection), selection),
    }
}

fn format_report_date(date: chrono::NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

fn resolve_input_range(
    start_input: &str,
    end_input: &str,
) -> Result<Option<LocalReportDateRange>, LocalReportDateRangeError> {
    let parsed_start = start_input.parse::<ReportDateParam>().ok();
    let parsed_end = end_input.parse::<ReportDateParam>().ok();

    match (parsed_start, parsed_end) {
        (Some(start), Some(end)) => {
            LocalReportDateRange::new(start.into_naive_date(), end.into_naive_date()).map(Some)
        }
        _ => Ok(None),
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("valid date")
    }

    fn range(
        start_year: i32,
        start_month: u32,
        start_day: u32,
        end_year: i32,
        end_month: u32,
        end_day: u32,
    ) -> LocalReportDateRange {
        LocalReportDateRange::new(
            date(start_year, start_month, start_day),
            date(end_year, end_month, end_day),
        )
        .expect("valid range")
    }

    #[test]
    fn explicit_valid_route_preserves_selection_and_disables_canonicalization() {
        let outcome = initialize_date_range_filter(
            DateRangeFilterPolicy::RequiredCanonicalRange,
            DateRangeRouteParams::new(
                Some("2026-01-01".to_string()),
                Some("2026-03-31".to_string()),
            ),
        );

        assert_eq!(outcome.state.start_input_value(), "2026-01-01");
        assert_eq!(outcome.state.end_input_value(), "2026-03-31");
        assert_eq!(
            outcome.state.selection(),
            DateRangeSelection::Range(range(2026, 1, 1, 2026, 3, 31))
        );
        assert_eq!(outcome.effect, DateRangeFilterEffect::None);
        assert_eq!(
            outcome.state.sync_mode,
            DateRangeFilterSyncMode::FollowingRoute {
                allow_server_canonicalization: false,
            }
        );
    }

    #[test]
    fn missing_required_range_route_allows_server_canonicalization() {
        let outcome = initialize_date_range_filter(
            DateRangeFilterPolicy::RequiredCanonicalRange,
            DateRangeRouteParams::new(None, None),
        );

        assert_eq!(outcome.state.start_input_value(), "");
        assert_eq!(outcome.state.end_input_value(), "");
        assert_eq!(outcome.state.selection(), DateRangeSelection::Empty);
        assert_eq!(
            outcome.state.sync_mode,
            DateRangeFilterSyncMode::FollowingRoute {
                allow_server_canonicalization: true,
            }
        );
    }

    #[test]
    fn invalid_optional_route_is_canonicalized_to_empty() {
        let outcome = initialize_date_range_filter(
            DateRangeFilterPolicy::OptionalRange,
            DateRangeRouteParams::new(Some("2026-01-01".to_string()), None),
        );

        assert_eq!(outcome.state.start_input_value(), "");
        assert_eq!(outcome.state.end_input_value(), "");
        assert_eq!(outcome.state.selection(), DateRangeSelection::Empty);
        assert_eq!(
            outcome.effect,
            DateRangeFilterEffect::ReplaceRoute(DateRangeSelection::Empty)
        );
    }

    #[test]
    fn valid_start_edit_replaces_route_and_detaches_from_route() {
        let state = initialize_date_range_filter(
            DateRangeFilterPolicy::RequiredCanonicalRange,
            DateRangeRouteParams::new(
                Some("2026-01-01".to_string()),
                Some("2026-03-31".to_string()),
            ),
        )
        .state;

        let outcome = transition_date_range_filter(
            DateRangeFilterPolicy::RequiredCanonicalRange,
            &state,
            DateRangeFilterEvent::StartEdited("2025-03-05".to_string()),
        );

        assert_eq!(outcome.state.start_input_value(), "2025-03-05");
        assert_eq!(outcome.state.end_input_value(), "2026-03-31");
        assert_eq!(
            outcome.state.selection(),
            DateRangeSelection::Range(range(2025, 3, 5, 2026, 3, 31))
        );
        assert_eq!(
            outcome.effect,
            DateRangeFilterEffect::ReplaceRoute(DateRangeSelection::Range(range(
                2025, 3, 5, 2026, 3, 31
            )))
        );
        assert_eq!(
            outcome.state.sync_mode,
            DateRangeFilterSyncMode::DetachedFromRoute
        );
    }

    #[test]
    fn partial_edit_updates_inputs_without_navigation() {
        let state = initialize_date_range_filter(
            DateRangeFilterPolicy::RequiredCanonicalRange,
            DateRangeRouteParams::new(
                Some("2026-01-01".to_string()),
                Some("2026-03-31".to_string()),
            ),
        )
        .state;

        let outcome = transition_date_range_filter(
            DateRangeFilterPolicy::RequiredCanonicalRange,
            &state,
            DateRangeFilterEvent::EndEdited(String::new()),
        );

        assert_eq!(outcome.state.start_input_value(), "2026-01-01");
        assert_eq!(outcome.state.end_input_value(), "");
        assert_eq!(
            outcome.state.selection(),
            DateRangeSelection::Range(range(2026, 1, 1, 2026, 3, 31))
        );
        assert_eq!(outcome.state.validation_message(), None);
        assert_eq!(outcome.effect, DateRangeFilterEffect::None);
    }

    #[test]
    fn clearing_both_inputs_replaces_route_with_empty_selection() {
        let state = initialize_date_range_filter(
            DateRangeFilterPolicy::OptionalRange,
            DateRangeRouteParams::new(
                Some("2026-01-01".to_string()),
                Some("2026-03-31".to_string()),
            ),
        )
        .state;

        let cleared_start = transition_date_range_filter(
            DateRangeFilterPolicy::OptionalRange,
            &state,
            DateRangeFilterEvent::StartEdited(String::new()),
        )
        .state;
        let outcome = transition_date_range_filter(
            DateRangeFilterPolicy::OptionalRange,
            &cleared_start,
            DateRangeFilterEvent::EndEdited(String::new()),
        );

        assert_eq!(outcome.state.start_input_value(), "");
        assert_eq!(outcome.state.end_input_value(), "");
        assert_eq!(outcome.state.selection(), DateRangeSelection::Empty);
        assert_eq!(
            outcome.effect,
            DateRangeFilterEffect::ReplaceRoute(DateRangeSelection::Empty)
        );
    }

    #[test]
    fn inverted_edit_sets_validation_without_navigation() {
        let state = initialize_date_range_filter(
            DateRangeFilterPolicy::RequiredCanonicalRange,
            DateRangeRouteParams::new(
                Some("2026-01-01".to_string()),
                Some("2026-03-31".to_string()),
            ),
        )
        .state;

        let outcome = transition_date_range_filter(
            DateRangeFilterPolicy::RequiredCanonicalRange,
            &state,
            DateRangeFilterEvent::StartEdited("2026-04-01".to_string()),
        );

        assert_eq!(
            outcome.state.validation_message(),
            Some("Start date must be on or before end date")
        );
        assert_eq!(outcome.effect, DateRangeFilterEffect::None);
    }

    #[test]
    fn server_resolved_canonicalizes_missing_required_route_once() {
        let state = initialize_date_range_filter(
            DateRangeFilterPolicy::RequiredCanonicalRange,
            DateRangeRouteParams::new(None, None),
        )
        .state;
        let canonical = range(2026, 1, 1, 2026, 3, 31);

        let first = transition_date_range_filter(
            DateRangeFilterPolicy::RequiredCanonicalRange,
            &state,
            DateRangeFilterEvent::ServerResolved(canonical),
        );
        assert_eq!(first.state.start_input_value(), "2026-01-01");
        assert_eq!(first.state.end_input_value(), "2026-03-31");
        assert_eq!(
            first.effect,
            DateRangeFilterEffect::ReplaceRoute(DateRangeSelection::Range(canonical))
        );

        let second = transition_date_range_filter(
            DateRangeFilterPolicy::RequiredCanonicalRange,
            &first.state,
            DateRangeFilterEvent::ServerResolved(canonical),
        );
        assert_eq!(second.effect, DateRangeFilterEffect::None);
        assert_eq!(second.state, first.state);
    }

    #[test]
    fn server_resolved_does_not_rewrite_explicit_valid_route() {
        let state = initialize_date_range_filter(
            DateRangeFilterPolicy::RequiredCanonicalRange,
            DateRangeRouteParams::new(
                Some("2025-01-01".to_string()),
                Some("2025-12-31".to_string()),
            ),
        )
        .state;

        let outcome = transition_date_range_filter(
            DateRangeFilterPolicy::RequiredCanonicalRange,
            &state,
            DateRangeFilterEvent::ServerResolved(range(2026, 1, 1, 2026, 3, 31)),
        );

        assert_eq!(outcome.effect, DateRangeFilterEffect::None);
        assert_eq!(outcome.state, state);
    }

    #[test]
    fn stale_server_resolution_is_ignored_after_user_navigation_intent() {
        let state = initialize_date_range_filter(
            DateRangeFilterPolicy::RequiredCanonicalRange,
            DateRangeRouteParams::new(
                Some("2026-01-01".to_string()),
                Some("2026-03-31".to_string()),
            ),
        )
        .state;

        let edited = transition_date_range_filter(
            DateRangeFilterPolicy::RequiredCanonicalRange,
            &state,
            DateRangeFilterEvent::StartEdited("2025-01-01".to_string()),
        );
        assert_eq!(
            edited.effect,
            DateRangeFilterEffect::ReplaceRoute(DateRangeSelection::Range(range(
                2025, 1, 1, 2026, 3, 31
            )))
        );

        let stale = transition_date_range_filter(
            DateRangeFilterPolicy::RequiredCanonicalRange,
            &edited.state,
            DateRangeFilterEvent::ServerResolved(range(2026, 1, 1, 2026, 3, 31)),
        );

        assert_eq!(stale.effect, DateRangeFilterEffect::None);
        assert_eq!(stale.state, edited.state);
    }

    #[test]
    fn clamped_display_shows_effective_range_without_changing_route() {
        let state = initialize_date_range_filter(
            DateRangeFilterPolicy::RequiredCanonicalRange,
            DateRangeRouteParams::new(
                Some("2024-01-01".to_string()),
                Some("2024-12-31".to_string()),
            ),
        )
        .state;

        let outcome = transition_date_range_filter(
            DateRangeFilterPolicy::RequiredCanonicalRange,
            &state,
            DateRangeFilterEvent::ClampedDisplay(range(2025, 10, 3, 2026, 7, 3)),
        );

        // Controls reflect the effective clamped range.
        assert_eq!(outcome.state.start_input_value(), "2025-10-03");
        assert_eq!(outcome.state.end_input_value(), "2026-07-03");
        assert_eq!(
            outcome.state.selection(),
            DateRangeSelection::Range(range(2025, 10, 3, 2026, 7, 3))
        );
        // The requested route is untouched, so the fetch still reports a clamp.
        assert_eq!(outcome.state.route().start.as_deref(), Some("2024-01-01"));
        assert_eq!(outcome.state.route().end.as_deref(), Some("2024-12-31"));
        // No navigation is triggered by the display-only sync.
        assert_eq!(outcome.effect, DateRangeFilterEffect::None);
        assert_eq!(
            outcome.state.sync_mode,
            DateRangeFilterSyncMode::DetachedFromRoute
        );
    }

    #[test]
    fn route_changed_resyncs_inputs_after_detached_state() {
        let state = initialize_date_range_filter(
            DateRangeFilterPolicy::RequiredCanonicalRange,
            DateRangeRouteParams::new(
                Some("2026-01-01".to_string()),
                Some("2026-03-31".to_string()),
            ),
        )
        .state;
        let detached = transition_date_range_filter(
            DateRangeFilterPolicy::RequiredCanonicalRange,
            &state,
            DateRangeFilterEvent::EndEdited(String::new()),
        )
        .state;

        let outcome = transition_date_range_filter(
            DateRangeFilterPolicy::RequiredCanonicalRange,
            &detached,
            DateRangeFilterEvent::RouteChanged(DateRangeRouteParams::new(
                Some("2025-01-01".to_string()),
                Some("2025-12-31".to_string()),
            )),
        );

        assert_eq!(outcome.state.start_input_value(), "2025-01-01");
        assert_eq!(outcome.state.end_input_value(), "2025-12-31");
        assert_eq!(outcome.state.validation_message(), None);
        assert_eq!(
            outcome.state.selection(),
            DateRangeSelection::Range(range(2025, 1, 1, 2025, 12, 31))
        );
        assert_eq!(
            outcome.state.sync_mode,
            DateRangeFilterSyncMode::FollowingRoute {
                allow_server_canonicalization: false,
            }
        );
    }
}
