use super::account_selector::AccountSelector;
use super::helpers::{
    AccountSelection, ExistingAccountAddressTypes, collect_existing_account_address_types,
    handle_session_expired, prevalidate_wallet_label_for_new_wallet, selected_scheme_summary,
    suggest_initial_account_selections, supported_address_schemes, trezor_error_text,
};
use crate::backend::{AccountView, WalletError, get_wallet_by_fingerprint, link_trezor_wallet};
use crate::trezor;
use crate::wallets::{
    AccountIndex, GetWalletByFingerprintRequest, LinkTrezorOutcome, LinkTrezorRequest,
    RawAccountIndex, RawLabel, RawMasterFingerprint, TrezorAccountLinkRequest, TrezorDeviceId,
    TrezorDeviceLabel, ValidatedMasterFingerprint,
};
use crate::{AuthState, AuthStatus, BannerState};
use dioxus::prelude::*;
use std::rc::Rc;

#[derive(Clone, PartialEq)]
pub(super) enum LinkFlowMode {
    NewWallet,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum LinkStep {
    /// Desktop only: checking if Trezor Bridge is running
    #[cfg(not(target_arch = "wasm32"))]
    CheckingBridge,
    /// Desktop only: Bridge not found, show installation guide
    #[cfg(not(target_arch = "wasm32"))]
    BridgeNotFound,
    /// Desktop only: selecting which device to use (when multiple connected)
    #[cfg(not(target_arch = "wasm32"))]
    SelectingDevice,
    /// Connecting to Trezor (web: initializing trezor-connect)
    Connecting,
    FetchingFingerprint,
    SelectingAccounts,
    FetchingPubkeys,
    Saving,
    Complete,
    Error,
}

#[component]
pub(super) fn LinkTrezorFlow(
    mode: LinkFlowMode,
    on_complete: EventHandler<()>,
    on_cancel: EventHandler<()>,
) -> Element {
    // On desktop, start with Bridge check; on web, go straight to Connecting
    #[cfg(target_arch = "wasm32")]
    let initial_step = LinkStep::Connecting;
    #[cfg(not(target_arch = "wasm32"))]
    let initial_step = LinkStep::CheckingBridge;

    let step = use_signal(|| initial_step);
    let trezor_error = use_signal(|| None::<trezor::TrezorError>);
    let server_error = use_signal(|| None::<WalletError>);
    let fingerprint = use_signal(|| None::<ValidatedMasterFingerprint>);
    let device_id = use_signal(|| None::<TrezorDeviceId>);
    let device_label = use_signal(|| None::<TrezorDeviceLabel>);
    let existing_wallet_label = use_signal(|| None::<String>);
    let mut wallet_label_input = use_signal(String::new);
    let mut wallet_label_error = use_signal(|| None::<String>);
    let existing_accounts = use_signal(Vec::<AccountIndex>::new);
    let existing_account_address_types = use_signal(Vec::<ExistingAccountAddressTypes>::new);
    let selected_accounts = use_signal(Vec::<AccountSelection>::new);
    let completed_outcome = use_signal(|| None::<LinkTrezorOutcome>);
    // Desktop only: available devices when multiple are connected
    #[cfg(not(target_arch = "wasm32"))]
    let available_devices = use_signal(Vec::<trezor::TrezorDevice>::new);

    let auth_state = use_context::<AuthState>();
    let banner_state = use_context::<BannerState>();

    let start_link: Rc<dyn Fn(Vec<AccountSelection>)> = {
        let step_signal = step;
        let trezor_error_signal = trezor_error;
        let server_error_signal = server_error;
        let completed_outcome_signal = completed_outcome;
        let auth_state_signal = auth_state;
        let banner_state_signal = banner_state;
        let fingerprint_state = fingerprint;
        let device_id_state = device_id;
        let device_label_state = device_label;
        let existing_wallet_label_state = existing_wallet_label;
        let wallet_label_input_state = wallet_label_input;
        let wallet_label_error_signal = wallet_label_error;

        Rc::new(move |account_selections: Vec<AccountSelection>| {
            let account_selections = account_selections.clone();
            let fingerprint = fingerprint_state().clone();
            let device_id = device_id_state().clone();
            let device_label = device_label_state().clone();
            let existing_wallet_label = existing_wallet_label_state().clone();
            let wallet_label_value = wallet_label_input_state().trim().to_string();
            let mut step = step_signal;
            let mut trezor_error = trezor_error_signal;
            let mut server_error = server_error_signal;
            let mut completed_outcome = completed_outcome_signal;
            let mut wallet_label_error = wallet_label_error_signal;
            let auth_state = auth_state_signal;
            let banner_state = banner_state_signal;

            let request_wallet_label = if let Some(existing_label) = existing_wallet_label {
                wallet_label_error.set(None);
                existing_label
            } else {
                if let Err(err) = prevalidate_wallet_label_for_new_wallet(&wallet_label_value) {
                    wallet_label_error.set(Some(err));
                    step.set(LinkStep::SelectingAccounts);
                    return;
                }
                wallet_label_error.set(None);
                wallet_label_value
            };

            spawn(async move {
                let fingerprint = match fingerprint {
                    Some(value) => value,
                    None => {
                        trezor_error.set(Some(trezor::TrezorError::missing_fingerprint()));
                        step.set(LinkStep::Error);
                        return;
                    }
                };

                if account_selections.is_empty() {
                    trezor_error.set(Some(trezor::TrezorError::no_accounts_selected()));
                    step.set(LinkStep::Error);
                    return;
                }

                // Extract user_id for trezor tracing
                let user_id = {
                    let auth_snapshot = auth_state.read();
                    match &*auth_snapshot {
                        AuthStatus::Authenticated(auth) => auth.user.user_id,
                        _ => {
                            trezor_error.set(Some(trezor::TrezorError::internal(
                                "Not authenticated".to_string(),
                            )));
                            step.set(LinkStep::Error);
                            return;
                        }
                    }
                };

                step.set(LinkStep::FetchingPubkeys);
                let mut accounts_payload = Vec::new();
                for scheme in supported_address_schemes() {
                    let accounts_for_scheme: Vec<AccountIndex> = account_selections
                        .iter()
                        .filter(|selection| selection.address_scheme == scheme)
                        .map(|selection| selection.account)
                        .collect();
                    if accounts_for_scheme.is_empty() {
                        continue;
                    }

                    let raw_indexes: Vec<RawAccountIndex> = accounts_for_scheme
                        .iter()
                        .map(|account| RawAccountIndex::new(account.as_u32()))
                        .collect();
                    let pubkey_results =
                        match trezor::get_account_pubkeys(user_id, raw_indexes, scheme).await {
                            Ok(results) => results,
                            Err(err) => {
                                trezor_error.set(Some(err));
                                step.set(LinkStep::Error);
                                return;
                            }
                        };

                    for result in pubkey_results {
                        let extended_pubkey = match result.extended_pubkey {
                            Some(value) => value,
                            None => {
                                trezor_error.set(Some(trezor::TrezorError::missing_zpub_data()));
                                step.set(LinkStep::Error);
                                return;
                            }
                        };
                        accounts_payload.push(TrezorAccountLinkRequest {
                            account_index: result.account_index,
                            address_scheme: result.address_scheme,
                            extended_pubkey,
                        });
                    }
                }

                step.set(LinkStep::Saving);
                let request = LinkTrezorRequest {
                    master_fingerprint: RawMasterFingerprint::new(fingerprint.as_str().to_string()),
                    wallet_label: RawLabel::new(request_wallet_label),
                    device_id,
                    device_label,
                    accounts: accounts_payload,
                };

                match link_trezor_wallet(request).await {
                    Ok(response) => {
                        completed_outcome.set(Some(response.outcome));
                        step.set(LinkStep::Complete);
                    }
                    Err(err) => {
                        if err.is_unauthorized() {
                            handle_session_expired(auth_state, banner_state, "trezor link save");
                        }
                        server_error.set(Some(err));
                        step.set(LinkStep::Error);
                    }
                }
            });
        })
    };

    let run_token = use_signal(|| 0u32);
    use_effect(move || {
        let _ = run_token();

        let mut step = step;
        let mut trezor_error = trezor_error;
        let mut server_error = server_error;
        let mut fingerprint_state = fingerprint;
        let mut device_id_state = device_id;
        let mut device_label_state = device_label;
        let mut existing_wallet_label_state = existing_wallet_label;
        let mut wallet_label_input_state = wallet_label_input;
        let mut wallet_label_error_state = wallet_label_error;
        let mut existing_accounts_state = existing_accounts;
        let mut existing_account_address_types_state = existing_account_address_types;
        let mut selected_accounts_state = selected_accounts;
        let mut completed_outcome_state = completed_outcome;
        #[cfg(not(target_arch = "wasm32"))]
        let mut available_devices_state = available_devices;
        let auth_state = auth_state;
        let banner_state = banner_state;

        spawn(async move {
            trezor_error.set(None);
            server_error.set(None);
            wallet_label_error_state.set(None);
            completed_outcome_state.set(None);

            // Extract user_id for trezor tracing
            let user_id = {
                let auth_snapshot = auth_state.read();
                match &*auth_snapshot {
                    AuthStatus::Authenticated(auth) => auth.user.user_id,
                    _ => {
                        trezor_error.set(Some(trezor::TrezorError::internal(
                            "Not authenticated".to_string(),
                        )));
                        step.set(LinkStep::Error);
                        return;
                    }
                }
            };

            // Desktop: Check Bridge and enumerate devices first
            #[cfg(not(target_arch = "wasm32"))]
            {
                step.set(LinkStep::CheckingBridge);
                if !trezor::is_bridge_running(user_id).await {
                    step.set(LinkStep::BridgeNotFound);
                    return;
                }

                // Enumerate devices to see if we need device selection
                match trezor::enumerate_devices(user_id).await {
                    Ok(devices) if devices.is_empty() => {
                        trezor_error.set(Some(trezor::TrezorError::no_devices()));
                        step.set(LinkStep::Error);
                        return;
                    }
                    Ok(devices) if devices.len() > 1 => {
                        // Multiple devices - let user choose
                        available_devices_state.set(devices);
                        step.set(LinkStep::SelectingDevice);
                        return;
                    }
                    Ok(devices) => {
                        // Single device - select it automatically
                        if let Some(device) = devices.first() {
                            trezor::set_selected_device(Some(device.path.clone()));
                        }
                    }
                    Err(err) => {
                        trezor_error.set(Some(err));
                        step.set(LinkStep::Error);
                        return;
                    }
                }
            }

            // Web: initialize trezor-connect JS SDK
            // (On desktop this is unnecessary — Bridge status was already checked above)
            #[cfg(target_arch = "wasm32")]
            {
                step.set(LinkStep::Connecting);
                if let Err(err) = trezor::initialize_trezor(user_id).await {
                    trezor_error.set(Some(err));
                    step.set(LinkStep::Error);
                    return;
                }
            }

            step.set(LinkStep::FetchingFingerprint);
            let fingerprint_result = match trezor::get_master_fingerprint(user_id).await {
                Ok(result) => result,
                Err(err) => {
                    trezor_error.set(Some(err));
                    step.set(LinkStep::Error);
                    return;
                }
            };

            let raw_fingerprint = match fingerprint_result.fingerprint {
                Some(value) => value,
                None => {
                    trezor_error.set(Some(trezor::TrezorError::missing_master_fingerprint()));
                    step.set(LinkStep::Error);
                    return;
                }
            };

            let validated_fingerprint = match raw_fingerprint.validate() {
                Ok(value) => value,
                Err(err) => {
                    trezor_error.set(Some(trezor::TrezorError::invalid_fingerprint(format!(
                        "invalid fingerprint: {err}"
                    ))));
                    step.set(LinkStep::Error);
                    return;
                }
            };

            fingerprint_state.set(Some(validated_fingerprint.clone()));
            device_id_state.set(fingerprint_result.device_id.clone());
            device_label_state.set(fingerprint_result.device_label.clone());

            // NewWallet: look up existing wallet by fingerprint
            {
                let lookup = GetWalletByFingerprintRequest {
                    master_fingerprint: RawMasterFingerprint::new(
                        validated_fingerprint.as_str().to_string(),
                    ),
                };
                match get_wallet_by_fingerprint(lookup).await {
                    Ok(Some(wallet)) => {
                        wallet_label_input_state.set(wallet.label.clone());
                        existing_wallet_label_state.set(Some(wallet.label.clone()));
                        let existing: Vec<AccountIndex> = {
                            let mut seen = std::collections::BTreeSet::new();
                            for account in &wallet.accounts {
                                if let AccountView::Native(account) = account
                                    && account.account_number > 0
                                {
                                    seen.insert(account.account_number);
                                }
                            }
                            seen.into_iter()
                                .filter_map(|n| AccountIndex::new(n - 1).ok())
                                .collect()
                        };
                        existing_accounts_state.set(existing.clone());
                        let existing_types = collect_existing_account_address_types(&wallet);
                        let suggested =
                            suggest_initial_account_selections(&existing, &existing_types);
                        existing_account_address_types_state.set(existing_types);
                        selected_accounts_state.set(suggested);
                        step.set(LinkStep::SelectingAccounts);
                    }
                    Ok(None) => {
                        wallet_label_input_state.set(String::new());
                        existing_wallet_label_state.set(None);
                        existing_accounts_state.set(Vec::new());
                        existing_account_address_types_state.set(Vec::new());
                        selected_accounts_state.set(suggest_initial_account_selections(&[], &[]));
                        step.set(LinkStep::SelectingAccounts);
                    }
                    Err(err) => {
                        if err.is_unauthorized() {
                            handle_session_expired(
                                auth_state,
                                banner_state,
                                "wallet lookup by fingerprint",
                            );
                        }
                        server_error.set(Some(err));
                        step.set(LinkStep::Error);
                    }
                }
            }
        });
    });

    let error_view = match (trezor_error(), server_error()) {
        (Some(err), _) => {
            let (message, troubleshooting) = trezor_error_text(err.kind);

            rsx! {
                div { class: "error-block",
                    p { class: "error-title", "{message}" }
                    p { class: "error-help",
                        "{troubleshooting}"
                    }
                }
            }
        }
        (None, Some(err)) => rsx! {
            div { class: "error-block",
                p { class: "error-title", "{err}" }
            }
        },
        _ => rsx! { div {} },
    };

    rsx! {
        div { class: "modal-overlay",
            div { class: "modal",
                div { class: "modal-header",
                    h3 { "Link Trezor Account" }
                }
                div { class: "modal-body",
                    match step() {
                        #[cfg(not(target_arch = "wasm32"))]
                        LinkStep::CheckingBridge => rsx! {
                            div { class: "flow-step",
                                p { "Checking Trezor Bridge..." }
                            }
                        },
                        #[cfg(not(target_arch = "wasm32"))]
                        LinkStep::BridgeNotFound => rsx! {
                            div { class: "flow-step",
                                {
                                    let (msg, help) = trezor_error_text(trezor::TrezorErrorKind::BridgeNotRunning);
                                    rsx! {
                                        p { class: "error-title", "{msg}" }
                                        p { class: "error-help", "{help}" }
                                    }
                                }
                                div { class: "modal-actions",
                                    button {
                                        class: "btn btn-secondary",
                                        onclick: move |_| on_cancel.call(()),
                                        "Cancel"
                                    }
                                    button {
                                        class: "btn btn-primary",
                                        onclick: move |_| {
                                            // Retry Bridge check
                                            let mut run_token_handle = run_token;
                                            run_token_handle.set(run_token_handle() + 1);
                                        },
                                        "Retry"
                                    }
                                }
                            }
                        },
                        #[cfg(not(target_arch = "wasm32"))]
                        LinkStep::SelectingDevice => {
                            let devices_list = available_devices();
                            let mut step_handle = step;
                            rsx! {
                                DevicePicker {
                                    devices: devices_list,
                                    on_select: move |device: trezor::TrezorDevice| {
                                        trezor::set_selected_device(Some(device.path.clone()));
                                        step_handle.set(LinkStep::Connecting);
                                    },
                                }
                            }
                        }
                        LinkStep::Connecting => rsx! {
                            div { class: "flow-step",
                                p { "Connecting to Trezor..." }
                            }
                        },
                        LinkStep::FetchingFingerprint => rsx! {
                            div { class: "flow-step",
                                p { "Reading wallet fingerprint..." }
                            }
                        },
                        LinkStep::SelectingAccounts => {
                            rsx! {
                                AccountSelector {
                                    existing_wallet_label: existing_wallet_label(),
                                    new_wallet_label: wallet_label_input(),
                                    wallet_label_error: wallet_label_error(),
                                    existing_accounts: existing_accounts(),
                                    existing_account_address_types: existing_account_address_types(),
                                    on_new_wallet_label_change: move |value| {
                                        wallet_label_input.set(value);
                                        wallet_label_error.set(None);
                                    },
                                    selected: selected_accounts(),
                                    on_change: move |accounts| {
                                        let mut selected_accounts = selected_accounts;
                                        selected_accounts.set(accounts);
                                    },
                                    on_continue: move |_| {
                                        (start_link)(selected_accounts());
                                    },
                                    on_cancel: move |_| on_cancel.call(()),
                                }
                            }
                        }
                        LinkStep::FetchingPubkeys => {
                            let summary = selected_scheme_summary(&selected_accounts());
                            rsx! {
                                div { class: "flow-step",
                                    p { "Fetching account public keys..." }
                                    p { class: "muted", "{summary}" }
                                    p { class: "muted", "Please confirm each request on your Trezor device." }
                                }
                            }
                        }
                        LinkStep::Saving => rsx! {
                            div { class: "flow-step",
                                p { "Saving wallet data..." }
                            }
                        },
                        LinkStep::Complete => {
                            let outcome = completed_outcome();
                            let outcome_text = match outcome {
                                Some(LinkTrezorOutcome::NewWallet) => "Wallet linked successfully.",
                                Some(LinkTrezorOutcome::ExistingWallet) => "Accounts added to existing wallet.",
                                None => "Wallet linked.",
                            };
                            rsx! {
                                div { class: "flow-step",
                                    p { "{outcome_text}" }
                                    button {
                                        class: "btn btn-primary",
                                        onclick: move |_| on_complete.call(()),
                                        "Done"
                                    }
                                }
                            }
                        }
                        LinkStep::Error => rsx! {
                            div { class: "flow-step",
                                {error_view}
                                div { class: "modal-actions",
                                    button {
                                        class: "btn btn-secondary",
                                        onclick: move |_| on_cancel.call(()),
                                        "Cancel"
                                    }
                                    button {
                                        class: "btn btn-secondary",
                                        onclick: move |_| {
                                            let mut run_token_handle = run_token;
                                            let next = run_token_handle() + 1;
                                            run_token_handle.set(next);
                                        },
                                        "Try Again"
                                    }
                                }
                            }
                        },
                    }
                }
            }
        }
    }
}

/// Device picker for desktop when multiple Trezors are connected.
#[component]
fn DevicePicker(
    devices: Vec<trezor::TrezorDevice>,
    on_select: EventHandler<trezor::TrezorDevice>,
) -> Element {
    rsx! {
        div { class: "device-picker",
            h4 { "Multiple Trezor devices detected" }
            p { class: "muted", "Select the device you want to use:" }
            div { class: "device-list",
                for device in devices.clone() {
                    button {
                        class: "device-item btn btn-outline",
                        onclick: {
                            let device_clone = device.clone();
                            move |_| on_select.call(device_clone.clone())
                        },
                        div { class: "device-info",
                            span { class: "device-product",
                                "{device.product_name.clone().or(device.product.clone()).unwrap_or_else(|| \"Trezor\".to_string())}"
                            }
                            span { class: "device-path muted",
                                "{device.path}"
                            }
                        }
                    }
                }
            }
        }
    }
}
