use super::super::ExternalLinkIcon;
use super::super::form_helpers::{is_form_field_error, primary_field_or_message};
use crate::backend::{get_account_sync_control_state, run_account_sync_control};
use crate::settings::SettingsState;
use crate::transactions::{RunAccountSyncControlRequest, SyncControlInvocationResponse};
use crate::wallets::WalletAccountId;
use dioxus::prelude::*;

#[component]
pub(super) fn SyncControlCard(
    account_id: WalletAccountId,
    loading: bool,
    on_complete: EventHandler<()>,
) -> Element {
    let settings_state = use_context::<SettingsState>();
    let mut iteration_input = use_signal(|| "1".to_string());
    let mut submitting = use_signal(|| false);
    let mut invocation_result = use_signal(|| None::<SyncControlInvocationResponse>);
    let mut invocation_error = use_signal(|| None::<crate::backend::ApiErrorEnvelope>);
    let mut local_iteration_error = use_signal(|| None::<String>);
    let mut state_resource = use_server_future(move || {
        let account_id = account_id;
        async move { get_account_sync_control_state(account_id).await }
    })?;

    let state_result = state_resource();
    let state_data = state_result
        .as_ref()
        .and_then(|result| result.as_ref().ok());
    let state_error = state_result
        .as_ref()
        .and_then(|result| result.as_ref().err());
    let iteration_error = local_iteration_error().or_else(|| {
        invocation_error()
            .as_ref()
            .map(|error| primary_field_or_message(error, &["iterations"]))
    });
    let invocation_error_message = invocation_error().as_ref().and_then(|error| {
        if is_form_field_error(error) {
            None
        } else {
            Some(error.message.clone())
        }
    });

    rsx! {
        div { class: "card sync-control-card",
            div { class: "card-body",
                h3 { class: "sync-control-title", "Sync Control" }
                p { class: "sync-control-hint muted",
                    "Developer mode: execute sync iterations one at a time"
                }

                // Context section
                if let Some(state) = state_data {
                    div { class: "sync-control-context",
                        div { class: "sync-control-meta",
                            span { class: "sync-control-meta-label", "Addresses: " }
                            span { "{state.addresses_total}" }
                            if let Some(integration) = &state.integration {
                                span { class: "sync-control-meta-label", " | Integration: " }
                                span { "{integration}" }
                            }
                        }
                        if !state.addresses.is_empty() {
                            div { class: "sync-control-addresses",
                                for addr_state in &state.addresses {
                                    div { class: "sync-control-address-row",
                                        {
                                            let url = crate::explorer_links::address_explorer_url(
                                                &settings_state,
                                                crate::explorer_links::DigitalAssetAddressRef::from_asset(
                                                    addr_state.asset_id,
                                                    addr_state.network,
                                                    &addr_state.full_address,
                                                ),
                                            )
                                            .ok();
                                            let label = addr_state.truncated_address.clone();
                                            match url {
                                                Some(url) => rsx! {
                                                    a {
                                                        class: "sync-control-address address-link",
                                                        href: "{url}",
                                                        target: "_blank",
                                                        rel: "noopener noreferrer",
                                                        title: "{addr_state.full_address}",
                                                        "{label}"
                                                        ExternalLinkIcon {}
                                                    }
                                                },
                                                None => rsx! {
                                                    code { class: "sync-control-address", "{label}" }
                                                },
                                            }
                                        }
                                        if let Some(last_sync) = &addr_state.last_sync_at {
                                            span { class: "sync-control-address-detail muted",
                                                " last: {last_sync}"
                                            }
                                        }
                                        if let Some(result) = &addr_state.last_result {
                                            span { class: "sync-control-address-detail sync-control-result-{result.to_lowercase()}",
                                                " {result}"
                                            }
                                        }
                                        if addr_state.backfill_active {
                                            span { class: "sync-control-backfill-badge",
                                                "Syncing: backfill in progress"
                                            }
                                            if let Some(cursor) = &addr_state.backfill_cursor_display {
                                                span { class: "sync-control-address-detail muted",
                                                    " cursor: {cursor}"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else if let Some(error) = state_error {
                    div { class: "sync-control-result-error",
                        p { "{error.message}" }
                    }
                } else {
                    div { class: "sync-control-loading muted",
                        span { class: "account-sync-spinner account-sync-spinner-small", "aria-label": "Loading", title: "Loading" }
                        span { "Loading sync state..." }
                    }
                }

                // Control section
                div { class: "sync-control-controls",
                    div { class: "sync-control-input-group",
                        label {
                            class: "form-label",
                            r#for: "sync-control-iterations",
                            "Run"
                        }
                        input {
                            id: "sync-control-iterations",
                            class: "form-input sync-control-iterations-input",
                            r#type: "number",
                            min: "1",
                            max: "100",
                            value: "{iteration_input}",
                            disabled: submitting() || loading,
                            onmounted: move |e| async move {
                                let _ = e.set_focus(true).await;
                            },
                            oninput: move |e| {
                                local_iteration_error.set(None);
                                iteration_input.set(e.value());
                            },
                        }
                        span { class: "sync-control-iterations-label", "sync iteration(s)" }
                    }
                    if let Some(message) = iteration_error {
                        p { class: "form-error sync-control-input-error", "{message}" }
                    }
                    button {
                        class: "btn btn-primary sync-control-run-btn",
                        r#type: "button",
                        disabled: submitting() || loading,
                        onclick: move |_| {
                            let iterations: u32 = match iteration_input().parse() {
                                Ok(v) => v,
                                Err(_) => {
                                    local_iteration_error
                                        .set(Some("Enter a whole number between 1 and 100".to_string()));
                                    return;
                                }
                            };
                            let account_id = account_id;
                            submitting.set(true);
                            local_iteration_error.set(None);
                            invocation_error.set(None);
                            invocation_result.set(None);
                            spawn(async move {
                                let request = RunAccountSyncControlRequest {
                                    iterations: crate::transactions::RawSyncIterationCount(iterations),
                                };
                                match run_account_sync_control(account_id, request).await {
                                    Ok(response) => {
                                        invocation_result.set(Some(response));
                                        submitting.set(false);
                                        state_resource.restart();
                                        on_complete.call(());
                                    }
                                    Err(err) => {
                                        invocation_error.set(Some(err));
                                        submitting.set(false);
                                    }
                                }
                            });
                        },
                        if submitting() {
                            span { class: "sync-control-run-label",
                                span { class: "account-sync-spinner account-sync-spinner-small", "aria-label": "Syncing", title: "Syncing" }
                                span { "Running..." }
                            }
                        } else {
                            "Run"
                        }
                    }
                }

                // Result section
                if let Some(result) = invocation_result() {
                    div { class: "sync-control-result",
                        div { class: "sync-control-result-row",
                            span { "Iterations: {result.iterations_completed}/{result.iterations_requested}" }
                        }
                        div { class: "sync-control-result-row",
                            span { "Addresses touched: {result.addresses_touched}" }
                        }
                        div { class: "sync-control-result-row",
                            span { "New txs: {result.total_new_transactions} | Updated: {result.total_updated_transactions}" }
                        }
                        if result.backfill_continuing {
                            div { class: "sync-control-result-row",
                                span { class: "sync-control-backfill-badge", "Syncing: backfill in progress" }
                            }
                        }
                        if result.stopped_early {
                            div { class: "sync-control-result-row",
                                span { class: "sync-control-stopped-badge", "Stopped early" }
                            }
                        }
                        if let Some(err) = &result.error_message {
                            div { class: "sync-control-result-row sync-control-result-error",
                                span { "{err}" }
                            }
                        }
                    }
                }

                if let Some(err) = invocation_error_message {
                    div { class: "sync-control-result-error",
                        p { "{err}" }
                    }
                }
            }
        }
    }
}
