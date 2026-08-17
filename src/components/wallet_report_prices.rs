use dioxus::prelude::*;

use crate::Route;
use crate::backend::{
    DeletePriceOverrideInput, PriceSourceView, ResolvedPriceView, UpsertPriceOverrideInput,
    delete_price_override, upsert_price_override,
};
use crate::models::{CurrencyCode, UserTimezone};
use crate::services::price_overrides::{BoundaryKind, PriceSubject, price_subject_sort_key};

#[derive(Clone, PartialEq, Props)]
pub(crate) struct PricesSectionProps {
    pub(crate) user_currency: CurrencyCode,
    pub(crate) price_requirements: Vec<(PriceSubject, BoundaryKind)>,
    pub(crate) subject_labels: Vec<(PriceSubject, String)>,
    pub(crate) opening_time_local: String,
    pub(crate) closing_time_local: String,
    pub(crate) user_timezone: UserTimezone,
    pub(crate) resolved_views: Vec<ResolvedPriceView>,
    pub(crate) can_edit_prices: bool,
    pub(crate) on_prices_changed: EventHandler<()>,
}

fn subject_display_label(subject: &PriceSubject, labels: &[(PriceSubject, String)]) -> String {
    match subject {
        PriceSubject::CatalogAsset(_) => labels
            .iter()
            .find(|(s, _)| s == subject)
            .map(|(_, l)| l.clone())
            .unwrap_or_else(|| "Unknown asset".to_string()),
    }
}

fn find_view<'a>(
    views: &'a [ResolvedPriceView],
    subject: &PriceSubject,
    boundary: BoundaryKind,
) -> Option<&'a ResolvedPriceView> {
    views
        .iter()
        .find(|view| view.subject == *subject && view.boundary == boundary)
}

fn provider_label(provider: &str) -> String {
    match provider {
        "coingecko" => "CoinGecko".to_string(),
        other => other
            .split(['_', '-'])
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => {
                        let mut word = first.to_uppercase().collect::<String>();
                        word.push_str(chars.as_str());
                        word
                    }
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn source_display_from_view(view: Option<&ResolvedPriceView>) -> Option<String> {
    view.and_then(|v| v.source.as_ref())
        .and_then(|source| match source {
            PriceSourceView::UserOverride { source_note, .. } => source_note.clone(),
            PriceSourceView::ProviderPrice { provider, .. } => Some(provider_label(provider)),
        })
}

fn source_note_for_editing(view: Option<&ResolvedPriceView>) -> Option<String> {
    view.and_then(|v| v.source.as_ref())
        .and_then(|source| match source {
            PriceSourceView::UserOverride { source_note, .. } => source_note.clone(),
            PriceSourceView::ProviderPrice { .. } => None,
        })
}

fn can_clear_price(view: Option<&ResolvedPriceView>) -> bool {
    matches!(
        view.and_then(|v| v.source.as_ref()),
        Some(PriceSourceView::UserOverride { .. })
    )
}

fn price_action_mode(can_edit_prices: bool, view: Option<&ResolvedPriceView>) -> &'static str {
    if !can_edit_prices {
        "upgrade"
    } else if can_clear_price(view) {
        "edit_clear"
    } else {
        "edit"
    }
}

fn price_upgrade_aria_label(asset_label: &str, boundary_label: &str) -> String {
    format!("Upgrade to add or edit report prices for {asset_label} {boundary_label}")
}

fn boundary_local(boundary: BoundaryKind, opening: &str, closing: &str) -> String {
    match boundary {
        BoundaryKind::Opening => opening.to_string(),
        BoundaryKind::Closing => closing.to_string(),
    }
}

fn boundary_sort_key(boundary: BoundaryKind) -> u8 {
    match boundary {
        BoundaryKind::Opening => 0,
        BoundaryKind::Closing => 1,
    }
}

fn ordered_price_requirements(
    requirements: &[(PriceSubject, BoundaryKind)],
) -> Vec<(PriceSubject, BoundaryKind)> {
    let mut requirements = requirements.to_vec();
    requirements.sort_by_key(|(subject, boundary)| {
        (
            price_subject_sort_key(subject),
            boundary_sort_key(*boundary),
        )
    });
    requirements.dedup();
    requirements
}

fn count_text(resolved: usize, missing: usize) -> String {
    if missing > 0 {
        format!("{missing} missing")
    } else {
        format!("{resolved} prices")
    }
}

#[component]
pub(crate) fn PricesSection(props: PricesSectionProps) -> Element {
    let requirements = ordered_price_requirements(&props.price_requirements);
    let total_boundaries = requirements.len();
    let resolved_count: usize = requirements
        .iter()
        .filter(|(s, b)| {
            find_view(&props.resolved_views, s, *b)
                .and_then(|v| v.price.as_deref())
                .is_some()
        })
        .count();
    let missing_count = total_boundaries.saturating_sub(resolved_count);

    let mut expanded = use_signal(|| missing_count > 0);
    let editing: Signal<Option<(PriceSubject, BoundaryKind)>> = use_signal(|| None);
    let draft_price: Signal<String> = use_signal(String::new);
    let draft_note: Signal<String> = use_signal(String::new);
    let row_error: Signal<Option<String>> = use_signal(|| None);

    let count_label = count_text(resolved_count, missing_count);
    let count_class = if missing_count > 0 {
        "wr-prices-count wr-prices-count-missing"
    } else {
        "wr-prices-count"
    };
    let chevron = if expanded() { "⌃" } else { "⌄" };

    rsx! {
        section { class: "wr-prices-section",
            button {
                class: "wr-prices-strip",
                r#type: "button",
                "aria-expanded": "{expanded()}",
                onclick: move |_| expanded.toggle(),
                span { class: "wr-prices-strip-left",
                    span { class: "wr-prices-title", "§ Prices" }
                    span { class: count_class, "{count_label}" }
                }
                span { class: "wr-prices-chevron", "{chevron}" }
            }
            if expanded() {
                div { class: "wr-prices-panel",
                    p { class: "wr-prices-timezone",
                        "Timezone: {props.user_timezone.name()}"
                    }
                    PricesTable {
                        requirements,
                        subject_labels: props.subject_labels.clone(),
                        user_currency: props.user_currency,
                        opening_time_local: props.opening_time_local.clone(),
                        closing_time_local: props.closing_time_local.clone(),
                        resolved_views: props.resolved_views.clone(),
                        can_edit_prices: props.can_edit_prices,
                        on_prices_changed: props.on_prices_changed,
                        editing,
                        draft_price,
                        draft_note,
                        row_error,
                    }
                }
            }
        }
    }
}

#[derive(Clone, PartialEq, Props)]
struct PricesTableProps {
    requirements: Vec<(PriceSubject, BoundaryKind)>,
    subject_labels: Vec<(PriceSubject, String)>,
    user_currency: CurrencyCode,
    opening_time_local: String,
    closing_time_local: String,
    resolved_views: Vec<ResolvedPriceView>,
    can_edit_prices: bool,
    on_prices_changed: EventHandler<()>,
    editing: Signal<Option<(PriceSubject, BoundaryKind)>>,
    draft_price: Signal<String>,
    draft_note: Signal<String>,
    row_error: Signal<Option<String>>,
}

#[component]
fn PricesTable(props: PricesTableProps) -> Element {
    rsx! {
        table { class: "wr-prices-table",
            thead {
                tr {
                    th { "Asset" }
                    th { "Date" }
                    th { "Price" }
                    th { "Source note" }
                    th { class: "wr-prices-action-col", "" }
                }
            }
            tbody {
                for (subject, boundary) in props.requirements.iter() {
                    PricesRow {
                        key: "{subject_display_label(subject, &props.subject_labels)}-{boundary:?}",
                        subject: subject.clone(),
                        boundary: *boundary,
                        user_currency: props.user_currency,
                        local_time: boundary_local(
                            *boundary,
                            &props.opening_time_local,
                            &props.closing_time_local,
                        ),
                        subject_labels: props.subject_labels.clone(),
                        view: find_view(&props.resolved_views, subject, *boundary).cloned(),
                        can_edit_prices: props.can_edit_prices,
                        on_prices_changed: props.on_prices_changed,
                        editing: props.editing,
                        draft_price: props.draft_price,
                        draft_note: props.draft_note,
                        row_error: props.row_error,
                    }
                }
            }
        }
    }
}

#[derive(Clone, PartialEq, Props)]
struct PricesRowProps {
    subject: PriceSubject,
    boundary: BoundaryKind,
    user_currency: CurrencyCode,
    subject_labels: Vec<(PriceSubject, String)>,
    local_time: String,
    view: Option<ResolvedPriceView>,
    can_edit_prices: bool,
    on_prices_changed: EventHandler<()>,
    editing: Signal<Option<(PriceSubject, BoundaryKind)>>,
    draft_price: Signal<String>,
    draft_note: Signal<String>,
    row_error: Signal<Option<String>>,
}

#[component]
fn PricesRow(props: PricesRowProps) -> Element {
    let key = (props.subject.clone(), props.boundary);
    let action_mode = price_action_mode(props.can_edit_prices, props.view.as_ref());
    let can_edit_prices = props.can_edit_prices;
    let is_editing = can_edit_prices && props.editing.read().as_ref() == Some(&key);
    let current_price = props.view.as_ref().and_then(|v| v.price.clone());
    let current_source_display = source_display_from_view(props.view.as_ref());
    let current_note_for_editing = source_note_for_editing(props.view.as_ref());
    let can_clear_current_price = can_edit_prices && action_mode == "edit_clear";

    let boundary_label = match props.boundary {
        BoundaryKind::Opening => "Opening",
        BoundaryKind::Closing => "Closing",
    };
    let date_display = props
        .local_time
        .split('T')
        .next()
        .unwrap_or(&props.local_time)
        .to_string();
    let asset_label = subject_display_label(&props.subject, &props.subject_labels);
    let upgrade_aria_label = price_upgrade_aria_label(&asset_label, boundary_label);
    let currency_code = props.user_currency.code().to_string();

    let mut editing_sig = props.editing;
    let mut draft_price_sig = props.draft_price;
    let mut draft_note_sig = props.draft_note;
    let mut row_error_sig = props.row_error;
    let on_changed = props.on_prices_changed;
    let currency = props.user_currency;
    let row_boundary = props.boundary;

    let start_edit = use_callback({
        let subject = props.subject.clone();
        let initial_price = current_price.clone().unwrap_or_default();
        let initial_note = current_note_for_editing.clone().unwrap_or_default();
        move |_evt: Event<MouseData>| {
            if !can_edit_prices {
                return;
            }
            editing_sig.set(Some((subject.clone(), row_boundary)));
            draft_price_sig.set(initial_price.clone());
            draft_note_sig.set(initial_note.clone());
            row_error_sig.set(None);
        }
    });

    let cancel_edit = use_callback(move |_evt: Event<MouseData>| {
        editing_sig.set(None);
        row_error_sig.set(None);
    });

    let save = use_callback({
        let subject = props.subject.clone();
        let local_time = props.local_time.clone();
        move |_evt: ()| {
            if !can_edit_prices {
                return;
            }
            let price = draft_price_sig.peek().clone();
            let note_raw = draft_note_sig.peek().clone();
            let trimmed_note = note_raw.trim();
            let source_note = if trimmed_note.is_empty() {
                None
            } else {
                Some(trimmed_note.to_string())
            };
            let subject = subject.clone();
            let local_time = local_time.clone();
            spawn(async move {
                let result = upsert_price_override(UpsertPriceOverrideInput {
                    subject,
                    quote_currency: currency,
                    price_time_local: local_time,
                    price,
                    source_note,
                })
                .await;
                match result {
                    Ok(_) => {
                        editing_sig.set(None);
                        row_error_sig.set(None);
                        on_changed.call(());
                    }
                    Err(err) => {
                        row_error_sig.set(Some(err.to_string()));
                    }
                }
            });
        }
    });

    let on_input_keydown = use_callback(move |evt: Event<KeyboardData>| {
        let key_str = evt.key().to_string();
        if key_str == "Enter" {
            evt.prevent_default();
            save.call(());
        } else if key_str == "Escape" {
            evt.prevent_default();
            editing_sig.set(None);
            row_error_sig.set(None);
        }
    });

    let on_save_click = use_callback(move |_evt: Event<MouseData>| save.call(()));

    let on_delete = use_callback({
        let subject = props.subject.clone();
        let local_time = props.local_time.clone();
        move |_evt: Event<MouseData>| {
            if !can_edit_prices || !can_clear_current_price {
                return;
            }
            let subject = subject.clone();
            let local_time = local_time.clone();
            spawn(async move {
                let result = delete_price_override(DeletePriceOverrideInput {
                    subject,
                    quote_currency: currency,
                    price_time_local: local_time,
                })
                .await;
                match result {
                    Ok(()) => {
                        editing_sig.set(None);
                        row_error_sig.set(None);
                        on_changed.call(());
                    }
                    Err(err) => {
                        row_error_sig.set(Some(err.to_string()));
                    }
                }
            });
        }
    });

    let row_error_text = props.row_error.read().clone();
    let show_error = is_editing && row_error_text.is_some();
    let draft_price_value = props.draft_price.read().clone();
    let draft_note_value = props.draft_note.read().clone();

    rsx! {
        tr {
            td {
                span { class: "wr-prices-asset-cell", "{asset_label}" }
            }
            td {
                span { class: "wr-prices-boundary-eyebrow", "{boundary_label}" }
                span { "{date_display}" }
            }
            td {
                if is_editing {
                    input {
                        class: if show_error { "wr-prices-input wr-prices-input-error" } else { "wr-prices-input" },
                        r#type: "text",
                        inputmode: "decimal",
                        value: "{draft_price_value}",
                        "aria-label": "Price per unit in {currency_code}",
                        autofocus: true,
                        oninput: move |evt| draft_price_sig.set(evt.value()),
                        onkeydown: on_input_keydown,
                    }
                    button {
                        r#type: "button",
                        class: "wr-prices-set-button",
                        onclick: on_save_click,
                        "Save"
                    }
                    if show_error {
                        if let Some(message) = row_error_text.as_ref() {
                            div { class: "wr-prices-error", "{message}" }
                        }
                    }
                } else if let Some(price) = current_price.as_ref() {
                    if can_edit_prices {
                        button {
                            r#type: "button",
                            class: "wr-prices-set-button wr-prices-price-display",
                            onclick: start_edit,
                            "{price}"
                            span { class: "wr-prices-price-suffix", "{currency_code}" }
                        }
                    } else {
                        span { class: "wr-prices-price-display",
                            "{price}"
                            span { class: "wr-prices-price-suffix", "{currency_code}" }
                        }
                    }
                } else if can_edit_prices {
                    button {
                        r#type: "button",
                        class: "wr-prices-set-button",
                        onclick: start_edit,
                        "Set price"
                    }
                } else {
                    span { class: "wr-prices-note-empty", "-" }
                }
            }
            td {
                if is_editing {
                    input {
                        class: "wr-prices-input",
                        r#type: "text",
                        value: "{draft_note_value}",
                        maxlength: "120",
                        placeholder: "Optional note",
                        "aria-label": "Source note",
                        oninput: move |evt| draft_note_sig.set(evt.value()),
                        onkeydown: on_input_keydown,
                    }
                } else if let Some(source_display) = current_source_display.as_ref() {
                    span { "{source_display}" }
                } else {
                    span { class: "wr-prices-note-empty", "-" }
                }
            }
            td { class: "wr-prices-action-col",
                if action_mode == "upgrade" {
                    Link {
                        class: "wr-prices-upgrade-link",
                        title: "Upgrade to add or edit report prices.",
                        "aria-label": "{upgrade_aria_label}",
                        to: Route::Payments,
                        "Upgrade"
                    }
                } else if is_editing {
                    button {
                        r#type: "button",
                        class: "wr-prices-clear-button",
                        onclick: cancel_edit,
                        "Cancel"
                    }
                } else if can_clear_current_price {
                    button {
                        r#type: "button",
                        class: "wr-prices-clear-button",
                        "aria-label": "Clear price",
                        onclick: on_delete,
                        "×"
                    }
                }
            }
        }
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use crate::asset_views::CatalogAssetKey;

    fn catalog(key: &str) -> PriceSubject {
        PriceSubject::CatalogAsset(CatalogAssetKey::try_new(key).expect("valid key"))
    }

    #[test]
    fn count_text_uses_missing_when_any_missing() {
        assert_eq!(count_text(2, 0), "2 prices");
        assert_eq!(count_text(3, 1), "1 missing");
    }

    #[test]
    fn ordered_price_requirements_dedupes_and_sorts() {
        let requirements = ordered_price_requirements(&[
            (catalog("ethereum"), BoundaryKind::Closing),
            (catalog("bitcoin"), BoundaryKind::Opening),
            (catalog("bitcoin"), BoundaryKind::Opening),
        ]);
        assert_eq!(
            requirements,
            vec![
                (catalog("bitcoin"), BoundaryKind::Opening),
                (catalog("ethereum"), BoundaryKind::Closing),
            ]
        );
    }

    #[test]
    fn price_source_display_handles_user_override_provider_and_missing() {
        let manual = ResolvedPriceView {
            subject: catalog("bitcoin"),
            boundary: BoundaryKind::Opening,
            price: Some("50000".to_string()),
            source: Some(PriceSourceView::UserOverride {
                source_note: Some("snapshot".to_string()),
                updated_at: chrono::Utc::now(),
            }),
        };
        let provider = ResolvedPriceView {
            subject: catalog("bitcoin"),
            boundary: BoundaryKind::Closing,
            price: Some("51000".to_string()),
            source: Some(PriceSourceView::ProviderPrice {
                provider: "coingecko".to_string(),
                provider_asset_id: Some("bitcoin".to_string()),
                provider_quote_id: Some("usd".to_string()),
                retrieved_at: chrono::Utc::now(),
                license_scope: "public_keyless".to_string(),
            }),
        };
        let unknown = ResolvedPriceView {
            subject: catalog("bitcoin"),
            boundary: BoundaryKind::Closing,
            price: Some("51000".to_string()),
            source: Some(PriceSourceView::ProviderPrice {
                provider: "coin_market_cap".to_string(),
                provider_asset_id: None,
                provider_quote_id: None,
                retrieved_at: chrono::Utc::now(),
                license_scope: "public_keyless".to_string(),
            }),
        };

        assert_eq!(
            source_display_from_view(Some(&manual)),
            Some("snapshot".to_string())
        );
        assert_eq!(
            source_display_from_view(Some(&provider)),
            Some("CoinGecko".to_string())
        );
        assert_eq!(
            source_display_from_view(Some(&unknown)),
            Some("Coin Market Cap".to_string())
        );
        assert_eq!(source_display_from_view(None), None);
    }

    #[test]
    fn price_source_note_for_editing_only_uses_user_override_notes() {
        let manual = ResolvedPriceView {
            subject: catalog("bitcoin"),
            boundary: BoundaryKind::Opening,
            price: Some("50000".to_string()),
            source: Some(PriceSourceView::UserOverride {
                source_note: Some("snapshot".to_string()),
                updated_at: chrono::Utc::now(),
            }),
        };
        let provider = ResolvedPriceView {
            subject: catalog("bitcoin"),
            boundary: BoundaryKind::Closing,
            price: Some("51000".to_string()),
            source: Some(PriceSourceView::ProviderPrice {
                provider: "coingecko".to_string(),
                provider_asset_id: Some("bitcoin".to_string()),
                provider_quote_id: Some("usd".to_string()),
                retrieved_at: chrono::Utc::now(),
                license_scope: "public_keyless".to_string(),
            }),
        };

        assert_eq!(
            source_note_for_editing(Some(&manual)),
            Some("snapshot".to_string())
        );
        assert_eq!(source_note_for_editing(Some(&provider)), None);
    }

    #[test]
    fn price_source_clear_action_only_applies_to_user_overrides() {
        let manual = ResolvedPriceView {
            subject: catalog("bitcoin"),
            boundary: BoundaryKind::Opening,
            price: Some("50000".to_string()),
            source: Some(PriceSourceView::UserOverride {
                source_note: None,
                updated_at: chrono::Utc::now(),
            }),
        };
        let provider = ResolvedPriceView {
            subject: catalog("bitcoin"),
            boundary: BoundaryKind::Closing,
            price: Some("51000".to_string()),
            source: Some(PriceSourceView::ProviderPrice {
                provider: "coingecko".to_string(),
                provider_asset_id: Some("bitcoin".to_string()),
                provider_quote_id: Some("usd".to_string()),
                retrieved_at: chrono::Utc::now(),
                license_scope: "public_keyless".to_string(),
            }),
        };

        assert!(can_clear_price(Some(&manual)));
        assert!(!can_clear_price(Some(&provider)));
        assert!(!can_clear_price(None));
    }

    #[test]
    fn price_action_mode_requires_edit_entitlement() {
        let manual = ResolvedPriceView {
            subject: catalog("bitcoin"),
            boundary: BoundaryKind::Opening,
            price: Some("50000".to_string()),
            source: Some(PriceSourceView::UserOverride {
                source_note: None,
                updated_at: chrono::Utc::now(),
            }),
        };

        assert_eq!(price_action_mode(false, Some(&manual)), "upgrade");
        assert_eq!(price_action_mode(true, Some(&manual)), "edit_clear");
    }

    #[test]
    fn price_upgrade_aria_label_includes_row_context() {
        assert_eq!(
            price_upgrade_aria_label("Bitcoin", "Opening"),
            "Upgrade to add or edit report prices for Bitcoin Opening"
        );
    }

    #[test]
    fn boundary_local_uses_correct_timestamp() {
        assert_eq!(
            boundary_local(
                BoundaryKind::Opening,
                "2025-01-01T00:00:00",
                "2025-12-31T23:59:59"
            ),
            "2025-01-01T00:00:00"
        );
        assert_eq!(
            boundary_local(
                BoundaryKind::Closing,
                "2025-01-01T00:00:00",
                "2025-12-31T23:59:59"
            ),
            "2025-12-31T23:59:59"
        );
    }
}
