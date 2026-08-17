//! Contact affordances: a sidebar footer showing the developer's email as a
//! copy-first control, plus a modal for the PGP public key.

use super::{CheckIcon, CloseIcon, CopyIcon, copy_to_clipboard};
use dioxus::prelude::*;

/// Where mail is sent. Shown in the sidebar and used in the `mailto:` link.
const CONTACT_EMAIL: &str = "hello@bitgarth.app";

/// ASCII-armored public key for [`CONTACT_EMAIL`], also offered as a download.
const PGP_PUBLIC_KEY: &str = include_str!("../../assets/hello-bitgarth-pubkey.asc");

/// The key as a downloadable `.asc`, bundled and served by the build.
const PGP_ASC: Asset = asset!("/assets/hello-bitgarth-pubkey.asc");

fn pgp_fingerprint_from_armor(key: &str) -> Option<&str> {
    key.lines()
        .find_map(|line| line.strip_prefix("Comment: Fingerprint: "))
}

/// Footer block pinned to the bottom of the sidebar. The email is shown
/// directly and clicking it copies (most people want to copy, not launch a
/// mail client); "Compose" is there for those who do, and "PGP key" opens
/// [`ContactModal`].
#[component]
pub fn SidebarContact(on_show_key: EventHandler<()>) -> Element {
    let mut copied = use_signal(|| false);
    let mailto = format!("mailto:{CONTACT_EMAIL}");

    rsx! {
        div { class: "sidebar-contact",
            p { class: "sidebar-contact-title", "Get in touch" }
            p { class: "sidebar-contact-note", "Anything you'd like improved." }
            button {
                class: "sidebar-contact-email",
                r#type: "button",
                "data-testid": "sidebar-contact-email",
                "aria-live": "polite",
                title: if copied() { "Copied" } else { "Copy email address" },
                onclick: move |_| {
                    copy_to_clipboard(CONTACT_EMAIL);
                    copied.set(true);
                    spawn(async move {
                        let mut wait = dioxus::document::eval(
                            "setTimeout(() => dioxus.send(true), 1600);",
                        );
                        let _ = wait.recv::<bool>().await;
                        copied.set(false);
                    });
                },
                span { class: "sidebar-contact-addr", "{CONTACT_EMAIL}" }
                span { class: "sidebar-contact-copy",
                    if copied() {
                        CheckIcon {}
                    } else {
                        CopyIcon {}
                    }
                }
            }
            div { class: "sidebar-contact-actions",
                a { class: "sidebar-contact-action", href: "{mailto}", "Compose" }
                span { class: "sidebar-contact-sep", "·" }
                button {
                    class: "sidebar-contact-action",
                    r#type: "button",
                    "data-testid": "sidebar-contact-pgp",
                    onclick: move |_| on_show_key.call(()),
                    "PGP key"
                }
            }
        }
    }
}

/// Modal showing the PGP public key for [`CONTACT_EMAIL`]. Opened from the
/// sidebar "PGP key" action; the key is shown directly (opening it is
/// already the explicit request, so there is nothing to hide behind a toggle).
#[component]
pub fn ContactModal(on_close: EventHandler<()>) -> Element {
    let mut key_copied = use_signal(|| false);
    let pgp_fingerprint = pgp_fingerprint_from_armor(PGP_PUBLIC_KEY);

    rsx! {
        dialog {
            id: "pgp-contact-dialog",
            class: "modal contact-modal",
            role: "dialog",
            "aria-modal": "true",
            "aria-labelledby": "pgp-modal-title",
            onmounted: move |_| {
                let _ = dioxus::document::eval(
                    "document.getElementById('pgp-contact-dialog')?.showModal();",
                );
            },
            oncancel: move |_| on_close.call(()),
            div { class: "modal-header",
                h3 { id: "pgp-modal-title", "PGP public key" }
                button {
                    class: "modal-close-btn",
                    r#type: "button",
                    "aria-label": "Close",
                    title: "Close",
                    autofocus: true,
                    onclick: move |_| on_close.call(()),
                    CloseIcon {}
                }
            }
            div { class: "modal-body",
                p { class: "muted contact-pgp-note",
                    "Encrypt your message to "
                    span { class: "contact-pgp-inline-addr", "{CONTACT_EMAIL}" }
                    ", or verify a signature from us."
                }
                if let Some(fingerprint) = pgp_fingerprint {
                    p { class: "contact-pgp-fp",
                        "Fingerprint "
                        span { "{fingerprint}" }
                    }
                }
                div { class: "code-card contact-pgp-key", "data-label": "public key",
                    button {
                        class: "code-card-copy",
                        r#type: "button",
                        "aria-live": "polite",
                        "aria-label": "Copy public key",
                        onclick: move |_| {
                            copy_to_clipboard(PGP_PUBLIC_KEY);
                            key_copied.set(true);
                            spawn(async move {
                                let mut wait = dioxus::document::eval(
                                    "setTimeout(() => dioxus.send(true), 1600);",
                                );
                                let _ = wait.recv::<bool>().await;
                                key_copied.set(false);
                            });
                        },
                        if key_copied() {
                            CheckIcon {}
                            span { "Copied" }
                        } else {
                            CopyIcon {}
                            span { "Copy" }
                        }
                    }
                    pre { code { "{PGP_PUBLIC_KEY}" } }
                }
                a {
                    class: "btn ghost contact-pgp-download",
                    href: "{PGP_ASC}",
                    download: "",
                    "Download .asc"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pgp_fingerprint_comes_from_armored_key() {
        assert_eq!(
            pgp_fingerprint_from_armor(PGP_PUBLIC_KEY),
            Some("40F4 6DAB EDD9 A5F4 4047  F0C2 C74A EC0F 8263 2843")
        );
    }
}
