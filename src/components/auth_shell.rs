use crate::backend::{AuthError, HostedRetentionStatus, hosted_retention_status, login, register};
use crate::components::{DiceIcon, ExternalLinkIcon, PasswordInput};
use crate::legal::{PRIVACY_URL, TERMS_URL, current_registration_acknowledgement};
use crate::models::{
    AuthEntryMode, AuthResponse, FieldErrors, PASSWORD_MIN_LENGTH, RawPlaintextPassword,
    RawUsername,
};
use crate::settings::{SettingsState, defaults_for_locale};
use crate::username_gen::generate_username;
use crate::{
    AuthState, AuthStatus, BannerMessage, BannerSeverity, BannerState, LoggedInOnce,
    LoginFormState, RegisterFormState, Route,
};
use dioxus::logger::tracing;
use dioxus::prelude::*;
use dioxus::router::Navigator;

use super::form_helpers::{
    begin_submit, field_errors_or_empty, finish_submit, is_form_field_error,
};

#[derive(Clone, Debug, PartialEq)]
enum AuthErrorPresentation {
    FieldErrors(FieldErrors),
    Banner(BannerMessage),
}

fn has_auth_form_field_errors(errors: &FieldErrors) -> bool {
    [
        "username",
        "password",
        "confirm_password",
        "legal_acknowledgement",
    ]
    .into_iter()
    .any(|field| errors.get(field).is_some())
}

fn classify_auth_error(error: &AuthError) -> AuthErrorPresentation {
    if is_form_field_error(error) {
        let errors = field_errors_or_empty(error);
        if has_auth_form_field_errors(&errors) {
            return AuthErrorPresentation::FieldErrors(errors);
        }

        return AuthErrorPresentation::Banner(BannerMessage::Custom {
            severity: BannerSeverity::Error,
            text: error.message.clone(),
        });
    }

    if error.is_internal() {
        return AuthErrorPresentation::Banner(BannerMessage::DatabaseUnavailable);
    }

    AuthErrorPresentation::Banner(BannerMessage::Custom {
        severity: BannerSeverity::Error,
        text: error.message.clone(),
    })
}

fn apply_authenticated_response(
    response: AuthResponse,
    settings_state: &SettingsState,
    mut auth_state: AuthState,
    mut logged_in_once: LoggedInOnce,
    mut banner_state: BannerState,
) {
    let settings = response.settings.clone();
    auth_state.set(AuthStatus::Authenticated(response));
    logged_in_once.set(true);

    let defaults = defaults_for_locale(
        (settings_state.language)(),
        (settings_state.timezone)().into(),
    );
    settings_state.apply_user_settings_with_defaults(&settings, &defaults);

    banner_state.set(None);
}

fn apply_auth_error(
    action: &'static str,
    error: AuthError,
    mut field_errors: Signal<FieldErrors>,
    mut banner_state: BannerState,
) {
    tracing::debug!(
        action,
        error = %error,
        "auth ui: auth request failed"
    );

    match classify_auth_error(&error) {
        AuthErrorPresentation::FieldErrors(errors) => {
            tracing::debug!(
                action,
                error_fields = errors.0.len(),
                "auth ui: validation errors"
            );
            field_errors.set(errors);
        }
        AuthErrorPresentation::Banner(message) => {
            banner_state.set(Some(message));
        }
    }
}

fn navigate_after_login(navigator: Navigator, destination: Route) {
    #[cfg(target_arch = "wasm32")]
    if matches!(destination, Route::PairingApproval { .. }) {
        let path = destination.to_string();
        if web_sys::window()
            .and_then(|window| window.location().set_href(&path).ok())
            .is_some()
        {
            return;
        }
    }

    navigator.push(destination);
}

#[component]
pub fn AuthShell(mode: AuthEntryMode, pairing_code: Option<String>) -> Element {
    let auth_state = use_context::<AuthState>();
    let logged_in_once = use_context::<LoggedInOnce>();
    let settings_state = use_context::<SettingsState>();
    let mut banner_state = use_context::<BannerState>();
    let login_form_state = use_context::<LoginFormState>();
    let register_form_state = use_context::<RegisterFormState>();
    let navigator = use_navigator();
    let is_pairing_login = mode == AuthEntryMode::Login && pairing_code.is_some();

    let (mut username, mut password) = match mode {
        AuthEntryMode::Login => (login_form_state.username, login_form_state.password),
        AuthEntryMode::Register => (register_form_state.username, register_form_state.password),
    };
    let mut confirm_password = register_form_state.confirm_password;

    let mut field_errors = use_signal(FieldErrors::new);
    let is_loading = use_signal(|| false);
    let mut legal_acknowledged = use_signal(|| false);

    let retention_resource =
        use_server_future(move || async move { hosted_retention_status().await })?;
    let show_retention_disclosure = mode == AuthEntryMode::Register
        && retention_resource()
            .and_then(|result| result.ok())
            .map(|status: HostedRetentionStatus| status.is_hosted)
            .unwrap_or(false);

    // Suggest an initial username for register mode on mount (client-side only,
    // post-hydration). If a username is already present (e.g. user navigated
    // back), do not overwrite it.
    use_effect(move || {
        if mode == AuthEntryMode::Register && username.peek().is_empty() {
            match generate_username() {
                Ok(name) => username.set(name),
                Err(e) => {
                    tracing::error!(error = %e, "auth ui: initial username generation failed")
                }
            }
        }
    });

    let settings_state_login = settings_state.clone();
    let settings_state_register = settings_state.clone();
    let password_mismatch_text = "Passwords do not match".to_string();

    let handle_login = move |evt: Event<FormData>| {
        evt.prevent_default();

        if !begin_submit(is_loading) {
            tracing::debug!("auth ui: login submit ignored (already loading)");
            return;
        }

        let username_value = username();
        let password_value = password();

        tracing::debug!(
            username = %username_value,
            "auth ui: login submit started"
        );

        field_errors.set(FieldErrors::new());
        banner_state.set(None);
        let settings_state_login = settings_state_login.clone();
        let pairing_code = pairing_code.clone();

        spawn(async move {
            let result = login(
                RawUsername::new(username_value),
                RawPlaintextPassword::new(password_value),
            )
            .await;

            match result {
                Ok(response) => {
                    let user_id = response.user.user_id;
                    tracing::debug!(
                        user_id = %user_id,
                        username = %response.user.username,
                        "auth ui: login succeeded"
                    );
                    let destination = pairing_code
                        .map(|code| Route::PairingApproval { code: Some(code) })
                        .unwrap_or(Route::Wallets);
                    finish_submit(is_loading);
                    apply_authenticated_response(
                        response,
                        &settings_state_login,
                        auth_state,
                        logged_in_once,
                        banner_state,
                    );
                    username.set(String::new());
                    password.set(String::new());
                    navigate_after_login(navigator, destination);
                }
                Err(e) => {
                    finish_submit(is_loading);
                    apply_auth_error("login", e, field_errors, banner_state);
                }
            }
        });
    };

    let handle_register = move |evt: Event<FormData>| {
        evt.prevent_default();

        if !begin_submit(is_loading) {
            tracing::debug!("auth ui: register submit ignored (already loading)");
            return;
        }

        let username_value = username();
        let password_value = password();
        let confirm_password_value = confirm_password();
        let legal_acknowledged_value = legal_acknowledged();

        field_errors.set(FieldErrors::new());
        banner_state.set(None);

        if !legal_acknowledged_value {
            tracing::debug!(
                username = %username_value,
                "auth ui: register legal acknowledgement missing"
            );
            let mut errors = FieldErrors::new();
            errors.add(
                "legal_acknowledgement",
                "You must agree to the Terms and acknowledge the Privacy Notice.".to_string(),
            );
            field_errors.set(errors);
            finish_submit(is_loading);
            return;
        }

        if password_value != confirm_password_value {
            tracing::debug!(
                username = %username_value,
                "auth ui: register password confirmation mismatch"
            );
            let mut errors = FieldErrors::new();
            errors.add("confirm_password", password_mismatch_text.clone());
            field_errors.set(errors);
            finish_submit(is_loading);
            return;
        }

        let settings_state_register = settings_state_register.clone();

        tracing::debug!(
            username = %username_value,
            "auth ui: register submit started"
        );

        spawn(async move {
            let result = register(
                RawUsername::new(username_value),
                RawPlaintextPassword::new(password_value),
                Some(current_registration_acknowledgement()),
            )
            .await;

            finish_submit(is_loading);

            match result {
                Ok(response) => {
                    let user_id = response.user.user_id;
                    tracing::debug!(
                        user_id = %user_id,
                        "auth ui: register succeeded"
                    );
                    apply_authenticated_response(
                        response,
                        &settings_state_register,
                        auth_state,
                        logged_in_once,
                        banner_state,
                    );
                    username.set(String::new());
                    password.set(String::new());
                    confirm_password.set(String::new());
                    legal_acknowledged.set(false);
                    navigator.push(Route::Wallets);
                }
                Err(e) => apply_auth_error("register", e, field_errors, banner_state),
            }
        });
    };

    let username_errors = field_errors().get("username").cloned().unwrap_or_default();
    let password_errors = field_errors().get("password").cloned().unwrap_or_default();
    let confirm_password_errors = field_errors()
        .get("confirm_password")
        .cloned()
        .unwrap_or_default();
    let legal_acknowledgement_errors = field_errors()
        .get("legal_acknowledgement")
        .cloned()
        .unwrap_or_default();

    let has_username_error = !username_errors.is_empty();
    let has_password_error = !password_errors.is_empty();
    let has_confirm_password_error = !confirm_password_errors.is_empty();
    let has_legal_acknowledgement_error = !legal_acknowledgement_errors.is_empty();

    let password_hint = format!(
        "At least {} characters with uppercase, lowercase, and a number",
        PASSWORD_MIN_LENGTH
    );

    let is_login = mode == AuthEntryMode::Login;

    let title_lead = if is_login {
        "Welcome back,"
    } else {
        "Create your account,"
    };
    let title_emph = if is_login {
        "your data is still yours."
    } else {
        "privately."
    };
    let lede = if is_pairing_login {
        "Sign in to review this CLI pairing request. Signing in does not approve it; verify the code and requested balances_read permission on the next screen."
    } else if is_login {
        "Your wallets are exactly where you left them. Password-locked, and yours alone."
    } else {
        "BitGarth is a private, local-first home for your self-custody numbers. No email required — just a username and a password."
    };
    let form_title = if is_login {
        "Sign in"
    } else {
        "Create your account"
    };
    let swap_prompt = if is_login {
        "New here? "
    } else {
        "Already have an account? "
    };
    let swap_label = if is_login {
        "Create an account"
    } else {
        "Sign in"
    };
    let swap_route = if is_login {
        Route::Register
    } else {
        Route::Login
    };

    rsx! {
        div { class: "auth-frame",
            // ── editorial value panel ──
            aside { class: "auth-prose reveal",
                span { class: "auth-num", "§ I." }
                h1 { class: "auth-display",
                    "{title_lead}"
                    br {}
                    em { "{title_emph}" }
                }
                p { class: "auth-lede", "{lede}" }

                if mode == AuthEntryMode::Register {
                    ol { class: "auth-reasons",
                        li {
                            span { class: "reason-num", "i." }
                            strong { "Yours alone." }
                            " Local-first, encrypted with a password only you hold."
                        }
                        li {
                            span { class: "reason-num", "ii." }
                            strong { "No email, no chase." }
                            " A username is all we need. No marketing list, no welcome funnel, no tracking pixels."
                        }
                        li {
                            span { class: "reason-num", "iii." }
                            strong { "Plain text out." }
                            " Export to hledger, ledger-cli, and every LLM you'll ever throw at them."
                        }
                    }
                } else {
                    p { class: "auth-aside",
                        em { "A small reminder. " }
                        "Your password unlocks your encrypted database. Keep it somewhere safe — lose it, and we cannot bring it back."
                    }
                }
            }

            // ── form panel (botanical certificate) ──
            section { class: "auth-card", "aria-label": if is_login { "Sign in" } else { "Create account" },
                // four hairline corner marks
                svg { class: "corner tl", view_box: "0 0 18 18", fill: "none", "aria-hidden": "true",
                    path { d: "M0 7V0h7", stroke: "currentColor", stroke_width: "0.8" }
                    path { d: "M1 1l4 4", stroke: "currentColor", stroke_width: "0.8" }
                }
                svg { class: "corner tr", view_box: "0 0 18 18", fill: "none", "aria-hidden": "true",
                    path { d: "M0 7V0h7", stroke: "currentColor", stroke_width: "0.8" }
                    path { d: "M1 1l4 4", stroke: "currentColor", stroke_width: "0.8" }
                }
                svg { class: "corner bl", view_box: "0 0 18 18", fill: "none", "aria-hidden": "true",
                    path { d: "M0 7V0h7", stroke: "currentColor", stroke_width: "0.8" }
                    path { d: "M1 1l4 4", stroke: "currentColor", stroke_width: "0.8" }
                }
                svg { class: "corner br", view_box: "0 0 18 18", fill: "none", "aria-hidden": "true",
                    path { d: "M0 7V0h7", stroke: "currentColor", stroke_width: "0.8" }
                    path { d: "M1 1l4 4", stroke: "currentColor", stroke_width: "0.8" }
                }

                header { class: "auth-header",
                    h2 { class: "auth-title", "{form_title}" }
                }

                div { class: "auth-body",
                    crate::components::Banner {}

                    if is_login {
                        form { onsubmit: handle_login,
                            div { class: "form-group",
                                label { class: "form-label", r#for: "username", "Username" }
                                input {
                                    class: if has_username_error { "form-input input-error" } else { "form-input" },
                                    r#type: "text",
                                    id: "username",
                                    placeholder: "Enter your username",
                                    autocomplete: "username",
                                    autocorrect: "off",
                                    autocapitalize: "none",
                                    spellcheck: "false",
                                    value: "{username}",
                                    disabled: is_loading(),
                                    oninput: move |evt| username.set(evt.value()),
                                    onmounted: move |event: MountedEvent| async move {
                                        let _ = event.set_focus(true).await;
                                    },
                                }
                                for err in username_errors.iter() {
                                    p { class: "form-error", "{err}" }
                                }
                            }

                            div { class: "form-group",
                                label { class: "form-label", r#for: "password", "Password" }
                                PasswordInput {
                                    id: "password".to_string(),
                                    value: password,
                                    placeholder: "Enter your password".to_string(),
                                    autocomplete: "current-password",
                                    has_error: has_password_error,
                                    disabled: is_loading(),
                                }
                                for err in password_errors.iter() {
                                    p { class: "form-error", "{err}" }
                                }
                            }

                            button {
                                class: "btn btn-primary btn-full",
                                r#type: "submit",
                                disabled: is_loading(),
                                if is_loading() {
                                    "Signing in…"
                                } else {
                                    "Sign in"
                                    span { class: "arrow", "aria-hidden": "true", "→" }
                                }
                            }

                            p { class: "form-hint auth-cookie",
                                "We use one cookie to keep you signed in. No third-party trackers."
                            }
                        }
                    } else {
                        form { onsubmit: handle_register,
                            div { class: "form-group",
                                label { class: "form-label", r#for: "username", "Username" }
                                div { class: "username-gen-row",
                                    input {
                                        class: if has_username_error { "form-input input-error" } else { "form-input" },
                                        r#type: "text",
                                        id: "username",
                                        placeholder: "Pick a username",
                                        autocomplete: "username",
                                        autocorrect: "off",
                                        autocapitalize: "none",
                                        spellcheck: "false",
                                        value: "{username}",
                                        disabled: is_loading(),
                                        oninput: move |evt| username.set(evt.value()),
                                        onmounted: move |event: MountedEvent| async move {
                                            let _ = event.set_focus(true).await;
                                        },
                                    }

                                    button {
                                        r#type: "button",
                                        class: "username-gen-btn",
                                        "aria-label": "Suggest a new username",
                                        title: "Suggest a new username",
                                        disabled: is_loading(),
                                        onclick: move |_| {
                                            match generate_username() {
                                                Ok(name) => username.set(name),
                                                Err(e) => tracing::error!(
                                                    error = %e,
                                                    "auth ui: regenerate username failed"
                                                ),
                                            }
                                        },
                                        DiceIcon {}
                                    }
                                }
                                p { class: "form-hint",
                                    "A random suggestion to keep your name out of the data. Pick your own if you prefer."
                                }
                                for err in username_errors.iter() {
                                    p { class: "form-error", "{err}" }
                                }
                            }

                            div { class: "form-group",
                                label { class: "form-label", r#for: "password", "Password" }
                                PasswordInput {
                                    id: "password".to_string(),
                                    value: password,
                                    placeholder: "Choose a password".to_string(),
                                    autocomplete: "new-password",
                                    has_error: has_password_error,
                                    disabled: is_loading(),
                                }
                                p { class: "form-hint", "{password_hint}" }
                                for err in password_errors.iter() {
                                    p { class: "form-error", "{err}" }
                                }
                            }

                            div { class: "form-group",
                                label { class: "form-label", r#for: "confirm-password", "Confirm password" }
                                PasswordInput {
                                    id: "confirm-password".to_string(),
                                    value: confirm_password,
                                    placeholder: "Repeat your password".to_string(),
                                    autocomplete: "new-password",
                                    has_error: has_confirm_password_error,
                                    disabled: is_loading(),
                                }
                                for err in confirm_password_errors.iter() {
                                    p { class: "form-error", "{err}" }
                                }
                            }

                            div { class: "form-group",
                                label { class: "checkbox",
                                    input {
                                        "data-testid": "legal-acknowledgement-checkbox",
                                        r#type: "checkbox",
                                        checked: legal_acknowledged(),
                                        disabled: is_loading(),
                                        "aria-invalid": if has_legal_acknowledgement_error { "true" } else { "false" },
                                        onchange: move |_| {
                                            legal_acknowledged.set(!legal_acknowledged());
                                        },
                                    }
                                    span {
                                        "I agree to the "
                                        a {
                                            href: TERMS_URL,
                                            target: "_blank",
                                            rel: "noopener noreferrer",
                                            title: "Terms open in a new tab",
                                            "Terms"
                                            ExternalLinkIcon {}
                                        }
                                        " and acknowledge the "
                                        a {
                                            href: PRIVACY_URL,
                                            target: "_blank",
                                            rel: "noopener noreferrer",
                                            title: "Privacy Notice opens in a new tab",
                                            "Privacy Notice"
                                            ExternalLinkIcon {}
                                        }
                                        "."
                                    }
                                }
                                for err in legal_acknowledgement_errors.iter() {
                                    p { class: "form-error", "{err}" }
                                }
                            }

                            if show_retention_disclosure {
                                p {
                                    class: "form-hint auth-retention-notice",
                                    "data-testid": "hosted-retention-disclosure",
                                    "On our hosted service, free accounts are kept as long as you sign in regularly. Inactive free hosted accounts are deleted after 180 days (6 months). Export your data before that, and you can import it into a local/self-hosted BitGarth instance. Paid hosted data is retained, even if you don't sign in. We don't collect your email, so we can't warn you first."
                                }
                            }

                            button {
                                class: "btn btn-primary btn-full",
                                r#type: "submit",
                                disabled: is_loading() || !legal_acknowledged(),
                                if is_loading() {
                                    "Opening your clearing…"
                                } else {
                                    "Open your clearing"
                                    span { class: "arrow", "aria-hidden": "true", "→" }
                                }
                            }
                        }
                    }

                    // hairline vine flourish
                    div { class: "auth-vine", "aria-hidden": "true",
                        svg { width: "180", height: "16", view_box: "0 0 260 22", fill: "none",
                            path { d: "M0 11 H100", stroke: "currentColor", stroke_width: "0.8" }
                            path { d: "M160 11 H260", stroke: "currentColor", stroke_width: "0.8" }
                            path {
                                d: "M115 11c2-4 6-6 10-3 3 2.5 3 7 0 9-3 2-7 1-10-1z M145 11c-2-4-6-6-10-3-3 2.5-3 7 0 9 3 2 7 1 10-1z",
                                fill: "currentColor",
                                opacity: "0.85",
                            }
                            circle { cx: "130", cy: "11", r: "1.7", fill: "currentColor" }
                            path {
                                d: "M122 6 c2-3 6-2 8 0",
                                stroke: "currentColor",
                                stroke_width: "0.7",
                                fill: "none",
                            }
                            path {
                                d: "M138 16 c-2 3-6 2-8 0",
                                stroke: "currentColor",
                                stroke_width: "0.7",
                                fill: "none",
                            }
                        }
                    }

                    div { class: "auth-footer",
                        p {
                            "{swap_prompt}"
                            Link { to: swap_route, class: "auth-link", "{swap_label}" }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn classify_auth_error_keeps_username_validation_inline() {
        let mut errors = FieldErrors::new();
        errors.add("username", "Username is required".to_string());
        let error = AuthError::validation("Validation error", errors.clone());

        assert_eq!(
            classify_auth_error(&error),
            AuthErrorPresentation::FieldErrors(errors)
        );
    }

    #[test]
    fn classify_auth_error_shows_banner_for_conflict_without_form_fields() {
        let error = AuthError::conflict(
            "Support message for a broken encrypted account".to_string(),
            FieldErrors::new(),
        );

        assert_eq!(
            classify_auth_error(&error),
            AuthErrorPresentation::Banner(BannerMessage::Custom {
                severity: BannerSeverity::Error,
                text: "Support message for a broken encrypted account".to_string(),
            })
        );
    }

    #[test]
    fn classify_auth_error_keeps_generic_internal_database_banner() {
        let error = AuthError::internal();

        assert_eq!(
            classify_auth_error(&error),
            AuthErrorPresentation::Banner(BannerMessage::DatabaseUnavailable)
        );
    }
}
