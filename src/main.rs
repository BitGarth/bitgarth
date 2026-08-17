#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

#[cfg(any(feature = "server", test))]
pub(crate) mod account_limits;
mod account_model;
mod amounts;
mod asset_capabilities;
mod asset_views;
mod backend;
mod balance_reliability;
mod channel;
#[cfg(feature = "server")]
mod client_capabilities;
mod components;
#[cfg(feature = "desktop")]
mod desktop_session;
#[cfg(feature = "server")]
mod pairing;
#[cfg(all(not(feature = "desktop"), test, feature = "server"))]
mod desktop_session {
    pub(crate) fn clear() {}
}
mod ethereum;
mod explorer_links;
#[cfg(feature = "server")]
mod hledger_owner;
mod hooks;
mod i18n;
mod integrations;
mod legal;
mod models;
mod payments;
mod report_access;
mod report_dates;
mod services;
mod settings;
mod timezone;
mod transactions;
mod trezor;
mod username_gen;
mod version;
mod wallets;

#[cfg(feature = "server")]
mod gen_asset_catalog;

#[cfg(feature = "server")]
mod gen_free_tier_defaults;

#[cfg(feature = "server")]
mod auth;

#[cfg(feature = "server")]
mod db;

#[cfg(feature = "server")]
mod exports;

#[cfg(all(feature = "server", not(feature = "desktop")))]
mod instance_notice;

#[cfg(all(
    feature = "server",
    feature = "dev-config",
    not(target_arch = "wasm32")
))]
mod perf;

#[cfg(feature = "server")]
mod project_paths;

#[cfg(feature = "server")]
mod raw_replay;

#[cfg(feature = "server")]
mod runtime_context;

#[cfg(feature = "server")]
mod sync_control;

#[cfg(feature = "server")]
mod sync_execution_lease;

#[cfg(feature = "server")]
mod traces;

#[cfg(feature = "server")]
mod user_agent;

#[cfg(feature = "server")]
mod tasks;

#[cfg(feature = "server")]
mod user_db_cli;

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod test;

use components::{
    AccountTransactions, HledgerExport, HoldingsReport, HomeView, InstanceNoticeState, Login,
    NavBar, NotFound, PairingApproval, PairingLogin, Payments, Register, RequireAuth, Settings,
    StyleGuide, WalletDataExport, WalletReport, Wallets,
};
use dioxus::logger::tracing;
use dioxus::prelude::*;
use i18n::Locale;
use models::{AuthEntryBannerKind, AuthEntryDecision, AuthResponse, SessionDuration, UserTimezone};
use settings::{
    SettingsState, default_currency, default_date_time_format, default_number_format,
    defaults_for_locale,
};
use timezone::use_timezone;

#[cfg(feature = "server")]
use axum as _;
#[cfg(test)]
use axum_test as _;
#[cfg(test)]
use dioxus_history as _;
#[cfg(test)]
use dioxus_ssr as _;
// Selects the backend for `zip`'s `deflate-flate2` feature.
#[cfg(feature = "server")]
use flate2 as _;
#[cfg(all(test, feature = "server"))]
use hex as _;
#[cfg(not(feature = "server"))]
use once_cell as _;
#[cfg(test)]
use proptest as _;
#[cfg(all(feature = "server", feature = "desktop"))]
use pulldown_cmark as _;
#[cfg(test)]
use syn as _;
#[cfg(test)]
use tokio as _;
use tracing::Level;
#[cfg(test)]
use tracing_subscriber as _;
// Most modules reach `tracing` through the `dioxus::logger::tracing` alias.
// Keep the bare crate explicitly marked as used so
// `unused_crate_dependencies` stays satisfied across all feature sets.
extern crate tracing as _tracing_crate_alias;
#[cfg(target_arch = "wasm32")]
use url as _;

#[cfg(all(
    feature = "server",
    not(feature = "desktop"),
    not(target_arch = "wasm32")
))]
const MAX_SERVER_FN_BODY_BYTES: usize = 12 * 1024 * 1024;

/// Auth status for session restoration.
#[derive(Clone, Debug)]
pub enum AuthStatus {
    /// Session restoration has not completed yet.
    Unknown,
    /// User is authenticated.
    Authenticated(AuthResponse),
    /// User is not authenticated.
    Unauthenticated,
}

/// Auth state - backed by AuthStatus to avoid impossible combinations.
pub type AuthState = Signal<AuthStatus>;
pub type LoggedInOnce = Signal<bool>;

/// Auth entry decision for unauthenticated routing.
pub type AuthEntryState = Signal<AuthEntryDecision>;

/// Login form state preserved across navigation back to `/login`.
#[derive(Clone)]
pub struct LoginFormState {
    pub username: Signal<String>,
    pub password: Signal<String>,
}

/// Register form state preserved across navigation back to `/register`.
#[derive(Clone)]
pub struct RegisterFormState {
    pub username: Signal<String>,
    pub password: Signal<String>,
    pub confirm_password: Signal<String>,
}

/// Banner severity for inline messages.
#[derive(Clone, Debug, PartialEq)]
pub enum BannerSeverity {
    Error,
    Warning,
    Info,
}

/// An inline banner message.
#[derive(Clone, Debug, PartialEq)]
pub enum BannerMessage {
    SessionExpired,
    DatabaseUnavailable,
    Custom {
        severity: BannerSeverity,
        text: String,
    },
}

/// Banner state - optional message to display.
pub type BannerState = Signal<Option<BannerMessage>>;

const TOKENS_CSS: Asset = asset!("/assets/tokens.css");
const MAIN_CSS: Asset = asset!("/assets/main.css");
const FAVICON_ICO: Asset = asset!("/assets/favicon.ico");
const FONT_FRAUNCES: Asset = asset!("/assets/fonts/fraunces-normal.woff2");
const FONT_FRAUNCES_ITALIC: Asset = asset!("/assets/fonts/fraunces-italic.woff2");
const FONT_INSTRUMENT_SANS: Asset = asset!("/assets/fonts/instrument-sans-normal.woff2");
const FONT_INSTRUMENT_SANS_ITALIC: Asset = asset!("/assets/fonts/instrument-sans-italic.woff2");
const FONT_JETBRAINS_MONO: Asset = asset!("/assets/fonts/jetbrains-mono-normal.woff2");
const FONT_JETBRAINS_MONO_ITALIC: Asset = asset!("/assets/fonts/jetbrains-mono-italic.woff2");

fn font_face_css() -> String {
    let fraunces = FONT_FRAUNCES.to_string();
    let fraunces_italic = FONT_FRAUNCES_ITALIC.to_string();
    let instrument = FONT_INSTRUMENT_SANS.to_string();
    let instrument_italic = FONT_INSTRUMENT_SANS_ITALIC.to_string();
    let jetbrains_mono = FONT_JETBRAINS_MONO.to_string();
    let jetbrains_mono_italic = FONT_JETBRAINS_MONO_ITALIC.to_string();
    format!(
        "@font-face{{font-family:'Fraunces';font-style:normal;font-weight:100 900;font-display:swap;src:url('{fraunces}') format('woff2');}}\
         @font-face{{font-family:'Fraunces';font-style:italic;font-weight:100 900;font-display:swap;src:url('{fraunces_italic}') format('woff2');}}\
         @font-face{{font-family:'Instrument Sans';font-style:normal;font-weight:100 900;font-display:swap;src:url('{instrument}') format('woff2');}}\
         @font-face{{font-family:'Instrument Sans';font-style:italic;font-weight:100 900;font-display:swap;src:url('{instrument_italic}') format('woff2');}}\
         @font-face{{font-family:'JetBrains Mono';font-style:normal;font-weight:100 900;font-display:swap;src:url('{jetbrains_mono}') format('woff2');}}\
         @font-face{{font-family:'JetBrains Mono';font-style:italic;font-weight:100 900;font-display:swap;src:url('{jetbrains_mono_italic}') format('woff2');}}"
    )
}

#[cfg(feature = "desktop")]
fn desktop_custom_head() -> String {
    let tokens_css_href = TOKENS_CSS.to_string();
    let main_css_href = MAIN_CSS.to_string();
    let favicon_href = FAVICON_ICO.to_string();
    let font_face = font_face_css();
    format!(
        r#"<link rel="stylesheet" href="{tokens_css_href}"><link rel="stylesheet" href="{main_css_href}"><link rel="icon" type="image/x-icon" href="{favicon_href}"><style>{font_face}</style>"#
    )
}

#[cfg(not(feature = "desktop"))]
#[component]
fn MainStylesheet() -> Element {
    let font_face = font_face_css();
    rsx! {
        document::Style { "{font_face}" }
        document::Stylesheet { href: TOKENS_CSS }
        document::Stylesheet { href: MAIN_CSS }
        document::Link {
            rel: "icon",
            r#type: "image/x-icon",
            href: FAVICON_ICO,
        }
    }
}

#[cfg(feature = "desktop")]
#[component]
fn MainStylesheet() -> Element {
    rsx! {}
}

#[derive(Routable, Clone, PartialEq)]
enum Route {
    #[layout(NavBar)]
    #[route("/login")]
    Login,

    #[route("/register")]
    Register,

    #[route("/pair/:code/login")]
    PairingLogin { code: String },

    #[route("/style-guide")]
    StyleGuide,

    #[route("/pair?:code")]
    PairingApproval { code: Option<String> },

    #[layout(RequireAuth)]
    #[route("/")]
    HomeView,

    #[route("/settings?:section")]
    Settings { section: Option<String> },

    #[route("/payments")]
    Payments,

    #[route("/wallets")]
    Wallets,

    #[route("/wallets/:wallet_id?:start&:end")]
    WalletReport {
        wallet_id: crate::wallets::WalletId,
        start: Option<String>,
        end: Option<String>,
    },

    #[route("/reports/holdings?:start&:end")]
    HoldingsReport {
        start: Option<String>,
        end: Option<String>,
    },

    #[route("/wallets/account/:account_id/transactions?:start&:end")]
    AccountTransactions {
        account_id: crate::wallets::WalletAccountId,
        start: Option<String>,
        end: Option<String>,
    },

    #[route("/exports/hledger")]
    HledgerExport,

    #[route("/exports/wallet-data")]
    WalletDataExport,
    #[end_layout]
    #[end_layout]
    #[route("/:..segments")]
    NotFound { segments: Vec<String> },
}

#[cfg(test)]
mod route_tests {
    use super::Route;
    use std::str::FromStr;

    #[test]
    fn pairing_login_route_preserves_code_path_segment() {
        let route = Route::from_str("/pair/544B-CQHN/login")
            .expect("pairing login route with code should parse");
        assert!(matches!(
            route,
            Route::PairingLogin { code } if code == "544B-CQHN"
        ));
    }
}

fn main() {
    #[cfg(all(feature = "server", not(target_arch = "wasm32")))]
    match user_db_cli::maybe_run_from_args() {
        Ok(true) => return,
        Ok(false) => {}
        Err(err) => {
            eprintln!("user-db command failed: {err}");
            std::process::exit(2);
        }
    }

    #[cfg(all(feature = "server", not(target_arch = "wasm32")))]
    match raw_replay::maybe_run_from_args() {
        Ok(true) => return,
        Ok(false) => {}
        Err(err) => {
            eprintln!("raw replay command failed: {err}");
            std::process::exit(2);
        }
    }

    #[cfg(all(
        feature = "server",
        feature = "dev-config",
        not(target_arch = "wasm32")
    ))]
    match perf::maybe_run_from_args() {
        Ok(true) => return,
        Ok(false) => {}
        Err(err) => {
            eprintln!("perf command failed: {err}");
            std::process::exit(2);
        }
    }

    #[cfg(all(feature = "server", not(target_arch = "wasm32")))]
    match gen_asset_catalog::maybe_run_from_args() {
        Ok(true) => return,
        Ok(false) => {}
        Err(err) => {
            eprintln!("gen-asset-catalog command failed: {err}");
            std::process::exit(2);
        }
    }

    #[cfg(all(feature = "server", not(target_arch = "wasm32")))]
    match gen_free_tier_defaults::maybe_run_from_args() {
        Ok(true) => return,
        Ok(false) => {}
        Err(err) => {
            eprintln!("gen-free-tier-defaults command failed: {err}");
            std::process::exit(2);
        }
    }

    // Reject unknown CLI arguments
    #[cfg(all(feature = "server", not(target_arch = "wasm32")))]
    {
        let first = std::env::args().nth(1);
        if let Some(arg) = first {
            eprintln!("unknown argument: {arg}");
            #[cfg(feature = "dev-config")]
            eprintln!(
                "usage: bitgarth [user-db | raw-replay | perf | gen-asset-catalog | gen-free-tier-defaults]"
            );
            #[cfg(not(feature = "dev-config"))]
            eprintln!(
                "usage: bitgarth [user-db | raw-replay | gen-asset-catalog | gen-free-tier-defaults]"
            );
            std::process::exit(1);
        }
    }

    // Init logger
    if let Err(err) = dioxus::logger::init(Level::INFO) {
        eprintln!("failed to init logger: {err}");
        std::process::exit(1);
    }

    #[cfg(feature = "desktop")]
    {
        #[cfg(feature = "server")]
        if let Err(err) = tasks::ensure_started() {
            eprintln!("failed to start background tasks: {err}");
            std::process::exit(1);
        }

        #[cfg(feature = "server")]
        if let Err(err) = db::initialize_prices_db() {
            eprintln!("failed to initialize prices database: {err}");
            std::process::exit(1);
        }

        #[cfg(feature = "server")]
        if let Err(err) = crate::asset_capabilities::load_registry() {
            eprintln!("failed to load asset catalog: {err}");
            std::process::exit(1);
        }

        #[cfg(feature = "server")]
        if let Err(err) = crate::asset_capabilities::load_unsynced_catalog() {
            eprintln!("failed to load unsynced asset catalog: {err}");
            std::process::exit(1);
        }

        tracing::info!(
            app = env!("CARGO_PKG_NAME"),
            version = version::version(),
            "Starting"
        );

        use dioxus::desktop::{Config, WindowBuilder};

        let app_title = "BitGarth";
        let window = WindowBuilder::new()
            .with_title(app_title)
            .with_maximized(true);
        let config = Config::default()
            .with_window(window)
            .with_custom_head(desktop_custom_head())
            .with_background_color((240, 247, 252, 255));

        dioxus::LaunchBuilder::new().with_cfg(config).launch(App);
    }

    #[cfg(all(
        feature = "server",
        not(feature = "desktop"),
        not(target_arch = "wasm32"),
        any(not(test), not(bitgarth_db_unit_only))
    ))]
    {
        if let Err(err) = tasks::ensure_started() {
            eprintln!("failed to start background tasks: {err}");
            std::process::exit(1);
        }

        if let Err(err) = db::initialize_prices_db() {
            eprintln!("failed to initialize prices database: {err}");
            std::process::exit(1);
        }

        instance_notice::load_from_env();

        if let Err(err) = crate::asset_capabilities::load_registry() {
            eprintln!("failed to load asset catalog: {err}");
            std::process::exit(1);
        }

        if let Err(err) = crate::asset_capabilities::load_unsynced_catalog() {
            eprintln!("failed to load unsynced asset catalog: {err}");
            std::process::exit(1);
        }

        let port = std::env::var("PORT")
            .ok()
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(8080);
        let ip = std::env::var("IP")
            .unwrap_or_else(|_| "127.0.0.1".to_owned())
            .parse::<std::net::IpAddr>()
            .unwrap_or_else(|err| {
                eprintln!("invalid IP address: {err}");
                std::process::exit(1);
            });
        let address = std::net::SocketAddr::new(ip, port);
        tracing::info!(
            app = env!("CARGO_PKG_NAME"),
            version = version::version(),
            %address,
            "Starting"
        );

        let pairing_store = std::sync::Arc::new(pairing::PairingStore::new());
        let pairing_cleanup_store = std::sync::Arc::clone(&pairing_store);
        let proxy_trust = backend::ProxyHeaderTrust::from_env();
        let router = axum::Router::new()
            .merge(backend::public_api::router(
                std::sync::Arc::clone(&pairing_store),
                proxy_trust,
            ))
            .route(
                "/_app/user/transactions/sync/events",
                axum::routing::get(backend::transactions_sync_events_sse),
            )
            .route(
                "/_app/user/exports/hledger/download",
                axum::routing::post(backend::download_hledger),
            )
            .route("/api/v1/build", axum::routing::get(backend::current_build))
            .serve_dioxus_application(dioxus::server::ServeConfig::new(), App)
            .layer(axum::extract::DefaultBodyLimit::max(
                MAX_SERVER_FN_BODY_BYTES,
            ))
            .layer(axum::middleware::from_fn(
                backend::normalize_server_fn_bad_request,
            ))
            .layer(axum::middleware::from_fn(
                backend::public_api::browser_pairing_no_store,
            ))
            .layer(axum::Extension(pairing_store))
            .layer(axum::Extension(proxy_trust));
        let runtime = tokio::runtime::Runtime::new().unwrap_or_else(|err| {
            eprintln!("failed to initialize server runtime: {err}");
            std::process::exit(1);
        });
        runtime.spawn(pairing_cleanup_store.run_expiry_cleanup());
        let result = runtime.block_on(async move {
            let listener = tokio::net::TcpListener::bind(address).await?;
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
        });
        if let Err(err) = result {
            eprintln!("server failed: {err}");
            std::process::exit(1);
        }
    }

    #[cfg(all(
        not(feature = "desktop"),
        not(all(feature = "server", not(target_arch = "wasm32")))
    ))]
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    // === All hooks are declared before the use_server_future suspension point ===

    // Track if we've ever logged in to avoid resetting settings on logout
    let mut logged_in_once: LoggedInOnce = use_signal(|| false);
    use_context_provider(|| logged_in_once);

    // Detect timezone for app defaults
    let detected_timezone = use_timezone();

    let mut date_time_format = use_signal(|| default_date_time_format(Locale::default()));
    let mut number_format = use_signal(|| default_number_format(Locale::default()));
    let mut currency = use_signal(|| default_currency(Locale::default()));
    let mut timezone = use_signal(|| UserTimezone::from(chrono_tz::Tz::UTC));
    let session_duration = use_signal(SessionDuration::default);
    let mempool_base_url = use_signal(|| None);
    let etherscan_base_url = use_signal(|| None);
    let price_fetching_enabled = use_signal(|| false);
    let has_coingecko_api_key = use_signal(|| false);

    use_effect(move || {
        let current_locale = Locale::default();
        if !logged_in_once() {
            date_time_format.set(default_date_time_format(current_locale));
            number_format.set(default_number_format(current_locale));
            currency.set(default_currency(current_locale));
        }
    });

    use_effect(move || {
        let detected = detected_timezone();
        if !logged_in_once() {
            timezone.set(UserTimezone::from(detected));
        }
    });

    let settings_state = SettingsState {
        language: use_signal(Locale::default),
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
    let settings_state_for_context = settings_state.clone();
    use_context_provider(move || settings_state_for_context);

    // Auth state - Unknown until synced from the server future below
    let mut auth_state: AuthState = use_signal(|| AuthStatus::Unknown);
    use_context_provider(|| auth_state);

    // Auth entry decision (default to register)
    let mut auth_entry: AuthEntryState = use_signal(AuthEntryDecision::default);
    use_context_provider(|| auth_entry);

    // Auth form state — separate per-mode contexts so a generated register
    // username does not bleed into the login form.
    let login_form_state = LoginFormState {
        username: use_signal(String::new),
        password: use_signal(String::new),
    };
    use_context_provider(|| login_form_state);

    let register_form_state = RegisterFormState {
        username: use_signal(String::new),
        password: use_signal(String::new),
        confirm_password: use_signal(String::new),
    };
    use_context_provider(|| register_form_state);

    // Banner state for inline notifications
    let mut banner_state: BannerState = use_signal(|| None);
    use_context_provider(|| banner_state);

    // Toast state for transient notifications
    let toast_state: components::ToastState = use_signal(Vec::new);
    use_context_provider(|| toast_state);

    let build_drift = components::BuildDriftState(use_signal(|| None));
    use_context_provider(|| build_drift);

    // Operator-supplied instance notice — fetched at App level so its suspense
    // sits at the root rather than inside any layout. Consumers read the html
    // via the `InstanceNoticeState` context.
    let mut instance_notice_state: InstanceNoticeState = use_signal(|| None);
    use_context_provider(|| instance_notice_state);

    // Guard: sync auth result into signals exactly once per mount
    let mut auth_synced = use_signal(|| false);
    let mut auth_entry_synced = use_signal(|| false);

    // === Suspension point: resolve auth during SSR ===
    // On the server, this awaits me() and serializes the result into the page.
    // On the client, the cached result is available immediately — no loading flash.
    let auth_entry_resource = use_server_future(move || async move { backend::auth_entry().await });
    let auth_resource = use_server_future(move || async move { backend::me().await });
    let instance_notice_resource =
        use_server_future(move || async move { backend::instance_notice_html().await });

    let auth_entry_resource = auth_entry_resource?;
    let auth_resource = auth_resource?;
    let instance_notice_resource = instance_notice_resource?;

    // === No hooks below this point ===

    // Sync the resolved auth result into app state (once per mount).
    // Signal writes take effect immediately for the current render, and the
    // queued re-render converges because auth_synced prevents a second sync.
    if !*auth_synced.peek() {
        auth_synced.set(true);
        let auth_value = auth_resource.value();
        let val = auth_value.peek();
        match val.as_ref() {
            Some(Ok(auth)) => {
                tracing::debug!(
                    user_id = %auth.user.user_id,
                    "auth: session restored via server future"
                );
                logged_in_once.set(true);

                let defaults = defaults_for_locale(Locale::default(), *detected_timezone.peek());
                settings_state.apply_user_settings_with_defaults(&auth.settings, &defaults);

                auth_state.set(AuthStatus::Authenticated(auth.clone()));
                banner_state.set(None);
            }
            Some(Err(err)) => {
                tracing::debug!(
                    error = %err,
                    "auth: session restore failed"
                );
                auth_state.set(AuthStatus::Unauthenticated);
            }
            None => {
                // Should not occur — use_server_future has resolved past the ? point
            }
        }
    }

    // Sync the resolved auth entry decision into app state (once per mount).
    if !*auth_entry_synced.peek() {
        auth_entry_synced.set(true);
        let entry_value = auth_entry_resource.value();
        let val = entry_value.peek();
        match val.as_ref() {
            Some(Ok(decision)) => {
                auth_entry.set(decision.clone());
                if let Some(AuthEntryBannerKind::DatabaseUnavailable) = decision.banner {
                    banner_state.set(Some(BannerMessage::DatabaseUnavailable));
                }
            }
            Some(Err(err)) => {
                tracing::error!(
                    error = %err,
                    "auth: entry decision failed"
                );
                auth_entry.set(AuthEntryDecision::default());
            }
            None => {
                tracing::error!(
                    "auth: Should not occur — use_server_future has resolved past the ? point"
                );
            }
        }
    }

    let resolved_notice = match instance_notice_resource.value().peek().as_ref() {
        Some(Ok(html)) => html.clone(),
        _ => None,
    };
    if *instance_notice_state.peek() != resolved_notice {
        instance_notice_state.set(resolved_notice);
    }

    rsx! {
        document::Meta {
            name: "robots",
            content: "noindex, nofollow",
        }
        MainStylesheet {}
        Router::<Route> {}
    }
}
