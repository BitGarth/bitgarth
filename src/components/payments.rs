use crate::backend::{
    cancel_premium_order, get_payment_catalog, get_payment_state_local, poll_premium_order,
    reconcile_payment_history, refresh_payment_state, refresh_premium_status, start_premium_order,
    start_premium_top_up,
};
use crate::components::formatting::{format_date_for_display, format_number_for_display};
use crate::components::wallets::truncate_reference_with_lengths;
use crate::legal::TERMS_URL;
use crate::models::DateTimeFormat;
use crate::models::NumberFormat;
use crate::payments::views::{
    AppCompatibilityStatusView, AppCompatibilityView, BulletSegmentView, PaymentCatalogView,
    PaymentOptionView, PaymentOrderHistoryView, PaymentOrderStatusView, PaymentStateStatus,
    PaymentStateView, PaymentSupportReferenceView, PaymentTierView, PremiumOrderLaunchView,
    TierBulletView,
};
use crate::settings::SettingsState;
use crate::timezone::format_timestamp;
use crate::{AuthState, AuthStatus, Route};
use chrono::{DateTime, Utc};
use dioxus::document::eval;
use dioxus::logger::tracing;
use dioxus::prelude::*;

use super::{CheckIcon, CopyIcon, ExternalLinkIcon, RefreshIcon, copy_to_clipboard};

const ATLOS_SCRIPT_URL: &str = "https://atlos.io/packages/app/atlos.js";

// ─────────────────────────── helpers ───────────────────────────

fn format_rfc3339_as_display_date(raw: &str) -> Option<String> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| format_date_for_display(dt.naive_utc().date(), DateTimeFormat::MonthDayYear12))
}

fn build_widget_script(start: &PremiumOrderLaunchView) -> Result<String, String> {
    let merchant = serde_json::to_string(&start.merchant_id).map_err(|e| e.to_string())?;
    let order = serde_json::to_string(&start.atlos_order_id).map_err(|e| e.to_string())?;
    let amount = serde_json::to_string(&start.order_amount).map_err(|e| e.to_string())?;
    let currency = serde_json::to_string(&start.order_currency).map_err(|e| e.to_string())?;
    Ok(format!(
        r#"
(() => {{
  try {{
    if (typeof window === "undefined") {{
      dioxus.send(JSON.stringify({{ kind: "error", message: "ATLOS widget is only available in the browser." }}));
      return;
    }}
    if (typeof atlos === "undefined" || typeof atlos.Pay !== "function") {{
      dioxus.send(JSON.stringify({{ kind: "error", message: "ATLOS widget is not available yet. Please refresh and try again." }}));
      return;
    }}
    atlos.Pay({{
      merchantId: {merchant},
      orderId: {order},
      orderAmount: Number({amount}),
      orderCurrency: {currency},
      onCompleted: () => {{ dioxus.send(JSON.stringify({{ kind: "completed" }})); }},
      onCanceled: () => {{ dioxus.send(JSON.stringify({{ kind: "canceled" }})); }}
    }});
    dioxus.send(JSON.stringify({{ kind: "launched" }}));
  }} catch (e) {{
    dioxus.send(JSON.stringify({{ kind: "error", message: "Failed to launch ATLOS widget: " + String(e) }}));
  }}
}})();
"#
    ))
}

#[derive(Debug, serde::Deserialize)]
struct WidgetMessage {
    kind: String,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WidgetPhase {
    Idle,
    Launching,
    Checking,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WidgetCancelBehavior {
    CancelOrder,
    KeepOrder,
}

fn history_reconcile_key(view: &PaymentStateView) -> Option<String> {
    match view.status {
        PaymentStateStatus::Canceled | PaymentStateStatus::Failed | PaymentStateStatus::Expired => {
            Some(format!(
                "{}:{}",
                view.status_tag(),
                view.order_id.as_deref().unwrap_or_default()
            ))
        }
        _ => None,
    }
}

/// Render the price for a purchase option using the user's number format
/// setting. Trailing zero cents are dropped — `$5.00` becomes `$5` — but a
/// non-zero fractional part (e.g. `$1.23`) is preserved verbatim. Currency
/// symbol prefix when available, currency code suffix otherwise.
fn format_payment_option_price(option: &PaymentOptionView, number_format: NumberFormat) -> String {
    let amount = trim_round_amount(&option.display_amount);
    let formatted = format_number_for_display(&amount, number_format);
    if option.currency_symbol.trim().is_empty() {
        format!("{formatted} {}", option.currency)
    } else {
        format!("{}{formatted}", option.currency_symbol)
    }
}

/// Strip a trailing `.00…0` (any number of zeroes) from a decimal string so
/// integer prices render as `5` instead of `5.00`. Non-zero fractional digits
/// stay intact (`1.23` → `1.23`, `5.50` → `5.5`).
fn trim_round_amount(raw: &str) -> String {
    let Some((whole, fraction)) = raw.split_once('.') else {
        return raw.to_string();
    };
    let trimmed = fraction.trim_end_matches('0');
    if trimmed.is_empty() {
        whole.to_string()
    } else {
        format!("{whole}.{trimmed}")
    }
}

fn tier_display_name(raw: &str) -> String {
    match raw {
        "free" => "Free".to_string(),
        "basic" => "Basic".to_string(),
        "premium" => "Premium".to_string(),
        other if other.trim().is_empty() => "Paid plan".to_string(),
        _ => "Paid plan".to_string(),
    }
}

fn status_uses_auto_poll(status: PaymentStateStatus) -> bool {
    matches!(
        status,
        PaymentStateStatus::Pending | PaymentStateStatus::Verifying
    )
}

fn is_in_flight_status(status: PaymentStateStatus) -> bool {
    matches!(
        status,
        PaymentStateStatus::Pending
            | PaymentStateStatus::Verifying
            | PaymentStateStatus::AdditionalPaymentRequired
            | PaymentStateStatus::ManualReview
    )
}

fn is_terminal_inflow_status(status: PaymentStateStatus) -> bool {
    matches!(
        status,
        PaymentStateStatus::Expired
            | PaymentStateStatus::Failed
            | PaymentStateStatus::Canceled
            | PaymentStateStatus::Unavailable
            | PaymentStateStatus::RecoveryFailed
    )
}

fn support_reference_renders_in_shell(status: PaymentStateStatus) -> bool {
    !matches!(
        status,
        PaymentStateStatus::ActiveWithSyncWarning | PaymentStateStatus::RecoveryFailed
    )
}

/// Pick the term label (e.g. "1 year", "1 month") that should be selected
/// globally given the server's catalog. We prefer whichever option is marked
/// `is_default` somewhere in the catalog; if multiple defaults exist with
/// different terms we take the first. If nothing is marked default we fall
/// back to the first option's term.
fn default_global_term(options: &[PaymentOptionView]) -> Option<String> {
    options
        .iter()
        .find(|o| o.is_default)
        .map(|o| o.term_label.clone())
        .or_else(|| options.first().map(|o| o.term_label.clone()))
}

/// Resolve the option a given tier should display, given the global term
/// preference. Falls back to the tier's own `is_default` (or first available
/// option) when the global term isn't on offer for this tier.
fn resolve_tier_option<'a>(
    tier: &str,
    options: &'a [PaymentOptionView],
    selected_term: Option<&str>,
) -> Option<&'a PaymentOptionView> {
    if let Some(term) = selected_term
        && let Some(match_) = options
            .iter()
            .find(|o| o.tier == tier && o.term_label == term)
    {
        return Some(match_);
    }
    options
        .iter()
        .find(|o| o.tier == tier && o.is_default)
        .or_else(|| options.iter().find(|o| o.tier == tier))
}

/// Reconciles signals derived from a freshly resolved plan status. Shared by
/// the SSR state effect and the post-paint Central refresh.
fn apply_resolved_payment_state(
    view: PaymentStateView,
    mut state: Signal<Option<PaymentStateView>>,
    mut active_order_id: Signal<Option<String>>,
    mut widget_phase: Signal<WidgetPhase>,
    mut auto_refresh_attempted: Signal<bool>,
    mut sync_warning_dismissed: Signal<bool>,
) {
    if status_uses_auto_poll(view.status) {
        if let Some(id) = view.order_id.clone()
            && active_order_id.peek().as_deref() != Some(id.as_str())
        {
            active_order_id.set(Some(id));
        }
    } else if active_order_id.peek().is_some() {
        active_order_id.set(None);
    }
    if view.status != PaymentStateStatus::Verifying && *widget_phase.peek() == WidgetPhase::Checking
    {
        widget_phase.set(WidgetPhase::Idle);
    }
    if view.status == PaymentStateStatus::ManualReview {
        widget_phase.set(WidgetPhase::Idle);
    }
    if view.status != PaymentStateStatus::Unavailable {
        auto_refresh_attempted.set(false);
    }
    if view.status != PaymentStateStatus::ActiveWithSyncWarning {
        sync_warning_dismissed.set(false);
    }
    state.set(Some(view));
}

// ─────────────────────────── component: Payments ───────────────────────────

#[component]
pub fn Payments() -> Element {
    let mut auth_state = use_context::<AuthState>();

    // Plan status resolves from local data only, so SSR renders it instantly.
    let state_resource = use_server_future(move || async move { get_payment_state_local().await })?;
    // The Central-dependent tier catalog loads client-side behind a skeleton,
    // so a slow or unreachable Central never blocks first paint.
    let mut catalog_resource = use_resource(move || async move { get_payment_catalog().await });

    let mut state = use_signal(|| None::<PaymentStateView>);
    let mut payment_options = use_signal(Vec::<PaymentOptionView>::new);
    let mut payment_tiers = use_signal(Vec::<PaymentTierView>::new);
    let mut order_history = use_signal(Vec::<PaymentOrderHistoryView>::new);
    let mut app_compatibility = use_signal(|| None::<AppCompatibilityView>);
    let mut options_message = use_signal(|| None::<String>);
    let mut pricing_summary = use_signal(|| None::<TierBulletView>);
    let mut selected_term = use_signal(|| None::<String>);
    let mut action_error = use_signal(|| None::<String>);
    let mut acting = use_signal(|| false);
    let mut active_order_id = use_signal(|| None::<String>);
    let mut widget_phase = use_signal(|| WidgetPhase::Idle);
    let mut auto_refresh_attempted = use_signal(|| false);
    let mut last_history_reconcile_key = use_signal(|| None::<String>);
    let mut sync_warning_dismissed = use_signal(|| false);
    let mut state_refreshing = use_signal(|| false);
    // True when an in-flight order poll could not reach Central; drives a
    // small non-blocking note while the poll loop keeps retrying.
    let mut poll_refresh_failed = use_signal(|| false);

    // SSR-resolved local plan status.
    let state_value = state_resource.value();
    use_effect(move || {
        let value = state_value.read().clone();
        match value {
            Some(Ok(view)) => apply_resolved_payment_state(
                view,
                state,
                active_order_id,
                widget_phase,
                auto_refresh_attempted,
                sync_warning_dismissed,
            ),
            Some(Err(err)) => {
                if err.is_unauthorized() {
                    auth_state.set(AuthStatus::Unauthenticated);
                } else {
                    action_error.set(Some(err.message.clone()));
                }
            }
            None => {}
        }
    });

    // Client-side tier catalog. Failures surface as the contained catalog
    // error in render; only an auth failure needs to be hoisted here.
    let catalog_value = catalog_resource.value();
    use_effect(move || {
        let value = catalog_value.read().clone();
        match value {
            Some(Ok(catalog)) => {
                let PaymentCatalogView {
                    tiers,
                    options,
                    app_compatibility: compatibility,
                    options_message: page_options_message,
                    order_history: page_order_history,
                    pricing_summary: page_pricing_summary,
                } = catalog;
                // Preserve the user's chosen term if it's still on offer
                // anywhere in the new catalog; otherwise seed from the
                // server's default.
                let prior_term = selected_term.peek().clone();
                let next_term = prior_term
                    .filter(|term| options.iter().any(|o| &o.term_label == term))
                    .or_else(|| default_global_term(&options));
                payment_tiers.set(tiers);
                payment_options.set(options);
                order_history.set(page_order_history);
                selected_term.set(next_term);
                app_compatibility.set(compatibility);
                options_message.set(page_options_message);
                pricing_summary.set(page_pricing_summary);
            }
            Some(Err(err)) if err.is_unauthorized() => {
                auth_state.set(AuthStatus::Unauthenticated);
            }
            Some(Err(_)) => {}
            None => {}
        }
    });

    // After first paint, reconcile the SSR-rendered last-known status with
    // Central. Failure is silent — the page keeps showing the local status.
    use_effect(move || {
        state_refreshing.set(true);
        spawn(async move {
            match refresh_payment_state().await {
                Ok(refreshed) => apply_resolved_payment_state(
                    refreshed,
                    state,
                    active_order_id,
                    widget_phase,
                    auto_refresh_attempted,
                    sync_warning_dismissed,
                ),
                Err(err) => {
                    if err.is_unauthorized() {
                        auth_state.set(AuthStatus::Unauthenticated);
                    } else {
                        tracing::debug!(error = %err, "payments: post-paint refresh failed");
                    }
                }
            }
            state_refreshing.set(false);
        });
    });

    use_effect(move || {
        let Some(order_id) = active_order_id() else {
            return;
        };
        poll_refresh_failed.set(false);
        spawn(async move {
            let mut elapsed_secs: u64 = 0;
            let mut verification_started_at_secs: Option<u64> = None;
            loop {
                if active_order_id.peek().as_deref() != Some(order_id.as_str()) {
                    break;
                }
                match poll_premium_order(order_id.clone()).await {
                    Ok(view) => {
                        poll_refresh_failed.set(false);
                        let continue_polling = status_uses_auto_poll(view.status);
                        let is_verifying = view.status == PaymentStateStatus::Verifying;
                        if is_verifying {
                            if verification_started_at_secs.is_none() {
                                verification_started_at_secs = Some(elapsed_secs);
                            }
                        } else {
                            verification_started_at_secs = None;
                        }
                        let verification_window_open = verification_started_at_secs
                            .is_none_or(|started_at_secs| elapsed_secs - started_at_secs < 120);
                        state.set(Some(view));
                        if continue_polling && verification_window_open {
                            widget_phase.set(WidgetPhase::Checking);
                        } else {
                            widget_phase.set(WidgetPhase::Idle);
                            active_order_id.set(None);
                            break;
                        }
                    }
                    Err(err) => {
                        tracing::debug!(error = %err, "payments: poll failed");
                        if err.is_unauthorized() {
                            auth_state.set(AuthStatus::Unauthenticated);
                            break;
                        }
                        // Central is unreachable: keep the last-known status
                        // and keep polling quietly with backoff.
                        poll_refresh_failed.set(true);
                    }
                }
                let interval_ms = if elapsed_secs < 120 {
                    10_000u64
                } else {
                    30_000u64
                };
                let mut timer = eval(&format!(
                    "setTimeout(() => {{ dioxus.send(null); }}, {interval_ms});"
                ));
                if timer.recv::<serde_json::Value>().await.is_err() {
                    break;
                }
                elapsed_secs += interval_ms / 1000;
            }
        });
    });

    use_effect(move || {
        let snapshot = state.read().clone();
        let Some(view) = snapshot else {
            return;
        };
        if view.status != PaymentStateStatus::Unavailable {
            return;
        }
        if *auto_refresh_attempted.peek() {
            return;
        }
        auto_refresh_attempted.set(true);
        spawn(async move {
            match refresh_premium_status().await {
                Ok(refreshed) => {
                    state.set(Some(refreshed));
                }
                Err(err) => {
                    tracing::debug!(error = %err, "payments: auto refresh failed");
                    if err.is_unauthorized() {
                        auth_state.set(AuthStatus::Unauthenticated);
                    }
                }
            }
        });
    });

    use_effect(move || {
        let Some(view) = state.read().clone() else {
            return;
        };
        let Some(key) = history_reconcile_key(&view) else {
            return;
        };
        if last_history_reconcile_key.peek().as_deref() == Some(key.as_str()) {
            return;
        }
        last_history_reconcile_key.set(Some(key));
        spawn(async move {
            if let Ok(refreshed) = reconcile_payment_history().await {
                state.set(Some(refreshed));
            }
        });
    });

    let is_authenticated = matches!(&*auth_state.read(), AuthStatus::Authenticated(_));

    let state_snapshot = state.read().clone();
    let payment_tiers_snapshot = payment_tiers.read().clone();
    let payment_options_snapshot = payment_options.read().clone();
    let order_history_snapshot = order_history.read().clone();
    let selected_term_snapshot = selected_term.read().clone();
    let app_compatibility_snapshot = app_compatibility.read().clone();
    let options_message_snapshot = options_message.read().clone();
    let pricing_summary_snapshot = pricing_summary.read().clone();

    let mut launch_widget =
        move |start: PremiumOrderLaunchView, cancel_behavior: WidgetCancelBehavior| {
            let script = match build_widget_script(&start) {
                Ok(script) => script,
                Err(err) => {
                    action_error.set(Some(err));
                    widget_phase.set(WidgetPhase::Idle);
                    return;
                }
            };
            widget_phase.set(WidgetPhase::Launching);
            spawn(async move {
                let mut channel = eval(script.as_str());
                while let Ok(raw) = channel.recv::<serde_json::Value>().await {
                    let Some(text) = raw.as_str() else {
                        continue;
                    };
                    let Ok(message) = serde_json::from_str::<WidgetMessage>(text) else {
                        continue;
                    };
                    match message.kind.as_str() {
                        "launched" => {}
                        "completed" => {
                            widget_phase.set(WidgetPhase::Checking);
                            let order_id = active_order_id
                                .peek()
                                .clone()
                                .unwrap_or_else(|| start.central_order_id.clone());
                            spawn(async move {
                                if let Ok(view) = poll_premium_order(order_id).await {
                                    if !status_uses_auto_poll(view.status) {
                                        active_order_id.set(None);
                                        widget_phase.set(WidgetPhase::Idle);
                                    } else {
                                        active_order_id.set(view.order_id.clone());
                                    }
                                    state.set(Some(view));
                                }
                            });
                        }
                        "canceled" => {
                            widget_phase.set(WidgetPhase::Idle);
                            match cancel_behavior {
                                WidgetCancelBehavior::CancelOrder => {
                                    let order_id_to_cancel = active_order_id.peek().clone();
                                    if order_id_to_cancel.is_some() {
                                        active_order_id.set(None);
                                    }
                                    if let Some(order_id_for_cancel) = order_id_to_cancel {
                                        spawn(async move {
                                            if let Ok(view) =
                                                cancel_premium_order(order_id_for_cancel).await
                                            {
                                                state.set(Some(view));
                                            }
                                        });
                                    }
                                }
                                WidgetCancelBehavior::KeepOrder => {
                                    active_order_id.set(None);
                                    state.set(Some(start.state.clone()));
                                }
                            }
                        }
                        "error" => {
                            widget_phase.set(WidgetPhase::Idle);
                            action_error.set(Some(
                                message.message.unwrap_or_else(|| {
                                    "Failed to launch ATLOS widget.".to_string()
                                }),
                            ));
                        }
                        _ => {}
                    }
                }
            });
        };

    let mut start_and_launch_for = move |product_option_id: String| {
        if acting() {
            return;
        }
        if let Some(view) = state.peek().clone() {
            match view.status {
                PaymentStateStatus::Pending | PaymentStateStatus::Verifying => {
                    action_error.set(Some(
                        "BitGarth is still checking the current payment.".to_string(),
                    ));
                    return;
                }
                PaymentStateStatus::AdditionalPaymentRequired => {
                    action_error.set(Some(
                        "Pay the remaining amount before starting another checkout.".to_string(),
                    ));
                    return;
                }
                PaymentStateStatus::ManualReview => {
                    action_error.set(Some(
                        "This payment is under manual review. Wait before starting another checkout."
                            .to_string(),
                    ));
                    return;
                }
                _ => {}
            }
        }
        let upgrade_blocked = app_compatibility
            .peek()
            .as_ref()
            .is_some_and(|compatibility| {
                compatibility.status == AppCompatibilityStatusView::UpgradeRequired
            });
        if upgrade_blocked {
            let detail = app_compatibility
                .peek()
                .as_ref()
                .map(|c| c.detail.clone())
                .unwrap_or_else(|| "BitGarth needs to be updated before checkout.".to_string());
            action_error.set(Some(detail));
            return;
        }
        if !payment_options
            .peek()
            .iter()
            .any(|option| option.id == product_option_id)
        {
            let message = options_message
                .peek()
                .clone()
                .unwrap_or_else(|| "Price unavailable. Try again in a moment.".to_string());
            action_error.set(Some(message));
            return;
        }
        acting.set(true);
        action_error.set(None);
        spawn(async move {
            match start_premium_order(product_option_id).await {
                Ok(start) => {
                    state.set(Some(start.state.clone()));
                    active_order_id.set(Some(start.central_order_id.clone()));
                    launch_widget(start, WidgetCancelBehavior::CancelOrder);
                }
                Err(err) => {
                    if err.is_unauthorized() {
                        auth_state.set(AuthStatus::Unauthenticated);
                    } else {
                        action_error.set(Some(err.message.clone()));
                    }
                }
            }
            acting.set(false);
        });
    };

    let mut start_top_up_and_launch = move || {
        if acting() {
            return;
        }
        let Some(order_id) = state.peek().as_ref().and_then(|view| view.order_id.clone()) else {
            return;
        };
        acting.set(true);
        action_error.set(None);
        spawn(async move {
            match start_premium_top_up(order_id).await {
                Ok(result) => {
                    state.set(Some(result.state.clone()));
                    if let Some(launch) = result.launch {
                        active_order_id.set(Some(launch.central_order_id.clone()));
                        launch_widget(launch, WidgetCancelBehavior::KeepOrder);
                    } else if status_uses_auto_poll(result.state.status) {
                        active_order_id.set(result.state.order_id.clone());
                    } else {
                        active_order_id.set(None);
                    }
                }
                Err(err) => {
                    if err.is_unauthorized() {
                        auth_state.set(AuthStatus::Unauthenticated);
                    } else {
                        action_error.set(Some(err.message.clone()));
                    }
                }
            }
            acting.set(false);
        });
    };

    let mut check_now = move || {
        let Some(order_id) = state.peek().as_ref().and_then(|view| view.order_id.clone()) else {
            return;
        };
        if acting() {
            return;
        }
        acting.set(true);
        action_error.set(None);
        spawn(async move {
            widget_phase.set(WidgetPhase::Checking);
            match poll_premium_order(order_id).await {
                Ok(view) => {
                    if !status_uses_auto_poll(view.status) {
                        widget_phase.set(WidgetPhase::Idle);
                        active_order_id.set(None);
                    } else {
                        active_order_id.set(view.order_id.clone());
                    }
                    state.set(Some(view));
                }
                Err(err) => {
                    widget_phase.set(WidgetPhase::Idle);
                    if err.is_unauthorized() {
                        auth_state.set(AuthStatus::Unauthenticated);
                    } else {
                        action_error.set(Some(err.message.clone()));
                    }
                }
            }
            acting.set(false);
        });
    };

    let mut refresh_status = move || {
        if acting() {
            return;
        }
        acting.set(true);
        action_error.set(None);
        sync_warning_dismissed.set(false);
        spawn(async move {
            match refresh_premium_status().await {
                Ok(refreshed) => {
                    state.set(Some(refreshed));
                }
                Err(err) => {
                    if err.is_unauthorized() {
                        auth_state.set(AuthStatus::Unauthenticated);
                    } else {
                        action_error.set(Some(err.message.clone()));
                    }
                }
            }
            acting.set(false);
        });
    };

    let mut reconcile_history = move || {
        if acting() {
            return;
        }
        acting.set(true);
        action_error.set(None);
        spawn(async move {
            match reconcile_payment_history().await {
                Ok(refreshed) => {
                    state.set(Some(refreshed));
                }
                Err(err) => {
                    if err.is_unauthorized() {
                        auth_state.set(AuthStatus::Unauthenticated);
                    } else {
                        action_error.set(Some(err.message.clone()));
                    }
                }
            }
            acting.set(false);
        });
    };

    // ── render ──────────────────────────────────────────────────────────

    let upgrade_required = app_compatibility_snapshot
        .as_ref()
        .is_some_and(|c| c.status == AppCompatibilityStatusView::UpgradeRequired);
    let upgrade_detail = app_compatibility_snapshot
        .as_ref()
        .map(|c| c.detail.clone())
        .or(options_message_snapshot.clone());
    // The tier catalog is still loading until its resource resolves.
    let catalog_loading = catalog_value.read().is_none();

    rsx! {
        div { class: "page-container payments-page",
            document::Script { src: ATLOS_SCRIPT_URL, r#async: true }

            div { class: "payments-section-head",
                h1 { class: "payments-section-title",
                    "Plans, "
                    em { "honestly drawn." }
                }
            }

            if state_refreshing() {
                p {
                    class: "muted payments-refreshing",
                    "data-testid": "payments-refreshing",
                    "Refreshing…"
                }
            }

            if !is_authenticated {
                div { class: "card payments-login-prompt",
                    div { class: "card-body",
                        p {
                            "You need to be logged in to manage paid plans."
                            " "
                            Link { to: Route::Login, class: "auth-link", "Go to login" }
                        }
                    }
                }
            } else if let Some(view) = state_snapshot.clone() {
                PaymentsBody {
                    view: view.clone(),
                    payment_tiers: payment_tiers_snapshot.clone(),
                    payment_options: payment_options_snapshot.clone(),
                    order_history: order_history_snapshot.clone(),
                    selected_term: selected_term_snapshot.clone(),
                    app_compatibility: app_compatibility_snapshot.clone(),
                    pricing_summary: pricing_summary_snapshot.clone(),
                    upgrade_detail,
                    upgrade_required,
                    catalog_loading,
                    poll_refresh_failed: poll_refresh_failed(),
                    acting: acting(),
                    widget_phase: widget_phase(),
                    action_error: action_error(),
                    sync_warning_dismissed: sync_warning_dismissed(),
                    on_select_term: EventHandler::new(move |term: String| {
                        selected_term.set(Some(term));
                    }),
                    on_retry_catalog: EventHandler::new(move |_| {
                        catalog_resource.restart();
                    }),
                    on_buy_option: EventHandler::new(move |id: String| start_and_launch_for(id)),
                    on_check_now: EventHandler::new(move |_| check_now()),
                    on_top_up: EventHandler::new(move |_| start_top_up_and_launch()),
                    on_refresh: EventHandler::new(move |_| refresh_status()),
                    on_reconcile_history: EventHandler::new(move |_| reconcile_history()),
                    on_dismiss_sync_warning: EventHandler::new(move |_| {
                        sync_warning_dismissed.set(true);
                    }),
                }
            } else if let Some(message) = action_error() {
                div { class: "card",
                    div { class: "card-body",
                        p { class: "alert alert-error", "{message}" }
                    }
                }
            } else {
                div { class: "card",
                    div { class: "card-body",
                        p { class: "muted", "Loading payment plans..." }
                    }
                }
            }
        }
    }
}

// ─────────────────────────── PaymentsBody (composition) ───────────────────────────

#[component]
fn PaymentsBody(
    view: PaymentStateView,
    payment_tiers: Vec<PaymentTierView>,
    payment_options: Vec<PaymentOptionView>,
    order_history: Vec<PaymentOrderHistoryView>,
    selected_term: Option<String>,
    app_compatibility: Option<AppCompatibilityView>,
    pricing_summary: Option<TierBulletView>,
    upgrade_detail: Option<String>,
    upgrade_required: bool,
    catalog_loading: bool,
    poll_refresh_failed: bool,
    acting: bool,
    widget_phase: WidgetPhase,
    action_error: Option<String>,
    sync_warning_dismissed: bool,
    on_select_term: EventHandler<String>,
    on_buy_option: EventHandler<String>,
    on_check_now: EventHandler<()>,
    on_top_up: EventHandler<()>,
    on_refresh: EventHandler<()>,
    on_reconcile_history: EventHandler<()>,
    on_dismiss_sync_warning: EventHandler<()>,
    on_retry_catalog: EventHandler<()>,
) -> Element {
    let in_flight = is_in_flight_status(view.status);
    let terminal = is_terminal_inflow_status(view.status);
    // Status panel only renders when the user actually needs an action from
    // the page (mid-flow, terminal/retry, unavailable, update-required). The
    // Active state shows its current-plan information and refresh control on
    // `CurrentPlanLine` instead.
    let show_status_panel = in_flight
        || terminal
        || view.status == PaymentStateStatus::UpgradeRequired
        || view.status == PaymentStateStatus::ActiveWithSyncWarning;
    let grid_disabled =
        in_flight || acting || widget_phase != WidgetPhase::Idle || upgrade_required;
    let is_active = matches!(
        view.status,
        PaymentStateStatus::Active | PaymentStateStatus::ActiveWithSyncWarning
    );
    let shell_support_reference = if support_reference_renders_in_shell(view.status) {
        view.support_reference.clone()
    } else {
        None
    };

    rsx! {
        div {
            class: "payments-shell",
            "data-testid": "payments-card",
            "data-status": view.status_tag(),

            // Central-authored plan comparison when available; bold runs
            // render as tier chips (first chip moss, second copper — see
            // .payments-intro em) so renamed tiers keep the treatment.
            // App-authored copy is the fallback while the catalog loads or
            // when Central omits the field.
            if let Some(summary) = pricing_summary.as_ref().filter(|s| !s.segments.is_empty()) {
                p { class: "payments-intro",
                    for (idx, segment) in summary.segments.iter().enumerate() {
                        match segment {
                            BulletSegmentView::Plain(text) => rsx! {
                                span { key: "{idx}", "{text}" }
                            },
                            BulletSegmentView::Bold(text) => rsx! {
                                em { key: "{idx}", "{text}" }
                            },
                        }
                    }
                }
            } else {
                p { class: "payments-intro",
                    em { "Free" }
                    " syncs your balances and tracks manual asset accounts. "
                    em { "Paid" }
                    " tiers add transaction-history sync and more synced accounts."
                }
            }

            if show_status_panel {
                StatusPanel {
                    view: view.clone(),
                    widget_phase,
                    acting,
                    upgrade_required,
                    upgrade_detail: app_compatibility
                        .as_ref()
                        .map(|c| c.detail.clone()),
                    on_check_now,
                    on_top_up,
                    on_refresh,
                    on_reconcile_history,
                    sync_warning_dismissed,
                    on_dismiss_sync_warning,
                }
            }

            if poll_refresh_failed && in_flight {
                p { class: "muted payments-poll-note", "data-testid": "payments-poll-retry-note",
                    "Couldn't reach BitGarth to refresh this payment — retrying…"
                }
            }

            if upgrade_required {
                UpgradeRequiredNotice { detail: upgrade_detail.clone() }
            }

            if catalog_loading {
                CatalogSkeleton {}
            } else if !payment_tiers.is_empty() {
                TierGrid {
                    tiers: payment_tiers.clone(),
                    options: payment_options.clone(),
                    current_tier: view.tier.clone(),
                    paid_through: view
                        .paid_through
                        .as_deref()
                        .and_then(format_rfc3339_as_display_date),
                    selected_term,
                    is_dimmed: in_flight,
                    disable_buys: grid_disabled,
                    is_active,
                    on_select_term,
                    on_buy_option,
                }
                p { class: "form-hint mt-sm", "data-testid": "payments-paid-plan-terms",
                    "By continuing, you agree to the "
                    a {
                        href: "{TERMS_URL}#paid-plans",
                        target: "_blank",
                        rel: "noopener noreferrer",
                        title: "Paid plan terms open in a new tab",
                        "paid plan terms"
                        ExternalLinkIcon {}
                    }
                    "."
                }
                div { class: "payments-commitments",
                    p {
                        em { "Your data is always yours." }
                        " If a paid plan expires, your app data stays with your app instance and remains exportable to hledger or ledger-cli. Only new transaction syncs pause."
                    }
                    p {
                        em { "Pay in your own coin." }
                        " Bitcoin, Ethereum, stablecoins (USDC, USDT), or Monero. Monero is the strongest payment privacy option among supported assets."
                    }
                    p {
                        em { "Clear limits." }
                        " BitGarth helps organize records. It is not tax, legal, financial, accounting, or investment advice."
                    }
                }
            } else if !upgrade_required {
                div { class: "card payments-catalog-error",
                    div { class: "card-body",
                        p { class: "alert alert-error",
                            "data-testid": "payments-catalog-error",
                            {upgrade_detail.clone().unwrap_or_else(|| "Could not reach BitGarth payment service.".to_string())}
                        }
                        button {
                            class: "btn btn-secondary",
                            "data-testid": "payments-catalog-retry",
                            r#type: "button",
                            onclick: move |_| on_retry_catalog.call(()),
                            "Retry"
                        }
                    }
                }
            }

            if let Some(message) = action_error {
                p { class: "alert alert-error payments-error", "{message}" }
            }

            CurrentPlanLine {
                view: view.clone(),
                acting,
                on_refresh,
            }

            if let Some(reference) = shell_support_reference {
                PaymentSupportReference { reference }
            }

            PaymentOrderHistory { orders: order_history }
        }
    }
}

// ─────────────────────────── CurrentPlanLine ───────────────────────────

/// Always-visible record of the user's plan. BGC controls the sellable
/// catalog and may unlist the current tier at any time, which removes the
/// tier card that carries the current-plan stamp — this line depends only
/// on `PaymentStateView`, so plan identity, the paid-through date, and the
/// refresh control keep a home regardless of catalog contents.
#[component]
fn CurrentPlanLine(view: PaymentStateView, acting: bool, on_refresh: EventHandler<()>) -> Element {
    let is_active = matches!(
        view.status,
        PaymentStateStatus::Active | PaymentStateStatus::ActiveWithSyncWarning
    );
    let paid_through = view
        .paid_through
        .as_deref()
        .and_then(format_rfc3339_as_display_date);

    rsx! {
        section {
            class: "payments-plan-line",
            "data-testid": "payments-current-plan",
            div { class: "payments-plan-line-main",
                span { class: "payments-plan-line-eyebrow", "Current plan" }
                span { class: "payments-plan-line-name",
                    "{view.tier_display_name}"
                    if is_active {
                        span { class: "payments-plan-line-status", " · Active" }
                    }
                }
            }
            div { class: "payments-plan-line-meta",
                if let Some(date) = paid_through.as_deref() {
                    span { class: "payments-plan-line-paid-through",
                        "Paid through "
                        strong { "{date}" }
                    }
                }
                if is_active {
                    button {
                        class: "payments-plan-refresh",
                        r#type: "button",
                        disabled: acting,
                        title: "Refresh plan status",
                        "aria-label": "Refresh plan status",
                        "data-testid": "payments-refresh-btn",
                        onclick: move |_| on_refresh.call(()),
                        RefreshIcon {}
                    }
                }
            }
        }
    }
}

// ─────────────────────────── CatalogSkeleton ───────────────────────────

/// Placeholder shown in the tier-grid region while the Central catalog loads.
/// Two cards to match the shipping catalog, so the grid doesn't jump width
/// when the real tiers arrive.
#[component]
fn CatalogSkeleton() -> Element {
    rsx! {
        div { class: "payments-catalog-skeleton", "data-testid": "payments-catalog-skeleton",
            for _ in 0..2 {
                div { class: "card skeleton-card",
                    div { class: "card-body",
                        div { class: "skeleton-line skeleton-line-title" }
                        div { class: "skeleton-line skeleton-line-full" }
                        div { class: "skeleton-line skeleton-line-full" }
                        div { class: "skeleton-line skeleton-line-medium" }
                    }
                }
            }
        }
    }
}

// ─────────────────────────── StatusPanel ───────────────────────────

#[component]
fn StatusPanel(
    view: PaymentStateView,
    widget_phase: WidgetPhase,
    acting: bool,
    upgrade_required: bool,
    upgrade_detail: Option<String>,
    on_check_now: EventHandler<()>,
    on_top_up: EventHandler<()>,
    on_refresh: EventHandler<()>,
    on_reconcile_history: EventHandler<()>,
    sync_warning_dismissed: bool,
    on_dismiss_sync_warning: EventHandler<()>,
) -> Element {
    let _ = upgrade_required;
    let _ = upgrade_detail;
    let status_class = match view.status {
        PaymentStateStatus::Active | PaymentStateStatus::ActiveWithSyncWarning => {
            "payments-status-dot payments-status-active"
        }
        PaymentStateStatus::RecoveryFailed => "payments-status-dot payments-status-unavailable",
        PaymentStateStatus::Pending | PaymentStateStatus::Verifying => {
            "payments-status-dot payments-status-pending"
        }
        PaymentStateStatus::AdditionalPaymentRequired => {
            "payments-status-dot payments-status-pending"
        }
        PaymentStateStatus::ManualReview => "payments-status-dot payments-status-failed",
        PaymentStateStatus::Expired | PaymentStateStatus::Failed => {
            "payments-status-dot payments-status-failed"
        }
        PaymentStateStatus::Canceled => "payments-status-dot payments-status-canceled",
        PaymentStateStatus::Unavailable => "payments-status-dot payments-status-unavailable",
        PaymentStateStatus::UpgradeRequired => "payments-status-dot payments-status-blocked",
        PaymentStateStatus::NotActive => "payments-status-dot payments-status-inactive",
    };

    rsx! {
        section { class: "payments-status-panel",
            "data-testid": "payments-status-panel",

            match view.status {
                PaymentStateStatus::ActiveWithSyncWarning => rsx! {
                    if !sync_warning_dismissed {
                        div {
                            class: "payments-sync-warning",
                            "data-testid": "payments-sync-warning",
                            div { class: "payments-status-row",
                                span { class: "{status_class}", "aria-hidden": "true" }
                                span { class: "payments-status-label",
                                    em { "Central sync issue" }
                                }
                            }
                            p { class: "payments-status-desc",
                                "Your local subscription is valid, but BitGarth could not verify the latest token state with Central."
                            }
                            div { class: "payments-actions",
                                button {
                                    class: "btn ghost",
                                    r#type: "button",
                                    disabled: acting,
                                    "data-testid": "payments-sync-warning-retry-btn",
                                    onclick: move |_| on_refresh.call(()),
                                    RefreshIcon {}
                                    span { "Retry sync" }
                                }
                                button {
                                    class: "btn ghost",
                                    r#type: "button",
                                    disabled: acting,
                                    "data-testid": "payments-sync-warning-dismiss-btn",
                                    onclick: move |_| on_dismiss_sync_warning.call(()),
                                    span { "Dismiss" }
                                }
                            }
                        }
                    }
                    if let Some(reference) = view.support_reference.as_ref() {
                        PaymentSupportReference { reference: reference.clone() }
                    }
                },
                PaymentStateStatus::RecoveryFailed => rsx! {
                    div { class: "payments-sync-warning",
                        "data-testid": "payments-recovery-failed",
                        div { class: "payments-status-row",
                            span { class: "{status_class}", "aria-hidden": "true" }
                            span { class: "payments-status-label",
                                em { "Subscription restore needed" }
                            }
                        }
                        p { class: "payments-status-desc",
                            "BitGarth found a previous paid subscription on this device, but could not restore it from Central right now."
                        }
                        div { class: "payments-actions",
                            button {
                                class: "btn ghost",
                                r#type: "button",
                                disabled: acting,
                                "data-testid": "payments-recovery-retry-btn",
                                onclick: move |_| on_reconcile_history.call(()),
                                "Retry restore"
                            }
                        }
                    }
                    if let Some(reference) = view.support_reference.as_ref() {
                        PaymentSupportReference { reference: reference.clone() }
                    }
                },
                PaymentStateStatus::Pending => rsx! {
                    div { class: "payments-status-row",
                        span { class: "{status_class}", "aria-hidden": "true" }
                        span { class: "payments-status-label",
                            em { "Payment pending" }
                        }
                    }
                    p { class: "payments-status-desc",
                        "Complete payment in the ATLOS window. BitGarth will unlock the selected plan after bitgarth.com confirms the payment."
                    }
                    p { class: "payments-status-subline",
                        strong { "Status: " }
                        match widget_phase {
                            WidgetPhase::Checking => "Checking confirmation\u{2026}",
                            _ => "Waiting for confirmation",
                        }
                    }
                    div { class: "payments-actions",
                        button {
                            class: "btn",
                            r#type: "button",
                            disabled: acting,
                            "data-testid": "payments-check-now-btn",
                            onclick: move |_| on_check_now.call(()),
                            CheckIcon {}
                            span { "Check now" }
                        }
                    }
                },
                PaymentStateStatus::Verifying => rsx! {
                    div { class: "payments-status-row",
                        span { class: "{status_class}", "aria-hidden": "true" }
                        span { class: "payments-status-label",
                            em { "Payment received" }
                        }
                    }
                    p { class: "payments-status-desc",
                        "BitGarth has seen a provider-confirmed payment and is still verifying it."
                    }
                    p { class: "payments-status-subline",
                        strong { "Status: " }
                        match widget_phase {
                            WidgetPhase::Checking => "Verifying payment\u{2026}",
                            _ => "Verifying payment",
                        }
                    }
                    if let Some(summary) = view.payment_summary.as_ref() {
                        PaymentSummary {
                            summary: summary.clone(),
                            order_id: view.order_id.clone(),
                        }
                    }
                    div { class: "payments-actions",
                        button {
                            class: "btn",
                            r#type: "button",
                            disabled: acting,
                            "data-testid": "payments-check-now-btn",
                            onclick: move |_| on_check_now.call(()),
                            CheckIcon {}
                            span { "Check now" }
                        }
                    }
                },
                PaymentStateStatus::AdditionalPaymentRequired => rsx! {
                    div { class: "payments-status-row",
                        span { class: "{status_class}", "aria-hidden": "true" }
                        span { class: "payments-status-label",
                            em { "Additional payment required" }
                        }
                    }
                    p { class: "payments-status-desc",
                        "The received payment was short. Pay the remaining amount to unlock the selected plan."
                    }
                    if let Some(additional_payment) = view.additional_payment.as_ref() {
                        dl { class: "payments-facts", "data-testid": "payments-top-up-summary",
                            div { class: "payments-fact",
                                dt { "Paid" }
                                dd { "{additional_payment.paid_amount} {additional_payment.paid_currency}" }
                            }
                            div { class: "payments-fact",
                                dt { "Remaining" }
                                dd { "{additional_payment.remaining_amount} {additional_payment.remaining_currency}" }
                            }
                        }
                    }
                    if let Some(summary) = view.payment_summary.as_ref() {
                        PaymentSummary {
                            summary: summary.clone(),
                            order_id: view.order_id.clone(),
                        }
                    } else if let Some(order_id) = view.order_id.as_ref() {
                        PaymentReference { order_id: order_id.clone() }
                    }
                    div { class: "payments-actions",
                        button {
                            class: "btn",
                            r#type: "button",
                            disabled: acting,
                            "data-testid": "payments-top-up-btn",
                            onclick: move |_| on_top_up.call(()),
                            if widget_phase == WidgetPhase::Launching { "Opening payment\u{2026}" } else { "Pay remaining amount" }
                        }
                        button {
                            class: "btn ghost",
                            r#type: "button",
                            disabled: acting,
                            "data-testid": "payments-check-now-btn",
                            onclick: move |_| on_check_now.call(()),
                            CheckIcon {}
                            span { "Check now" }
                        }
                    }
                },
                PaymentStateStatus::ManualReview => rsx! {
                    div { class: "payments-status-row",
                        span { class: "{status_class}", "aria-hidden": "true" }
                        span { class: "payments-status-label",
                            em { "Payment needs review" }
                        }
                    }
                    p { class: "payments-status-desc",
                        "BitGarth has seen your payment, but this order needs manual review before the selected plan can be granted."
                    }
                    if let Some(message) = view.message.as_deref() {
                        p { class: "payments-status-subline", "{message}" }
                    }
                    if let Some(summary) = view.payment_summary.as_ref() {
                        PaymentSummary {
                            summary: summary.clone(),
                            order_id: view.order_id.clone(),
                        }
                    } else if let Some(order_id) = view.order_id.as_ref() {
                        PaymentReference { order_id: order_id.clone() }
                    }
                    div { class: "payments-actions",
                        button {
                            class: "btn ghost",
                            r#type: "button",
                            disabled: acting,
                            "data-testid": "payments-check-later-btn",
                            onclick: move |_| on_check_now.call(()),
                            CheckIcon {}
                            span { "Check again later" }
                        }
                    }
                },
                PaymentStateStatus::Expired
                | PaymentStateStatus::Failed
                | PaymentStateStatus::Canceled => rsx! {
                    div { class: "payments-status-row",
                        span { class: "{status_class}", "aria-hidden": "true" }
                        span { class: "payments-status-label",
                            em { "Payment not completed" }
                        }
                    }
                    p { class: "payments-status-desc",
                        "No paid plan was granted for this payment attempt."
                    }
                    div { class: "payments-actions",
                        button {
                            class: "btn ghost",
                            r#type: "button",
                            disabled: acting,
                            "data-testid": "payments-check-history-btn",
                            onclick: move |_| on_reconcile_history.call(()),
                            CheckIcon {}
                            span { "Check payment" }
                        }
                    }
                },
                PaymentStateStatus::Unavailable => rsx! {
                    div { class: "payments-status-row",
                        span { class: "{status_class}", "aria-hidden": "true" }
                        span { class: "payments-status-label",
                            em { "Plan status unavailable" }
                        }
                    }
                    p { class: "payments-status-desc",
                        {view.message.clone().unwrap_or_else(|| "Could not refresh plan status.".to_string())}
                    }
                    div { class: "payments-actions",
                        button {
                            class: "btn ghost",
                            r#type: "button",
                            disabled: acting,
                            "data-testid": "payments-refresh-btn",
                            onclick: move |_| on_refresh.call(()),
                            RefreshIcon {}
                            span { "Try again" }
                        }
                    }
                },
                PaymentStateStatus::UpgradeRequired
                | PaymentStateStatus::NotActive
                | PaymentStateStatus::Active => rsx! {},
            }
        }
    }
}

// ─────────────────────────── UpgradeRequiredNotice ───────────────────────────

#[component]
fn UpgradeRequiredNotice(detail: Option<String>) -> Element {
    let copy = detail.unwrap_or_else(|| {
        "BitGarth needs an update before paid plans can be purchased.".to_string()
    });
    rsx! {
        div { class: "payments-update-notice",
            "data-testid": "payments-update-required-notice",
            span { class: "payments-update-notice-label", "Update required" }
            span { class: "payments-update-notice-copy", "{copy}" }
        }
    }
}

// ─────────────────────────── TierGrid ───────────────────────────

#[component]
fn TierGrid(
    tiers: Vec<PaymentTierView>,
    options: Vec<PaymentOptionView>,
    current_tier: String,
    paid_through: Option<String>,
    selected_term: Option<String>,
    is_dimmed: bool,
    disable_buys: bool,
    is_active: bool,
    on_select_term: EventHandler<String>,
    on_buy_option: EventHandler<String>,
) -> Element {
    let settings_state = use_context::<SettingsState>();
    let number_format = (settings_state.number_format)();
    let dim_class = if is_dimmed {
        " payments-tier-grid--dimmed"
    } else {
        ""
    };

    // CSS subgrid lets each tier card share row tracks (price, summary,
    // bullet 1, bullet 2, …) so equivalent rows line up across cards even
    // when content height varies. The parent grid declares the row
    // template; each card opts into it via `display: grid; grid-template-rows: subgrid`.
    let max_bullets = tiers.iter().map(|t| t.bullets.len()).max().unwrap_or(0);
    // One column per tier: the catalog currently ships two tiers, and
    // reserving empty tracks squeezes the cards (900px container ÷ 3).
    let column_count = tiers.len().max(1);

    rsx! {
        div { class: "payments-tier-grid{dim_class}",
            "data-testid": "payments-tier-grid",
            style: "--payments-tier-bullets: {max_bullets}; --payments-tier-columns: {column_count};",
            for tier in tiers {
                {
                    let selected_option = resolve_tier_option(
                        &tier.tier,
                        &options,
                        selected_term.as_deref(),
                    )
                    .cloned();
                    let tier_alt_options: Vec<PaymentOptionView> = options
                        .iter()
                        .filter(|o| {
                            o.tier == tier.tier
                                && selected_option
                                    .as_ref()
                                    .map(|s| s.id != o.id)
                                    .unwrap_or(true)
                        })
                        .cloned()
                        .collect();
                    let is_current = tier.tier == current_tier;
                    rsx! {
                        TierCard {
                            key: "{tier.tier}",
                            tier: tier.clone(),
                            selected_option,
                            alternates: tier_alt_options,
                            max_bullets,
                            is_current,
                            is_active_subscription: is_current && is_active,
                            paid_through: if is_current { paid_through.clone() } else { None },
                            disable_buys,
                            number_format,
                            on_select_term,
                            on_buy: on_buy_option,
                        }
                    }
                }
            }
        }
    }
}

// ─────────────────────────── TierCard ───────────────────────────

#[component]
fn TierCard(
    tier: PaymentTierView,
    selected_option: Option<PaymentOptionView>,
    alternates: Vec<PaymentOptionView>,
    max_bullets: usize,
    is_current: bool,
    is_active_subscription: bool,
    paid_through: Option<String>,
    disable_buys: bool,
    number_format: NumberFormat,
    on_select_term: EventHandler<String>,
    on_buy: EventHandler<String>,
) -> Element {
    let card_class = if tier.is_featured {
        "payments-tier-card payments-tier-card--featured"
    } else {
        "payments-tier-card"
    };
    let buy_label = match (&selected_option, is_active_subscription) {
        (Some(option), true) => format!(
            "Renew {} — {} / {}",
            tier.display_name,
            format_payment_option_price(option, number_format),
            option.term_label
        ),
        (Some(option), false) => format!(
            "Buy {} — {} / {}",
            tier.display_name,
            format_payment_option_price(option, number_format),
            option.term_label
        ),
        (None, _) => String::new(),
    };
    let cta_test_id = if is_active_subscription {
        format!("payments-renew-btn-{}", tier.tier)
    } else {
        format!("payments-buy-btn-{}", tier.tier)
    };
    // The under-CTA link reflects the *first* alternate (typically the other
    // term in a two-option tier). Clicking it flips the global term, so all
    // cards switch together.
    let primary_alt = alternates.first().cloned();
    let switch_link_label = primary_alt.as_ref().map(|alt| {
        let unit = match alt.term_unit.as_deref() {
            Some("month") if alt.term_quantity == Some(1) => "monthly".to_string(),
            Some("month") if alt.term_quantity == Some(12) => "yearly".to_string(),
            _ => alt.term_label.clone(),
        };
        format!("Switch to {unit} billing")
    });

    rsx! {
        article {
            class: "{card_class}",
            "data-testid": format!("payments-tier-card-{}", tier.tier),
            "data-tier": "{tier.tier}",
            style: "--payments-tier-bullets: {max_bullets};",
            // four corner marks — vintage botanical-print frame
            // The TL mark is suppressed when the current-plan stamp is
            // shown, so the stamp can own that corner without fighting
            // the corner ornament.
            if !is_current {
                CornerMark { position: "tl" }
            }
            CornerMark { position: "tr" }
            CornerMark { position: "bl" }
            CornerMark { position: "br" }

            if let Some(label) = tier.ribbon_label.as_deref() {
                div { class: "payments-tier-ribbon",
                    "data-testid": format!("payments-tier-ribbon-{}", tier.tier),
                    "{label}"
                }
            }
            if is_current {
                div { class: "payments-tier-current-stamp",
                    "data-testid": format!("payments-tier-current-{}", tier.tier),
                    "aria-label": "Current plan",
                    span { class: "payments-tier-current-stamp-line payments-tier-current-stamp-line--top",
                        "aria-hidden": "true",
                        "Current"
                    }
                    span { class: "payments-tier-current-stamp-rule", "aria-hidden": "true" }
                    span { class: "payments-tier-current-stamp-line payments-tier-current-stamp-line--bottom",
                        "aria-hidden": "true",
                        "plan"
                    }
                }
            }

            div { class: "payments-tier-name", "{tier.display_name}" }

            if let Some(option) = selected_option.as_ref() {
                div { class: "payments-tier-price",
                    "data-testid": format!("payments-tier-price-{}", tier.tier),
                    {format_payment_option_price(option, number_format)}
                    small { " / {option.term_label}" }
                }
            } else if tier.tier == "free" {
                // The free tier never carries purchase options; price it
                // explicitly so cost stays legible even if Central renames
                // the tier's display_name.
                div { class: "payments-tier-price",
                    "data-testid": format!("payments-tier-price-{}", tier.tier),
                    "$0"
                    small { "free" }
                }
            } else {
                // A paid tier with no resolvable purchase option: the price
                // is unavailable, not zero.
                div { class: "payments-tier-price payments-tier-price--empty",
                    "data-testid": format!("payments-tier-price-{}", tier.tier),
                    "—"
                }
            }

            if let Some(alt) = primary_alt.as_ref() {
                div { class: "payments-term-alternates",
                    TermToggle {
                        option: alt.clone(),
                        disabled: disable_buys,
                        number_format,
                        on_select: on_select_term,
                    }
                }
            } else {
                div { class: "payments-term-alternates payments-term-alternates--empty" }
            }

            p { class: "payments-tier-summary", "{tier.summary}" }

            ul { class: "payments-tier-bullets",
                for index in 0..max_bullets {
                    li { key: "{index}", class: "payments-tier-bullet",
                        if let Some(bullet) = tier.bullets.get(index) {
                            BulletText { bullet: bullet.clone() }
                        }
                    }
                }
            }

            if is_current {
                if let Some(date) = paid_through.as_deref() {
                    p { class: "payments-tier-paid-through",
                        "Paid through "
                        strong { "{date}" }
                    }
                }
            }

            if let Some(option) = selected_option.as_ref() {
                {
                    let option_id = option.id.clone();
                    let alt_for_link = primary_alt.clone();
                    let link_label = switch_link_label.clone();
                    rsx! {
                        button {
                            class: if tier.is_featured { "btn payments-tier-cta" } else { "btn ghost payments-tier-cta" },
                            r#type: "button",
                            disabled: disable_buys,
                            "data-testid": "{cta_test_id}",
                            onclick: move |_| on_buy.call(option_id.clone()),
                            "{buy_label}"
                        }
                        // Below-CTA term-switch link (mirrors the "or $X"
                        // link above, but anchored to the action). Same
                        // global effect — flips every card.
                        if let (Some(alt), Some(label)) = (alt_for_link, link_label) {
                            button {
                                r#type: "button",
                                class: "payments-tier-switch-link",
                                disabled: disable_buys,
                                "data-testid": format!("payments-switch-link-{}", tier.tier),
                                onclick: move |_| on_select_term.call(alt.term_label.clone()),
                                "{label}"
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn CornerMark(position: &'static str) -> Element {
    rsx! {
        svg {
            class: "payments-tier-corner payments-tier-corner-{position}",
            "viewBox": "0 0 18 18",
            "fill": "none",
            "aria-hidden": "true",
            path { d: "M0 7V0h7", stroke: "currentColor", "stroke-width": "0.8" }
            path { d: "M1 1l4 4", stroke: "currentColor", "stroke-width": "0.8" }
        }
    }
}

#[component]
fn TermToggle(
    option: PaymentOptionView,
    disabled: bool,
    number_format: NumberFormat,
    on_select: EventHandler<String>,
) -> Element {
    let label = format!(
        "or {} / {}",
        format_payment_option_price(&option, number_format),
        option.term_label
    );
    let term_label = option.term_label.clone();
    rsx! {
        button {
            r#type: "button",
            class: "payments-term-toggle",
            "data-testid": format!("payments-term-toggle-{}", option.id),
            "data-option-id": "{option.id}",
            disabled,
            onclick: move |_| on_select.call(term_label.clone()),
            "{label}"
        }
    }
}

#[component]
fn BulletText(bullet: TierBulletView) -> Element {
    rsx! {
        for (idx, seg) in bullet.segments.iter().enumerate() {
            match seg {
                BulletSegmentView::Plain(text) => rsx! {
                    span { key: "{idx}", "{text}" }
                },
                BulletSegmentView::Bold(text) => rsx! {
                    strong { key: "{idx}", "{text}" }
                },
            }
        }
    }
}

// ─────────────────────────── PaymentOrderHistory ───────────────────────────

#[component]
fn PaymentOrderHistory(orders: Vec<PaymentOrderHistoryView>) -> Element {
    if orders.is_empty() {
        return rsx! {};
    }

    let mut ordered_orders = orders;
    ordered_orders.reverse();

    rsx! {
        section { class: "payments-history", "data-testid": "payments-order-history",
            div { class: "payments-history-header",
                h3 { "Order history" }
            }
            div { class: "payments-history-list",
                for order in ordered_orders {
                    PaymentOrderHistoryRow { order }
                }
            }
        }
    }
}

#[component]
fn PaymentOrderHistoryRow(order: PaymentOrderHistoryView) -> Element {
    let settings_state = use_context::<SettingsState>();
    let date_time_format = (settings_state.date_time_format)();
    let timezone = (settings_state.timezone)();
    let paid_at = order.paid_at.as_deref().and_then(|raw| {
        DateTime::parse_from_rfc3339(raw)
            .ok()
            .map(|dt| format_timestamp(&dt.with_timezone(&Utc), timezone.into(), date_time_format))
    });
    let amount = format!("{} {}", order.display_amount, order.currency);
    let status_tag = payment_order_status_tag(order.status);
    let status_label = payment_order_status_label(order.status);
    let plan_label = tier_display_name(&order.product_tier);

    rsx! {
        div {
            class: "payments-history-row",
            "data-testid": "payments-order-history-row",
            "data-status": status_tag,
            div { class: "payments-history-main",
                div { class: "payments-history-title-row",
                    span { class: format!("payments-history-status payments-history-status-{status_tag}"),
                        "{status_label}"
                    }
                    span { class: "payments-history-amount", "{amount}" }
                }
                div { class: "payments-history-order-id",
                    code { "{order.order_id}" }
                    PaymentsCopyButton {
                        value: order.order_id.clone(),
                        aria_label: "Copy order ID".to_string(),
                    }
                }
            }
            div { class: "payments-history-meta",
                span { "{plan_label}" }
                if let Some(paid_at) = paid_at.as_deref() {
                    span { "{paid_at}" }
                }
            }
        }
    }
}

fn payment_order_status_tag(status: PaymentOrderStatusView) -> &'static str {
    match status {
        PaymentOrderStatusView::Pending => "pending",
        PaymentOrderStatusView::Paid => "paid",
        PaymentOrderStatusView::Expired => "expired",
        PaymentOrderStatusView::Failed => "failed",
        PaymentOrderStatusView::Canceled => "canceled",
    }
}

fn payment_order_status_label(status: PaymentOrderStatusView) -> &'static str {
    match status {
        PaymentOrderStatusView::Pending => "Pending",
        PaymentOrderStatusView::Paid => "Paid",
        PaymentOrderStatusView::Expired => "Expired",
        PaymentOrderStatusView::Failed => "Failed",
        PaymentOrderStatusView::Canceled => "Canceled",
    }
}

impl PaymentStateView {
    fn status_tag(&self) -> &'static str {
        match self.status {
            PaymentStateStatus::Active => "active",
            PaymentStateStatus::ActiveWithSyncWarning => "active_with_sync_warning",
            PaymentStateStatus::RecoveryFailed => "recovery_failed",
            PaymentStateStatus::NotActive => "not_active",
            PaymentStateStatus::Pending => "pending",
            PaymentStateStatus::Verifying => "verifying",
            PaymentStateStatus::AdditionalPaymentRequired => "additional_payment_required",
            PaymentStateStatus::ManualReview => "manual_review",
            PaymentStateStatus::Expired => "expired",
            PaymentStateStatus::Failed => "failed",
            PaymentStateStatus::Canceled => "canceled",
            PaymentStateStatus::Unavailable => "unavailable",
            PaymentStateStatus::UpgradeRequired => "upgrade_required",
        }
    }
}

// ─────────────────────────── PaymentReference / PaymentSummary ───────────────────────────

fn support_reference_copy_value(reference: &PaymentSupportReferenceView) -> String {
    let mut lines = Vec::new();
    if let Some(token_id) = reference.token_id.as_deref() {
        lines.push(format!("Token ID: {token_id}"));
    }
    if let Some(order_id) = reference.order_id.as_deref() {
        lines.push(format!("Order ID: {order_id}"));
    }
    if let Some(subscription_subject_id) = reference.subscription_subject_id.as_deref() {
        lines.push(format!("Subscription ID: {subscription_subject_id}"));
    }
    lines.push(format!(
        "Entitlement holder ID: {}",
        reference.entitlement_holder_id
    ));
    lines.join("\n")
}

#[component]
fn SupportReferenceRow(label: &'static str, value: String) -> Element {
    let copy_value = value.clone();
    rsx! {
        div { class: "payments-fact",
            dt { "{label}" }
            dd {
                div { class: "payments-fact-copy",
                    code { "{value}" }
                    PaymentsCopyButton {
                        value: copy_value,
                        aria_label: format!("Copy {label}"),
                    }
                }
            }
        }
    }
}

#[component]
fn PaymentSupportReference(reference: PaymentSupportReferenceView) -> Element {
    let copy_value = support_reference_copy_value(&reference);
    rsx! {
        section {
            class: "payments-support-reference",
            "data-testid": "payments-support-reference",
            div { class: "payments-support-reference-header",
                h3 { "Support reference" }
                PaymentsCopyButton {
                    value: copy_value,
                    aria_label: "Copy support reference".to_string(),
                }
            }
            dl { class: "payments-facts payments-support-reference-facts",
                if let Some(token_id) = reference.token_id.as_ref() {
                    SupportReferenceRow {
                        label: "Token ID",
                        value: token_id.clone(),
                    }
                }
                if let Some(order_id) = reference.order_id.as_ref() {
                    SupportReferenceRow {
                        label: "Order ID",
                        value: order_id.clone(),
                    }
                }
                if let Some(subscription_subject_id) = reference.subscription_subject_id.as_ref() {
                    SupportReferenceRow {
                        label: "Subscription ID",
                        value: subscription_subject_id.clone(),
                    }
                }
                SupportReferenceRow {
                    label: "Entitlement holder ID",
                    value: reference.entitlement_holder_id.clone(),
                }
            }
        }
    }
}

#[component]
fn PaymentReference(order_id: String) -> Element {
    let copy_value = order_id.clone();
    rsx! {
        dl { class: "payments-facts", "data-testid": "payments-order-reference",
            div { class: "payments-fact",
                dt { "Order ID" }
                dd {
                    div { class: "payments-fact-copy",
                        code { "{order_id}" }
                        PaymentsCopyButton {
                            value: copy_value,
                            aria_label: "Copy order ID".to_string(),
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn PaymentSummary(
    summary: crate::payments::views::PaymentSummaryView,
    order_id: Option<String>,
) -> Element {
    let settings_state = use_context::<SettingsState>();
    let date_time_format = (settings_state.date_time_format)();
    let timezone = (settings_state.timezone)();

    let confirmed_at = summary.confirmed_at.as_deref().and_then(|raw| {
        DateTime::parse_from_rfc3339(raw)
            .ok()
            .map(|dt| format_timestamp(&dt.with_timezone(&Utc), timezone.into(), date_time_format))
    });

    let amount_received = match (
        summary.paid_asset_amount.as_deref(),
        summary.paid_asset_code.as_deref(),
    ) {
        (Some(amount), Some(code)) => Some(format!("{amount} {code}")),
        (Some(amount), None) => Some(amount.to_string()),
        _ => Some(format!(
            "{} {}",
            summary.paid_order_amount, summary.paid_order_currency
        )),
    };

    rsx! {
        dl { class: "payments-facts", "data-testid": "payments-summary",
            if let Some(order_id) = order_id.as_deref() {
                div { class: "payments-fact",
                    dt { "Order ID" }
                    dd {
                        div { class: "payments-fact-copy",
                            code { "{order_id}" }
                            PaymentsCopyButton {
                                value: order_id.to_string(),
                                aria_label: "Copy order ID".to_string(),
                            }
                        }
                    }
                }
            }
            if let Some(amount_received) = amount_received.as_deref() {
                div { class: "payments-fact",
                    dt { "Amount received" }
                    dd { "{amount_received}" }
                }
            }
            if let Some(blockchain_hash) = summary.blockchain_hash.as_deref() {
                div { class: "payments-fact",
                    dt { "Blockchain hash" }
                    dd {
                        div { class: "payments-fact-copy",
                            code { title: "{blockchain_hash}", "{truncate_reference_with_lengths(blockchain_hash, 8, 8)}" }
                            PaymentsCopyButton {
                                value: blockchain_hash.to_string(),
                                aria_label: "Copy blockchain hash".to_string(),
                            }
                        }
                    }
                }
            }
            if let Some(confirmed_at) = confirmed_at.as_deref() {
                div { class: "payments-fact",
                    dt { "Confirmed" }
                    dd { "{confirmed_at}" }
                }
            }
        }
    }
}

#[component]
fn PaymentsCopyButton(value: String, aria_label: String) -> Element {
    let mut copied = use_signal(|| false);

    rsx! {
        button {
            class: if copied() { "inline-copy-btn copied" } else { "inline-copy-btn" },
            r#type: "button",
            "aria-label": if copied() { "Copied!" } else { aria_label.as_str() },
            title: if copied() { "Copied!" } else { aria_label.as_str() },
            onclick: move |_| {
                copy_to_clipboard(&value);
                copied.set(true);
                spawn(async move {
                    let mut timer = eval(r#"setTimeout(() => { dioxus.send(null); }, 1500);"#);
                    let _ = timer.recv::<serde_json::Value>().await;
                    copied.set(false);
                });
            },
            if copied() {
                CheckIcon {}
            } else {
                CopyIcon {}
            }
        }
    }
}

// ─────────────────────────── tests ───────────────────────────

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use crate::payments::views::parse_bullet;

    #[cfg(feature = "server")]
    use crate::models::{DateTimeFormat, NumberFormat, SessionDuration, UserTimezone};
    #[cfg(feature = "server")]
    use crate::payments::views::{
        PaymentOrderHistoryView, PaymentOrderStatusView, PaymentSupportReferenceView,
    };
    #[cfg(feature = "server")]
    use crate::settings::{SettingsState, default_currency};
    #[cfg(feature = "server")]
    use chrono_tz::Tz;
    #[cfg(feature = "server")]
    use dioxus::prelude::{
        Element, EventHandler, VirtualDom, rsx, use_context_provider, use_signal,
    };

    fn sample_option(
        id: &str,
        tier: &str,
        label: &str,
        amount: &str,
        is_default: bool,
    ) -> PaymentOptionView {
        PaymentOptionView {
            id: id.to_string(),
            tier: tier.to_string(),
            tier_display_name: "Premium".to_string(),
            term_quantity: Some(12),
            term_unit: Some("month".to_string()),
            term_label: label.to_string(),
            minor_units: 0,
            decimal_precision: 2,
            display_amount: amount.to_string(),
            currency: "USD".to_string(),
            currency_symbol: "$".to_string(),
            is_default,
        }
    }

    #[cfg(feature = "server")]
    fn payment_state_for_render() -> PaymentStateView {
        PaymentStateView {
            status: PaymentStateStatus::Pending,
            tier: "free".to_string(),
            tier_display_name: "Free".to_string(),
            sync_account_slots_limit: 5,
            historical_backfill_enabled: false,
            historical_backfill_transactions_per_account: 0,
            order_id: Some("01JQABCDEF000000000000000E".to_string()),
            paid_through: None,
            display_amount: Some("9.99".to_string()),
            currency: Some("USD".to_string()),
            message: None,
            payment_summary: None,
            additional_payment: None,
            support_reference: Some(PaymentSupportReferenceView {
                token_id: None,
                subscription_subject_id: None,
                entitlement_holder_id: "01JQABCDEF000000000000000D".to_string(),
                order_id: Some("01JQABCDEF000000000000000E".to_string()),
            }),
        }
    }

    #[cfg(feature = "server")]
    fn active_state_with_tier(tier: &str, display_name: &str) -> PaymentStateView {
        PaymentStateView {
            status: PaymentStateStatus::Active,
            tier: tier.to_string(),
            tier_display_name: display_name.to_string(),
            sync_account_slots_limit: 25,
            historical_backfill_enabled: true,
            historical_backfill_transactions_per_account: 500,
            order_id: None,
            paid_through: Some("2026-08-03T00:00:00Z".to_string()),
            display_amount: None,
            currency: None,
            message: None,
            payment_summary: None,
            additional_payment: None,
            support_reference: None,
        }
    }

    #[cfg(feature = "server")]
    fn payment_tiers_for_render() -> Vec<PaymentTierView> {
        vec![
            PaymentTierView {
                tier: "free".to_string(),
                display_name: "Free".to_string(),
                summary: "Free balance sync.".to_string(),
                bullets: Vec::new(),
                is_featured: false,
                ribbon_label: None,
            },
            PaymentTierView {
                tier: "premium".to_string(),
                display_name: "Premium".to_string(),
                summary: "Premium transaction sync.".to_string(),
                bullets: Vec::new(),
                is_featured: true,
                ribbon_label: Some("Best value".to_string()),
            },
        ]
    }

    #[cfg(feature = "server")]
    fn payment_options_for_render() -> Vec<PaymentOptionView> {
        vec![PaymentOptionView {
            id: "premium_12_months_usd".to_string(),
            tier: "premium".to_string(),
            tier_display_name: "Premium".to_string(),
            term_quantity: Some(12),
            term_unit: Some("month".to_string()),
            term_label: "1 year".to_string(),
            minor_units: 999,
            decimal_precision: 2,
            display_amount: "9.99".to_string(),
            currency: "USD".to_string(),
            currency_symbol: "$".to_string(),
            is_default: true,
        }]
    }

    #[cfg(feature = "server")]
    fn payment_history_for_render() -> Vec<PaymentOrderHistoryView> {
        vec![PaymentOrderHistoryView {
            order_id: "01JQABCDEF000000000000000E".to_string(),
            product_tier: "premium".to_string(),
            display_amount: "9.99".to_string(),
            currency: "USD".to_string(),
            status: PaymentOrderStatusView::Pending,
            paid_at: None,
        }]
    }

    #[cfg(feature = "server")]
    fn noop<T: 'static>() -> EventHandler<T> {
        EventHandler::new(|_| {})
    }

    #[cfg(feature = "server")]
    fn render_payments_body_with_state(view: PaymentStateView) -> String {
        #[component]
        fn Harness(view: PaymentStateView) -> Element {
            let language = use_signal(crate::i18n::Locale::default);
            let date_time_format = use_signal(|| DateTimeFormat::MonthDayYear12);
            let number_format = use_signal(|| NumberFormat::DotComma);
            let currency = use_signal(|| default_currency(crate::i18n::Locale::English));
            let timezone = use_signal(|| UserTimezone::from(Tz::UTC));
            let session_duration = use_signal(SessionDuration::default);
            let mempool_base_url = use_signal(|| None);
            let etherscan_base_url = use_signal(|| None);
            let price_fetching_enabled = use_signal(|| false);
            let has_coingecko_api_key = use_signal(|| false);
            use_context_provider(|| SettingsState {
                language,
                date_time_format,
                number_format,
                currency,
                timezone,
                session_duration,
                mempool_base_url,
                etherscan_base_url,
                price_fetching_enabled,
                has_coingecko_api_key,
            });

            rsx! {
                PaymentsBody {
                    view,
                    payment_tiers: payment_tiers_for_render(),
                    payment_options: payment_options_for_render(),
                    order_history: payment_history_for_render(),
                    selected_term: Some("1 year".to_string()),
                    app_compatibility: None,
                    pricing_summary: Some(crate::payments::views::parse_bullet(
                        "**Free** tracks holdings. **Paid** does the accounting.",
                    )),
                    upgrade_detail: None,
                    upgrade_required: false,
                    catalog_loading: false,
                    poll_refresh_failed: false,
                    acting: false,
                    widget_phase: WidgetPhase::Idle,
                    action_error: Some("Temporary action error.".to_string()),
                    sync_warning_dismissed: false,
                    on_select_term: noop::<String>(),
                    on_buy_option: noop::<String>(),
                    on_check_now: noop::<()>(),
                    on_top_up: noop::<()>(),
                    on_refresh: noop::<()>(),
                    on_reconcile_history: noop::<()>(),
                    on_dismiss_sync_warning: noop::<()>(),
                    on_retry_catalog: noop::<()>(),
                }
            }
        }

        let mut dom = VirtualDom::new_with_props(Harness, HarnessProps { view });
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    #[cfg(feature = "server")]
    fn render_payments_body_for_ordering() -> String {
        render_payments_body_with_state(payment_state_for_render())
    }

    #[test]
    fn rfc3339_formats_as_month_day_year() {
        let formatted = format_rfc3339_as_display_date("2027-04-16T12:00:00Z").unwrap();
        assert_eq!(formatted, "Apr 16, 2027");
    }

    #[test]
    fn build_widget_script_embeds_server_issued_values() {
        let start = PremiumOrderLaunchView {
            state: PaymentStateView {
                status: PaymentStateStatus::Pending,
                tier: "free".to_string(),
                tier_display_name: "Free".to_string(),
                sync_account_slots_limit: 5,
                historical_backfill_enabled: false,
                historical_backfill_transactions_per_account: 0,
                order_id: Some("01JQABCDEF000000000000000E".to_string()),
                paid_through: None,
                display_amount: Some("9.99".to_string()),
                currency: Some("USD".to_string()),
                message: None,
                payment_summary: None,
                additional_payment: None,
                support_reference: None,
            },
            merchant_id: "8MY8BXTU15".to_string(),
            central_order_id: "01JQABCDEF000000000000000E".to_string(),
            atlos_order_id: "01JQABCDEF000000000000000F".to_string(),
            order_amount: "9.99".to_string(),
            order_currency: "USD".to_string(),
        };
        let script = build_widget_script(&start).expect("widget script should build");
        assert!(script.contains("8MY8BXTU15"));
        assert!(!script.contains("01JQABCDEF000000000000000E"));
        assert!(script.contains("01JQABCDEF000000000000000F"));
        assert!(script.contains("9.99"));
        assert!(script.contains("USD"));
        assert!(script.contains("onCompleted"));
        assert!(script.contains("onCanceled"));
        assert!(!script.contains("management_secret"));
        assert!(!script.contains("order_secret"));
    }

    #[test]
    fn status_tag_covers_all_variants() {
        for status in [
            PaymentStateStatus::Active,
            PaymentStateStatus::ActiveWithSyncWarning,
            PaymentStateStatus::RecoveryFailed,
            PaymentStateStatus::NotActive,
            PaymentStateStatus::Pending,
            PaymentStateStatus::Verifying,
            PaymentStateStatus::AdditionalPaymentRequired,
            PaymentStateStatus::ManualReview,
            PaymentStateStatus::Expired,
            PaymentStateStatus::Failed,
            PaymentStateStatus::Canceled,
            PaymentStateStatus::Unavailable,
            PaymentStateStatus::UpgradeRequired,
        ] {
            let view = PaymentStateView {
                status,
                tier: "free".to_string(),
                tier_display_name: "Free".to_string(),
                sync_account_slots_limit: 5,
                historical_backfill_enabled: false,
                historical_backfill_transactions_per_account: 0,
                order_id: None,
                paid_through: None,
                display_amount: None,
                currency: None,
                message: None,
                payment_summary: None,
                additional_payment: None,
                support_reference: None,
            };
            assert!(!view.status_tag().is_empty());
        }
    }

    #[test]
    fn auto_poll_includes_pending_and_verifying_only() {
        assert!(status_uses_auto_poll(PaymentStateStatus::Pending));
        assert!(status_uses_auto_poll(PaymentStateStatus::Verifying));
        assert!(!status_uses_auto_poll(
            PaymentStateStatus::AdditionalPaymentRequired
        ));
        assert!(!status_uses_auto_poll(PaymentStateStatus::ManualReview));
        assert!(!status_uses_auto_poll(PaymentStateStatus::Expired));
    }

    #[test]
    fn default_global_term_picks_is_default_when_present() {
        let options = vec![
            sample_option(
                "premium_test_1_day_usd",
                "premium",
                "1 day (test)",
                "0.01",
                false,
            ),
            sample_option("premium_12_months_usd", "premium", "1 year", "9.99", true),
        ];

        let term = default_global_term(&options);
        assert_eq!(term.as_deref(), Some("1 year"));
    }

    #[test]
    fn default_global_term_falls_back_to_first_option_in_response_order() {
        let options = vec![
            sample_option("premium_12_months_usd", "premium", "1 year", "9.99", false),
            sample_option(
                "premium_test_1_day_usd",
                "premium",
                "1 day (test)",
                "0.01",
                false,
            ),
        ];

        let term = default_global_term(&options);
        assert_eq!(term.as_deref(), Some("1 year"));
    }

    #[test]
    fn resolve_tier_option_prefers_matching_term() {
        let options = vec![
            sample_option("basic_1_month_usd", "basic", "1 month", "5.00", false),
            sample_option("basic_12_months_usd", "basic", "1 year", "50.00", true),
            sample_option("premium_12_months_usd", "premium", "1 year", "500.00", true),
        ];

        let basic_at_monthly = resolve_tier_option("basic", &options, Some("1 month"));
        assert_eq!(
            basic_at_monthly.map(|o| o.id.as_str()),
            Some("basic_1_month_usd")
        );
        let basic_at_yearly = resolve_tier_option("basic", &options, Some("1 year"));
        assert_eq!(
            basic_at_yearly.map(|o| o.id.as_str()),
            Some("basic_12_months_usd")
        );
    }

    #[test]
    fn resolve_tier_option_falls_back_to_is_default_when_no_match() {
        // Premium doesn't have a "1 day (test)" option — should fall back to
        // its is_default option.
        let options = vec![
            sample_option(
                "premium_test_1_day_usd",
                "premium",
                "1 day (test)",
                "0.01",
                false,
            ),
            sample_option("premium_12_months_usd", "premium", "1 year", "500.00", true),
        ];

        let resolved = resolve_tier_option("premium", &options, Some("nonexistent term"));
        assert_eq!(
            resolved.map(|o| o.id.as_str()),
            Some("premium_12_months_usd")
        );
    }

    #[test]
    fn trim_round_amount_drops_zero_cents_but_keeps_real_decimals() {
        assert_eq!(trim_round_amount("5.00"), "5");
        assert_eq!(trim_round_amount("5"), "5");
        assert_eq!(trim_round_amount("5.50"), "5.5");
        assert_eq!(trim_round_amount("1.23"), "1.23");
        assert_eq!(trim_round_amount("0.01"), "0.01");
        assert_eq!(trim_round_amount("100.000"), "100");
    }

    #[test]
    fn format_payment_option_price_uses_number_format_and_trims_round_amounts() {
        let option = sample_option("basic_12_months_usd", "basic", "1 year", "1234.00", true);
        assert_eq!(
            format_payment_option_price(&option, NumberFormat::DotComma),
            "$1,234"
        );
        let option_with_cents =
            sample_option("basic_test_usd", "basic", "1 day (test)", "1.23", false);
        assert_eq!(
            format_payment_option_price(&option_with_cents, NumberFormat::DotComma),
            "$1.23"
        );
        let option_eu = sample_option("basic_year_eu", "basic", "1 year", "1234.50", true);
        assert_eq!(
            format_payment_option_price(&option_eu, NumberFormat::CommaDot),
            "$1.234,5"
        );
    }

    #[test]
    fn parse_bullet_re_export_works_for_inline_use() {
        // Light smoke-test confirming the component file can use parse_bullet
        // directly (the tier-card renderer relies on it).
        let bullet = parse_bullet("**5** synced accounts");
        assert_eq!(bullet.segments.len(), 2);
    }

    #[test]
    fn support_reference_shell_skips_status_panel_owned_references() {
        assert!(!support_reference_renders_in_shell(
            PaymentStateStatus::ActiveWithSyncWarning
        ));
        assert!(!support_reference_renders_in_shell(
            PaymentStateStatus::RecoveryFailed
        ));
        assert!(support_reference_renders_in_shell(
            PaymentStateStatus::Active
        ));
    }

    #[cfg(feature = "server")]
    #[test]
    fn payments_commitments_render_after_terms_link() {
        let rendered = render_payments_body_for_ordering();

        let tier_grid = rendered
            .find(r#"data-testid="payments-tier-grid""#)
            .expect("tier grid should render");
        let terms = rendered
            .find(r#"data-testid="payments-paid-plan-terms""#)
            .expect("terms link copy should render");
        let commitments = rendered
            .find("Your data is always yours.")
            .expect("commitments copy should render");

        assert!(
            tier_grid < terms && terms < commitments,
            "commitments should render after the terms link, below the tier grid: {rendered}"
        );
    }

    #[cfg(feature = "server")]
    #[test]
    fn current_plan_line_renders_when_tier_missing_from_catalog() {
        // "legacy_pro" is not in payment_tiers_for_render() (free + premium),
        // simulating BGC unlisting the user's tier from product-options.
        let rendered =
            render_payments_body_with_state(active_state_with_tier("legacy_pro", "Legacy Pro"));

        assert!(
            !rendered.contains("payments-tier-current-stamp"),
            "no tier card should carry the current-plan stamp for an unlisted tier: {rendered}"
        );
        let plan_line = rendered
            .find(r#"data-testid="payments-current-plan""#)
            .expect("current-plan line should render without a catalog match");
        let after_line = &rendered[plan_line..];
        assert!(
            after_line.contains("Legacy Pro"),
            "plan line should show the server-provided tier display name: {rendered}"
        );
        assert!(
            after_line.contains("Paid through") && after_line.contains("Aug 03, 2026"),
            "plan line should show the formatted paid-through date: {rendered}"
        );
        assert!(
            after_line.contains(r#"data-testid="payments-refresh-btn""#),
            "plan line should carry the refresh button for an active plan: {rendered}"
        );
    }

    #[cfg(feature = "server")]
    #[test]
    fn refresh_button_renders_once_on_plan_line_not_on_tier_card() {
        // "premium" IS in the catalog, so the current-tier card renders with
        // its stamp and paid-through — but the refresh button must only
        // appear once, on the plan line below the commitments block.
        let rendered =
            render_payments_body_with_state(active_state_with_tier("premium", "Premium"));

        assert_eq!(
            rendered.matches("payments-refresh-btn").count(),
            1,
            "exactly one refresh button should render: {rendered}"
        );
        let commitments = rendered
            .find("Your data is always yours.")
            .expect("commitments copy should render");
        let refresh = rendered
            .find(r#"data-testid="payments-refresh-btn""#)
            .expect("refresh button should render for an active plan");
        assert!(
            commitments < refresh,
            "refresh button should sit on the plan line below the tier grid, not on the card: {rendered}"
        );
    }

    #[cfg(feature = "server")]
    #[test]
    fn current_plan_line_renders_between_commitments_and_support_reference() {
        let rendered = render_payments_body_for_ordering();

        let commitments = rendered
            .find("Your data is always yours.")
            .expect("commitments copy should render");
        let plan_line = rendered
            .find(r#"data-testid="payments-current-plan""#)
            .expect("current-plan line should render");
        let support_reference = rendered
            .find(r#"data-testid="payments-support-reference""#)
            .expect("support reference should render");

        assert!(
            commitments < plan_line && plan_line < support_reference,
            "plan line should open the account region, after commitments and before the support reference: {rendered}"
        );
    }

    #[cfg(feature = "server")]
    #[test]
    fn current_plan_line_shows_free_plan_without_refresh_when_not_active() {
        // Default fixture: tier "free", status Pending, no paid_through.
        let rendered = render_payments_body_for_ordering();

        let plan_line = rendered
            .find(r#"data-testid="payments-current-plan""#)
            .expect("current-plan line should render for the free tier too");
        assert!(
            rendered[plan_line..].contains("Free"),
            "plan line should name the free plan: {rendered}"
        );
        assert!(
            !rendered.contains("payments-refresh-btn"),
            "refresh button should not render when the plan is not active: {rendered}"
        );
        assert!(
            !rendered.contains("· Active"),
            "status word should not render when the plan is not active: {rendered}"
        );
    }

    #[cfg(feature = "server")]
    #[test]
    fn payments_intro_renders_central_pricing_summary_bold_runs_as_chips() {
        let rendered = render_payments_body_for_ordering();

        assert!(
            rendered.contains("<em>Free</em>") && rendered.contains("<em>Paid</em>"),
            "bold runs should render as em chips: {rendered}"
        );
        assert!(
            rendered.contains("tracks holdings") && rendered.contains("does the accounting"),
            "plain runs should render as text: {rendered}"
        );
        assert!(
            !rendered.contains("syncs your balances"),
            "app fallback copy should not render when Central supplies a summary: {rendered}"
        );
    }

    #[cfg(feature = "server")]
    #[test]
    fn support_reference_renders_immediately_before_order_history_region() {
        let rendered = render_payments_body_for_ordering();

        let action_error = rendered
            .find("Temporary action error.")
            .expect("action error should render");
        let support_reference = rendered
            .find(r#"data-testid="payments-support-reference""#)
            .expect("support reference should render");
        let order_history = rendered
            .find(r#"data-testid="payments-order-history""#)
            .expect("order history should render");

        assert!(
            support_reference < order_history,
            "support reference should render before order history: {rendered}"
        );
        let between_support_reference_and_order_history =
            &rendered[support_reference..order_history];
        for unexpected in [
            "payments-commitments",
            "Your data is always yours.",
            r#"data-testid="payments-paid-plan-terms""#,
            "Temporary action error.",
        ] {
            assert!(
                !between_support_reference_and_order_history.contains(unexpected),
                "{unexpected} should not render between support reference and order history: {rendered}"
            );
        }

        assert!(
            action_error < support_reference && support_reference < order_history,
            "support reference should render after action errors and before order history: {rendered}"
        );
    }
}
