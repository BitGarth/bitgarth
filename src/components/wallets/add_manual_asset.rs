use super::helpers::{handle_session_expired, prevalidate_wallet_label_for_new_wallet};
use super::wallet_dropdown::{
    AccountNameField, WalletChoice, WalletDropdown, initial_wallet_dropdown_choice,
    wallet_options_for_dropdown,
};
use crate::backend::{
    WalletView, add_manual_asset_account, get_wallets, manual_asset_catalog_total,
    manual_asset_discovery_detail, manual_asset_discovery_price, search_manual_asset_instances,
};
use crate::components::form_helpers::{
    begin_submit, finish_submit, first_matching_field_error, is_form_field_error,
    primary_field_or_message,
};
use crate::components::{ToastLevel, ToastState, push_toast};
use crate::models::CurrencyCode;
use crate::settings::SettingsState;
use crate::wallets::{
    AddManualAssetAccountAssetRequest, AddManualAssetAccountRequest,
    CoinGeckoManualAssetPrecisionSourceRequest, CoinGeckoManualAssetSnapshotRequest,
    ManualAssetCatalogTotalResponse, ManualAssetDiscoveryDetailRequest,
    ManualAssetDiscoveryDetailResponse, ManualAssetDiscoveryPlatformRow,
    ManualAssetDiscoveryPriceRequest, ManualAssetInstanceSearchRow, ManualAssetSearchSource,
    RawLabel, SearchManualAssetInstancesRequest, SearchManualAssetInstancesResponse, WalletId,
};
use crate::{AuthState, BannerState};
use dioxus::prelude::*;

#[component]
pub(crate) fn AddManualAssetFlow(
    default_wallet_id: Option<WalletId>,
    on_complete: EventHandler<crate::wallets::WalletAccountId>,
    on_cancel: EventHandler<()>,
) -> Element {
    let auth_state = use_context::<AuthState>();
    let banner_state = use_context::<BannerState>();
    let toast_state = use_context::<ToastState>();
    let settings_state = use_context::<SettingsState>();
    let mut search_query = use_signal(String::new);
    let mut allow_catalog_refresh = use_signal(|| false);
    let mut selected_asset = use_signal(|| None::<ManualAssetInstanceSearchRow>);
    let mut allow_detail_lookup = use_signal(|| false);
    let mut allow_catalog_price_lookup = use_signal(|| false);
    let mut detail_prefill_coingecko_id = use_signal(|| None::<String>);
    let mut detail_lookup_cache = use_signal(|| None::<ManualAssetDetailCacheEntry>);
    let mut price_lookup_cache = use_signal(|| None::<ManualAssetPriceCacheEntry>);
    let mut selected_platform_id = use_signal(|| None::<String>);
    let mut unit_code_input = use_signal(String::new);
    let mut precision_input = use_signal(|| "6".to_string());
    let mut native_network_name_input = use_signal(|| "Native".to_string());
    let mut wallet_choice = use_signal(|| None::<WalletChoice>);
    let mut wallet_label_input = use_signal(String::new);
    let mut wallet_label_error = use_signal(|| None::<String>);
    let mut account_label_input = use_signal(String::new);
    let mut account_label_error = use_signal(|| None::<String>);
    let mut field_error = use_signal(|| None::<String>);
    let mut save_error = use_signal(|| None::<String>);
    let saving = use_signal(|| false);
    // Bump after a CoinGecko-refresh-enabled search returns, so the placeholder
    // total re-reads the (possibly grown) local cache — never before the refresh.
    let mut catalog_total_refresh_epoch = use_signal(|| 0_u32);

    let search_resource = use_resource(move || {
        let query = search_query();
        let allow_coingecko_catalog_refresh =
            allow_catalog_refresh() || (settings_state.price_fetching_enabled)();
        async move {
            if query.trim().is_empty() {
                // `total_match_count` only satisfies the type here; the label's
                // empty-query branch shows the idle total, never this match count.
                return Ok(SearchManualAssetInstancesResponse {
                    results: Vec::new(),
                    total_match_count: 0,
                });
            }
            let response = search_manual_asset_instances(SearchManualAssetInstancesRequest {
                query,
                allow_coingecko_catalog_refresh,
            })
            .await;
            if response.is_ok() && allow_coingecko_catalog_refresh {
                catalog_total_refresh_epoch.with_mut(|epoch| *epoch += 1);
            }
            response
        }
    });

    let catalog_total_resource = use_resource(move || {
        // Re-run after an allowed CoinGecko refresh (epoch bump) or when price
        // fetching toggles. No network side effects of its own.
        let _epoch = catalog_total_refresh_epoch();
        let _price_fetching = (settings_state.price_fetching_enabled)();
        async move { manual_asset_catalog_total().await }
    });

    let detail_resource = use_resource(move || {
        let selected = selected_asset();
        let allow_remote_lookup =
            allow_detail_lookup() || (settings_state.price_fetching_enabled)();
        let key = manual_asset_detail_request_key(selected.as_ref(), allow_remote_lookup);
        let cached = detail_lookup_cache();
        async move {
            let key = key?;
            if let Some(entry) = cached
                && entry.key == key
            {
                return Some(Ok(entry.value));
            }

            let value = manual_asset_discovery_detail(ManualAssetDiscoveryDetailRequest {
                coingecko_id: key.coingecko_id.clone(),
                allow_remote_lookup: key.allow_remote_lookup,
            })
            .await;
            if let Ok(response) = &value {
                detail_lookup_cache.set(Some(ManualAssetDetailCacheEntry {
                    key,
                    value: response.clone(),
                }));
            }
            Some(value)
        }
    });

    let price_resource = use_resource(move || {
        let asset = selected_asset();
        let detail_state = detail_resource.value().read().clone();
        let coingecko_allowed = allow_detail_lookup() || (settings_state.price_fetching_enabled)();
        let catalog_allowed =
            allow_catalog_price_lookup() || (settings_state.price_fetching_enabled)();
        let quote_currency = (settings_state.currency)();
        let cached = price_lookup_cache();
        async move {
            let asset = asset?;
            let detail = match detail_state {
                Some(Some(Ok(ref detail))) => {
                    current_detail_for_selected_asset(Some(&asset), detail)
                }
                _ => None,
            };
            let allow_remote_lookup = match asset.source {
                ManualAssetSearchSource::CoinGeckoCatalog => coingecko_allowed,
                ManualAssetSearchSource::BitGarthCatalog => catalog_allowed,
            };
            let key = manual_asset_price_request_key(
                &asset,
                detail,
                quote_currency,
                allow_remote_lookup,
            )?;

            if let Some(entry) = cached
                && entry.key == key
            {
                return Some(Ok(entry.value));
            }

            let value = manual_asset_discovery_price(ManualAssetDiscoveryPriceRequest {
                asset_id: key.asset_id.clone(),
                coingecko_id: key.coingecko_id.clone(),
                quote_currency: key.quote_currency,
                allow_remote_lookup: key.allow_remote_lookup,
            })
            .await;
            if let Ok(response) = &value
                && response.price.is_some()
            {
                price_lookup_cache.set(Some(ManualAssetPriceCacheEntry {
                    key,
                    value: response.clone(),
                }));
            }
            Some(value)
        }
    });

    let wallets_resource = use_resource(move || async move { get_wallets().await });
    let wallets: Vec<WalletView> = wallets_resource
        .value()
        .read()
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .map_or_else(Vec::new, |r| r.wallets.clone());
    let wallet_options = wallet_options_for_dropdown(&wallets, default_wallet_id);
    let wallets_loading = wallets_resource.value().read().is_none();
    if !wallets_loading && wallet_choice.peek().is_none() {
        wallet_choice.set(Some(initial_wallet_dropdown_choice(
            default_wallet_id,
            None,
            wallet_options.len(),
        )));
    }

    let save = move |_| {
        if !begin_submit(saving) {
            return;
        }

        field_error.set(None);
        wallet_label_error.set(None);
        account_label_error.set(None);
        save_error.set(None);

        let choice = match wallet_choice() {
            Some(choice) => choice,
            None => {
                field_error.set(Some("Wallets are still loading.".to_string()));
                finish_submit(saving);
                return;
            }
        };

        let wallet_label_raw = wallet_label_input().trim().to_string();
        let (wallet_id, wallet_label) = match choice {
            WalletChoice::Unselected => {
                field_error.set(Some(
                    "Select an existing wallet or create a new one".to_string(),
                ));
                finish_submit(saving);
                return;
            }
            WalletChoice::Existing(wallet_id) => (Some(wallet_id), None),
            WalletChoice::CreateNew => {
                if let Err(err) = prevalidate_wallet_label_for_new_wallet(&wallet_label_raw) {
                    wallet_label_error.set(Some(err));
                    finish_submit(saving);
                    return;
                }
                (None, Some(RawLabel::new(wallet_label_raw)))
            }
        };

        let Some(asset) = selected_asset() else {
            field_error.set(Some("Select a manual asset.".to_string()));
            finish_submit(saving);
            return;
        };

        let asset_request = match asset.source {
            ManualAssetSearchSource::BitGarthCatalog => {
                let Some(asset_instance_id) = asset.asset_instance_id else {
                    field_error.set(Some(
                        "Select a supported manual asset from the BitGarth catalog.".to_string(),
                    ));
                    finish_submit(saving);
                    return;
                };
                AddManualAssetAccountAssetRequest::BitGarthCatalog { asset_instance_id }
            }
            ManualAssetSearchSource::CoinGeckoCatalog => {
                let detail = match detail_resource.value().read().as_ref() {
                    Some(Some(Ok(detail))) => {
                        match current_detail_for_selected_asset(Some(&asset), detail) {
                            Some(detail) => detail.clone(),
                            None => {
                                field_error.set(Some(
                                    "Confirm CoinGecko asset details first.".to_string(),
                                ));
                                finish_submit(saving);
                                return;
                            }
                        }
                    }
                    Some(Some(Err(err))) => {
                        field_error.set(Some(err.to_string()));
                        finish_submit(saving);
                        return;
                    }
                    _ => {
                        field_error.set(Some("Confirm CoinGecko asset details first.".to_string()));
                        finish_submit(saving);
                        return;
                    }
                };
                let snapshot = match build_coingecko_snapshot(
                    &detail,
                    selected_platform_id(),
                    unit_code_input(),
                    precision_input(),
                    native_network_name_input(),
                ) {
                    Ok(snapshot) => snapshot,
                    Err(message) => {
                        field_error.set(Some(message));
                        finish_submit(saving);
                        return;
                    }
                };
                AddManualAssetAccountAssetRequest::CoinGeckoDiscovery { snapshot }
            }
        };

        let account_label_raw = account_label_input().trim().to_string();
        let account_label = if account_label_raw.is_empty() {
            None
        } else {
            Some(RawLabel::new(account_label_raw))
        };

        let request = AddManualAssetAccountRequest {
            wallet_id,
            wallet_label,
            asset: Some(asset_request),
            asset_instance_id: None,
            account_label,
        };

        spawn(async move {
            match add_manual_asset_account(request).await {
                Ok(response) => {
                    finish_submit(saving);
                    if let Some(notice) = response.account_limit_notice {
                        push_toast(toast_state, ToastLevel::Info, notice.message);
                    }
                    on_complete.call(response.account_id);
                }
                Err(err) if err.is_unauthorized() => {
                    finish_submit(saving);
                    handle_session_expired(auth_state, banner_state, "add manual asset");
                }
                Err(err) if is_form_field_error(&err) => {
                    finish_submit(saving);
                    if let Some(message) = first_matching_field_error(&err, &["wallet_label"]) {
                        wallet_label_error.set(Some(message));
                    } else if let Some(message) =
                        first_matching_field_error(&err, &["account_label", "label"])
                    {
                        account_label_error.set(Some(message));
                    } else {
                        let message =
                            primary_field_or_message(&err, &["asset_instance_id", "wallet_id"]);
                        field_error.set(Some(message));
                    }
                }
                Err(err) => {
                    finish_submit(saving);
                    save_error.set(Some(err.to_string()));
                }
            }
        });
    };

    let query_text = search_query();
    let combo_expanded = if query_text.trim().is_empty() {
        "false"
    } else {
        "true"
    };

    if let Some(Some(Ok(detail))) = detail_resource.value().read().as_ref()
        && let Some(detail) = current_detail_for_selected_asset(selected_asset().as_ref(), detail)
        && detail_prefill_coingecko_id.peek().as_deref() != Some(detail.coingecko_id.as_str())
    {
        prefill_coingecko_detail(
            detail,
            detail_prefill_coingecko_id,
            selected_platform_id,
            unit_code_input,
            precision_input,
            native_network_name_input,
        );
    }

    let price_loaded: DetailPriceSlot = match price_resource.value().read().as_ref() {
        Some(Some(Ok(resp))) => match resp.price.as_ref() {
            Some(amount) => DetailPriceSlot::Value {
                amount: amount.clone(),
                code: resp.quote_currency.code().to_string(),
            },
            None => DetailPriceSlot::Unavailable,
        },
        Some(Some(Err(_))) => DetailPriceSlot::Unavailable,
        _ => DetailPriceSlot::Loading,
    };
    let catalog_price_allowed =
        allow_catalog_price_lookup() || (settings_state.price_fetching_enabled)();

    rsx! {
        div { class: "modal-overlay",
            div { class: "modal",
                div { class: "modal-header",
                    h3 { "Add Manual Asset" }
                }
                div { class: "modal-body",
                    div { class: "flow-step",
                        p { class: "muted",
                            if default_wallet_id.is_some() {
                                "Create a manual-asset account inside this wallet."
                            } else {
                                "Create a manual-asset account."
                            }
                        }

                        label {
                            class: "form-label",
                            id: "manual-asset-search-label",
                            {
                                let catalog_total = catalog_total_resource
                                    .value()
                                    .read()
                                    .as_ref()
                                    .and_then(|r: &Result<ManualAssetCatalogTotalResponse, _>| {
                                        r.as_ref().ok().cloned()
                                    });
                                let total = catalog_total.as_ref().map(|resp| resp.total);
                                // Nudge to enable CoinGecko only when the local
                                // catalog is empty and the user has not enabled it.
                                let offer_coingecko = catalog_total
                                    .as_ref()
                                    .is_some_and(|resp| resp.coingecko_catalog_empty)
                                    && !allow_catalog_refresh()
                                    && !(settings_state.price_fetching_enabled)();
                                let match_total = search_resource
                                    .value()
                                    .read()
                                    .as_ref()
                                    .and_then(|r| r.as_ref().ok())
                                    .map(|resp| resp.total_match_count);
                                search_label_text(
                                    &search_query(),
                                    total,
                                    match_total,
                                    offer_coingecko,
                                )
                            }
                        }
                        if let Some(asset) = selected_asset() {
                            div { class: "manual-asset-selected",
                                span { class: "ticker", "{asset.unit_code}" }
                                span { class: "name", "{asset.asset_name}" }
                                span { class: "net", "{selected_asset_context(&asset)}" }
                                button {
                                    class: "manual-asset-clear",
                                    r#type: "button",
                                    aria_label: "Change asset",
                                    onclick: move |_| {
                                        selected_asset.set(None);
                                        search_query.set(String::new());
                                        allow_detail_lookup.set(false);
                                        allow_catalog_price_lookup.set(false);
                                        detail_prefill_coingecko_id.set(None);
                                        selected_platform_id.set(None);
                                        unit_code_input.set(String::new());
                                        precision_input.set("6".to_string());
                                        native_network_name_input.set("Native".to_string());
                                        field_error.set(None);
                                    },
                                    super::super::CloseIcon {}
                                }
                            }
                            if asset.source == ManualAssetSearchSource::CoinGeckoCatalog {
                                if !(settings_state.price_fetching_enabled)() && !allow_detail_lookup() {
                                    div { class: "manual-asset-consent",
                                        p { "CoinGecko detail lookup is optional and sends only the selected CoinGecko asset id." }
                                        button {
                                            class: "btn btn-secondary",
                                            r#type: "button",
                                            disabled: saving(),
                                            onclick: move |_| {
                                                allow_detail_lookup.set(true);
                                                field_error.set(None);
                                            },
                                            "Look Up Details"
                                        }
                                    }
                                } else if let Some(Some(Ok(detail))) = detail_resource.value().read().as_ref()
                                    && let Some(detail) = current_detail_for_selected_asset(Some(&asset), detail) {
                                    ManualAssetDetailPanel {
                                        platforms: detail.platforms.clone(),
                                        single_network_name: native_network_name_input(),
                                        default_decimal_precision: detail.default_decimal_precision,
                                        selected_platform_id,
                                        precision_input,
                                        field_error,
                                        unit_code: unit_code_input(),
                                        decimals_editable: true,
                                        decimals_value: 0,
                                        price: price_loaded.clone(),
                                        on_price_consent: move |_| {},
                                    }
                                } else if let Some(Some(Err(err))) = detail_resource.value().read().as_ref() {
                                    div { class: "alert alert-error",
                                        strong { "CoinGecko lookup failed: " }
                                        "{err}"
                                    }
                                } else {
                                    div { class: "manual-asset-loading", "Loading CoinGecko details..." }
                                }
                            } else {
                                ManualAssetDetailPanel {
                                    platforms: Vec::new(),
                                    single_network_name: asset.network_name.clone(),
                                    default_decimal_precision: asset.decimal_precision.unwrap_or_default(),
                                    selected_platform_id,
                                    precision_input,
                                    field_error,
                                    unit_code: asset.unit_code.clone(),
                                    decimals_editable: false,
                                    decimals_value: asset.decimal_precision.unwrap_or_default(),
                                    price: if catalog_price_allowed {
                                        price_loaded.clone()
                                    } else {
                                        DetailPriceSlot::ConsentNeeded
                                    },
                                    on_price_consent: move |_| {
                                        allow_catalog_price_lookup.set(true);
                                        field_error.set(None);
                                    },
                                }
                            }
                        } else {
                            div { class: "manual-asset-combo",
                                span { class: "manual-asset-search-icon", super::super::SearchIcon {} }
                                input {
                                    class: "manual-asset-input",
                                    r#type: "search",
                                    role: "combobox",
                                    aria_autocomplete: "list",
                                    aria_expanded: combo_expanded,
                                    aria_controls: "manual-asset-results",
                                    aria_labelledby: "manual-asset-search-label",
                                    autocomplete: "off",
                                    placeholder: "Search by name, ticker, or network…",
                                    value: "{search_query}",
                                    oninput: move |e| {
                                        search_query.set(e.value());
                                        allow_catalog_refresh.set(false);
                                        field_error.set(None);
                                    },
                                    onmounted: move |e| async move { let _ = e.set_focus(true).await; },
                                }

                                if let Some(Ok(response)) = search_resource.value().read().as_ref() {
                                    if !query_text.trim().is_empty() {
                                        div {
                                            class: "manual-asset-results",
                                            id: "manual-asset-results",
                                            role: "listbox",
                                            if response.results.is_empty() {
                                                div { class: "manual-asset-empty",
                                                    "No assets match “{query_text.trim()}”."
                                                }
                                            } else {
                                                for result in response.results.iter() {
                                                    button {
                                                        class: "manual-asset-result",
                                                        role: "option",
                                                        r#type: "button",
                                                        onclick: {
                                                            let result = result.clone();
                                                            move |_| {
                                                                selected_asset.set(Some(result.clone()));
                                                                allow_detail_lookup.set(false);
                                                                allow_catalog_price_lookup.set(false);
                                                                detail_prefill_coingecko_id.set(None);
                                                                selected_platform_id.set(None);
                                                                unit_code_input.set(String::new());
                                                                precision_input.set("6".to_string());
                                                                native_network_name_input.set("Native".to_string());
                                                                field_error.set(None);
                                                            }
                                                        },
                                                        span { class: "ticker", "{result.unit_code}" }
                                                        span { class: "name", "{result.asset_name}" }
                                                        span { class: "source", "{search_source_label(result)}" }
                                                        span { class: "net", "{selected_asset_context(result)}" }
                                                    }
                                                }
                                            }
                                            if !query_text.trim().is_empty()
                                                && !allow_catalog_refresh()
                                                && !(settings_state.price_fetching_enabled)()
                                            {
                                                div { class: "manual-asset-catalog-refresh",
                                                    button {
                                                        class: "btn btn-secondary",
                                                        r#type: "button",
                                                        onclick: move |_| {
                                                            allow_catalog_refresh.set(true);
                                                            field_error.set(None);
                                                        },
                                                        "Search CoinGecko"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        if wallets_loading {
                            label { class: "form-label", "Wallet" }
                            select {
                                class: "selector",
                                disabled: true,
                                option {
                                    value: super::CREATE_NEW_WALLET_OPTION_VALUE,
                                    "Loading wallets..."
                                }
                            }
                        } else if let Some(choice) = wallet_choice() {
                            WalletDropdown {
                                wallets: wallet_options.clone(),
                                choice,
                                default_wallet_id,
                                pinned_wallet: None,
                                new_wallet_label: wallet_label_input(),
                                wallet_label_error: wallet_label_error(),
                                on_choice_change: move |choice| {
                                    wallet_choice.set(Some(choice));
                                    field_error.set(None);
                                    wallet_label_error.set(None);
                                },
                                on_new_wallet_label_change: move |value| {
                                    wallet_label_input.set(value);
                                    wallet_label_error.set(None);
                                },
                            }
                        }

                        AccountNameField {
                            value: account_label_input(),
                            placeholder: match selected_asset() {
                                Some(asset) => format!("{} Account 1", asset.unit_code),
                                None => "Manual asset account".to_string(),
                            },
                            error: account_label_error(),
                            on_input: move |value| {
                                account_label_input.set(value);
                                account_label_error.set(None);
                            },
                        }

                        if let Some(error) = field_error() {
                            div { class: "alert alert-error",
                                strong { "Validation error: " }
                                "{error}"
                            }
                        }

                        if let Some(error) = save_error() {
                            div { class: "alert alert-error",
                                strong { "Error: " }
                                "{error}"
                            }
                        }

                        div { class: "modal-actions",
                            button {
                                class: "btn btn-secondary",
                                disabled: saving(),
                                onclick: move |_| on_cancel.call(()),
                                "Cancel"
                            }
                            button {
                                class: "btn btn-primary",
                                disabled: saving() || wallets_loading || wallet_choice().is_none(),
                                onclick: save,
                                if saving() { "Adding..." } else { "Add Manual Asset" }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ManualAssetDetailRequestKey {
    coingecko_id: String,
    allow_remote_lookup: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ManualAssetPriceRequestKey {
    asset_id: String,
    coingecko_id: String,
    quote_currency: CurrencyCode,
    allow_remote_lookup: bool,
}

#[derive(Clone)]
struct ManualAssetDetailCacheEntry {
    key: ManualAssetDetailRequestKey,
    value: ManualAssetDiscoveryDetailResponse,
}

#[derive(Clone)]
struct ManualAssetPriceCacheEntry {
    key: ManualAssetPriceRequestKey,
    value: crate::wallets::ManualAssetDiscoveryPriceResponse,
}

fn manual_asset_detail_request_key(
    selected: Option<&ManualAssetInstanceSearchRow>,
    allow_remote_lookup: bool,
) -> Option<ManualAssetDetailRequestKey> {
    let asset = selected?;
    if asset.source != ManualAssetSearchSource::CoinGeckoCatalog || !allow_remote_lookup {
        return None;
    }
    Some(ManualAssetDetailRequestKey {
        coingecko_id: asset.coingecko_id.clone()?,
        allow_remote_lookup,
    })
}

fn manual_asset_price_request_key(
    asset: &ManualAssetInstanceSearchRow,
    detail: Option<&ManualAssetDiscoveryDetailResponse>,
    quote_currency: CurrencyCode,
    allow_remote_lookup: bool,
) -> Option<ManualAssetPriceRequestKey> {
    if !allow_remote_lookup {
        return None;
    }

    match asset.source {
        ManualAssetSearchSource::CoinGeckoCatalog => {
            let detail = detail?;
            let selected_coingecko_id = asset.coingecko_id.as_deref()?;
            if selected_coingecko_id != detail.coingecko_id {
                return None;
            }
            Some(ManualAssetPriceRequestKey {
                asset_id: detail.coingecko_id.clone(),
                coingecko_id: detail.coingecko_id.clone(),
                quote_currency,
                allow_remote_lookup,
            })
        }
        ManualAssetSearchSource::BitGarthCatalog => Some(ManualAssetPriceRequestKey {
            asset_id: asset.asset_instance_id.clone()?.asset_id,
            coingecko_id: asset.coingecko_id.clone()?,
            quote_currency,
            allow_remote_lookup,
        }),
    }
}

fn current_detail_for_selected_asset<'a>(
    selected: Option<&ManualAssetInstanceSearchRow>,
    detail: &'a ManualAssetDiscoveryDetailResponse,
) -> Option<&'a ManualAssetDiscoveryDetailResponse> {
    let asset = selected?;
    if asset.source != ManualAssetSearchSource::CoinGeckoCatalog {
        return None;
    }
    if asset.coingecko_id.as_deref()? != detail.coingecko_id {
        return None;
    }
    Some(detail)
}

#[derive(Clone, PartialEq)]
enum DetailPriceSlot {
    ConsentNeeded,
    Loading,
    Value { amount: String, code: String },
    Unavailable,
}

/// Shared asset detail panel used by both the BitGarth-catalog and CoinGecko
/// add flows. It renders network, unit code, decimals and current price with a
/// single layout; the parent decides which fields are editable and how price is
/// sourced.
#[component]
fn ManualAssetDetailPanel(
    platforms: Vec<ManualAssetDiscoveryPlatformRow>,
    single_network_name: String,
    default_decimal_precision: u8,
    selected_platform_id: Signal<Option<String>>,
    precision_input: Signal<String>,
    field_error: Signal<Option<String>>,
    unit_code: String,
    decimals_editable: bool,
    decimals_value: u8,
    price: DetailPriceSlot,
    on_price_consent: EventHandler<()>,
) -> Element {
    let mut selected_platform_id = selected_platform_id;
    let mut precision_input = precision_input;
    let mut field_error = field_error;

    let network_is_dropdown = platforms.len() > 1;
    let single_network = if platforms.len() == 1 {
        platforms[0].network_name.clone()
    } else {
        single_network_name
    };
    let selected_value = selected_platform_id()
        .or_else(|| {
            platforms
                .first()
                .map(|platform| platform.provider_platform_id.clone())
        })
        .unwrap_or_default();

    rsx! {
        div { class: "manual-asset-detail",
            div { class: "manual-asset-row",
                span { class: "manual-asset-row-label", "Network" }
                if network_is_dropdown {
                    select {
                        class: "selector manual-asset-row-control",
                        value: "{selected_value}",
                        onchange: {
                            let platforms = platforms.clone();
                            move |event: Event<FormData>| {
                                let value = event.value();
                                selected_platform_id.set(Some(value.clone()));
                                if let Some(platform) = platforms
                                    .iter()
                                    .find(|platform| platform.provider_platform_id == value)
                                {
                                    precision_input.set(
                                        platform
                                            .suggested_decimal_precision
                                            .unwrap_or(default_decimal_precision)
                                            .to_string(),
                                    );
                                }
                                field_error.set(None);
                            }
                        },
                        for platform in platforms.iter() {
                            option {
                                value: "{platform.provider_platform_id}",
                                "{platform.network_name}"
                            }
                        }
                    }
                } else {
                    span { class: "manual-asset-row-value", "{single_network}" }
                }
            }

            div { class: "manual-asset-row",
                span { class: "manual-asset-row-label", "Unit Code" }
                span { class: "manual-asset-row-value is-mono", "{unit_code}" }
            }

            div { class: "manual-asset-row manual-asset-row-stacked",
                div { class: "manual-asset-row-line",
                    span { class: "manual-asset-row-label", "Decimals" }
                    if decimals_editable {
                        input {
                            class: "manual-asset-input manual-asset-plain-input manual-asset-row-control is-narrow",
                            r#type: "number",
                            min: "0",
                            max: "18",
                            step: "1",
                            value: "{precision_input}",
                            oninput: move |e| {
                                precision_input.set(e.value());
                                field_error.set(None);
                            },
                        }
                    } else {
                        span { class: "manual-asset-row-value is-mono", "{decimals_value}" }
                    }
                }
                p { class: "manual-asset-field-hint",
                    "The number of digits this asset can have after the decimal point."
                }
            }

            {
                match price {
                    DetailPriceSlot::ConsentNeeded => rsx! {
                        div { class: "manual-asset-row",
                            span { class: "manual-asset-row-label", "Current price" }
                            button {
                                class: "btn btn-secondary btn-inline",
                                r#type: "button",
                                onclick: move |_| on_price_consent.call(()),
                                "Look up price"
                            }
                        }
                    },
                    DetailPriceSlot::Loading => rsx! {
                        div { class: "manual-asset-row",
                            span { class: "manual-asset-row-label", "Current price" }
                            span { class: "muted", "Loading…" }
                        }
                    },
                    DetailPriceSlot::Value { amount, code } => rsx! {
                        div { class: "manual-asset-row",
                            span { class: "manual-asset-row-label", "Current price" }
                            span { class: "manual-asset-row-value is-mono", "{amount} {code}" }
                        }
                    },
                    DetailPriceSlot::Unavailable => rsx! {
                        div { class: "manual-asset-row",
                            span { class: "manual-asset-row-label", "Current price" }
                            span { class: "muted", "Unavailable" }
                        }
                    },
                }
            }
        }
    }
}

fn selected_asset_context(asset: &ManualAssetInstanceSearchRow) -> String {
    asset
        .platform_hint
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| asset.network_name.clone())
}

fn search_source_label(asset: &ManualAssetInstanceSearchRow) -> &'static str {
    match asset.source {
        ManualAssetSearchSource::BitGarthCatalog => "Catalog",
        ManualAssetSearchSource::CoinGeckoCatalog => "CoinGecko",
    }
}

pub(crate) fn route_for_added_manual_asset(
    account_id: crate::wallets::WalletAccountId,
) -> crate::Route {
    crate::Route::AccountTransactions {
        account_id,
        start: None,
        end: None,
    }
}

fn selected_platform_value(
    detail: &ManualAssetDiscoveryDetailResponse,
    selected_platform_id: Option<String>,
) -> String {
    selected_platform_id
        .or_else(|| {
            detail
                .platforms
                .first()
                .map(|platform| platform.provider_platform_id.clone())
        })
        .unwrap_or_default()
}

fn selected_platform(
    detail: &ManualAssetDiscoveryDetailResponse,
    selected_platform_id: Option<String>,
) -> Option<&ManualAssetDiscoveryPlatformRow> {
    let selected = selected_platform_value(detail, selected_platform_id);
    detail
        .platforms
        .iter()
        .find(|platform| platform.provider_platform_id == selected)
        .or_else(|| detail.platforms.first())
}

fn prefill_coingecko_detail(
    detail: &ManualAssetDiscoveryDetailResponse,
    mut detail_prefill_coingecko_id: Signal<Option<String>>,
    mut selected_platform_id: Signal<Option<String>>,
    mut unit_code_input: Signal<String>,
    mut precision_input: Signal<String>,
    mut native_network_name_input: Signal<String>,
) {
    detail_prefill_coingecko_id.set(Some(detail.coingecko_id.clone()));
    selected_platform_id.set(
        detail
            .platforms
            .first()
            .map(|platform| platform.provider_platform_id.clone()),
    );
    unit_code_input.set(detail.suggested_unit_code.clone().unwrap_or_default());
    precision_input.set(
        detail
            .platforms
            .first()
            .and_then(|platform| platform.suggested_decimal_precision)
            .unwrap_or(detail.default_decimal_precision)
            .to_string(),
    );
    native_network_name_input.set("Native".to_string());
}

fn build_coingecko_snapshot(
    detail: &ManualAssetDiscoveryDetailResponse,
    selected_platform_id: Option<String>,
    unit_code_input: String,
    precision_input: String,
    native_network_name_input: String,
) -> Result<CoinGeckoManualAssetSnapshotRequest, String> {
    let unit_code = unit_code_input.trim().to_ascii_uppercase();
    if unit_code.is_empty() {
        return Err("Enter a unit code for this manual asset.".to_string());
    }

    let decimal_precision = precision_input
        .trim()
        .parse::<i64>()
        .map_err(|_| "Decimals must be a whole number from 0 to 18.".to_string())?;
    if !(0..=18).contains(&decimal_precision) {
        return Err("Decimals must be a whole number from 0 to 18.".to_string());
    }

    let platform = selected_platform(detail, selected_platform_id);
    let network_name = platform
        .map(|platform| platform.network_name.clone())
        .unwrap_or_else(|| {
            let trimmed = native_network_name_input.trim();
            if trimmed.is_empty() {
                "Native".to_string()
            } else {
                trimmed.to_string()
            }
        });
    let network_id = platform
        .map(|platform| platform.network_id.clone())
        .unwrap_or_else(|| "native".to_string());
    let precision_source = match platform.and_then(|platform| platform.suggested_decimal_precision)
    {
        Some(suggested) if decimal_precision == i64::from(suggested) => {
            CoinGeckoManualAssetPrecisionSourceRequest::CoingeckoPlatform
        }
        Some(_) => CoinGeckoManualAssetPrecisionSourceRequest::UserOverride,
        None if decimal_precision == i64::from(detail.default_decimal_precision) => {
            CoinGeckoManualAssetPrecisionSourceRequest::UserDefault
        }
        None => CoinGeckoManualAssetPrecisionSourceRequest::UserOverride,
    };

    Ok(CoinGeckoManualAssetSnapshotRequest {
        asset_id: detail.coingecko_id.clone(),
        network_id,
        decimal_precision,
        unit_code,
        symbol: if detail.symbol.trim().is_empty() {
            None
        } else {
            Some(detail.symbol.trim().to_string())
        },
        asset_name: detail.name.clone(),
        network_name,
        coingecko_id: detail.coingecko_id.clone(),
        coingecko_platform_id: platform.map(|platform| platform.provider_platform_id.clone()),
        provider_platform_asset_ref: platform
            .and_then(|platform| platform.contract_address.clone()),
        precision_source,
    })
}

/// Renders the search label: a live asset count when idle, and match feedback
/// while a query is active. `total` is the placeholder grand total (`None` while
/// loading); `match_total` is the true match count (`None` while the search is
/// pending). When `offer_coingecko` is set, the idle label appends a nudge to
/// enable CoinGecko for a far larger catalog.
fn search_label_text(
    query: &str,
    total: Option<usize>,
    match_total: Option<usize>,
    offer_coingecko: bool,
) -> String {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return match total {
            Some(count) => {
                let mut label = format!(
                    "Search {} {}",
                    group_thousands(count),
                    pluralize(count, "asset")
                );
                if offer_coingecko {
                    label.push_str(". Enable CoinGecko to find more than 17,000 assets.");
                }
                label
            }
            None => "Search assets".to_string(),
        };
    }
    match match_total {
        Some(count) => format!(
            "{} matching {} for {trimmed}",
            group_thousands(count),
            pluralize(count, "asset")
        ),
        None => format!("Searching \u{201c}{trimmed}\u{201d}\u{2026}"),
    }
}

fn pluralize(count: usize, noun: &str) -> String {
    if count == 1 {
        noun.to_string()
    } else {
        format!("{noun}s")
    }
}

/// Formats a count with comma thousands separators (e.g. 15212 -> "15,212").
fn group_thousands(value: usize) -> String {
    let digits = value.to_string();
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Route;

    #[test]
    fn search_label_text_idle_states() {
        assert_eq!(search_label_text("", None, None, false), "Search assets");
        assert_eq!(
            search_label_text("", Some(0), None, false),
            "Search 0 assets"
        );
        assert_eq!(
            search_label_text("", Some(1), None, false),
            "Search 1 asset"
        );
        assert_eq!(
            search_label_text("", Some(42), None, false),
            "Search 42 assets"
        );
        assert_eq!(
            search_label_text("", Some(15212), None, false),
            "Search 15,212 assets"
        );
        assert_eq!(
            search_label_text("", Some(1_000_000), None, false),
            "Search 1,000,000 assets"
        );
    }

    #[test]
    fn search_label_text_offers_coingecko_when_catalog_empty() {
        assert_eq!(
            search_label_text("", Some(20), None, true),
            "Search 20 assets. Enable CoinGecko to find more than 17,000 assets."
        );
        assert_eq!(
            search_label_text("", Some(1), None, true),
            "Search 1 asset. Enable CoinGecko to find more than 17,000 assets."
        );
        // Unknown total (still loading): no nudge appended.
        assert_eq!(search_label_text("", None, None, true), "Search assets");
        // Active query: nudge is idle-only.
        assert_eq!(
            search_label_text("btc", None, Some(1), true),
            "1 matching asset for btc"
        );
    }

    #[test]
    fn search_label_text_query_states() {
        // Pending search (no match total yet).
        assert_eq!(
            search_label_text("btc", None, None, false),
            "Searching \u{201c}btc\u{201d}\u{2026}"
        );
        // Trims the echoed query.
        assert_eq!(
            search_label_text("  btc ", None, None, false),
            "Searching \u{201c}btc\u{201d}\u{2026}"
        );
        // Resolved match totals, singular/plural + grouping.
        assert_eq!(
            search_label_text("btc", None, Some(1), false),
            "1 matching asset for btc"
        );
        assert_eq!(
            search_label_text("btc", None, Some(3), false),
            "3 matching assets for btc"
        );
        assert_eq!(
            search_label_text("btc", None, Some(0), false),
            "0 matching assets for btc"
        );
        assert_eq!(
            search_label_text("a", None, Some(1234), false),
            "1,234 matching assets for a"
        );
    }

    fn coingecko_search_row(coingecko_id: &str) -> ManualAssetInstanceSearchRow {
        ManualAssetInstanceSearchRow {
            source: ManualAssetSearchSource::CoinGeckoCatalog,
            asset_instance_id: None,
            coingecko_id: Some(coingecko_id.to_string()),
            unit_code: "GGG".to_string(),
            asset_name: "Good Games Guild".to_string(),
            network_name: "CoinGecko".to_string(),
            decimal_precision: None,
            platform_count: Some(2),
            platform_hint: Some("2 platforms".to_string()),
        }
    }

    fn bitgarth_search_row() -> ManualAssetInstanceSearchRow {
        ManualAssetInstanceSearchRow {
            source: ManualAssetSearchSource::BitGarthCatalog,
            asset_instance_id: Some(crate::asset_views::ManualAssetInstanceIdView {
                asset_id: "cardano".to_string(),
                network_id: "cardano-mainnet".to_string(),
            }),
            coingecko_id: Some("cardano".to_string()),
            unit_code: "ADA".to_string(),
            asset_name: "Cardano".to_string(),
            network_name: "Cardano".to_string(),
            decimal_precision: Some(6),
            platform_count: None,
            platform_hint: None,
        }
    }

    fn detail_response(coingecko_id: &str) -> ManualAssetDiscoveryDetailResponse {
        ManualAssetDiscoveryDetailResponse {
            coingecko_id: coingecko_id.to_string(),
            name: "Good Games Guild".to_string(),
            symbol: "ggg".to_string(),
            suggested_unit_code: Some("GGG".to_string()),
            default_decimal_precision: 6,
            platforms: Vec::new(),
        }
    }

    #[test]
    fn manual_asset_detail_request_key_requires_coingecko_asset_and_permission() {
        let asset = coingecko_search_row("good-games-guild");
        assert_eq!(
            manual_asset_detail_request_key(Some(&asset), true),
            Some(ManualAssetDetailRequestKey {
                coingecko_id: "good-games-guild".to_string(),
                allow_remote_lookup: true,
            })
        );
        assert_eq!(manual_asset_detail_request_key(Some(&asset), false), None);
        assert_eq!(
            manual_asset_detail_request_key(Some(&bitgarth_search_row()), true),
            None
        );
        assert_eq!(manual_asset_detail_request_key(None, true), None);
    }

    #[test]
    fn price_request_key_uses_only_remote_price_inputs() {
        let usd = crate::models::CurrencyCode::from_code("USD").expect("USD should parse");
        let eur = crate::models::CurrencyCode::from_code("EUR").expect("EUR should parse");
        let coingecko_asset = coingecko_search_row("good-games-guild");
        let detail = detail_response("good-games-guild");
        assert_eq!(
            manual_asset_price_request_key(&coingecko_asset, Some(&detail), usd, true,),
            Some(ManualAssetPriceRequestKey {
                asset_id: "good-games-guild".to_string(),
                coingecko_id: "good-games-guild".to_string(),
                quote_currency: usd,
                allow_remote_lookup: true,
            })
        );

        let catalog_asset = bitgarth_search_row();
        assert_eq!(
            manual_asset_price_request_key(&catalog_asset, None, eur, true,),
            Some(ManualAssetPriceRequestKey {
                asset_id: "cardano".to_string(),
                coingecko_id: "cardano".to_string(),
                quote_currency: eur,
                allow_remote_lookup: true,
            })
        );
    }

    #[test]
    fn price_request_key_requires_permission_and_detail_when_needed() {
        let usd = crate::models::CurrencyCode::from_code("USD").expect("USD should parse");
        let coingecko_asset = coingecko_search_row("good-games-guild");
        assert_eq!(
            manual_asset_price_request_key(&coingecko_asset, None, usd, true,),
            None
        );
        assert_eq!(
            manual_asset_price_request_key(
                &coingecko_asset,
                Some(&detail_response("good-games-guild")),
                usd,
                false,
            ),
            None
        );
        assert_eq!(
            manual_asset_price_request_key(&bitgarth_search_row(), None, usd, false,),
            None
        );
        assert_eq!(
            manual_asset_price_request_key(
                &coingecko_asset,
                Some(&detail_response("different-asset")),
                usd,
                true,
            ),
            None
        );
    }

    #[test]
    fn current_detail_for_selected_asset_rejects_mismatched_or_catalog_asset() {
        let coingecko_asset = coingecko_search_row("good-games-guild");
        let matching_detail = detail_response("good-games-guild");
        let other_detail = detail_response("different-asset");

        assert_eq!(
            current_detail_for_selected_asset(Some(&coingecko_asset), &matching_detail)
                .map(|detail| detail.coingecko_id.as_str()),
            Some("good-games-guild")
        );
        assert_eq!(
            current_detail_for_selected_asset(Some(&coingecko_asset), &other_detail),
            None
        );
        assert_eq!(
            current_detail_for_selected_asset(Some(&bitgarth_search_row()), &matching_detail),
            None
        );
        assert_eq!(
            current_detail_for_selected_asset(None, &matching_detail),
            None
        );
    }

    #[test]
    fn added_manual_asset_route_opens_account_transactions_without_date_filters() {
        let account_id = crate::wallets::WalletAccountId::new();

        let route = route_for_added_manual_asset(account_id);

        match route {
            Route::AccountTransactions {
                account_id: route_account_id,
                start: None,
                end: None,
            } => assert_eq!(route_account_id, account_id),
            _ => panic!("expected account transactions route"),
        }
    }
}
