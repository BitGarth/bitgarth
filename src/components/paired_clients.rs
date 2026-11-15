use crate::backend::{
    LimitPairedClientRequest, PairedClientView, RevokePairedClientRequest, limit_paired_client,
    list_paired_clients, revoke_paired_client,
};
use dioxus::prelude::*;

fn optional_time(value: Option<&str>, empty: &str) -> String {
    value.map(str::to_owned).unwrap_or_else(|| empty.to_owned())
}

fn client_state(client: &PairedClientView) -> String {
    client
        .revoked_at
        .as_deref()
        .map(|at| format!("Revoked {at}"))
        .unwrap_or_else(|| "Active".to_owned())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingAction {
    Revoke,
    MakeReadOnly,
}

/// Only full access can be lowered from here; read-only is the useful floor.
fn can_make_read_only(client: &PairedClientView) -> bool {
    client.revoked_at.is_none() && client.permission == "full"
}

#[component]
pub(crate) fn PairedClients() -> Element {
    let mut confirming = use_signal(|| None::<(PairedClientView, PendingAction)>);
    let mut revoking = use_signal(|| None::<String>);
    let mut action_error = use_signal(|| None::<String>);
    let mut clients_resource =
        use_server_future(move || async move { list_paired_clients().await })?;

    let Some(result) = clients_resource() else {
        return rsx! {};
    };
    let clients = match result {
        Ok(clients) => clients,
        Err(error) => {
            return rsx! {
                div { class: "card",
                    div { class: "card-header", h3 { class: "card-title", "Paired clients" } }
                    div { class: "card-body",
                        p { role: "alert", class: "settings-status-error",
                            "Paired clients could not be loaded: {error}"
                        }
                    }
                }
            };
        }
    };

    rsx! {
        div { class: "card", "data-testid": "paired-clients-card",
            div { class: "card-header",
                h3 { class: "card-title", "Paired clients" }
            }
            div { class: "card-body",
                p {
                    "Client keys are password-free access credentials. Revoking one immediately blocks new CLI access without affecting your password or other paired clients."
                }
                if let Some(error) = action_error() {
                    p { role: "alert", class: "settings-status-error", "{error}" }
                }
                if clients.is_empty() {
                    p { class: "muted", "No paired clients." }
                } else {
                    div { class: "paired-clients-table-wrap",
                        table { class: "paired-clients-table",
                            thead {
                                tr {
                                    th { "Name" }
                                    th { "Permission" }
                                    th { "Created" }
                                    th { "Expiry" }
                                    th { "Last used" }
                                    th { "State" }
                                    th { "Action" }
                                }
                            }
                            tbody {
                                for client in clients {
                                    {
                                        let capability_id = client.capability_id.clone();
                                        let name = client.name.clone();
                                        let revoke_label = format!("Revoke paired client {name}");
                                        let is_revoking = revoking().as_deref() == Some(capability_id.as_str());
                                        let is_revoked = client.revoked_at.is_some();
                                        let limit_label = format!("Make paired client {name} read-only");
                                        let show_limit = can_make_read_only(&client);
                                        let limit_client = client.clone();
                                        let expiry = optional_time(client.expires_at.as_deref(), "Never");
                                        let last_used = optional_time(client.last_used_at.as_deref(), "Never used");
                                        let state = client_state(&client);
                                        rsx! {
                                            tr { key: "{capability_id}",
                                                td { "{client.name}" }
                                                td { code { "{client.permission}" } }
                                                td { "{client.created_at}" }
                                                td { "{expiry}" }
                                                td { "{last_used}" }
                                                td { "{state}" }
                                                td {
                                                    if show_limit {
                                                        button {
                                                            class: "btn btn-secondary btn-sm",
                                                            r#type: "button",
                                                            aria_label: "{limit_label}",
                                                            disabled: is_revoking,
                                                            onclick: move |_| {
                                                                action_error.set(None);
                                                                confirming.set(Some((limit_client.clone(), PendingAction::MakeReadOnly)));
                                                            },
                                                            "Make read-only"
                                                        }
                                                        " "
                                                    }
                                                    button {
                                                        class: "btn btn-danger btn-sm",
                                                        r#type: "button",
                                                        aria_label: "{revoke_label}",
                                                        disabled: is_revoked || is_revoking,
                                                        onclick: move |_| {
                                                            action_error.set(None);
                                                            confirming.set(Some((client.clone(), PendingAction::Revoke)));
                                                        },
                                                        if is_revoking { "Revoking..." } else if is_revoked { "Revoked" } else { "Revoke" }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Some((client, action)) = confirming() {
            {
                let capability_id = client.capability_id.clone();
                let name = client.name.clone();
                let (title, body, note, confirm, busy, confirm_class) = match action {
                    PendingAction::Revoke => (
                        "Revoke Paired Client".to_owned(),
                        format!("Revoke \"{name}\"? Its Client Key will immediately lose CLI access."),
                        "Your password and other paired clients will continue to work.",
                        "Revoke",
                        "Revoking...",
                        "btn btn-danger",
                    ),
                    PendingAction::MakeReadOnly => (
                        "Make Paired Client Read-Only".to_owned(),
                        format!("Make \"{name}\" read-only? It will keep reading your data but can no longer change anything or start a sync."),
                        "To give it full access again, log out of this CLI profile with `bitgarth auth logout`, then pair it with `bitgarth auth login`.",
                        "Make read-only",
                        "Saving...",
                        "btn btn-primary",
                    ),
                };
                rsx! {
                    div { class: "modal-overlay",
                        div {
                            class: "modal",
                            role: "dialog",
                            aria_modal: "true",
                            aria_labelledby: "revoke-paired-client-title",
                            div { class: "modal-header",
                                h3 { id: "revoke-paired-client-title", "{title}" }
                            }
                            div { class: "modal-body",
                                p { "{body}" }
                                p { class: "muted", "{note}" }
                                div { class: "modal-actions",
                                    button {
                                        class: "btn btn-secondary",
                                        r#type: "button",
                                        disabled: revoking().is_some(),
                                        onclick: move |_| confirming.set(None),
                                        "Cancel"
                                    }
                                    button {
                                        class: "{confirm_class}",
                                        r#type: "button",
                                        aria_label: "{confirm} paired client {name}",
                                        disabled: revoking().is_some(),
                                        onclick: move |_| {
                                            action_error.set(None);
                                            revoking.set(Some(capability_id.clone()));
                                            let capability_id = capability_id.clone();
                                            spawn(async move {
                                                let result = match action {
                                                    PendingAction::Revoke => {
                                                        revoke_paired_client(RevokePairedClientRequest { capability_id }).await
                                                    }
                                                    PendingAction::MakeReadOnly => {
                                                        limit_paired_client(LimitPairedClientRequest {
                                                            capability_id,
                                                            permission: "read".to_owned(),
                                                        })
                                                        .await
                                                    }
                                                };
                                                match result {
                                                    Ok(_) => {
                                                        confirming.set(None);
                                                        clients_resource.restart();
                                                    }
                                                    Err(error) => action_error.set(Some(error.to_string())),
                                                }
                                                revoking.set(None);
                                            });
                                        },
                                        if revoking().is_some() { "{busy}" } else { "{confirm}" }
                                    }
                                }
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

    #[test]
    fn paired_client_labels_preserve_never_and_revoked_state() {
        let client = PairedClientView {
            capability_id: "id".to_owned(),
            name: "business".to_owned(),
            permission: "balances_read".to_owned(),
            created_at: "2026-07-31T15:00:00Z".to_owned(),
            expires_at: None,
            last_used_at: None,
            revoked_at: Some("2026-08-01T15:00:00Z".to_owned()),
        };
        assert_eq!(
            optional_time(client.expires_at.as_deref(), "Never"),
            "Never"
        );
        assert_eq!(
            optional_time(client.last_used_at.as_deref(), "Never used"),
            "Never used"
        );
        assert_eq!(client_state(&client), "Revoked 2026-08-01T15:00:00Z");
        assert!(!can_make_read_only(&client));
        let full = PairedClientView {
            permission: "full".to_owned(),
            revoked_at: None,
            ..client.clone()
        };
        assert!(can_make_read_only(&full));
        let read = PairedClientView {
            permission: "read".to_owned(),
            ..full
        };
        assert!(!can_make_read_only(&read));
    }
}
