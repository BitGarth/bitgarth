use crate::backend::{
    UpdateStatus, change_password, get_settings, refresh_update_status, save_coingecko_api_key,
    save_currency, save_date_time_format, save_etherscan_api_key, save_etherscan_base_url,
    save_mempool_base_url, save_number_format, save_timezone, set_price_fetching_enabled,
    set_update_check_enabled, update_status,
};
use crate::hooks::use_session_guard;
use crate::models::{
    CurrencyCode, DateTimeFormat, EtherscanBaseUrlSource, FieldErrors, MempoolBaseUrlSource,
    NumberFormat, PASSWORD_MIN_LENGTH, RawEtherscanApiKey, RawEtherscanBaseUrl, RawMempoolBaseUrl,
    RawPlaintextPassword, SimpleApiKey, UserTimezone, resolve_effective_etherscan_base_url,
    resolve_effective_mempool_base_url,
};
use crate::settings::{SettingsState, common_currencies, defaults_for_locale};
use crate::timezone::format_timestamp;
use crate::version;
use crate::{AuthState, AuthStatus, BannerMessage, BannerState};
use chrono::{DateTime, Utc};
use chrono_tz::TZ_VARIANTS;
use dioxus::logger::tracing;
use dioxus::prelude::*;

use super::form_helpers::{
    begin_submit, field_errors_or_empty, finish_submit, is_form_field_error,
    primary_field_or_fallback,
};
use super::{
    ExternalLinkIcon, HostedRetentionNotice, PairedClients, PasswordInput,
    UpdateAwarenessRefreshState, UserIdenticon,
};
use crate::components::nav::build_identicon;

/// Resolve the initially-active settings tab from the optional `section` query.
/// Only known section ids select a tab; anything else falls back to "regional".
fn initial_active_section(section: Option<&str>) -> String {
    match section {
        Some("digital-assets") => "digital-assets".to_string(),
        // `security` is the legacy key; keep accepting it for old links.
        Some("account") | Some("security") => "account".to_string(),
        Some("system-info") => "system-info".to_string(),
        Some("regional") => "regional".to_string(),
        _ => "regional".to_string(),
    }
}

/// CSS class + label for the Etherscan API key indicator badge.
fn etherscan_key_badge(present: bool) -> (&'static str, &'static str) {
    if present {
        ("etherscan-key-badge is-set", "Key set")
    } else {
        ("etherscan-key-badge is-unset", "No key set")
    }
}

fn coingecko_key_badge(present: bool) -> (&'static str, &'static str) {
    if present {
        ("etherscan-key-badge is-set", "Key set")
    } else {
        ("etherscan-key-badge is-unset", "No key set")
    }
}

const COINGECKO_KEY_BLANK_SAVE_MESSAGE: &str = "Enter a key or use Clear API Key.";

fn coingecko_key_save_payload(candidate: String) -> Result<SimpleApiKey, &'static str> {
    SimpleApiKey::new(candidate).ok_or(COINGECKO_KEY_BLANK_SAVE_MESSAGE)
}

fn price_fetching_status_text(enabled: bool) -> String {
    if enabled {
        "CoinGecko price fetching enabled.".to_string()
    } else {
        "CoinGecko price fetching disabled.".to_string()
    }
}

fn update_check_status_text(enabled: bool) -> String {
    if enabled {
        "Automatic update checks enabled.".to_string()
    } else {
        "Automatic update checks disabled.".to_string()
    }
}

fn update_time_label(raw: Option<&str>, timezone: UserTimezone, format: DateTimeFormat) -> String {
    raw.and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| format_timestamp(&value.with_timezone(&Utc), timezone.into(), format))
        .unwrap_or_else(|| "Not checked yet".to_string())
}

#[component]
pub fn Settings(section: Option<String>) -> Element {
    let mut auth_state = use_context::<AuthState>();
    let mut banner_state = use_context::<BannerState>();
    let settings_state = use_context::<SettingsState>();
    let guard = use_session_guard();
    let update_refresh_state = try_consume_context::<UpdateAwarenessRefreshState>();
    let mut settings_synced = use_signal(|| false);
    let settings_state_for_sync = settings_state.clone();
    let mut active_section = use_signal(|| initial_active_section(section.as_deref()));
    let price_fetching_saving = use_signal(|| false);
    let mut price_fetching_status = use_signal(|| None::<String>);
    let mut price_fetching_error = use_signal(|| None::<String>);
    let mut price_fetching_status_seen = use_signal(|| None::<bool>);
    let mut mempool_base_url_input = use_signal(String::new);
    let mut mempool_base_url_error = use_signal(|| None::<String>);
    let mut mempool_base_url_status = use_signal(|| None::<String>);
    let mempool_base_url_saving = use_signal(|| false);
    let mut etherscan_base_url_input = use_signal(String::new);
    let mut etherscan_base_url_error = use_signal(|| None::<String>);
    let mut etherscan_base_url_status = use_signal(|| None::<String>);
    let etherscan_base_url_saving = use_signal(|| false);
    let mut etherscan_api_key_input = use_signal(String::new);
    let mut etherscan_api_key_error = use_signal(|| None::<String>);
    let mut etherscan_api_key_status = use_signal(|| None::<String>);
    let etherscan_api_key_saving = use_signal(|| false);
    let mut etherscan_key_present = use_signal(|| false);
    let mut coingecko_api_key_input = use_signal(String::new);
    let mut coingecko_api_key_error = use_signal(|| None::<String>);
    let mut coingecko_api_key_status = use_signal(|| None::<String>);
    let coingecko_api_key_saving = use_signal(|| false);
    let mut coingecko_key_configured = use_signal(|| false);
    let mut regional_save_tick = use_signal(|| 0u32);
    let mut change_pw_old = use_signal(String::new);
    let mut change_pw_new = use_signal(String::new);
    let mut change_pw_confirm = use_signal(String::new);
    let mut change_pw_field_errors = use_signal(FieldErrors::new);
    let change_pw_saving = use_signal(|| false);
    let mut change_pw_success = use_signal(|| None::<String>);
    let update_check_saving = use_signal(|| false);
    let mut update_check_status = use_signal(|| None::<String>);
    let mut update_check_error = use_signal(|| None::<String>);
    let mut update_check_enabled_override = use_signal(|| None::<bool>);

    let mut date_time_format = settings_state.date_time_format;
    let mut number_format = settings_state.number_format;
    let mut currency = settings_state.currency;
    let mut timezone = settings_state.timezone;
    let mut mempool_base_url = settings_state.mempool_base_url;
    let mut etherscan_base_url = settings_state.etherscan_base_url;
    let mut price_fetching_enabled = settings_state.price_fetching_enabled;
    let currency_options = common_currencies();

    use_effect(move || {
        let current = price_fetching_enabled();
        let previous_status_value = *price_fetching_status_seen.peek();
        match previous_status_value {
            None => {
                price_fetching_status_seen.set(Some(current));
            }
            Some(previous) if previous != current => {
                price_fetching_status_seen.set(Some(current));
                price_fetching_error.set(None);
                price_fetching_status.set(Some(price_fetching_status_text(current)));
            }
            Some(_) => {}
        }
    });

    let settings_resource = use_server_future(move || async move { get_settings().await })?;
    // Initial status only. Re-checks publish their result through
    // `update_refresh_state` instead of restarting this resource, because a
    // restart re-suspends this mounted page and rebuilds its subtree.
    let update_status_resource = use_server_future(move || async move { update_status().await })?;
    let settings_value = settings_resource.value();
    let update_status_result = update_status_resource();

    // Sync settings/auth state once the server future resolves.
    let value = settings_value.read().clone();
    match value {
        Some(Ok(settings)) if !*settings_synced.peek() => {
            let current_language = *settings_state_for_sync.language.peek();
            let current_timezone = *settings_state_for_sync.timezone.peek();
            let defaults = defaults_for_locale(current_language, current_timezone.into());
            price_fetching_status_seen.set(Some(settings.price_fetching_enabled));
            settings_state_for_sync.apply_user_settings_with_defaults(&settings, &defaults);
            mempool_base_url_input.set(
                settings
                    .mempool_base_url
                    .as_ref()
                    .map(|value| value.as_str().to_string())
                    .unwrap_or_default(),
            );
            mempool_base_url_error.set(None);
            mempool_base_url_status.set(None);
            etherscan_base_url_input.set(
                settings
                    .etherscan_base_url
                    .as_ref()
                    .map(|value| value.as_str().to_string())
                    .unwrap_or_default(),
            );
            etherscan_base_url_error.set(None);
            etherscan_base_url_status.set(None);
            etherscan_api_key_input.set(String::new());
            etherscan_api_key_error.set(None);
            etherscan_api_key_status.set(None);
            etherscan_key_present.set(settings.has_etherscan_api_key);
            coingecko_api_key_input.set(String::new());
            coingecko_api_key_error.set(None);
            coingecko_api_key_status.set(None);
            coingecko_key_configured.set(settings.has_coingecko_api_key);
            settings_synced.set(true);
            tracing::debug!("settings ui: settings fetch succeeded");
        }
        Some(Err(err)) if err.is_unauthorized() && !*settings_synced.peek() => {
            let (was_authenticated, already_unauthenticated, user_id) = {
                let auth_snapshot = auth_state.read();
                let user_id = match &*auth_snapshot {
                    AuthStatus::Authenticated(auth) => Some(auth.user.user_id),
                    _ => None,
                };
                (
                    matches!(&*auth_snapshot, AuthStatus::Authenticated(_)),
                    matches!(&*auth_snapshot, AuthStatus::Unauthenticated),
                    user_id,
                )
            };

            tracing::debug!(
                user_id = ?user_id.map(|id| id.as_ulid().to_string()),
                "settings ui: session expired while fetching settings"
            );

            if !already_unauthenticated {
                auth_state.set(AuthStatus::Unauthenticated);
            }
            if was_authenticated {
                banner_state.set(Some(BannerMessage::SessionExpired));
            }
            settings_synced.set(true);
        }
        Some(Err(err)) if !*settings_synced.peek() => {
            tracing::debug!(
                error = %err,
                "settings ui: settings fetch failed"
            );
            settings_synced.set(true);
        }
        _ => {}
    }

    let settings_fetch_unauthorized = {
        let value = settings_value.read();
        matches!(&*value, Some(Err(err)) if err.is_unauthorized())
    };
    let (is_logged_in, is_auth_unknown, user_info) = {
        let auth_snapshot = auth_state.read();
        match &*auth_snapshot {
            AuthStatus::Authenticated(auth) if !settings_fetch_unauthorized => {
                let tz = timezone().into();
                let format = date_time_format();
                let created_at = format_timestamp(&auth.user.created_at, tz, format);
                (
                    true,
                    false,
                    Some((auth.user.user_id, auth.user.username.clone(), created_at)),
                )
            }
            AuthStatus::Authenticated(_) => (false, false, None),
            AuthStatus::Unknown => (false, true, None),
            AuthStatus::Unauthenticated => (false, false, None),
        }
    };
    let configured_mempool_base_url = mempool_base_url();
    let effective_mempool_base_url =
        resolve_effective_mempool_base_url(configured_mempool_base_url.as_ref());
    let effective_mempool_base_url_label =
        effective_mempool_base_url
            .as_ref()
            .ok()
            .map(|(effective, source)| {
                if *source == MempoolBaseUrlSource::UserOverride {
                    format!("Current explorer base URL: {}", effective.as_str())
                } else {
                    format!(
                        "Current explorer base URL: {} (default)",
                        effective.as_str()
                    )
                }
            });
    let effective_mempool_base_url_error = effective_mempool_base_url
        .as_ref()
        .err()
        .map(|err| err.to_string());
    let configured_etherscan_base_url = etherscan_base_url();
    let effective_etherscan_base_url =
        resolve_effective_etherscan_base_url(configured_etherscan_base_url.as_ref());
    let effective_etherscan_base_url_label =
        effective_etherscan_base_url
            .as_ref()
            .ok()
            .map(|(effective, source)| {
                if *source == EtherscanBaseUrlSource::UserOverride {
                    format!("Current Etherscan base URL: {}", effective.as_str())
                } else {
                    format!(
                        "Current Etherscan base URL: {} (default)",
                        effective.as_str()
                    )
                }
            });
    let effective_etherscan_base_url_error = effective_etherscan_base_url
        .as_ref()
        .err()
        .map(|err| err.to_string());
    let update_status_snapshot = update_refresh_state
        .and_then(|latest| latest())
        .or_else(|| {
            update_status_result
                .as_ref()
                .and_then(|result| result.as_ref().ok())
                .cloned()
        });
    let update_checks_enabled = update_check_enabled_override()
        .or_else(|| {
            update_status_snapshot
                .as_ref()
                .map(|status| status.update_check_enabled)
        })
        .unwrap_or(true);
    let update_current_version = update_status_snapshot
        .as_ref()
        .map(|status| status.current.clone())
        .unwrap_or_else(|| version::version().to_string());
    let update_latest_version = update_status_snapshot
        .as_ref()
        .and_then(|status| status.latest.clone());
    let update_latest_label = match update_status_snapshot.as_ref() {
        Some(status) if status.available && update_latest_version.is_some() => "Update available:",
        Some(status) if status.available => "Update available.",
        Some(_) if update_latest_version.is_some() => "Latest seen:",
        Some(_) => "Latest seen: Not checked yet",
        None => "Latest seen: Loading...",
    };
    let update_last_checked_label = update_time_label(
        update_status_snapshot
            .as_ref()
            .and_then(|status| status.last_checked_at.as_deref()),
        timezone(),
        date_time_format(),
    );

    let check_for_updates = move |_| {
        if !begin_submit(update_check_saving) {
            return;
        }

        update_check_status.set(None);
        update_check_error.set(None);

        spawn(async move {
            let result = refresh_update_status(true).await;
            finish_submit(update_check_saving);

            match result {
                Ok(status) => {
                    update_check_enabled_override.set(Some(status.update_check_enabled));
                    update_check_status.set(Some("Update check complete.".to_string()));
                    if let Some(mut refresh_state) = update_refresh_state {
                        refresh_state.set(Some(status));
                    }
                }
                Err(err) if err.is_unauthorized() => {
                    let mut guard = guard;
                    guard.check::<UpdateStatus>(Err(err));
                }
                Err(other) => {
                    update_check_error.set(Some(other.to_string()));
                }
            }
        });
    };

    let toggle_update_checks = move |_| {
        if !begin_submit(update_check_saving) {
            return;
        }

        update_check_status.set(None);
        update_check_error.set(None);
        let next = !update_checks_enabled;
        update_check_enabled_override.set(Some(next));

        spawn(async move {
            let result = set_update_check_enabled(next).await;
            finish_submit(update_check_saving);

            match result {
                Ok(status) => {
                    update_check_enabled_override.set(Some(status.update_check_enabled));
                    update_check_status
                        .set(Some(update_check_status_text(status.update_check_enabled)));
                    if let Some(mut refresh_state) = update_refresh_state {
                        refresh_state.set(Some(status));
                    }
                }
                Err(err) if err.is_unauthorized() => {
                    update_check_enabled_override.set(None);
                    let mut guard = guard;
                    guard.check::<UpdateStatus>(Err(err));
                }
                Err(other) => {
                    update_check_enabled_override.set(None);
                    update_check_error.set(Some(other.to_string()));
                }
            }
        });
    };

    let save_mempool_override = move |_| {
        if !begin_submit(mempool_base_url_saving) {
            return;
        }

        mempool_base_url_error.set(None);
        mempool_base_url_status.set(None);
        let candidate = mempool_base_url_input();

        spawn(async move {
            let payload = if candidate.trim().is_empty() {
                None
            } else {
                Some(RawMempoolBaseUrl::new(candidate))
            };
            let result = save_mempool_base_url(payload).await;
            finish_submit(mempool_base_url_saving);

            match result {
                Ok(saved) => {
                    mempool_base_url.set(saved.clone());
                    mempool_base_url_input.set(
                        saved
                            .as_ref()
                            .map(|value| value.as_str().to_string())
                            .unwrap_or_default(),
                    );
                    if saved.is_some() {
                        mempool_base_url_status.set(Some("Custom mempool URL saved.".to_string()));
                    } else {
                        mempool_base_url_status
                            .set(Some("Using default https://mempool.space.".to_string()));
                    }
                }
                Err(err) if err.is_unauthorized() => {
                    let mut guard = guard;
                    guard.check::<Option<RawMempoolBaseUrl>>(Err(err));
                }
                Err(err) if is_form_field_error(&err) => {
                    let message = primary_field_or_fallback(
                        &err,
                        &["mempool_base_url"],
                        "Invalid mempool URL override.",
                    );
                    mempool_base_url_error.set(Some(message));
                }
                Err(other) => {
                    mempool_base_url_error.set(Some(other.to_string()));
                }
            }
        });
    };

    let clear_mempool_override = move |_| {
        if !begin_submit(mempool_base_url_saving) {
            return;
        }

        mempool_base_url_error.set(None);
        mempool_base_url_status.set(None);

        spawn(async move {
            let result = save_mempool_base_url(None).await;
            finish_submit(mempool_base_url_saving);

            match result {
                Ok(saved) => {
                    mempool_base_url.set(saved);
                    mempool_base_url_input.set(String::new());
                    mempool_base_url_status
                        .set(Some("Using default https://mempool.space.".to_string()));
                }
                Err(err) if err.is_unauthorized() => {
                    let mut guard = guard;
                    guard.check::<Option<RawMempoolBaseUrl>>(Err(err));
                }
                Err(err) if is_form_field_error(&err) => {
                    let message = primary_field_or_fallback(
                        &err,
                        &["mempool_base_url"],
                        "Invalid mempool URL override.",
                    );
                    mempool_base_url_error.set(Some(message));
                }
                Err(other) => {
                    mempool_base_url_error.set(Some(other.to_string()));
                }
            }
        });
    };

    let save_etherscan_override = move |_| {
        if !begin_submit(etherscan_base_url_saving) {
            return;
        }

        etherscan_base_url_error.set(None);
        etherscan_base_url_status.set(None);
        let candidate = etherscan_base_url_input();

        spawn(async move {
            let payload = if candidate.trim().is_empty() {
                None
            } else {
                Some(RawEtherscanBaseUrl::new(candidate))
            };
            let result = save_etherscan_base_url(payload).await;
            finish_submit(etherscan_base_url_saving);

            match result {
                Ok(saved) => {
                    etherscan_base_url.set(saved.clone());
                    etherscan_base_url_input.set(
                        saved
                            .as_ref()
                            .map(|value| value.as_str().to_string())
                            .unwrap_or_default(),
                    );
                    if saved.is_some() {
                        etherscan_base_url_status
                            .set(Some("Custom Etherscan URL saved.".to_string()));
                    } else {
                        etherscan_base_url_status.set(Some(
                            "Using default https://api.etherscan.io/v2/api.".to_string(),
                        ));
                    }
                }
                Err(err) if err.is_unauthorized() => {
                    let mut guard = guard;
                    guard.check::<Option<RawEtherscanBaseUrl>>(Err(err));
                }
                Err(err) if is_form_field_error(&err) => {
                    let message = primary_field_or_fallback(
                        &err,
                        &["etherscan_base_url"],
                        "Invalid Etherscan URL override.",
                    );
                    etherscan_base_url_error.set(Some(message));
                }
                Err(other) => {
                    etherscan_base_url_error.set(Some(other.to_string()));
                }
            }
        });
    };

    let clear_etherscan_override = move |_| {
        if !begin_submit(etherscan_base_url_saving) {
            return;
        }

        etherscan_base_url_error.set(None);
        etherscan_base_url_status.set(None);

        spawn(async move {
            let result = save_etherscan_base_url(None).await;
            finish_submit(etherscan_base_url_saving);

            match result {
                Ok(saved) => {
                    etherscan_base_url.set(saved);
                    etherscan_base_url_input.set(String::new());
                    etherscan_base_url_status.set(Some(
                        "Using default https://api.etherscan.io/v2/api.".to_string(),
                    ));
                }
                Err(err) if err.is_unauthorized() => {
                    let mut guard = guard;
                    guard.check::<Option<RawEtherscanBaseUrl>>(Err(err));
                }
                Err(err) if is_form_field_error(&err) => {
                    let message = primary_field_or_fallback(
                        &err,
                        &["etherscan_base_url"],
                        "Invalid Etherscan URL override.",
                    );
                    etherscan_base_url_error.set(Some(message));
                }
                Err(other) => {
                    etherscan_base_url_error.set(Some(other.to_string()));
                }
            }
        });
    };

    let save_etherscan_key = move |_| {
        if !begin_submit(etherscan_api_key_saving) {
            return;
        }

        etherscan_api_key_error.set(None);
        etherscan_api_key_status.set(None);
        let candidate = etherscan_api_key_input();

        spawn(async move {
            let payload = if candidate.trim().is_empty() {
                None
            } else {
                Some(RawEtherscanApiKey::new(candidate))
            };

            let result = save_etherscan_api_key(payload.clone()).await;
            finish_submit(etherscan_api_key_saving);

            match result {
                Ok(_) => {
                    etherscan_api_key_input.set(String::new());
                    etherscan_key_present.set(payload.is_some());
                    if payload.is_some() {
                        etherscan_api_key_status
                            .set(Some("Etherscan API key saved and validated.".to_string()));
                    } else {
                        etherscan_api_key_status
                            .set(Some("Etherscan API key cleared.".to_string()));
                    }
                }
                Err(err) if err.is_unauthorized() => {
                    let mut guard = guard;
                    guard.check::<()>(Err(err));
                }
                Err(err) if is_form_field_error(&err) => {
                    let message = primary_field_or_fallback(
                        &err,
                        &["etherscan_api_key"],
                        "Invalid Etherscan API key.",
                    );
                    etherscan_api_key_error.set(Some(message));
                }
                Err(other) => {
                    etherscan_api_key_error.set(Some(other.to_string()));
                }
            }
        });
    };

    let clear_etherscan_key = move |_| {
        if !begin_submit(etherscan_api_key_saving) {
            return;
        }

        etherscan_api_key_error.set(None);
        etherscan_api_key_status.set(None);

        spawn(async move {
            let result = save_etherscan_api_key(None).await;
            finish_submit(etherscan_api_key_saving);

            match result {
                Ok(_) => {
                    etherscan_api_key_input.set(String::new());
                    etherscan_key_present.set(false);
                    etherscan_api_key_status.set(Some("Etherscan API key cleared.".to_string()));
                }
                Err(err) if err.is_unauthorized() => {
                    let mut guard = guard;
                    guard.check::<()>(Err(err));
                }
                Err(err) if is_form_field_error(&err) => {
                    let message = primary_field_or_fallback(
                        &err,
                        &["etherscan_api_key"],
                        "Invalid Etherscan API key.",
                    );
                    etherscan_api_key_error.set(Some(message));
                }
                Err(other) => {
                    etherscan_api_key_error.set(Some(other.to_string()));
                }
            }
        });
    };

    let toggle_price_fetching = move |_| {
        if !begin_submit(price_fetching_saving) {
            return;
        }

        price_fetching_status.set(None);
        price_fetching_error.set(None);
        let next = !price_fetching_enabled();

        spawn(async move {
            let result = set_price_fetching_enabled(next).await;
            finish_submit(price_fetching_saving);

            match result {
                Ok(saved) => {
                    price_fetching_enabled.set(saved);
                    price_fetching_status.set(Some(price_fetching_status_text(saved)));
                }
                Err(err) if err.is_unauthorized() => {
                    let mut guard = guard;
                    guard.check::<bool>(Err(err));
                }
                Err(other) => {
                    price_fetching_error.set(Some(other.to_string()));
                }
            }
        });
    };

    let save_coingecko_key = move |_| {
        coingecko_api_key_error.set(None);
        coingecko_api_key_status.set(None);
        let candidate = coingecko_api_key_input();
        let payload = match coingecko_key_save_payload(candidate) {
            Ok(payload) => payload,
            Err(message) => {
                coingecko_api_key_error.set(Some(message.to_string()));
                return;
            }
        };

        if !begin_submit(coingecko_api_key_saving) {
            return;
        }

        spawn(async move {
            let result = save_coingecko_api_key(Some(payload)).await;
            finish_submit(coingecko_api_key_saving);

            match result {
                Ok(_) => {
                    coingecko_api_key_input.set(String::new());
                    coingecko_key_configured.set(true);
                    coingecko_api_key_status.set(Some("CoinGecko Pro API key saved.".to_string()));
                }
                Err(err) if err.is_unauthorized() => {
                    let mut guard = guard;
                    guard.check::<()>(Err(err));
                }
                Err(err) if is_form_field_error(&err) => {
                    let message = primary_field_or_fallback(
                        &err,
                        &["api_key"],
                        "Invalid CoinGecko Pro API key.",
                    );
                    coingecko_api_key_error.set(Some(message));
                }
                Err(other) => {
                    coingecko_api_key_error.set(Some(other.to_string()));
                }
            }
        });
    };

    let clear_coingecko_key = move |_| {
        if !begin_submit(coingecko_api_key_saving) {
            return;
        }

        coingecko_api_key_error.set(None);
        coingecko_api_key_status.set(None);

        spawn(async move {
            let result = save_coingecko_api_key(None).await;
            finish_submit(coingecko_api_key_saving);

            match result {
                Ok(_) => {
                    coingecko_api_key_input.set(String::new());
                    coingecko_key_configured.set(false);
                    coingecko_api_key_status
                        .set(Some("CoinGecko Pro API key cleared.".to_string()));
                }
                Err(err) if err.is_unauthorized() => {
                    let mut guard = guard;
                    guard.check::<()>(Err(err));
                }
                Err(err) if is_form_field_error(&err) => {
                    let message = primary_field_or_fallback(
                        &err,
                        &["api_key"],
                        "Invalid CoinGecko Pro API key.",
                    );
                    coingecko_api_key_error.set(Some(message));
                }
                Err(other) => {
                    coingecko_api_key_error.set(Some(other.to_string()));
                }
            }
        });
    };

    let password_hint_text = format!(
        "At least {} characters with uppercase, lowercase, and a number",
        PASSWORD_MIN_LENGTH
    );
    let t_success = "Password changed successfully.".to_string();

    let handle_change_password = move |evt: Event<FormData>| {
        evt.prevent_default();

        if !begin_submit(change_pw_saving) {
            return;
        }

        let old_value = change_pw_old();
        let new_value = change_pw_new();
        let confirm_value = change_pw_confirm();

        change_pw_field_errors.set(FieldErrors::new());
        change_pw_success.set(None);

        if new_value != confirm_value {
            let mut errors = FieldErrors::new();
            errors.add("confirm_password", "Passwords do not match".to_string());
            change_pw_field_errors.set(errors);
            finish_submit(change_pw_saving);
            return;
        }

        let t_success = t_success.clone();
        spawn(async move {
            let result = change_password(
                RawPlaintextPassword::new(old_value),
                RawPlaintextPassword::new(new_value),
            )
            .await;

            finish_submit(change_pw_saving);

            match result {
                Ok(()) => {
                    change_pw_old.set(String::new());
                    change_pw_new.set(String::new());
                    change_pw_confirm.set(String::new());
                    change_pw_success.set(Some(t_success));
                }
                Err(err) if err.is_unauthorized() => {
                    let mut guard = guard;
                    guard.check::<()>(Err(err));
                }
                Err(err) if is_form_field_error(&err) => {
                    change_pw_field_errors.set(field_errors_or_empty(&err));
                }
                Err(other) => {
                    let mut errors = FieldErrors::new();
                    errors.add("old_password", other.to_string());
                    change_pw_field_errors.set(errors);
                }
            }
        });
    };

    let old_pw_errors = change_pw_field_errors()
        .get("old_password")
        .cloned()
        .unwrap_or_default();
    let has_old_pw_error = !old_pw_errors.is_empty();
    let new_pw_errors = change_pw_field_errors()
        .get("new_password")
        .cloned()
        .unwrap_or_default();
    let has_new_pw_error = !new_pw_errors.is_empty();
    let confirm_pw_errors = change_pw_field_errors()
        .get("confirm_password")
        .cloned()
        .unwrap_or_default();
    let has_confirm_pw_error = !confirm_pw_errors.is_empty();

    rsx! {
        div { class: "page-container",
            if is_logged_in {
                div { class: "page-header",
                    h1 { class: "page-title", "Settings" }
                }

                // Tab navigation
                div { class: "settings-nav",
                    button {
                        class: if active_section() == "regional" { "settings-nav-link active" } else { "settings-nav-link" },
                        onclick: move |_| active_section.set("regional".to_string()),
                        "Regional"
                    }
                    button {
                        class: if active_section() == "account" { "settings-nav-link active" } else { "settings-nav-link" },
                        onclick: move |_| active_section.set("account".to_string()),
                        "Account"
                    }
                    button {
                        class: if active_section() == "digital-assets" { "settings-nav-link active" } else { "settings-nav-link" },
                        onclick: move |_| active_section.set("digital-assets".to_string()),
                        "Digital Assets"
                    }
                    button {
                        class: if active_section() == "system-info" { "settings-nav-link active" } else { "settings-nav-link" },
                        onclick: move |_| active_section.set("system-info".to_string()),
                        "System Info"
                    }
                }

                // Tab content — only the active section is rendered
                if active_section() == "regional" {
                    div { class: "settings-section",
                        div { class: "card",
                            div { class: "card-header",
                                h3 { class: "card-title",
                                    "Regional Preferences"
                                    if regional_save_tick() > 0 {
                                        span {
                                            key: "{regional_save_tick}",
                                            class: "settings-auto-saved",
                                            "Saved"
                                        }
                                    }
                                }
                            }
                            div { class: "card-body",
                                div { class: "form-group",
                                    label { class: "form-label", "Timezone" }
                                    select {
                                        class: "selector",
                                        id: "timezone-selector",
                                        "aria-label": "Select timezone",
                                        value: "{timezone.read().name()}",
                                        onmounted: move |e| async move { let _ = e.set_focus(true).await; },
                                        onchange: move |evt| {
                                            if let Ok(tz) = evt.value().parse::<chrono_tz::Tz>() {
                                                let tz_value = UserTimezone(tz);
                                                timezone.set(tz_value);
                                                if matches!(&*auth_state.read(), AuthStatus::Authenticated(_)) {
                                                    let mut guard = guard;
                                                    spawn(async move {
                                                        guard.check(save_timezone(tz_value).await);
                                                        *regional_save_tick.write() += 1;
                                                    });
                                                }
                                            }
                                        },
                                        for tz in TZ_VARIANTS.iter() {
                                            option {
                                                value: "{UserTimezone(*tz).name()}",
                                                selected: timezone() == UserTimezone(*tz),
                                                "{UserTimezone(*tz).name()}"
                                            }
                                        }
                                    }
                                }
                                div { class: "form-group",
                                    label { class: "form-label", "Date & Time Format" }
                                    select {
                                        class: "selector",
                                        id: "date-time-format-selector",
                                        "aria-label": "Select date time format",
                                        value: "{date_time_format.read().code()}",
                                        onchange: move |evt| {
                                            if let Some(format) = DateTimeFormat::from_code(&evt.value()) {
                                                date_time_format.set(format);
                                                if matches!(&*auth_state.read(), AuthStatus::Authenticated(_)) {
                                                    let mut guard = guard;
                                                    spawn(async move {
                                                        guard.check(save_date_time_format(format).await);
                                                        *regional_save_tick.write() += 1;
                                                    });
                                                }
                                            }
                                        },
                                        for format in [
                                            DateTimeFormat::YearMonthDay24,
                                            DateTimeFormat::DayMonthYear24,
                                            DateTimeFormat::MonthDayYear12,
                                        ] {
                                            option {
                                                value: "{format.code()}",
                                                selected: date_time_format() == format,
                                                "{format.label()}"
                                            }
                                        }
                                    }
                                }
                                div { class: "form-group",
                                    label { class: "form-label", "Number Format" }
                                    select {
                                        class: "selector",
                                        id: "number-format-selector",
                                        "aria-label": "Select number format",
                                        value: "{number_format.read().code()}",
                                        onchange: move |evt| {
                                            if let Some(format) = NumberFormat::from_code(&evt.value()) {
                                                number_format.set(format);
                                                if matches!(&*auth_state.read(), AuthStatus::Authenticated(_)) {
                                                    let mut guard = guard;
                                                    spawn(async move {
                                                        guard.check(save_number_format(format).await);
                                                        *regional_save_tick.write() += 1;
                                                    });
                                                }
                                            }
                                        },
                                        for format in [
                                            NumberFormat::DotComma,
                                            NumberFormat::CommaDot,
                                            NumberFormat::CommaSpace,
                                        ] {
                                            option {
                                                value: "{format.code()}",
                                                selected: number_format() == format,
                                                "{format.label()}"
                                            }
                                        }
                                    }
                                }
                                div { class: "form-group",
                                    label { class: "form-label", "Default Currency" }
                                    select {
                                        class: "selector",
                                        id: "currency-selector",
                                        "aria-label": "Select default currency",
                                        value: "{currency.read().code()}",
                                        onchange: move |evt| {
                                            if let Some(new_currency) = CurrencyCode::from_code(&evt.value()) {
                                                if matches!(&*auth_state.read(), AuthStatus::Authenticated(_)) {
                                                    let mut guard = guard;
                                                    spawn(async move {
                                                        let result = save_currency(new_currency).await;
                                                        if result.is_ok() {
                                                            currency.set(new_currency);
                                                            *regional_save_tick.write() += 1;
                                                        }
                                                        guard.check(result);
                                                    });
                                                } else {
                                                    currency.set(new_currency);
                                                }
                                            }
                                        },
                                        for currency_value in currency_options.iter().copied() {
                                            option {
                                                value: "{currency_value.code()}",
                                                selected: currency() == currency_value,
                                                "{currency_value.label()}"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                if active_section() == "account" {
                    div { class: "settings-section",
                        if let Some((user_id, username, created_at)) = user_info.clone() {
                            div { class: "card",
                                div { class: "card-header",
                                    h3 { class: "card-title", "Account" }
                                }
                                div { class: "card-body account-identity",
                                    div { class: "account-identicon",
                                        UserIdenticon {
                                            icon: build_identicon(&user_id.to_string()),
                                        }
                                    }
                                    div { class: "account-identity-fields",
                                        div { class: "form-group",
                                            label { class: "form-label", "Username" }
                                            p { class: "form-value", "{username}" }
                                        }
                                        div { class: "form-group",
                                            label { class: "form-label", "User ID" }
                                            p { class: "form-value", "{user_id}" }
                                        }
                                        div { class: "form-group",
                                            label { class: "form-label", "Account Created" }
                                            p { class: "form-value", "{created_at}" }
                                        }
                                    }
                                }
                            }
                        }
                        HostedRetentionNotice {}
                        PairedClients {}
                        div { class: "card",
                            div { class: "card-header",
                                h3 { class: "card-title", "Change Password" }
                            }
                            div { class: "card-body",
                                form {
                                    onsubmit: handle_change_password,
                                    div { class: "form-group",
                                        label { class: "form-label", r#for: "change-pw-old",
                                            "Current Password"
                                        }
                                        PasswordInput {
                                            id: "change-pw-old".to_string(),
                                            value: change_pw_old,
                                            placeholder: "Enter your current password".to_string(),
                                            autocomplete: "current-password",
                                            has_error: has_old_pw_error,
                                            autofocus: true,
                                            on_change: move |_| {
                                                change_pw_field_errors.set(FieldErrors::new());
                                                change_pw_success.set(None);
                                            },
                                        }
                                        for err in old_pw_errors.iter() {
                                            p { class: "form-error", "{err}" }
                                        }
                                    }
                                    div { class: "form-group",
                                        label { class: "form-label", r#for: "change-pw-new",
                                            "New Password"
                                        }
                                        PasswordInput {
                                            id: "change-pw-new".to_string(),
                                            value: change_pw_new,
                                            placeholder: "Enter your new password".to_string(),
                                            autocomplete: "new-password",
                                            has_error: has_new_pw_error,
                                            on_change: move |_| {
                                                change_pw_field_errors.set(FieldErrors::new());
                                                change_pw_success.set(None);
                                            },
                                        }
                                        p { class: "form-hint", "{password_hint_text}" }
                                        for err in new_pw_errors.iter() {
                                            p { class: "form-error", "{err}" }
                                        }
                                    }
                                    div { class: "form-group",
                                        label { class: "form-label", r#for: "change-pw-confirm",
                                            "Confirm New Password"
                                        }
                                        PasswordInput {
                                            id: "change-pw-confirm".to_string(),
                                            value: change_pw_confirm,
                                            placeholder: "Confirm your new password".to_string(),
                                            autocomplete: "new-password",
                                            has_error: has_confirm_pw_error,
                                            on_change: move |_| {
                                                change_pw_field_errors.set(FieldErrors::new());
                                                change_pw_success.set(None);
                                            },
                                        }
                                        for err in confirm_pw_errors.iter() {
                                            p { class: "form-error", "{err}" }
                                        }
                                    }
                                    div { class: "form-group",
                                        button {
                                            class: "btn btn-primary",
                                            r#type: "submit",
                                            disabled: change_pw_saving(),
                                            if change_pw_saving() {
                                                "Changing password..."
                                            } else {
                                                "Change Password"
                                            }
                                        }
                                    }
                                    if let Some(success) = change_pw_success() {
                                        p { class: "settings-status-success", "{success}" }
                                    }
                                }
                            }
                        }
                    }
                }

                if active_section() == "digital-assets" {
                    div { class: "settings-section",
                        div { class: "card",
                            div { class: "card-header",
                                h3 { class: "card-title", "Market Prices" }
                            }
                            div { class: "card-body",
                                div { class: "form-group",
                                    label { class: "checkbox",
                                        input {
                                            "data-testid": "price-fetching-toggle",
                                            r#type: "checkbox",
                                            checked: price_fetching_enabled(),
                                            disabled: price_fetching_saving(),
                                            onchange: toggle_price_fetching,
                                        }
                                        " Fetch current prices from CoinGecko"
                                    }
                                    p { class: "form-help-text",
                                        "When enabled, BitGarth requests prices for your assets and selected currency."
                                    }
                                }
                                if let Some(status) = price_fetching_status() {
                                    p { class: "settings-status-success", "{status}" }
                                }
                                if let Some(error) = price_fetching_error() {
                                    p { class: "settings-status-error", "{error}" }
                                }
                                div { class: "form-group",
                                    label { class: "form-label", r#for: "coingecko-api-key",
                                        "CoinGecko Pro API Key"
                                        {
                                            let (badge_class, badge_label) = coingecko_key_badge(coingecko_key_configured());
                                            rsx! {
                                                span {
                                                    class: "{badge_class}",
                                                    "data-testid": "coingecko-key-badge",
                                                    "{badge_label}"
                                                }
                                            }
                                        }
                                    }
                                    PasswordInput {
                                        id: "coingecko-api-key".to_string(),
                                        value: coingecko_api_key_input,
                                        placeholder: "Paste Pro API key".to_string(),
                                        autocomplete: "off",
                                        on_change: move |_| {
                                            coingecko_api_key_error.set(None);
                                            coingecko_api_key_status.set(None);
                                        },
                                    }
                                    p { class: "form-help-text",
                                        "Optional. Used for CoinGecko Pro price requests and never returned after storage."
                                    }
                                }
                                div { class: "form-group",
                                    button {
                                        class: "btn btn-primary",
                                        r#type: "button",
                                        disabled: coingecko_api_key_saving(),
                                        onclick: save_coingecko_key,
                                        "Save API Key"
                                    }
                                    button {
                                        class: "btn btn-secondary",
                                        r#type: "button",
                                        disabled: coingecko_api_key_saving(),
                                        onclick: clear_coingecko_key,
                                        "Clear API Key"
                                    }
                                }
                                if let Some(status) = coingecko_api_key_status() {
                                    p { class: "settings-status-success", "{status}" }
                                }
                                if let Some(error) = coingecko_api_key_error() {
                                    p { class: "settings-status-error", "{error}" }
                                }
                            }
                        }
                    }

                    div { class: "settings-section",
                        div { class: "card",
                            div { class: "card-header",
                                h3 { class: "card-title", "Bitcoin Explorer" }
                            }
                            div { class: "card-body",
                                div { class: "form-group",
                                    label { class: "form-label", "Mempool Base URL" }
                                    input {
                                        class: "form-input",
                                        r#type: "url",
                                        autocomplete: "off",
                                        placeholder: "https://mempool.space",
                                        value: "{mempool_base_url_input}",
                                        oninput: move |evt| {
                                            mempool_base_url_input.set(evt.value());
                                            mempool_base_url_error.set(None);
                                            mempool_base_url_status.set(None);
                                        },
                                    }
                                    p { class: "form-help-text",
                                        "Leave blank to use https://mempool.space. If you configure a URL, BitGarth uses only that URL for transaction sync and address links."
                                    }
                                }
                                div { class: "form-group",
                                    button {
                                        class: "btn btn-primary",
                                        r#type: "button",
                                        disabled: mempool_base_url_saving(),
                                        onclick: save_mempool_override,
                                        "Save Explorer URL"
                                    }
                                    button {
                                        class: "btn btn-secondary",
                                        r#type: "button",
                                        disabled: mempool_base_url_saving(),
                                        onclick: clear_mempool_override,
                                        "Use Default (mempool.space)"
                                    }
                                }
                                if let Some(status) = mempool_base_url_status() {
                                    p { class: "settings-status-success", "{status}" }
                                }
                                if let Some(error) = mempool_base_url_error() {
                                    p { class: "settings-status-error", "{error}" }
                                }
                                if let Some(label) = effective_mempool_base_url_label.clone() {
                                    p { class: "muted", "{label}" }
                                }
                                if let Some(err) = effective_mempool_base_url_error.clone() {
                                    p { class: "settings-status-error", "Current explorer URL is invalid: {err}" }
                                }
                            }
                        }
                    }

                    div { class: "settings-section",
                        div { class: "card",
                            div { class: "card-header",
                                h3 { class: "card-title", "Ethereum API" }
                            }
                            div { class: "card-body",
                                div { class: "form-group",
                                    label { class: "form-label", "Etherscan Base URL" }
                                    input {
                                        class: "form-input",
                                        r#type: "url",
                                        autocomplete: "off",
                                        placeholder: "https://api.etherscan.io/v2/api",
                                        value: "{etherscan_base_url_input}",
                                        oninput: move |evt| {
                                            etherscan_base_url_input.set(evt.value());
                                            etherscan_base_url_error.set(None);
                                            etherscan_base_url_status.set(None);
                                        },
                                    }
                                    p { class: "form-help-text",
                                        "Leave blank to use the default Etherscan API. If you configure a URL, BitGarth uses only that URL for Ethereum transaction sync."
                                    }
                                }
                                div { class: "form-group",
                                    button {
                                        class: "btn btn-primary",
                                        r#type: "button",
                                        disabled: etherscan_base_url_saving(),
                                        onclick: save_etherscan_override,
                                        "Save Etherscan URL"
                                    }
                                    button {
                                        class: "btn btn-secondary",
                                        r#type: "button",
                                        disabled: etherscan_base_url_saving(),
                                        onclick: clear_etherscan_override,
                                        "Use Default (etherscan.io)"
                                    }
                                }
                                if let Some(status) = etherscan_base_url_status() {
                                    p { class: "settings-status-success", "{status}" }
                                }
                                if let Some(error) = etherscan_base_url_error() {
                                    p { class: "settings-status-error", "{error}" }
                                }
                                if let Some(label) = effective_etherscan_base_url_label.clone() {
                                    p { class: "muted", "{label}" }
                                }
                                if let Some(err) = effective_etherscan_base_url_error.clone() {
                                    p { class: "settings-status-error", "Current Etherscan URL is invalid: {err}" }
                                }
                                div { class: "form-group",
                                    label { class: "form-label", r#for: "etherscan-api-key",
                                        "Etherscan API Key"
                                        {
                                            let (badge_class, badge_label) = etherscan_key_badge(etherscan_key_present());
                                            rsx! {
                                                span {
                                                    class: "{badge_class}",
                                                    "data-testid": "etherscan-key-badge",
                                                    "{badge_label}"
                                                }
                                            }
                                        }
                                    }
                                    PasswordInput {
                                        id: "etherscan-api-key".to_string(),
                                        value: etherscan_api_key_input,
                                        placeholder: "Enter Etherscan API key".to_string(),
                                        autocomplete: "off",
                                        on_change: move |_| {
                                            etherscan_api_key_error.set(None);
                                            etherscan_api_key_status.set(None);
                                        },
                                    }
                                    p { class: "form-help-text",
                                        "Required for Ethereum sync. The key is validated on save and is never returned to the UI after storage."
                                    }
                                }
                                div { class: "form-group",
                                    button {
                                        class: "btn btn-primary",
                                        r#type: "button",
                                        disabled: etherscan_api_key_saving(),
                                        onclick: save_etherscan_key,
                                        "Save API Key"
                                    }
                                    button {
                                        class: "btn btn-secondary",
                                        r#type: "button",
                                        disabled: etherscan_api_key_saving(),
                                        onclick: clear_etherscan_key,
                                        "Clear API Key"
                                    }
                                }
                                if let Some(status) = etherscan_api_key_status() {
                                    p { class: "settings-status-success", "{status}" }
                                }
                                if let Some(error) = etherscan_api_key_error() {
                                    p { class: "settings-status-error", "{error}" }
                                }
                            }
                        }
                    }
                }

                if active_section() == "system-info" {
                    div { class: "settings-section",
                        div { class: "card",
                            div { class: "card-header",
                                h3 { class: "card-title", "Application" }
                            }
                            div { class: "card-body",
                                div { class: "form-group",
                                    label { class: "form-label", "Version" }
                                    p { class: "form-value", {version::version()} }
                                }
                            }
                        }
                    }
                    div { class: "settings-section",
                        div { class: "card settings-card",
                            div { class: "card-header",
                                h3 { class: "card-title", "Software updates" }
                            }
                            div { class: "card-body",
                                div { class: "form-group",
                                    label { class: "form-label", "Current version" }
                                    p { class: "form-value",
                                        a {
                                            href: "https://bitgarth.app/releases.html#{update_current_version}",
                                            target: "_blank",
                                            rel: "noopener noreferrer",
                                            title: "Release notes for {update_current_version} (opens in a new tab)",
                                            "{update_current_version}"
                                            ExternalLinkIcon {}
                                        }
                                    }
                                }
                                div { class: "form-group",
                                    label { class: "form-label", "Latest version" }
                                    p { class: "form-value",
                                        if let Some(latest) = update_latest_version {
                                            "{update_latest_label} "
                                            a {
                                                href: "https://bitgarth.app/releases.html#{latest}",
                                                target: "_blank",
                                                rel: "noopener noreferrer",
                                                title: "Release notes for {latest} (opens in a new tab)",
                                                "{latest}"
                                                ExternalLinkIcon {}
                                            }
                                        } else {
                                            "{update_latest_label}"
                                        }
                                    }
                                }
                                div { class: "form-group",
                                    label { class: "form-label", "Last checked" }
                                    p { class: "form-value", "{update_last_checked_label}" }
                                }
                                div { class: "form-group",
                                    label { class: "checkbox",
                                        input {
                                            "data-testid": "update-checks-toggle",
                                            r#type: "checkbox",
                                            checked: update_checks_enabled,
                                            disabled: update_check_saving(),
                                            onchange: toggle_update_checks,
                                        }
                                        " Enable automatic update checks"
                                    }
                                }
                                div { class: "form-group",
                                    button {
                                        class: "btn btn-primary",
                                        r#type: "button",
                                        disabled: update_check_saving(),
                                        onclick: check_for_updates,
                                        if update_check_saving() {
                                            "Checking..."
                                        } else {
                                            "Check now"
                                        }
                                    }
                                }
                                if let Some(status) = update_check_status() {
                                    p { class: "settings-status-success", "{status}" }
                                }
                                if let Some(error) = update_check_error() {
                                    p { class: "settings-status-error", "{error}" }
                                }
                            }
                        }
                    }
                }
            } else {
                div { class: "card",
                    div { class: "card-body",
                        if is_auth_unknown {
                            p { "Checking session..." }
                        } else {
                            p { "Redirecting to login..." }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    #[cfg(feature = "server")]
    use super::*;
    #[cfg(feature = "server")]
    use crate::i18n::Locale;
    #[cfg(feature = "server")]
    use crate::models::SessionDuration;
    #[cfg(feature = "server")]
    use crate::settings::{
        SettingsState, default_currency, default_date_time_format, default_number_format,
    };
    #[cfg(feature = "server")]
    use chrono_tz::Tz;
    #[cfg(feature = "server")]
    use dioxus::prelude::{Element, Router, VirtualDom, rsx, use_context_provider, use_signal};
    #[cfg(feature = "server")]
    use dioxus_history::{History, MemoryHistory};
    #[cfg(feature = "server")]
    use std::rc::Rc;

    #[cfg(feature = "server")]
    fn settings_test_app() -> Element {
        use_context_provider(|| {
            Rc::new(MemoryHistory::with_initial_path("/settings")) as Rc<dyn History>
        });

        let locale = use_signal(Locale::default);
        use_context_provider(|| locale);

        let date_time_format = use_signal(|| default_date_time_format(Locale::English));
        let number_format = use_signal(|| default_number_format(Locale::English));
        let currency = use_signal(|| default_currency(Locale::English));
        let timezone = use_signal(|| UserTimezone::from(Tz::UTC));
        let session_duration = use_signal(SessionDuration::default);
        let mempool_base_url = use_signal(|| None);

        let etherscan_base_url = use_signal(|| None);
        let price_fetching_enabled = use_signal(|| false);
        let has_coingecko_api_key = use_signal(|| false);

        let settings_state = SettingsState {
            language: locale,
            date_time_format,
            number_format,
            currency,
            timezone,
            session_duration,
            mempool_base_url,
            etherscan_base_url,
            price_fetching_enabled,
            has_coingecko_api_key,
        };
        use_context_provider(|| settings_state);

        let auth_state: AuthState = use_signal(|| AuthStatus::Unauthenticated);
        use_context_provider(|| auth_state);

        let banner_state: crate::BannerState = use_signal(|| None);
        use_context_provider(|| banner_state);

        rsx! { Router::<crate::Route> {} }
    }

    #[cfg(feature = "server")]
    #[test]
    fn settings_renders_within_router_when_logged_out() {
        // RequireAuth (the Route layout) handles the redirect now; Settings
        // itself does not mount when unauthenticated. Just verify the test
        // app constructs without panicking.
        let mut dom = VirtualDom::new(settings_test_app);
        dom.rebuild_in_place();
        let _ = dioxus_ssr::render(&dom);
    }

    #[cfg(feature = "server")]
    #[test]
    fn etherscan_key_badge_reflects_presence() {
        assert_eq!(
            etherscan_key_badge(true),
            ("etherscan-key-badge is-set", "Key set"),
        );
        assert_eq!(
            etherscan_key_badge(false),
            ("etherscan-key-badge is-unset", "No key set"),
        );
    }

    #[cfg(feature = "server")]
    #[test]
    fn coingecko_key_save_payload_rejects_blank_input() {
        assert_eq!(
            coingecko_key_save_payload(String::new()).expect_err("blank should fail"),
            COINGECKO_KEY_BLANK_SAVE_MESSAGE,
        );
        assert_eq!(
            coingecko_key_save_payload("   ".to_string()).expect_err("blank should fail"),
            COINGECKO_KEY_BLANK_SAVE_MESSAGE,
        );
        assert!(coingecko_key_save_payload("SECRET".to_string()).is_ok());
    }

    #[cfg(feature = "server")]
    #[test]
    fn initial_active_section_maps_query_to_tab() {
        assert_eq!(
            initial_active_section(Some("digital-assets")),
            "digital-assets"
        );
        assert_eq!(initial_active_section(Some("regional")), "regional");
        assert_eq!(initial_active_section(Some("account")), "account");
        // Legacy bookmarks used `security`; they must still land on Account.
        assert_eq!(initial_active_section(Some("security")), "account");
        assert_eq!(initial_active_section(Some("system-info")), "system-info");
        assert_eq!(initial_active_section(Some("bogus")), "regional");
        assert_eq!(initial_active_section(Some("")), "regional");
        assert_eq!(initial_active_section(None), "regional");
    }
}
