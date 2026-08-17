use crate::Route;
use crate::backend::{
    ApprovePairingRequest, DenyPairingRequest, PairedClientView, PairingReviewResponse,
    approve_pairing, deny_pairing, list_paired_clients, review_pairing,
};
use chrono::{DateTime, Utc};
use dioxus::document::eval;
use dioxus::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PairingPageState {
    Review,
    Denied,
    WaitingForCli,
    Succeeded,
    NotConfirmed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PairingCompletionDecision {
    Waiting,
    Succeeded,
    NotConfirmed,
}

fn pairing_completion_decision(
    pairing_id: &str,
    expires_at: &str,
    now: DateTime<Utc>,
    clients: &[PairedClientView],
) -> Result<PairingCompletionDecision, ()> {
    if clients.iter().any(|client| {
        client.capability_id == pairing_id
            && client.revoked_at.is_none()
            && match client.expires_at.as_deref() {
                None => true,
                Some(expires_at) => DateTime::parse_from_rfc3339(expires_at)
                    .is_ok_and(|expires_at| expires_at.with_timezone(&Utc) > now),
            }
    }) {
        return Ok(PairingCompletionDecision::Succeeded);
    }

    let expires_at = DateTime::parse_from_rfc3339(expires_at)
        .map_err(|_| ())?
        .with_timezone(&Utc);
    Ok(if now >= expires_at {
        PairingCompletionDecision::NotConfirmed
    } else {
        PairingCompletionDecision::Waiting
    })
}

#[component]
pub fn PairingApproval(code: Option<String>) -> Element {
    let mut code_matches = use_signal(|| false);
    let mut expires_at = use_signal(String::new);
    let mut page_state = use_signal(|| PairingPageState::Review);
    let mut action_in_flight = use_signal(|| false);
    let mut action_error = use_signal(|| None::<String>);
    let mut completion_warning = use_signal(|| None::<String>);
    let code_for_review = code.unwrap_or_default();
    let review_code = code_for_review.clone();
    let review = use_server_future(move || {
        let review_code = review_code.clone();
        async move { review_pairing(review_code).await }
    })?;

    let Some(result) = review() else {
        return rsx! {};
    };
    let response = match result {
        Ok(response) => response,
        Err(error) => {
            #[cfg(feature = "server")]
            dioxus::fullstack::FullstackContext::commit_http_status(error.code.status_code(), None);
            #[cfg(not(feature = "server"))]
            let _ = error;
            return rsx! {
                main { class: "page-container",
                    section { class: "settings-section", aria_labelledby: "pairing-title",
                        h1 { id: "pairing-title", class: "page-title", "Pairing unavailable" }
                        p { "This pairing code is invalid, expired, or no longer awaiting approval." }
                    }
                }
            };
        }
    };

    match response {
        PairingReviewResponse::LoginRequired { code } => rsx! {
            main { class: "page-container",
                section { class: "settings-section", aria_labelledby: "pairing-title",
                    h1 { id: "pairing-title", class: "page-title", "Sign in to review this pairing" }
                    p { "Unlock BitGarth before approving a Client Key." }
                    Link {
                        class: "btn btn-primary",
                        to: Route::PairingLogin { code },
                        "Sign in"
                    }
                }
            }
        },
        PairingReviewResponse::Ready { pairing } => {
            let approve_pairing_id = pairing.pairing_id.clone();
            let approve_code = pairing.code.clone();
            let approve_pairing_expires_at = pairing.expires_at.clone();
            let deny_pairing_id = pairing.pairing_id.clone();
            let deny_code = pairing.code.clone();
            rsx! {
                main { class: "page-container",
                    section { class: "settings-section", aria_labelledby: "pairing-title",
                        h1 { id: "pairing-title", class: "page-title", "Review Client Key pairing" }
                        p { class: "text-muted", "Code" }
                        p { class: "pairing-code", "{pairing.code}" }
                        p { "Client: " strong { "{pairing.client_name}" } }
                        p { "Requested permission: read wallet names and balances." }
                        p {
                            "This creates a second unlock capability that does not require your normal password after approval. Anyone holding the Client Key can read wallet names and balances."
                        }
                        p {
                            "It cannot read transactions, export data, change settings, write financial data, start sync, or create a browser session. You can revoke it from Paired Clients in Settings."
                        }

                        if let Some(error) = action_error() {
                            p { role: "alert", class: "form-error", "{error}" }
                        }

                        if page_state() == PairingPageState::Review {
                            form {
                                onsubmit: move |event| {
                                    event.prevent_default();
                                    if action_in_flight() {
                                        return;
                                    }
                                    action_error.set(None);
                                    action_in_flight.set(true);
                                    let expiry = expires_at();
                                    let pairing_id = approve_pairing_id.clone();
                                    let pairing_expires_at = approve_pairing_expires_at.clone();
                                    let request = ApprovePairingRequest {
                                        pairing_id: approve_pairing_id.clone(),
                                        code: approve_code.clone(),
                                        permissions: vec!["balances_read".to_owned()],
                                        code_matches: code_matches(),
                                        expires_at: (!expiry.is_empty()).then(|| format!("{expiry}:00Z")),
                                    };
                                    spawn(async move {
                                        match approve_pairing(request).await {
                                            Ok(_) => {
                                                page_state.set(PairingPageState::WaitingForCli);

                                                // ponytail: reuse the small paired-client list; add a status endpoint only if
                                                // list size or polling load becomes material.
                                                loop {
                                                    let clients = match list_paired_clients().await {
                                                        Ok(clients) => {
                                                            completion_warning.set(None);
                                                            clients
                                                        }
                                                        Err(_) => {
                                                            completion_warning.set(Some(
                                                                "BitGarth cannot currently confirm completion. Pairing may still finish in the CLI."
                                                                    .to_owned(),
                                                            ));
                                                            Vec::new()
                                                        }
                                                    };

                                                    match pairing_completion_decision(
                                                        &pairing_id,
                                                        &pairing_expires_at,
                                                        Utc::now(),
                                                        &clients,
                                                    ) {
                                                        Ok(PairingCompletionDecision::Succeeded) => {
                                                            completion_warning.set(None);
                                                            page_state.set(PairingPageState::Succeeded);
                                                            break;
                                                        }
                                                        Ok(PairingCompletionDecision::NotConfirmed) => {
                                                            completion_warning.set(None);
                                                            page_state.set(PairingPageState::NotConfirmed);
                                                            break;
                                                        }
                                                        Ok(PairingCompletionDecision::Waiting) => {}
                                                        Err(()) => {
                                                            completion_warning.set(Some(
                                                                "Pairing was approved, but BitGarth could not read its completion deadline. Check the CLI or Paired Clients."
                                                                    .to_owned(),
                                                            ));
                                                            page_state.set(PairingPageState::NotConfirmed);
                                                            break;
                                                        }
                                                    }

                                                    let mut timer = eval(r#"setTimeout(() => { dioxus.send(null); }, 5000);"#);
                                                    if timer.recv::<serde_json::Value>().await.is_err() {
                                                        break;
                                                    }
                                                }
                                            }
                                            Err(error) => {
                                                action_in_flight.set(false);
                                                action_error.set(Some(error.to_string()));
                                            }
                                        }
                                    });
                                },
                                div { class: "form-group",
                                    label { class: "form-label", r#for: "pairing-expiry",
                                        "Client expiry (UTC, optional)"
                                    }
                                    input {
                                        id: "pairing-expiry",
                                        class: "form-input",
                                        r#type: "datetime-local",
                                        value: "{expires_at}",
                                        onchange: move |event| expires_at.set(event.value()),
                                    }
                                    p { class: "form-hint", "Leave empty for Never (valid until revoked)." }
                                }
                                label { class: "checkbox",
                                    input {
                                        r#type: "checkbox",
                                        checked: code_matches(),
                                        onchange: move |event| code_matches.set(event.checked()),
                                    }
                                    span { "I confirm this code matches the code shown by the initiating CLI." }
                                }
                                div { class: "form-actions mt-md",
                                    button {
                                        class: "btn btn-primary",
                                        r#type: "submit",
                                        disabled: action_in_flight(),
                                        "Approve"
                                    }
                                    button {
                                        class: "btn btn-secondary",
                                        r#type: "button",
                                        onclick: move |_| {
                                            if action_in_flight() {
                                                return;
                                            }
                                            action_error.set(None);
                                            action_in_flight.set(true);
                                            let request = DenyPairingRequest {
                                                pairing_id: deny_pairing_id.clone(),
                                                code: deny_code.clone(),
                                            };
                                            spawn(async move {
                                                match deny_pairing(request).await {
                                                    Ok(_) => page_state.set(PairingPageState::Denied),
                                                    Err(error) => {
                                                        action_in_flight.set(false);
                                                        action_error.set(Some(error.to_string()));
                                                    }
                                                }
                                            });
                                        },
                                        disabled: action_in_flight(),
                                        "Deny"
                                    }
                                }
                            }
                        } else if page_state() == PairingPageState::Denied {
                            p { role: "status", "Pairing denied. You may return to the CLI." }
                        } else if page_state() == PairingPageState::WaitingForCli {
                            p { role: "status",
                                "Pairing approved. Waiting for the CLI to finish… You may close this page; pairing will continue in the CLI."
                            }
                            if let Some(warning) = completion_warning() {
                                p { role: "status", class: "text-muted", "{warning}" }
                            }
                        } else if page_state() == PairingPageState::Succeeded {
                            p { role: "status", class: "settings-status-success",
                                "Pairing successful. Profile “{pairing.client_name}” is ready."
                            }
                            a {
                                class: "btn btn-primary",
                                href: "/settings?section=account",
                                "View paired clients"
                            }
                        } else if page_state() == PairingPageState::NotConfirmed {
                            if let Some(error) = completion_warning() {
                                p { role: "alert", "{error}" }
                            } else {
                                p { role: "status",
                                    "Pairing could not be confirmed before it expired. Check the CLI or Paired Clients. If it did not finish, run bitgarth pair again."
                                }
                            }
                            a {
                                class: "btn btn-primary",
                                href: "/settings?section=account",
                                "View paired clients"
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paired_client(capability_id: &str) -> PairedClientView {
        PairedClientView {
            capability_id: capability_id.to_owned(),
            name: "client".to_owned(),
            permission: "balances_read".to_owned(),
            created_at: "2026-08-09T12:00:00Z".to_owned(),
            expires_at: None,
            last_used_at: None,
            revoked_at: None,
        }
    }

    #[test]
    fn pairing_completion_uses_capability_id_and_deadline() {
        let before = DateTime::parse_from_rfc3339("2026-08-09T12:09:59Z")
            .expect("valid test timestamp")
            .with_timezone(&Utc);
        let at_expiry = DateTime::parse_from_rfc3339("2026-08-09T12:10:00Z")
            .expect("valid test timestamp")
            .with_timezone(&Utc);
        let expires_at = "2026-08-09T12:10:00Z";

        assert_eq!(
            pairing_completion_decision(
                "wanted",
                expires_at,
                before,
                &[paired_client("other"), paired_client("wanted")],
            ),
            Ok(PairingCompletionDecision::Succeeded)
        );
        assert_eq!(
            pairing_completion_decision("wanted", expires_at, before, &[]),
            Ok(PairingCompletionDecision::Waiting)
        );
        assert_eq!(
            pairing_completion_decision("wanted", expires_at, at_expiry, &[]),
            Ok(PairingCompletionDecision::NotConfirmed)
        );
    }

    #[test]
    fn pairing_completion_requires_an_active_matching_client() {
        let now = DateTime::parse_from_rfc3339("2026-08-09T12:05:00Z")
            .expect("valid test timestamp")
            .with_timezone(&Utc);
        let pairing_expires_at = "2026-08-09T12:10:00Z";

        let mut revoked = paired_client("wanted");
        revoked.revoked_at = Some("2026-08-09T12:04:00Z".to_owned());
        let mut expired = paired_client("wanted");
        expired.expires_at = Some("2026-08-09T12:05:00Z".to_owned());
        let mut malformed_expiry = paired_client("wanted");
        malformed_expiry.expires_at = Some("not-a-timestamp".to_owned());

        for (case, client) in [
            ("revoked", revoked),
            ("expired", expired),
            ("malformed expiry", malformed_expiry),
        ] {
            assert_eq!(
                pairing_completion_decision("wanted", pairing_expires_at, now, &[client]),
                Ok(PairingCompletionDecision::Waiting),
                "{case} client must not confirm pairing"
            );
        }

        let mut active = paired_client("wanted");
        active.expires_at = Some("2026-08-09T12:05:01Z".to_owned());
        assert_eq!(
            pairing_completion_decision("wanted", pairing_expires_at, now, &[active]),
            Ok(PairingCompletionDecision::Succeeded)
        );
    }
}
