//! Prices page: saved daily prices for the assets the user holds.

use super::coingecko_price_control::SYNC_HELP;
use super::date_range_filter::{
    DateRangeFilterEffect, DateRangeFilterEvent, DateRangeFilterPolicy, DateRangeRouteParams,
    DateRangeSelection, initialize_date_range_filter,
};
use super::date_range_toolbar::DateRangeToolbar;
use super::report_common::dispatch_report_date_event;
use super::wallets::{CopyFeedback, use_copy_feedback};
use super::{
    CheckIcon, CopyIcon, SortAscIcon, SortDescIcon, ToastLevel, ToastState,
    format_date_for_display, format_number_for_display, push_toast,
};
use crate::backend::{PriceHistoryView, get_price_history, set_price_fetching_enabled};
use crate::hooks::handle_session_expired;
use crate::hooks::use_session_guard;
use crate::models::{CurrencyCode, DateTimeFormat, NumberFormat, UserTimezone};
use crate::report_dates::{dial_year_range, displayed_calendar_year};
use crate::services::price_history_view::{
    PRICE_HISTORY_PAGE_SIZE, PriceHistoryEntry, PriceHistorySort, PriceSourceFilter,
};
use crate::settings::SettingsState;
use crate::wallets::WalletReportDateRange;
use crate::{AuthState, BannerState, Route};
use chrono::{Datelike, NaiveDate};
use dioxus::prelude::*;
use rust_decimal::{Decimal, RoundingStrategy};

const WALLETS_CSS: Asset = asset!("/assets/wallets.css");
const PRICES_CSS: Asset = asset!("/assets/prices.css");
const COPIED_MS: u32 = 1600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoinGeckoState {
    /// The user's CoinGecko toggle is off: no CoinGecko prices, offer the switch.
    Off,
    /// Switched on after this page loaded: wait for the next relevant click.
    AwaitingReload,
    Shown,
}

fn coingecko_state(toggle_on: bool, response_enabled: bool) -> CoinGeckoState {
    match (toggle_on, response_enabled) {
        (false, _) => CoinGeckoState::Off,
        (true, false) => CoinGeckoState::AwaitingReload,
        (true, true) => CoinGeckoState::Shown,
    }
}

/// Decimal places for one page: about five significant digits of the smallest
/// price, clamped to 2..=10, so every row lines up on the decimal point.
fn page_decimals<'a>(prices: impl IntoIterator<Item = &'a Decimal>) -> u32 {
    let Some(smallest) = prices
        .into_iter()
        .map(|price| price.abs())
        .filter(|price| !price.is_zero())
        .min()
    else {
        return 2;
    };
    let text = smallest.normalize().to_string();
    let wanted = if smallest >= Decimal::ONE {
        let integer_digits = text.split('.').next().map_or(1, str::len);
        5_i64 - i64::try_from(integer_digits).unwrap_or(i64::MAX)
    } else {
        let fraction = text.split('.').nth(1).unwrap_or_default();
        let leading_zeros = fraction.chars().take_while(|digit| *digit == '0').count();
        i64::try_from(leading_zeros)
            .unwrap_or(i64::MAX)
            .saturating_add(5)
    };
    u32::try_from(wanted.clamp(2, 10)).unwrap_or(2)
}

/// Grouped integer part and fraction (with its separator) for display. Rounds
/// only when the stored value has more places than `decimals`; pads with zeros
/// only up to the currency's minor unit, so long fractions keep their precision.
fn split_for_alignment(
    price: Decimal,
    decimals: u32,
    minor_unit_places: u32,
    number_format: NumberFormat,
) -> (String, String) {
    let mut shown = if price.scale() > decimals {
        price.round_dp_with_strategy(decimals, RoundingStrategy::MidpointAwayFromZero)
    } else {
        price
    };
    if shown.scale() < minor_unit_places {
        shown.rescale(minor_unit_places);
    }
    let text = format_number_for_display(&shown.to_string(), number_format);
    let decimal_separator = match number_format {
        NumberFormat::DotComma => '.',
        NumberFormat::CommaDot | NumberFormat::CommaSpace => ',',
    };
    match text.rsplit_once(decimal_separator) {
        Some((integer, fraction)) => (
            integer.to_string(),
            format!("{decimal_separator}{fraction}"),
        ),
        None => (text, String::new()),
    }
}

fn route_for_prices(asset: Option<String>, selection: DateRangeSelection) -> Route {
    Route::Prices {
        asset,
        start: selection.start_query_value(),
        end: selection.end_query_value(),
    }
}

#[component]
pub(crate) fn Prices(asset: Option<String>, start: Option<String>, end: Option<String>) -> Element {
    let auth_state = use_context::<AuthState>();
    let banner_state = use_context::<BannerState>();
    let navigator = use_navigator();
    let settings = use_context::<SettingsState>();
    let mut sync_enabled = settings.price_fetching_enabled;
    let local_sync = sync_enabled();
    let currency = (settings.currency)();
    let mut previous_preferences = use_signal(|| (local_sync, currency));
    let mut history_revision = use_signal(|| 0_u64);

    let current_route = DateRangeRouteParams::new(start.clone(), end.clone());
    let initial_filter = initialize_date_range_filter(
        DateRangeFilterPolicy::RequiredCanonicalRange,
        current_route.clone(),
    );
    let initial_selection = initial_filter.state.selection();
    let initial_pending_route_selection = match initial_filter.effect {
        DateRangeFilterEffect::None => None,
        DateRangeFilterEffect::ReplaceRoute(selection) => Some(selection),
    };
    let filter_state = use_signal(|| initial_filter.state.clone());
    let active_selection = use_signal(|| initial_selection);
    let mut pending_route_selection = use_signal(|| initial_pending_route_selection);
    let mut requested_asset = use_signal(|| asset.clone());
    let mut page = use_signal(|| 1_u32);
    let mut sort = use_signal(PriceHistorySort::default);
    let mut source = use_signal(PriceSourceFilter::default);
    let mut unauthorized_handled = use_signal(|| false);

    let (previous_sync, previous_currency) = *previous_preferences.peek();
    if (previous_sync, previous_currency) != (local_sync, currency) {
        previous_preferences.set((local_sync, currency));
        // Opting in waits for a click. Opting out or changing currency must
        // immediately discard the old quotes and reload the first page.
        if !local_sync || previous_currency != currency {
            if !local_sync {
                source.set(PriceSourceFilter::All);
            }
            page.set(1);
            history_revision.with_mut(|revision| *revision = revision.wrapping_add(1));
        }
    }
    if filter_state.peek().route() != &current_route {
        dispatch_report_date_event(
            filter_state,
            active_selection,
            pending_route_selection,
            DateRangeFilterEvent::RouteChanged(current_route),
        );
    }
    if *requested_asset.peek() != asset {
        requested_asset.set(asset.clone());
        page.set(1);
    }

    use_effect(move || {
        if let Some(selection) = pending_route_selection() {
            navigator.replace(route_for_prices(requested_asset.peek().clone(), selection));
            pending_route_selection.set(None);
        }
    });

    let mut history = use_server_future(move || {
        let selection = active_selection();
        let asset = requested_asset();
        let page = page();
        let sort = sort();
        let source = source();
        let requested_revision = history_revision();
        let requested_sync = *sync_enabled.peek();
        async move {
            let result = get_price_history(
                asset,
                selection.start_param(),
                selection.end_param(),
                Some(page),
                Some(sort),
                Some(source),
            )
            .await;
            (requested_revision, requested_sync, result)
        }
    })?;

    let result = history().and_then(|(requested_revision, requested_sync, result)| {
        if requested_revision != *history_revision.peek() {
            return None;
        }
        // A fresh response also updates the navbar. A response from before a
        // local toggle must not undo that choice or trigger a price fetch.
        if let Ok(view) = &result
            && requested_sync == local_sync
            && view.prices_enabled != local_sync
        {
            sync_enabled.set(view.prices_enabled);
        }
        Some(result)
    });
    if let Some(Err(err)) = result.as_ref() {
        if err.is_unauthorized() && !*unauthorized_handled.peek() {
            unauthorized_handled.set(true);
            handle_session_expired(auth_state, banner_state, "prices");
        }
    } else if *unauthorized_handled.peek() {
        unauthorized_handled.set(false);
    }
    let view: Option<&PriceHistoryView> = result.as_ref().and_then(|r| r.as_ref().ok());

    // The resolved range is the requested one: the Free window only limits
    // CoinGecko prices, never the user's own.
    if let Some(view) = view
        && let Ok(canonical) = WalletReportDateRange::new(view.resolved_from, view.resolved_to)
    {
        dispatch_report_date_event(
            filter_state,
            active_selection,
            pending_route_selection,
            DateRangeFilterEvent::ServerResolved(canonical),
        );
    }
    let coingecko_clamped = view.is_some_and(|view| {
        view.access.range_clamped
            && view.prices_enabled
            && view.source != PriceSourceFilter::YourPrices
    });

    let this_year_preset = view.and_then(|view| {
        WalletReportDateRange::new(view.default_this_year_from, view.default_this_year_to).ok()
    });
    let snapshot = filter_state();
    let active_range = match snapshot.selection() {
        DateRangeSelection::Range(range) => Some(range),
        DateRangeSelection::Empty => None,
    };
    let current_year = this_year_preset.map(|range| range.from().year());
    let displayed_year =
        active_range.and_then(|range| displayed_calendar_year(range, this_year_preset));
    let year_label = displayed_year
        .map(|year| year.to_string())
        .unwrap_or_else(|| "Custom range".to_string());
    let disable_next_year = match (displayed_year, current_year) {
        (Some(year), Some(current)) => year >= current,
        _ => true,
    };
    let mut preset = move |range: Option<WalletReportDateRange>| {
        if let Some(range) = range {
            page.set(1);
            dispatch_report_date_event(
                filter_state,
                active_selection,
                pending_route_selection,
                DateRangeFilterEvent::PresetChosen(range),
            );
        }
    };
    let selected_asset = view.and_then(|view| view.selected.clone());
    let selection_now = *active_selection.peek();
    let asset_row = view.filter(|view| !view.assets.is_empty()).map(|view| {
        rsx! {
            div { class: "tx-toolbar-secondary-row",
                div { class: "tx-toolbar-secondary-group",
                    span { class: "tx-filter-label", id: "prices-asset-label", "Asset" }
                    div { class: "tx-filter-pills", role: "group", "aria-labelledby": "prices-asset-label",
                        for held in view.assets.iter() {
                            Link {
                                key: "{held.key.as_str()}",
                                class: if selected_asset.as_ref() == Some(&held.key) { "tx-filter-pill active" } else { "tx-filter-pill" },
                                to: route_for_prices(Some(held.key.as_str().to_string()), selection_now),
                                "{held.name}"
                            }
                        }
                    }
                }
                // Only sources the user has turned on are listed, so the filter
                // never hints at what the shared database holds.
                if view.prices_enabled {
                    div { class: "tx-toolbar-secondary-group",
                        span { class: "tx-filter-label", id: "prices-source-label", "Source" }
                        div { class: "tx-filter-pills", role: "group", "aria-labelledby": "prices-source-label",
                            for (filter, label) in [
                                (PriceSourceFilter::All, "All"),
                                (PriceSourceFilter::YourPrices, "Your prices"),
                                (PriceSourceFilter::CoinGecko, "CoinGecko"),
                            ] {
                                button {
                                    key: "{label}",
                                    r#type: "button",
                                    class: if view.source == filter { "tx-filter-pill active" } else { "tx-filter-pill" },
                                    "aria-pressed": view.source == filter,
                                    onclick: move |_| {
                                        source.set(filter);
                                        page.set(1);
                                    },
                                    "{label}"
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    rsx! {
        document::Stylesheet { href: WALLETS_CSS }
        document::Stylesheet { href: PRICES_CSS }

        div { class: "page-container transactions-page",
            div { class: "page-header",
                div {
                    h1 { class: "page-title page-title-display", "Prices" }
                    p { class: "page-subtitle",
                        if let Some(view) = view {
                            "Saved prices for the assets in your wallets, in {view.currency.code()}, your reporting currency."
                        } else {
                            "Saved prices for the assets in your wallets."
                        }
                    }
                }
            }

            div { class: "card",
                div { class: "card-body",
                    DateRangeToolbar {
                        start_input_id: "prices-start".to_string(),
                        start_input_value: snapshot.start_input_value().to_string(),
                        end_input_id: "prices-end".to_string(),
                        end_input_value: snapshot.end_input_value().to_string(),
                        validation_message: snapshot.validation_message().map(str::to_string),
                        disable_date_inputs: false,
                        year_label,
                        disable_previous_year: displayed_year.is_none(),
                        disable_next_year,
                        show_this_year: displayed_year != current_year,
                        custom_range_open: displayed_year.is_none(),
                        on_start_change: move |value| {
                            page.set(1);
                            dispatch_report_date_event(filter_state, active_selection, pending_route_selection, DateRangeFilterEvent::StartEdited(value));
                        },
                        on_end_change: move |value| {
                            page.set(1);
                            dispatch_report_date_event(filter_state, active_selection, pending_route_selection, DateRangeFilterEvent::EndEdited(value));
                        },
                        on_this_year: move |_| preset(this_year_preset),
                        on_previous_year: move |_| {
                            preset(displayed_year.and_then(|year| dial_year_range(year - 1, current_year.unwrap_or(year), this_year_preset)));
                        },
                        on_next_year: move |_| {
                            preset(displayed_year.and_then(|year| dial_year_range(year + 1, current_year.unwrap_or(year + 1), this_year_preset)));
                        },
                        secondary_row: asset_row,
                    }
                }
            }

            if coingecko_clamped {
                p { class: "prices-scope-note",
                    "Free plans show the last 9 months of CoinGecko prices. "
                    Link { to: Route::Payments, "Upgrade" }
                    " for full history."
                }
            }

            match result.as_ref() {
                None => rsx! {
                    div { class: "card skeleton-card",
                        div { class: "card-body",
                            div { class: "skeleton-line skeleton-line-title" }
                            div { class: "skeleton-line skeleton-line-full" }
                            div { class: "skeleton-line skeleton-line-medium" }
                        }
                    }
                },
                Some(Err(err)) => rsx! {
                    if !err.is_unauthorized() {
                        div { class: "error-block", p { "{err}" } }
                    }
                },
                Some(Ok(view)) => rsx! {
                    PriceCard {
                        view: view.clone(),
                        on_reload: move |_| history.restart(),
                        on_sort: move |next| {
                            sort.set(next);
                            page.set(1);
                        },
                        on_page: move |next| page.set(next),
                    }
                },
            }
        }
    }
}

#[component]
fn PriceCard(
    view: PriceHistoryView,
    on_reload: EventHandler<()>,
    on_sort: EventHandler<PriceHistorySort>,
    on_page: EventHandler<u32>,
) -> Element {
    let settings_state = use_context::<SettingsState>();
    let toggle_on = (settings_state.price_fetching_enabled)();

    if view.assets.is_empty() {
        return rsx! {
            div { class: "card empty-state prices-empty",
                div { class: "card-body",
                    p { class: "empty-state-heading", "No prices to show yet" }
                    p { class: "empty-state-description", "Add a wallet and its saved prices will appear here." }
                    Link { class: "btn btn-primary", to: Route::Wallets, "Add a wallet" }
                }
            }
        };
    }

    rsx! {
        match coingecko_state(toggle_on, view.prices_enabled) {
            CoinGeckoState::Off => rsx! { CoinGeckoOffNotice {} },
            CoinGeckoState::AwaitingReload => rsx! {
                div { class: "alert alert-info prices-source-notice", "aria-live": "polite",
                    p { class: "prices-source-notice-body", "CoinGecko syncing is on. Its prices load the next time you pick an asset or change the dates." }
                    button {
                        class: "btn btn-secondary",
                        r#type: "button",
                        onclick: move |_| on_reload.call(()),
                        "Show CoinGecko prices"
                    }
                }
            },
            CoinGeckoState::Shown => rsx! {},
        }
        PriceTable { view, on_sort, on_page }
    }
}

#[component]
fn CoinGeckoOffNotice() -> Element {
    let settings_state = use_context::<SettingsState>();
    let toast_state = use_context::<ToastState>();
    let guard = use_session_guard();
    let mut enabled = settings_state.price_fetching_enabled;
    let mut saving = use_signal(|| false);

    let enable = move |_| {
        if saving() {
            return;
        }
        saving.set(true);
        spawn(async move {
            match set_price_fetching_enabled(true).await {
                Ok(saved) => enabled.set(saved),
                Err(err) if err.is_unauthorized() => {
                    let mut guard = guard;
                    guard.check::<bool>(Err(err));
                }
                Err(_) => push_toast(
                    toast_state,
                    ToastLevel::Error,
                    "Couldn't update CoinGecko prices - try again.".to_string(),
                ),
            }
            saving.set(false);
        });
    };

    rsx! {
        div { class: "alert alert-info prices-source-notice", "aria-live": "polite",
            p { class: "prices-source-notice-body", "CoinGecko prices are hidden while CoinGecko syncing is off." }
            label { class: "nav-price-switch prices-source-switch",
                input {
                    r#type: "checkbox",
                    checked: false,
                    disabled: saving(),
                    "aria-describedby": "prices-source-help",
                    onchange: enable,
                }
                span { class: "nav-price-track", span { class: "nav-price-knob" } }
                span { "Sync CoinGecko prices" }
            }
            p { class: "prices-source-help", id: "prices-source-help",
                "{SYNC_HELP} You can turn it off later from the top bar."
            }
        }
    }
}

#[component]
fn PriceTable(
    view: PriceHistoryView,
    on_sort: EventHandler<PriceHistorySort>,
    on_page: EventHandler<u32>,
) -> Element {
    let settings_state = use_context::<SettingsState>();
    let date_format = (settings_state.date_time_format)();
    let timezone = (settings_state.timezone)();
    let currency = view.currency;
    let asset_name = view
        .selected
        .as_ref()
        .and_then(|key| view.assets.iter().find(|held| &held.key == key))
        .map(|held| held.name.clone())
        .unwrap_or_default();

    if view.entries.is_empty() {
        let (heading, description) = match (view.prices_enabled, view.source) {
            (false, _) | (true, PriceSourceFilter::YourPrices) => (
                format!("You haven't set prices for {asset_name} in these dates"),
                "Prices you set in a wallet report appear here.",
            ),
            (true, _) if view.access.requested_overlap().is_none() => (
                format!("No prices to show for {asset_name} in these dates"),
                "These dates are outside the last 9 months of CoinGecko prices included in your Free plan.",
            ),
            (true, PriceSourceFilter::CoinGecko) => (
                format!("No CoinGecko prices for {asset_name} yet"),
                "They appear here once CoinGecko syncing has saved them for these dates.",
            ),
            (true, PriceSourceFilter::All) => (
                format!("No prices saved for {asset_name} yet"),
                "CoinGecko prices appear once syncing has saved them for these dates. Prices you set in a wallet report appear here too.",
            ),
        };
        return rsx! {
            div { class: "card empty-state prices-empty",
                div { class: "card-body",
                    p { class: "empty-state-heading", "{heading}" }
                    p { class: "empty-state-description", "{description}" }
                }
            }
        };
    }

    let decimals = page_decimals(view.entries.iter().filter_map(|entry| match entry {
        PriceHistoryEntry::Daily { price, .. } | PriceHistoryEntry::YourPrice { price, .. } => {
            Some(price)
        }
        PriceHistoryEntry::Gap { .. } => None,
    }));
    let page_size = u32::try_from(PRICE_HISTORY_PAGE_SIZE).unwrap_or(u32::MAX);
    let last_page = view.total_entries.div_ceil(page_size).max(1);
    let first_shown = (view.page - 1) * page_size + 1;
    let last_shown = (first_shown + page_size - 1).min(view.total_entries);
    let current_sort = view.sort;
    let next_sort = match current_sort {
        PriceHistorySort::NewestFirst => PriceHistorySort::OldestFirst,
        PriceHistorySort::OldestFirst => PriceHistorySort::NewestFirst,
    };
    let page = view.page;
    let scope_note = match (view.prices_enabled, view.source) {
        (true, PriceSourceFilter::All) => {
            "CoinGecko prices are grouped by UTC date. Prices you set keep the time you gave them."
        }
        (true, PriceSourceFilter::CoinGecko) => "CoinGecko prices are grouped by UTC date.",
        (false, _) | (true, PriceSourceFilter::YourPrices) => {
            "Prices you set keep the time you gave them."
        }
    };

    rsx! {
        section { class: "transactions-table-section card",
            div { class: "transactions-table-header",
                h2 { class: "transactions-table-title", "{asset_name} in {currency.code()}" }
                div { class: "transactions-table-header-right",
                    button {
                        class: "btn btn-secondary tx-sort-toggle",
                        r#type: "button",
                        onclick: move |_| on_sort.call(next_sort),
                        match current_sort {
                            PriceHistorySort::NewestFirst => rsx! { SortDescIcon {} "Newest first" },
                            PriceHistorySort::OldestFirst => rsx! { SortAscIcon {} "Oldest first" },
                        }
                    }
                }
            }
            div { class: "prices-table-wrap",
                p { class: "prices-scope-note", "{scope_note}" }
                table { class: "prices-table",
                    thead {
                        tr {
                            th { "Date" }
                            th { "Source" }
                            th { class: "prices-num", "Price" }
                        }
                    }
                    tbody {
                        for entry in view.entries.iter() {
                            PriceRow { entry: entry.clone(), decimals, currency, date_format, timezone }
                        }
                    }
                }
            }
            div { class: "transactions-pagination-row",
                span { class: "muted", "{first_shown}–{last_shown} of {view.total_entries}" }
                if last_page > 1 {
                    div { class: "transactions-pagination",
                        button {
                            class: "btn btn-secondary",
                            r#type: "button",
                            disabled: page <= 1,
                            onclick: move |_| on_page.call(page - 1),
                            "Previous"
                        }
                        button {
                            class: "btn btn-secondary",
                            r#type: "button",
                            disabled: page >= last_page,
                            onclick: move |_| on_page.call(page + 1),
                            "Next"
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn PriceRow(
    entry: PriceHistoryEntry,
    decimals: u32,
    currency: CurrencyCode,
    date_format: DateTimeFormat,
    timezone: UserTimezone,
) -> Element {
    match entry {
        PriceHistoryEntry::Gap { from, to } => {
            let days = (to - from).num_days() + 1;
            let noun = if days == 1 { "day" } else { "days" };
            let range = if from == to {
                format_date_for_display(from, date_format)
            } else {
                format!(
                    "{} – {}",
                    format_date_for_display(from, date_format),
                    format_date_for_display(to, date_format)
                )
            };
            rsx! {
                tr { class: "prices-gap",
                    td { colspan: "3",
                        span { class: "prices-gap-count", "No price saved for {days} {noun}" }
                        span { class: "prices-gap-range", "{range}" }
                    }
                }
            }
        }
        PriceHistoryEntry::Daily { date_utc, price } => rsx! {
            tr {
                td { class: "prices-date", {format_date_for_display(date_utc, date_format)} }
                td { class: "prices-source", "CoinGecko" }
                td { class: "prices-num", PriceCell { price, decimals, currency } }
            }
        },
        PriceHistoryEntry::YourPrice { at, price, note } => {
            let tz: chrono_tz::Tz = timezone.into();
            let local = at.with_timezone(&tz);
            let date: NaiveDate = local.date_naive();
            rsx! {
                tr {
                    td { class: "prices-date",
                        {format_date_for_display(date, date_format)}
                        span { class: "prices-time", {local.format("%H:%M:%S %Z").to_string()} }
                    }
                    td { class: "prices-source",
                        span { class: "prices-source-own", "Your price" }
                        if let Some(note) = note {
                            span { class: "prices-note", "{note}" }
                        }
                    }
                    td { class: "prices-num", PriceCell { price, decimals, currency } }
                }
            }
        }
    }
}

/// Decimal-aligned price; hover or focus shows the exact value, click copies it.
#[component]
fn PriceCell(price: Decimal, decimals: u32, currency: CurrencyCode) -> Element {
    let (feedback, copy) = use_copy_feedback(COPIED_MS);
    let number_format = (use_context::<SettingsState>().number_format)();
    let minor_unit_places = u32::try_from(currency.decimal_places()).unwrap_or(2);
    let (integer, fraction) =
        split_for_alignment(price, decimals, minor_unit_places, number_format);
    let full = format!("{price} {}", currency.code());
    let confirmation = format!("Copied {full}");
    let tooltip = feedback.label(&full, &confirmation);
    let exact = price.to_string();
    let fraction_width = decimals + 1;

    rsx! {
        button {
            class: if feedback == CopyFeedback::Copied { "prices-copy copied" } else { "prices-copy" },
            r#type: "button",
            "data-full": "{tooltip}",
            "aria-label": "Copy price {full}",
            onclick: move |_| copy.call(exact.clone()),
            span { class: "prices-copy-state", "aria-hidden": "true",
                if feedback == CopyFeedback::Copied { CheckIcon {} } else { CopyIcon {} }
            }
            span { class: "prices-int", "{integer}" }
            span { class: "prices-frac", style: "width: {fraction_width}ch", "{fraction}" }
            span { class: "prices-unit", "{currency.code()}" }
            span { class: "visually-hidden", "aria-live": "polite",
                {feedback.label("", &confirmation)}
            }
        }
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn p(raw: &str) -> Decimal {
        Decimal::from_str(raw).expect("decimal")
    }

    #[test]
    fn decimals_target_five_significant_digits_of_the_smallest_price() {
        assert_eq!(page_decimals(&[p("95871.3304219"), p("96144.12")]), 2);
        assert_eq!(page_decimals(&[p("0.17500474831529667"), p("0.21")]), 5);
        assert_eq!(page_decimals(&[p("12.345")]), 3);
        assert_eq!(page_decimals(&[p("0.0000123")]), 9);
        assert_eq!(page_decimals(&[p("0.0000000000123")]), 10);
        assert_eq!(page_decimals(std::iter::empty()), 2);
    }

    #[test]
    fn alignment_groups_rounds_and_pads_only_to_the_currency_minor_unit() {
        let split = |raw, decimals, minor| {
            split_for_alignment(p(raw), decimals, minor, NumberFormat::DotComma)
        };
        assert_eq!(
            split("95871.3354219", 2, 2),
            ("95,871".to_string(), ".34".to_string())
        );
        assert_eq!(
            split("89516.1", 2, 2),
            ("89,516".to_string(), ".10".to_string())
        );
        assert_eq!(
            split("3400", 2, 2),
            ("3,400".to_string(), ".00".to_string())
        );
        assert_eq!(split("0.18", 5, 2), ("0".to_string(), ".18".to_string()));
        assert_eq!(split("3400", 2, 0), ("3,400".to_string(), String::new()));
        assert_eq!(
            split_for_alignment(p("94129.09"), 2, 2, NumberFormat::CommaSpace),
            ("94 129".to_string(), ",09".to_string())
        );
        assert_eq!(
            split_for_alignment(p("1234567.5"), 2, 2, NumberFormat::CommaDot),
            ("1.234.567".to_string(), ",50".to_string())
        );
    }

    #[test]
    fn coingecko_state_follows_toggle_then_loaded_response() {
        assert_eq!(coingecko_state(false, true), CoinGeckoState::Off);
        assert_eq!(coingecko_state(false, false), CoinGeckoState::Off);
        assert_eq!(coingecko_state(true, false), CoinGeckoState::AwaitingReload);
        assert_eq!(coingecko_state(true, true), CoinGeckoState::Shown);
    }
}
