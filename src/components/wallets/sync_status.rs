use super::super::CheckIcon;
use super::helpers::{format_sync_absolute_time, format_sync_relative_time, sync_result_word};
use super::sync_state::{
    AccountSyncNowSignal, AccountSyncStateSignal, SyncDisplayStatus, derive_sync_display_status,
};
use crate::transactions::AccountSyncResult;
use dioxus::prelude::*;

/// Compact sync status mark for account rows. Click opens the sync record.
#[component]
pub(crate) fn AccountSyncStatusPill(account_id: crate::wallets::DigitalAssetAccountId) -> Element {
    let sync_state = use_context::<AccountSyncStateSignal>();
    let now_signal = use_context::<AccountSyncNowSignal>();
    let now = now_signal();
    let mut record_open = use_signal(|| false);

    let state_map = sync_state.read();
    let state = state_map.get(&account_id);
    let status = state
        .map(derive_sync_display_status)
        .unwrap_or(SyncDisplayStatus::NotSynced);

    let aria_label = match &status {
        SyncDisplayStatus::Syncing => "Syncing".to_string(),
        SyncDisplayStatus::Blocked => "Sync blocked: needs an Etherscan API key".to_string(),
        SyncDisplayStatus::Failing { streak } => format!("Sync failing for {streak} runs"),
        SyncDisplayStatus::Retrying => "Last sync did not finish; retrying".to_string(),
        SyncDisplayStatus::Synced { at } => match (at, now) {
            (Some(at), Some(now_utc)) => {
                format!("Synced {}", format_sync_relative_time(now_utc, *at))
            }
            _ => "Synced".to_string(),
        },
        SyncDisplayStatus::NotSynced => "Not synced yet".to_string(),
    };

    let mark = match &status {
        SyncDisplayStatus::Syncing => rsx! {
            span { class: "account-sync-spinner account-sync-spinner-small", "aria-hidden": "true" }
            span { class: "sync-mark-label is-moss", "Syncing" }
        },
        SyncDisplayStatus::Blocked => rsx! {
            span { class: "sync-mark-pill is-blocked",
                span { class: "sync-mark-pill-dot", "aria-hidden": "true" }
                "Needs API key"
            }
        },
        SyncDisplayStatus::Failing { .. } => rsx! {
            span { class: "sync-mark-pill is-failing",
                span { class: "sync-mark-pill-dot", "aria-hidden": "true" }
                "Sync failing"
            }
        },
        SyncDisplayStatus::Retrying => rsx! {
            span { class: "sync-mark-ring", "aria-hidden": "true" }
            span { class: "sync-mark-label", "Retrying" }
        },
        SyncDisplayStatus::Synced { .. } => rsx! {
            span { class: "sync-mark-tick", "aria-hidden": "true", CheckIcon {} }
        },
        SyncDisplayStatus::NotSynced => rsx! {
            span { class: "sync-mark-hollow", "aria-hidden": "true" }
            span { class: "sync-mark-label", "Not synced" }
        },
    };

    rsx! {
        span { class: "sync-status",
            if record_open() {
                div {
                    class: "kebab-menu-dismiss-overlay",
                    onclick: move |_| record_open.set(false),
                }
            }
            button {
                class: "sync-mark",
                r#type: "button",
                "data-testid": "sync-mark",
                "aria-label": "{aria_label}",
                "aria-haspopup": "dialog",
                "aria-expanded": record_open(),
                onclick: move |_| record_open.set(!record_open()),
                onkeydown: move |event| {
                    if event.key() == Key::Escape {
                        record_open.set(false);
                    }
                },
                {mark}
            }
            if record_open() {
                SyncRecord { account_id, status: status.clone() }
            }
        }
    }
}

/// The sync record: a small ledger entry answering, in order — when did this
/// last work, what happened most recently, what happens next.
#[component]
fn SyncRecord(
    account_id: crate::wallets::DigitalAssetAccountId,
    status: SyncDisplayStatus,
) -> Element {
    let sync_state = use_context::<AccountSyncStateSignal>();
    let now_signal = use_context::<AccountSyncNowSignal>();
    let now = now_signal();

    let state_map = sync_state.read();
    let Some(state) = state_map.get(&account_id) else {
        return rsx! {
            div { class: "sync-record", role: "dialog", "data-testid": "sync-record",
                "aria-label": "Sync record",
                p { class: "sync-record-empty", "No sync activity yet." }
            }
        };
    };
    let snapshot = &state.snapshot;

    let last_success = snapshot.last_success_at;
    let latest_run_at = snapshot.last_completed_at;
    let latest_run_word = sync_result_word(snapshot.last_result);
    let streak = snapshot.max_consecutive_failures.value();
    let show_error = matches!(
        snapshot.last_result,
        Some(AccountSyncResult::Failure) | Some(AccountSyncResult::Partial)
    );
    // The record's error block wraps, so show the full message rather than the
    // pill-length truncation from sync_status_error_message.
    let error_text = show_error.then(|| {
        snapshot
            .last_error
            .as_ref()
            .map(|error| error.as_str().to_string())
            .unwrap_or_else(|| "Unknown sync error".to_string())
    });
    let next_retry = state
        .integration_progress
        .values()
        .filter_map(|progress| progress.retry_after)
        .filter(|retry_after| now.is_some_and(|now_utc| *retry_after > now_utc))
        .min();
    let failing_addresses_context = (snapshot.addresses_total.value() > 1
        && snapshot.addresses_failed.value() > 0)
        .then(|| {
            format!(
                "{} of {} addresses failing",
                snapshot.addresses_failed.value(),
                snapshot.addresses_total.value()
            )
        });

    rsx! {
        div { class: "sync-record", role: "dialog", "data-testid": "sync-record",
            "aria-label": "Sync record",
            p { class: "sync-record-eyebrow", "sync record" }
            div { class: "sync-record-line",
                span { class: "sync-record-k", "Last successful sync" }
                span { class: "sync-record-leader", "aria-hidden": "true" }
                span { class: "sync-record-v",
                    match (last_success, now) {
                        (Some(at), Some(now_utc)) => rsx! {
                            "{format_sync_absolute_time(at)} "
                            span { class: "sync-record-rel", "· {format_sync_relative_time(now_utc, at)}" }
                        },
                        (Some(at), None) => rsx! { "{format_sync_absolute_time(at)}" },
                        (None, _) => rsx! { "never" },
                    }
                }
            }
            div { class: "sync-record-line",
                span { class: "sync-record-k", "Latest run" }
                span { class: "sync-record-leader", "aria-hidden": "true" }
                span { class: "sync-record-v",
                    span { class: "sync-record-result is-{latest_run_word}", "{latest_run_word}" }
                    if let Some(at) = latest_run_at {
                        " · {format_sync_absolute_time(at)}"
                    }
                }
            }
            if streak >= 1 {
                div { class: "sync-record-line",
                    span { class: "sync-record-k", "Failure streak" }
                    span { class: "sync-record-leader", "aria-hidden": "true" }
                    span { class: "sync-record-v", "{streak} runs" }
                }
            }
            if let Some(context) = failing_addresses_context {
                p { class: "sync-record-context", "{context}" }
            }
            if let Some(error) = error_text {
                p { class: "sync-record-error", "{error}" }
            }
            if status == SyncDisplayStatus::Blocked {
                p { class: "sync-record-foot",
                    Link { to: crate::Route::Settings { section: None }, "Add API key in Settings" }
                }
            } else if let Some(retry_at) = next_retry {
                p { class: "sync-record-foot", "Next retry ≈ {format_sync_absolute_time(retry_at)}" }
            }
        }
    }
}
