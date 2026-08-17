use super::super::{KebabIcon, PlusIcon};
use dioxus::prelude::*;

#[derive(Clone, PartialEq)]
pub(crate) struct KebabMenuItem {
    pub(crate) label: String,
    pub(crate) test_id: Option<String>,
    pub(crate) on_click: EventHandler<()>,
    pub(crate) danger: bool,
    pub(crate) disabled: bool,
    pub(crate) title: Option<String>,
}

#[component]
pub(crate) fn KebabMenu(items: Vec<KebabMenuItem>, aria_label: String) -> Element {
    let mut open = use_signal(|| false);

    let dropdown_class = if open() {
        "kebab-menu-dropdown visible"
    } else {
        "kebab-menu-dropdown"
    };

    rsx! {
        div { class: "kebab-menu",
            if open() {
                div {
                    class: "kebab-menu-dismiss-overlay",
                    onclick: move |_| open.set(false),
                }
            }
            button {
                class: "kebab-menu-trigger",
                r#type: "button",
                "aria-label": "{aria_label}",
                "aria-haspopup": "menu",
                "aria-expanded": open(),
                onclick: move |_| open.set(!open()),
                KebabIcon {}
            }
            div { class: "{dropdown_class}", role: "menu",
                for item in items {
                    {
                        let item_class = if item.danger {
                            "kebab-menu-item danger"
                        } else {
                            "kebab-menu-item"
                        };
                        let on_click = item.on_click;
                        rsx! {
                            button {
                                class: "{item_class}",
                                role: "menuitem",
                                "data-testid": item.test_id,
                                disabled: item.disabled,
                                title: item.title,
                                onclick: move |_| {
                                    if item.disabled {
                                        return;
                                    }
                                    open.set(false);
                                    on_click.call(());
                                },
                                "{item.label}"
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub(crate) fn AddDropdownButton(
    on_link_trezor: EventHandler<()>,
    on_add_xpub: EventHandler<()>,
    on_add_bitcoin: EventHandler<()>,
    on_add_ethereum: EventHandler<()>,
    on_add_manual_asset: EventHandler<()>,
) -> Element {
    let mut open = use_signal(|| false);

    let dropdown_class = if open() {
        "add-dropdown-menu visible"
    } else {
        "add-dropdown-menu"
    };

    rsx! {
        div { class: "add-dropdown",
            if open() {
                div {
                    class: "kebab-menu-dismiss-overlay",
                    onclick: move |_| open.set(false),
                }
            }
            button {
                class: "btn btn-primary",
                r#type: "button",
                "data-testid": "wallets-add-button",
                "aria-haspopup": "menu",
                "aria-expanded": open(),
                onclick: move |_| open.set(!open()),
                PlusIcon {}
                " Add"
            }
            div { class: "{dropdown_class}", role: "menu",
                if super::TREZOR_LINK_ENABLED {
                    button {
                        class: "add-dropdown-item",
                        role: "menuitem",
                        "data-testid": super::ACTION_LINK_TREZOR_TEST_ID,
                        onclick: move |_| {
                            open.set(false);
                            on_link_trezor.call(());
                        },
                        "Trezor Account"
                    }
                }
                button {
                    class: "add-dropdown-item",
                    role: "menuitem",
                    "data-testid": super::ACTION_ADD_XPUB_TEST_ID,
                    onclick: move |_| {
                        open.set(false);
                        on_add_xpub.call(());
                    },
                    "Bitcoin Extended Public Key"
                }
                button {
                    class: "add-dropdown-item",
                    role: "menuitem",
                    "data-testid": super::ACTION_ADD_BITCOIN_ADDRESS_TEST_ID,
                    onclick: move |_| {
                        open.set(false);
                        on_add_bitcoin.call(());
                    },
                    "Bitcoin Address"
                }
                button {
                    class: "add-dropdown-item",
                    role: "menuitem",
                    "data-testid": super::ACTION_ADD_ETHEREUM_ADDRESS_TEST_ID,
                    onclick: move |_| {
                        open.set(false);
                        on_add_ethereum.call(());
                    },
                    "Ethereum Address"
                }
                button {
                    class: "add-dropdown-item",
                    role: "menuitem",
                    "data-testid": super::ACTION_ADD_MANUAL_ASSET_TEST_ID,
                    onclick: move |_| {
                        open.set(false);
                        on_add_manual_asset.call(());
                    },
                    "Manual Asset"
                }
            }
        }
    }
}

#[component]
pub(crate) fn AddressSchemeDeleteConfirmDialog(
    scheme_label: String,
    on_confirm: EventHandler<()>,
    on_cancel: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "modal-overlay",
            div { class: "modal",
                div { class: "modal-header",
                    h3 { "Delete Address Type" }
                }
                div { class: "modal-body",
                    p {
                        "Delete the {scheme_label} address type and all of its receive/change addresses for this account?"
                    }
                    p { class: "muted", "You will have to re-add this account to get it back." }
                    div { class: "modal-actions",
                        button {
                            class: "btn btn-secondary",
                            onclick: move |_| on_cancel.call(()),
                            "Cancel"
                        }
                        button {
                            class: "btn btn-danger",
                            onclick: move |_| on_confirm.call(()),
                            "Delete"
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub(super) fn DeleteAccountConfirmDialog(
    account_label: String,
    on_confirm: EventHandler<()>,
    on_cancel: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "modal-overlay",
            div { class: "modal",
                div { class: "modal-header",
                    h3 { "Delete Account" }
                }
                div { class: "modal-body",
                    p { "Delete the account \"{account_label}\"?" }
                    p { class: "muted", "You will have to create this custom account again if you want it back." }
                    div { class: "modal-actions",
                        button {
                            class: "btn btn-secondary",
                            onclick: move |_| on_cancel.call(()),
                            "Cancel"
                        }
                        button {
                            class: "btn btn-danger",
                            onclick: move |_| on_confirm.call(()),
                            "Delete"
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub(super) fn DeleteWalletConfirmDialog(
    wallet_label: String,
    on_confirm: EventHandler<bool>,
    on_cancel: EventHandler<()>,
) -> Element {
    let mut delete_accounts = use_signal(|| true);

    rsx! {
        div { class: "modal-overlay",
            div { class: "modal",
                div { class: "modal-header",
                    h3 { "Delete Wallet" }
                }
                div { class: "modal-body",
                    p { "Delete wallet \"{wallet_label}\"?" }
                    label { class: "checkbox",
                        input {
                            r#type: "checkbox",
                            checked: delete_accounts(),
                            onchange: move |_| delete_accounts.set(!delete_accounts()),
                        }
                        span { "Delete all linked accounts (required)" }
                    }
                    div { class: "modal-actions",
                        button {
                            class: "btn btn-secondary",
                            onclick: move |_| on_cancel.call(()),
                            "Cancel"
                        }
                        button {
                            class: "btn btn-danger",
                            disabled: !delete_accounts(),
                            onclick: move |_| on_confirm.call(delete_accounts()),
                            "Delete"
                        }
                    }
                    if !delete_accounts() {
                        p { class: "error-text", "Keeping accounts is not supported yet." }
                    }
                }
            }
        }
    }
}
