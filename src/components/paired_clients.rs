use crate::backend::{
    PairedClientView, RevokePairedClientRequest, list_paired_clients, revoke_paired_client,
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

#[component]
pub(crate) fn PairedClients() -> Element {
    let mut confirming = use_signal(|| None::<PairedClientView>);
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
                    div { class: "card-header", h3 { class: "card-title", "Paired Clients" } }
                    div { class: "card-body",
                        p { role: "alert", class: "settings-status-error",
                            "Paired Clients could not be loaded: {error}"
                        }
                    }
                }
            };
        }
    };

    rsx! {
        div { class: "card", "data-testid": "paired-clients-card",
            div { class: "card-header",
                h3 { class: "card-title", "Paired Clients" }
            }
            div { class: "card-body",
                p {
                    "Client Keys are password-free access credentials. Revoking one immediately blocks new CLI access without affecting your password or other Paired Clients."
                }
                if let Some(error) = action_error() {
                    p { role: "alert", class: "settings-status-error", "{error}" }
                }
                if clients.is_empty() {
                    p { class: "muted", "No Paired Clients." }
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
                                                    button {
                                                        class: "btn btn-danger btn-sm",
                                                        r#type: "button",
                                                        aria_label: "{revoke_label}",
                                                        disabled: is_revoked || is_revoking,
                                                        onclick: move |_| {
                                                            action_error.set(None);
                                                            confirming.set(Some(client.clone()));
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

        if let Some(client) = confirming() {
            {
                let capability_id = client.capability_id.clone();
                let name = client.name.clone();
                rsx! {
                    div { class: "modal-overlay",
                        div {
                            class: "modal",
                            role: "dialog",
                            aria_modal: "true",
                            aria_labelledby: "revoke-paired-client-title",
                            div { class: "modal-header",
                                h3 { id: "revoke-paired-client-title", "Revoke Paired Client" }
                            }
                            div { class: "modal-body",
                                p { "Revoke \"{name}\"? Its Client Key will immediately lose CLI access." }
                                p { class: "muted", "Your password and other Paired Clients will continue to work." }
                                div { class: "modal-actions",
                                    button {
                                        class: "btn btn-secondary",
                                        r#type: "button",
                                        disabled: revoking().is_some(),
                                        onclick: move |_| confirming.set(None),
                                        "Cancel"
                                    }
                                    button {
                                        class: "btn btn-danger",
                                        r#type: "button",
                                        aria_label: "Revoke paired client {name}",
                                        disabled: revoking().is_some(),
                                        onclick: move |_| {
                                            action_error.set(None);
                                            revoking.set(Some(capability_id.clone()));
                                            let request = RevokePairedClientRequest {
                                                capability_id: capability_id.clone(),
                                            };
                                            spawn(async move {
                                                match revoke_paired_client(request).await {
                                                    Ok(_) => {
                                                        confirming.set(None);
                                                        clients_resource.restart();
                                                    }
                                                    Err(error) => action_error.set(Some(error.to_string())),
                                                }
                                                revoking.set(None);
                                            });
                                        },
                                        if revoking().is_some() { "Revoking..." } else { "Revoke" }
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
    }
}
